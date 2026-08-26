//! forgelinkd：ForgeLink Core 的唯一运行入口（方案 §25）。
//!
//! M0 装配：扫描 drivers/ -> 注入内置默认 Endpoint（simulator）-> 启动 REST。
//! 配置持久化与 CRUD 在 Phase 1 接入 ConfigStore 后替换内置注入路径。

use forgelink_core_types::{AcquisitionTask, DriverBinding, TaskMode};

/// 默认 HTTP 端口。仅 loopback 可见（§4.2）。
const DEFAULT_HTTP_PORT: u16 = 8132;

#[tokio::main]
async fn main() {
    // 日志级别可用 RUST_LOG 覆盖，默认 info
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = parse_args();
    let drivers_dir = std::path::PathBuf::from(&args.drivers_dir);
    tracing::info!(dir = %drivers_dir.display(), "scanning driver manifests");

    let manager = forgelink_driver_manager::ForgeLinkManager::discover(&drivers_dir);

    // ---- M0 内置默认配置：一个 simulator Endpoint（替代 Phase 1 前的 ConfigStore）----
    let builtin = builtin_endpoint();
    match manager.start_builtin_endpoint(builtin) {
        Ok(()) => tracing::info!("builtin endpoint `sim-001` scheduled"),
        Err(e) => {
            tracing::error!("{e}");
            tracing::error!("hint: run `cargo build` first, then start forgelinkd from the workspace root");
        }
    }

    // ---- REST 服务 ----
    let api_shutdown = manager.shutdown_token().child_token();
    let api = tokio::spawn(forgelink_core_api::serve(manager.snapshot(), args.http_port, api_shutdown));

    print_banner(args.http_port);

    // ---- 优雅停机：Ctrl-C / 服务停止信号 → 取消全部运行时 → 收尾 ----
    wait_for_interrupt().await;
    tracing::info!("shutdown signal received, stopping...");
    manager.shutdown().await;
    api.abort();
    tracing::info!("bye");
}

struct Args {
    drivers_dir: String,
    http_port: u16,
}

fn parse_args() -> Args {
    let mut out = Args { drivers_dir: "drivers".into(), http_port: DEFAULT_HTTP_PORT };
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
            other => {
                eprintln!("unknown arg: {other}");
                i += 1;
            }
        }
    }
    out
}

fn print_banner(http_port: u16) {
    eprintln!();
    eprintln!("  ForgeLink Core (M0) is running");
    eprintln!("  REST  : http://127.0.0.1:{http_port}/api/v1/drivers");
    eprintln!("          http://127.0.0.1:{http_port}/api/v1/endpoints");
    eprintln!("          http://127.0.0.1:{http_port}/api/v1/points/latest");
    eprintln!("  Stop  : Ctrl+C");
    eprintln!();
}

#[cfg(unix)]
async fn wait_for_interrupt() {
    use tokio::signal::unix::{signal, SignalKind};
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

/// M0 内置端点定义。字段语义与 §5.3/§5.5 一致；binding 由 simulator 解释。
fn builtin_endpoint() -> forgelink_driver_manager::endpoint::BuiltinEndpoint {
    let tasks = vec![AcquisitionTask {
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
    }];
    forgelink_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "sim-001".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks,
    }
}
