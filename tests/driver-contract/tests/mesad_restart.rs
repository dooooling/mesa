//! Mesad 进程级 restart recovery Gate（P1-2，PR7 v1.1 §23）。
//!
//! 与 `event_ids_unique_across_driver_process_restart`（同进程内换 Driver
//! 进程）互补：本测试杀掉整个 Mesad 进程再用同一数据目录拉起新进程，证明：
//! EventTask 配置恢复、desired_running 恢复、旧 Event history 仍在、
//! seq 继续增长、新 epoch 产生新 Event。
//! kill 不优雅（厘米 crash 语义）：落盘即事实，WAL + COMMIT 可见性保证恢复。

mod common;

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use common::*;

/// 裸 TCP HTTP/1.1（无客户端依赖；mesad 只回完整 JSON + content-length）。
async fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, serde_json::Value) {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (rd, mut wr) = stream.into_split();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    wr.write_all(req.as_bytes()).await.unwrap();
    // NOTE: 写半部必须持有到响应读完——提前 shutdown 即 FIN 半关闭，
    // hyper 会杀掉未完成的响应（SSE 裸客户端同款教训）。读完即 drop。
    let mut reader = BufReader::new(rd);
    let _wr = wr;
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let status: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut content_length: Option<usize> = None;
    loop {
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        let t = line.trim_end_matches(['\r', '\n']);
        if t.is_empty() {
            break;
        }
        if let Some(v) = t.strip_prefix("content-length:") {
            content_length = v.trim().parse().ok();
        } else if let Some(v) = t.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().ok();
        }
    }
    let body_bytes = match content_length {
        Some(0) | None if content_length == Some(0) => Vec::new(),
        Some(n) => {
            let mut buf = vec![0u8; n];
            tokio::io::AsyncReadExt::read_exact(&mut reader, &mut buf)
                .await
                .unwrap();
            buf
        }
        None => {
            let mut buf = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf)
                .await
                .unwrap();
            buf
        }
    };
    let v = if body_bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body_bytes).unwrap()
    };
    (status, v)
}

