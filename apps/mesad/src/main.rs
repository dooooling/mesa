//! Mesad：Mesa Core 的唯一运行入口（方案 §25）。
//!
//! SQLite 配置持久化 + REST CRUD + 开机恢复（配置真值只在 Core）。空库不再
//! seeding 任何演示 endpoint（含历史 `sim-001` test-driver 演示种子，已随
//! test-driver 退役产品身份删除）；后续以 REST 为准。

use std::sync::Arc;

use mesa_config_store::ConfigStore;
use mesa_driver_manager::StorePointIdSource;

/// 默认 HTTP 端口。仅 loopback 可见（§4.2）。
const DEFAULT_HTTP_PORT: u16 = 8132;
/// 默认库路径（workspace 根下）。
const DEFAULT_DB_PATH: &str = "mesa.db";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = parse_args();
    let drivers_dir = std::path::PathBuf::from(&args.drivers_dir);
    let db_path = std::path::PathBuf::from(&args.db_path);
    tracing::info!(dir = %drivers_dir.display(), db = %db_path.display(), "starting mesad");

    // ---- ConfigStore ----
    let store = match ConfigStore::open(&db_path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::error!("open db {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };

    // 空库不再 seeding（test-driver 退役产品身份后，无演示种子；
    // 后续以 REST 为准）。
    if let Err(e) = maybe_seed_demo(&store) {
        tracing::warn!("seed demo failed: {e}");
    }

    // ---- DriverManager（持久版 ID 源）----
    let source = Arc::new(StorePointIdSource::new(store.clone()));
    let manager = Arc::new(mesa_driver_manager::MesaManager::with_source(
        &drivers_dir,
        source,
    ));

    // ---- EventStore（PR7 v1.1 §2/§22）：独立 events.db，失败不连累 Data Plane ----
    // 打开失败 → Unavailable 模式：普通 Data-only Endpoint 照常运行，
    // 有 EventTask 的 Endpoint 拒绝 Start（EVENT_STORE_UNAVAILABLE），
    // /api/v1/events 系接口 503（step ⑤）。
    let events_db_path = args
        .events_db_path
        .clone()
        .unwrap_or_else(|| default_events_db_path(&db_path));
    // Option 分两步：manager（运行期）与 AppState（REST/SSE）各持一份
    let event_services: Option<Arc<mesa_event_store::EventServices>> =
        match mesa_event_store::EventStore::open(&events_db_path) {
            Ok(event_store) => {
                let services = mesa_event_store::EventServices::new(
                    Arc::new(event_store),
                    mesa_event_store::EventHub::new(mesa_event_store::EVENT_HUB_CAPACITY),
                );
                manager.set_event_services(services.clone());
                tracing::info!(db = %events_db_path.display(), "events.db opened");
                Some(services)
            }
            Err(e) => {
                tracing::error!(
                    db = %events_db_path.display(),
                    "events.db unavailable ({e}); data-only endpoints still run, \
                     event-enabled endpoints will refuse to start"
                );
                None
            }
        };
    // ---- Retention sweeper（⑧b，v1.1 §17 冻结默认：30d/1M/10min/batch1000）----
    // 无 CLI flag（调参需求出现时再加）；events.db 不可用时不启动。
    // shutdown 由 manager token 级联（shutdown_all 取消 → sweeper 退出）。
    let retention_shutdown = manager.shutdown_token().child_token();
    let retention_task = event_services.clone().map(|svc| {
        let shutdown = retention_shutdown.clone();
        tokio::spawn(mesa_event_store::retention::run_retention_loop(
            svc.store.clone(),
            svc.diagnostics.clone(),
            mesa_event_store::RetentionConfig::default(),
            shutdown,
        ))
    });

    // ---- PKI 初始化（必须在恢复 Endpoint/启动 Driver 之前，确保 OPC UA Secure 证书就绪）----
    // 只 resolve 一次 PKI 路径，Core 与 Driver 共用（自定义 MESA_OPCUA_PKI_DIR 时避免分叉）
    let pki_dir = std::env::var_os("MESA_OPCUA_PKI_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(mesa_core_api::certificates::CertStore::default_path);
    if std::env::var("MESA_OPCUA_PKI_DIR").is_err() {
        unsafe {
            std::env::set_var("MESA_OPCUA_PKI_DIR", &pki_dir);
        }
        tracing::info!(pki=%pki_dir.display(), "set MESA_OPCUA_PKI_DIR (default)");
    } else {
        tracing::info!(pki=%pki_dir.display(), "using MESA_OPCUA_PKI_DIR");
    }

    let app_state = match mesa_core_api::AppState::try_new_with_cert_dir_and_control(
        manager.clone(),
        store.clone(),
        args.drivers_dir.clone(),
        pki_dir,
        args.enable_control,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("certificate initialization failed: {e}");
            std::process::exit(1);
        }
    };
    // REST/SSE 的事件面（⑥）：有则注入，无则 /events* 系 503
    if let Some(svc) = event_services {
        app_state.set_event_services(svc);
    }

    // ---- 恢复期望运行的 Endpoint（已保证 PKI/证书就绪，避免 Secure Endpoint 竞态）----
    // 读取失败不得伪装成“空库”：否则所有 desired_running 端点会静默不恢复。
    let stored_eps = match store.list_endpoints() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("读取 Endpoint 列表失败，跳过本次开机恢复: {e}");
            Vec::new()
        }
    };
    if stored_eps.is_empty() {
        tracing::warn!("no endpoints in store; create one via REST POST /api/v1/endpoints");
    }
    // G7 S2 / P0-4（**默认不恢复**，评审指定的默认值翻转）：
    // 修复前是"命中围栏才不恢复"（默认恢复）。现在只有**能正向证明**运行意图
    // 已持久化才恢复：DB 可写 + 围栏可读 + 意图确认凭证可读，且该端点在凭证里
    // 被确认为 `true`。任一步做不到 ⇒ 不恢复（宁可少恢复，不可误拉起机床端点）。
    //
    // 为什么围栏（负向证据）不够：Stop 的 `desired_running=false` 落盘失败时，
    // 围栏本身也可能写不进去（同一个不可写的 DB）。正向凭证把"没写进去"与
    // "写了但没证明"统一成同一种保守结果。
    //
    // 人工确认方式：显式 POST /start（成功后门会写凭证并清除围栏）。
    let (fenced, confirmed): (Vec<String>, std::collections::BTreeSet<String>) = match store
        .recovery_plan()
    {
        mesa_config_store::RecoveryPlan::Proceed { fenced, confirmed } => (fenced, confirmed),
        mesa_config_store::RecoveryPlan::DenyAll { reason } => {
            tracing::error!(
                reason = %reason,
                "开机恢复被拒绝（fail-closed）：跳过本次全部自动恢复，需人工核验后显式 POST /start"
            );
            (
                stored_eps.iter().map(|r| r.id.clone()).collect(),
                std::collections::BTreeSet::new(),
            )
        }
    };
    for rec in &stored_eps {
        if fenced.iter().any(|f| f == &rec.id) {
            if rec.desired_running {
                tracing::error!(
                    endpoint = %rec.id,
                    "SKIP auto-restore: 该端点上次停止意图未能持久化（围栏命中）；\
                     禁止自动拉起，需人工核验后显式 POST /start 确认"
                );
            } else {
                // 意图已经是 false：风险消失，围栏可以安全清除（否则会永久累积）。
                if let Err(e) = store.clear_stop_intent_unpersisted(&rec.id) {
                    tracing::warn!(endpoint = %rec.id, error = %e, "清除停止意图围栏失败");
                }
            }
            continue;
        }
        if !rec.desired_running {
            continue;
        }
        // 828D 一期产品约束（**不是**通用生命周期语义）：`sinumerik-nck` 端点
        // 开机一律不自动恢复——无论凭证、围栏、`desired_running` 为何，必须由
        // 现场显式 `POST /start` 才启动。其他驱动行为不变。
        //
        // 为什么放在凭证判定之前：这是产品策略，不依赖"能否证明用户意图"；
        // 即使正向凭证完整（升级后正常 Start 过），NCK 也不得被自动拉起。
        if !mesa_core_api::recovery::boot_auto_restore_allowed(&rec.driver_id) {
            tracing::warn!(
                endpoint = %rec.id,
                driver = %rec.driver_id,
                "SKIP auto-restore: 828D 一期产品约束——sinumerik-nck 开机一律不自动恢复；\
                 需现场显式 POST /start"
            );
            continue;
        }
        // P0-4：`desired_running=true` 但没有正向凭证 ⇒ 无法证明这是用户意图
        // （可能是一次失败的 Stop 留下的旧值）⇒ 不恢复。
        if !confirmed.contains(&rec.id) {
            tracing::error!(
                endpoint = %rec.id,
                "SKIP auto-restore: 无法正向证明其运行意图已持久化（无确认凭证）；\
                 禁止自动拉起，需人工核验后显式 POST /start 确认"
            );
            continue;
        }
        // C1 P0：开机恢复必须与 REST Start / Bootstrap start 走**同一条**
        // 运行期构造路径（Descriptor + Secret 物化 + Data/Event 任务门禁）。
        // 此前这里直接传持久化的 `connection_json`，其中的 Secret 字段是
        // `{"secret_set": true}` marker，会被当作密码交给 Driver：带认证的
        // 连接重启后必然 BAD_CONFIG，而配置类失败是终态，端点永久 FAILED。
        let cfg = match mesa_core_api::runtime::build_runtime_endpoint(&app_state, &rec.id).await {
            Ok(cfg) => cfg,
            Err(e) => {
                // fail-closed：单个端点构造失败只跳过该端点，不影响其它端点。
                tracing::error!(endpoint = %rec.id, "skip restore: {e}");
                continue;
            }
        };
        match manager.start_endpoint(cfg) {
            Ok(()) => tracing::info!(endpoint = %rec.id, "restored endpoint (desired=running)"),
            Err(e) => tracing::error!(endpoint = %rec.id, "{e}"),
        }
    }

    // ---- REST 服务 ----
    if args.enable_control {
        tracing::warn!("control plane ENABLED (--enable-control)");
    } else {
        tracing::info!("control plane disabled (use --enable-control to enable)");
    }
    let api_shutdown = manager.shutdown_token().child_token();
    let api = tokio::spawn(mesa_core_api::serve(
        app_state,
        args.http_port,
        api_shutdown,
    ));

    print_banner(args.http_port, &args.db_path);

    // ---- 优雅停机 ----
    wait_for_interrupt().await;
    tracing::info!("shutdown signal received, stopping...");
    manager.shutdown_all().await;
    // retention sweeper 随 token 退出；abort 兜底（已退出的 abort 无害）
    if let Some(t) = retention_task {
        t.abort();
    }
    api.abort();
    tracing::info!("bye");
}

