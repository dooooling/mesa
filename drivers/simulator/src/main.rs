//! Simulator Driver 进程入口。
//!
//! 启动序列（方案 §14）：
//! 1. 从 stdin 首行读取 session token；
//! 2. 启动父进程 liveness 守护线程（EOF 即退出，孤儿防护第一层，§14.5）；
//! 3. 监听 `--port` 指定的 loopback 端口并进入 SDK 服务循环。

fn main() {
    let port: u16 = match std::env::args().nth(2).or_else(|| std::env::args().nth(1)) {
        Some(arg) => arg
            .parse()
            .unwrap_or_else(|_| panic!("invalid --port value")),
        None => panic!("usage: forgelink-driver-simulator --port <u16>"),
    };

    // token 必须先于 liveness 守护读取：守护线程会消费 stdin 剩余字节直到 EOF
    let session_token = forgelink_driver_sdk::read_session_token_from_stdin();
    forgelink_driver_sdk::spawn_parent_liveness_guard();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        // 绑定失败通常意味着端口被占用或权限问题：快速失败交由 Core 诊断
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| panic!("bind 127.0.0.1:{port} failed: {e}"));

        let shutdown = tokio_util::sync::CancellationToken::new();
        // Ctrl-C 仅在交互式前台运行时生效；服务化场景由 Core 的 Shutdown 消息驱动
        {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                shutdown.cancel();
            });
        }

        if let Err(e) =
            forgelink_driver_sdk::serve(forgelink_driver_simulator::SimulatorDriver, listener, session_token, shutdown)
                .await
        {
            eprintln!("simulator driver exited with error: {e}");
            std::process::exit(1);
        }
    });
}
