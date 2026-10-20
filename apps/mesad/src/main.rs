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
    // G7 S2（失败关闭）：上次进程有"停止意图未持久化"的围栏记录 ⇒ 这些端点
    // **一律不自动恢复**。场景：Stop 已执行、但 `desired_running=false` 没写进
    // DB（DB busy/失败），随后进程崩溃 ⇒ DB 仍是 true。若照常恢复，就会把用户
    // 明确要停的机床端点重新拉起来（危险动作）。围栏是持久化事实，因此跨崩溃有效。
    //
    // 读失败同样 fail-closed：把"读不懂围栏"当成"可能有围栏"，宁可少恢复，
    // 也不误拉起。人工确认方式：显式 POST /start（成功后门会清除围栏）。
    let fenced: Vec<String> = match store.stop_intent_unpersisted() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                error = %e,
                "停止意图围栏读取失败：按 fail-closed 跳过本次全部自动恢复，需人工核验后显式启动"
            );
            stored_eps.iter().map(|r| r.id.clone()).collect()
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
