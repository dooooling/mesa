//! Mesad：Mesa Core 的唯一运行入口（方案 §25）。
//!
//! SQLite 配置持久化 + REST CRUD + 开机恢复（配置真值只在 Core）。`sim-001` 仅在空库时作为演示种子
//! 后续以库为准（配置真值只在 Core）。

use std::sync::Arc;

use mesa_config_store::{ConfigStore, DeviceRecord, EndpointRecord};
use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode};
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

    // 空库时写入演示种子（仅一次，保持开箱可用；后续以 REST 为准）
    if let Err(e) = maybe_seed_demo(&store) {
        tracing::warn!("seed demo failed: {e}");
    }

    // ---- DriverManager（持久版 ID 源）----
    let source = Arc::new(StorePointIdSource::new(store.clone()));
    let manager = Arc::new(mesa_driver_manager::MesaManager::with_source(
        &drivers_dir,
        source,
    ));

    // ---- PKI 初始化（必须在恢复 Endpoint/启动 Driver 之前，确保 OPC UA Secure 证书就绪）----
    if std::env::var("MESA_OPCUA_PKI_DIR").is_err() {
        let pki = mesa_core_api::certificates::CertStore::default_path();
        unsafe {
            std::env::set_var("MESA_OPCUA_PKI_DIR", &pki);
        }
        tracing::info!(pki=%pki.display(), "set MESA_OPCUA_PKI_DIR");
    }

    let app_state = match mesa_core_api::AppState::try_new_with_control(
        manager.clone(),
        store.clone(),
        args.drivers_dir.clone(),
        args.enable_control,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("certificate initialization failed: {e}");
            std::process::exit(1);
        }
    };

    // ---- 恢复期望运行的 Endpoint（已保证 PKI/证书就绪，避免 Secure Endpoint 竞态）----
    let stored_eps = store.list_endpoints().unwrap_or_default();
    if stored_eps.is_empty() {
        tracing::warn!("no endpoints in store; create one via REST POST /api/v1/endpoints");
    }
    for rec in &stored_eps {
        if !rec.desired_running {
            continue;
        }
        let tasks = store.list_tasks(&rec.id).unwrap_or_default();
        let cfg = mesa_driver_manager::endpoint::BuiltinEndpoint {
            endpoint_id: rec.id.clone(),
            driver_id: rec.driver_id.clone(),
            connection_json: rec.connection_json.clone(),
            tasks,
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
    api.abort();
    tracing::info!("bye");
}

struct Args {
    drivers_dir: String,
    http_port: u16,
    db_path: String,
    enable_control: bool,
}

fn parse_args() -> Args {
    let mut out = Args {
        drivers_dir: "drivers".into(),
        http_port: DEFAULT_HTTP_PORT,
        db_path: DEFAULT_DB_PATH.into(),
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
            "--enable-control" => {
                out.enable_control = true;
                i += 1;
            }
            other => {
                eprintln!(
                    "unknown arg: {other} (supported: --drivers-dir --http-port --db --enable-control)"
                );
                i += 1;
            }
        }
    }
    out
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

/// 空库时写入演示种子：device + endpoint + tasks，desired_running=true。
/// 若库已有任意 endpoint 则不操作。
fn maybe_seed_demo(store: &ConfigStore) -> Result<bool, String> {
    let eps = store.list_endpoints().map_err(|e| e.to_string())?;
    if !eps.is_empty() {
        return Ok(false);
    }
    // 幂等：device 已存在则复用
    let dev = DeviceRecord {
        id: "sim-device".into(),
        name: "Simulator Device".into(),
        profile: None,
    };
    let _ = store.create_device(&dev);
    let rec = EndpointRecord {
        id: "sim-001".into(),
        device_id: "sim-device".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        desired_running: true,
        updated_at_ns: mesa_core_types::now_unix_ns(),
    };
    match store.create_endpoint(&rec) {
        Ok(()) => {}
        Err(e) if e.to_string().contains("已存在") => return Ok(false),
        Err(e) => return Err(e.to_string()),
    }
    store
        .replace_tasks("sim-001", &demo_tasks())
        .map_err(|e| e.to_string())?;
    tracing::info!("seeded demo endpoint `sim-001` (first run only)");
    Ok(true)
}

fn demo_tasks() -> Vec<AcquisitionTask> {
    vec![AcquisitionTask {
        id: "default".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(200),
        binding: DriverBinding {
            kind: "simulator.points".into(),
            config: serde_json::json!({
                "points": [
                    { "key": "sim.counter", "kind": "counter", "start": 0, "step": 1 },
                    { "key": "sim.sine",    "kind": "sine", "amplitude": 100, "period_ms": 5000, "offset": 50 },
                    { "key": "sim.toggle",  "kind": "toggle", "initial": false },
                    { "key": "sim.const",   "kind": "constant", "value": 42 },
                    { "key": "sim.random",  "kind": "random", "min": -5, "max": 5, "seed": 7 }
                ]
            }),
        },
    }]
}
