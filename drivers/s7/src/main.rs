//! S7 Driver 进程入口（方案 §14）。
//!
//! 序列与 simulator 保持一致：读 token → liveness 守护 → 监听 loopback 端口 → SDK serve。

fn main() {
    let port: u16 = match std::env::args().nth(2).or_else(|| std::env::args().nth(1)) {
        Some(arg) => arg
            .parse()
            .unwrap_or_else(|_| panic!("invalid --port value {arg}")),
        None => panic!("usage: Mesa-driver-s7 --port <u16>"),
    };

    let session_token = mesa_driver_sdk::read_session_token_from_stdin();
    mesa_driver_sdk::spawn_parent_liveness_guard();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| panic!("bind 127.0.0.1:{port} failed: {e}"));

        let shutdown = tokio_util::sync::CancellationToken::new();
        {
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                shutdown.cancel();
            });
        }

        if let Err(e) =
            mesa_driver_sdk::serve(mesa_driver_s7::S7Driver, listener, session_token, shutdown)
                .await
        {
            eprintln!("s7 driver exited with error: {e}");
            std::process::exit(1);
        }
    });
}