struct Args {
    drivers_dir: String,
    http_port: u16,
    db_path: String,
    /// events.db 路径（默认与 --db 同目录的 events.db，v1.1 §19）。
    /// retention 用冻结默认（⑧b，无 flag）。
    events_db_path: Option<std::path::PathBuf>,
    enable_control: bool,
}

fn parse_args() -> Args {
    let mut out = Args {
        drivers_dir: "drivers".into(),
        http_port: DEFAULT_HTTP_PORT,
        db_path: DEFAULT_DB_PATH.into(),
        events_db_path: None,
        enable_control: false,
    };
    let argv: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--drivers-dir" => {
                out.drivers_dir = argv.get(i + 1).cloned().unwrap_or(out.drivers_dir);
                i += 2;
            }
            "--http-port" => {
                if let Some(p) = argv.get(i + 1).and_then(|s| s.parse().ok()) {
                    out.http_port = p;
                }
                i += 2;
            }
            "--db" => {
                out.db_path = argv.get(i + 1).cloned().unwrap_or(out.db_path);
                i += 2;
            }
            "--events-db" => {
                out.events_db_path = argv.get(i + 1).map(std::path::PathBuf::from);
                i += 2;
            }
            "--enable-control" => {
                out.enable_control = true;
                i += 1;
            }
            other => {
                eprintln!(
                    "unknown arg: {other} (supported: --drivers-dir --http-port --db --events-db --enable-control)"
                );
                i += 1;
            }
        }
    }
    out
}