async fn wait_ready(port: u16) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Ok(Ok(s)) = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        {
            drop(s);
            // 端口通后再确认 REST 就绪（mesad 开机恢复在 serve 之前完成，
            // 端口可连即恢复已执行）。
            if let Ok((200, _)) = tokio::time::timeout(
                Duration::from_secs(2),
                http(port, "GET", "/api/v1/drivers", None),
            )
            .await
            {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("mesad on {port} not ready in 30s");
}

fn spawn_mesad(dir: &std::path::Path, port: u16) -> tokio::process::Child {
    tokio::process::Command::new(mesad_exe())
        .arg("--db")
        .arg(dir.join("mesa.db"))
        .arg("--drivers-dir")
        .arg(repo_root().join("drivers"))
        .arg("--http-port")
        .arg(port.to_string())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn mesad")
}

#[tokio::test]
async fn mesad_process_restart_recovers_events_and_history() {
    common::init_log();
    let dir = std::env::temp_dir().join(format!(
        "mesa-mesad-restart-{}-{}",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // ---- Phase 1：全新目录拉起，配齐 device/endpoint/tasks/event-tasks 并启动
    let port1 = free_port().await;
    let mut mesad1 = spawn_mesad(&dir, port1);
    wait_ready(port1).await;

    let (st, _) = http(
        port1,
        "POST",
        "/api/v1/devices",
        Some(r#"{"id":"d1","name":"RestartGate"}"#),
    )
    .await;
    assert_eq!(st, 201, "create device");
    let (st, _) = http(
        port1,
        "POST",
        "/api/v1/endpoints",
        Some(r#"{"id":"ep-restart","device_id":"d1","driver_id":"simulator","connection":{}}"#),
    )
    .await;
    assert!(st == 200 || st == 201, "create endpoint, got {st}");
    let (st, _) = http(
        port1,
        "PUT",
        "/api/v1/tasks/ep-restart",
        Some(
            r#"{"tasks":[{"id":"t1","mode":"poll","interval_ms":50,"binding":{"kind":"simulator.points","config":{"points":[{"key":"k.counter","kind":"counter"}]}}}]}"#,
        ),
    )
    .await;
    assert_eq!(st, 200, "put data tasks");
    let (st, _) = http(
        port1,
        "PUT",
        "/api/v1/endpoints/ep-restart/event-tasks",
        Some(
            r#"{"event_tasks":[{"id":"al","mode":"subscribe","interval_ms":null,"binding":{"kind":"simulator.events","config":{"stream":"sim.events.alarm-cycle"}}}]}"#,
        ),
    )
    .await;
    assert_eq!(st, 200, "put event tasks");
    let (st, _) = http(port1, "POST", "/api/v1/endpoints/ep-restart/start", None).await;
    assert_eq!(st, 200, "start endpoint");

    // 等第一轮 alarm 四态入库（查到 = 已 COMMIT）
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let (r1, s1, e1): (usize, i64, u64);
    let old_ids: Vec<String>;
    loop {
        let (st, v) = http(
            port1,
            "GET",
            "/api/v1/events?endpoint_id=ep-restart&limit=100",
            None,
        )
        .await;
        assert_eq!(st, 200);
        let arr = v["events"].as_array().unwrap();
        if arr.len() >= 4 {
            r1 = arr.len();
            s1 = arr
                .iter()
                .map(|e| e["seq"].as_i64().unwrap())
                .max()
                .unwrap();
            e1 = arr[0]["stream_epoch"].as_u64().unwrap();
            old_ids = arr
                .iter()
                .map(|e| e["event"]["event_id"].as_str().unwrap().to_string())
                .collect();
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "phase1 events not produced in 30s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(e1 != 0, "epoch recorded");

    // crash（非优雅）：落盘即事实，恢复不得丢
    mesad1.kill().await.unwrap();
    let _ = mesad1.wait().await;

    // ---- Phase 2：同一数据目录拉起新进程
    let port2 = free_port().await;
    assert_ne!(port1, port2);
    let mut mesad2 = spawn_mesad(&dir, port2);
    wait_ready(port2).await;

    // desired_running + EventTask 恢复：endpoint 自动回到 running
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (st, v) = http(port2, "GET", "/api/v1/endpoints/ep-restart/state", None).await;
        assert_eq!(st, 200);
        if v["state"] == "RUNNING" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "endpoint not running after restart: {v}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // 旧 history 仍在（event_id 全保留），seq 继续沿增长，新 epoch 产生新行
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (st, v) = http(
            port2,
            "GET",
            "/api/v1/events?endpoint_id=ep-restart&limit=500",
            None,
        )
        .await;
        assert_eq!(st, 200);
        let arr = v["events"].as_array().unwrap();
        let max_seq = arr
            .iter()
            .map(|e| e["seq"].as_i64().unwrap())
            .max()
            .unwrap_or(0);
        if arr.len() > r1 && max_seq > s1 {
            let epochs: std::collections::HashSet<u64> = arr
                .iter()
                .map(|e| e["stream_epoch"].as_u64().unwrap())
                .collect();
            assert!(epochs.len() >= 2, "新进程必须开新 epoch，epochs={epochs:?}");
            assert!(!epochs.contains(&0), "epoch 非零");
            let new_ids: std::collections::HashSet<&str> = arr
                .iter()
                .map(|e| e["event"]["event_id"].as_str().unwrap())
                .collect();
            for id in &old_ids {
                assert!(new_ids.contains(id.as_str()), "旧 history 行 {id} 必须保留");
            }
            // 新 epoch 的行与旧 epoch 互异（epoch 作用域 ID，无 collision 顶替）
            let fresh: Vec<&&str> = new_ids
                .iter()
                .filter(|id| !old_ids.iter().any(|o| o == *id))
                .collect();
            assert!(!fresh.is_empty(), "新 epoch 必须产生新 event_id");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "seq did not grow after restart"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    mesad2.kill().await.unwrap();
    let _ = mesad2.wait().await;
    // P0-1：生产路径 key 只进数据目录，不污染源码树
    assert!(!repo_root().join("data/master.key").exists());
    assert!(!repo_root().join("master.key").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