/// 默认 events.db 路径：与 --db 同目录（v1.1 §19，不搞两个存储根）。
fn default_events_db_path(db_path: &std::path::Path) -> std::path::PathBuf {
    match db_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("events.db"),
        _ => std::path::PathBuf::from("events.db"),
    }
}

fn print_banner(http_port: u16, db_path: &str) {
    eprintln!();
    eprintln!("  Mesa Core is running");
    eprintln!("  REST  : http://127.0.0.1:{http_port}/api/v1/drivers");
    eprintln!("          http://127.0.0.1:{http_port}/api/v1/endpoints");
    eprintln!("          http://127.0.0.1:{http_port}/api/v1/points/latest");
    eprintln!("          http://127.0.0.1:{http_port}/api/v1/diagnostics");
    eprintln!("  DB    : {db_path}");
    eprintln!("  Stop  : Ctrl+C");
    eprintln!();
}

#[cfg(unix)]
async fn wait_for_interrupt() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_interrupt() {
    let _ = tokio::signal::ctrl_c().await;
}

/// 空库 seeding 已删除（历史 `sim-001` test-driver 演示种子随退役移除）。
/// 若库已有任意 endpoint 则不操作；空库直接返回 false，不写任何种子。
fn maybe_seed_demo(store: &ConfigStore) -> Result<bool, String> {
    let eps = store.list_endpoints().map_err(|e| e.to_string())?;
    if !eps.is_empty() {
        return Ok(false);
    }
    tracing::info!("empty store: no demo seed (test-driver retired from production)");
    Ok(false)
}
