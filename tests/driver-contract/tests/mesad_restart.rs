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

fn spawn_mesad(
    dir: &std::path::Path,
    port: u16,
    drivers_dir: &std::path::Path,
) -> tokio::process::Child {
    tokio::process::Command::new(mesad_exe())
        .arg("--db")
        .arg(dir.join("mesa.db"))
        .arg("--drivers-dir")
        .arg(drivers_dir)
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

    // staged drivers 目录（含 test-driver；mesad 子进程全程需要，guard 与子进程同寿）。
    let _staged = common::staged_drivers_with_test_driver();

    // ---- Phase 1：全新目录拉起，配齐 device/endpoint/tasks/event-tasks 并启动
    let port1 = free_port().await;
    let mut mesad1 = spawn_mesad(&dir, port1, _staged.path());
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
        Some(r#"{"id":"ep-restart","name":"Restart","device_id":"d1","driver_id":"test-driver","connection":{}}"#),
    )
    .await;
    assert!(st == 200 || st == 201, "create endpoint, got {st}");
    let (st, _) = http(
        port1,
        "PUT",
        "/api/v1/tasks/ep-restart",
        Some(
            r#"{"tasks":[{"id":"t1","schedule":{"mode":"poll","interval_ms":50},"binding":{"kind":"mesa.resources.v1","config":{"selections":[{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]}}}]}"#,
        ),
    )
    .await;
    assert_eq!(st, 200, "put data tasks");
    let (st, _) = http(
        port1,
        "PUT",
        "/api/v1/endpoints/ep-restart/event-tasks",
        Some(
            r#"{"event_tasks":[{"id":"al","mode":"subscribe","interval_ms":null,"binding":{"kind":"mesa.events.v1","config":{"stream_id":"sim.events.alarm-cycle","parameters":{}}}}]}"#,
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

    // ---- Phase 2：同一数据目录拉起新进程（同 staged drivers 目录）
    let port2 = free_port().await;
    assert_ne!(port1, port2);
    let mut mesad2 = spawn_mesad(&dir, port2, _staged.path());
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

/// G7 S2（失败关闭）：**停止意图未持久化 + 崩溃 ⇒ 重启不得自动拉起该端点**。
///
/// 场景复现（与生产事故链逐环对应）：
/// 1. 端点正常运行，DB 里 `desired_running=true`；
/// 2. Stop 已受理并**真正停掉**，但 `desired_running=false` 落盘失败
///    （DB busy/磁盘错误）⇒ DB 仍是 true；
/// 3. 进程在此刻崩溃（`kill`，非优雅退出）。
///
/// 若无围栏：重启时开机恢复读到 `desired_running=true` ⇒ 把用户**明确要停**的
/// 机床端点重新拉起来（危险动作）。围栏（`meta.stop_intent_unpersisted`）是
/// 持久化事实，因此跨崩溃有效：命中即跳过自动恢复，并要求人工确认
/// （显式 `POST /start` 成功后才清除围栏）。
///
/// 判别力：同一库里另有一个**未命中围栏**的 `desired_running=true` 端点作为
/// 对照组——它必须被正常恢复。否则"没被拉起"可能只是恢复逻辑整体失效。
#[tokio::test]
async fn stop_intent_fence_blocks_auto_restore_after_crash() {
    common::init_log();
    let dir = std::env::temp_dir().join(format!(
        "mesa-g7-fence-{}-{}",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _staged = common::staged_drivers_with_test_driver();
    let db = dir.join("mesa.db");

    // ---- Phase 1：两个端点都启动（desired_running 落盘 true）----
    let port1 = free_port().await;
    let mut mesad1 = spawn_mesad(&dir, port1, _staged.path());
    wait_ready(port1).await;
    let (st, _) = http(
        port1,
        "POST",
        "/api/v1/devices",
        Some(r#"{"id":"dfence","name":"Fence"}"#),
    )
    .await;
    assert_eq!(st, 201, "create device");
    for ep in ["ep-fenced", "ep-control"] {
        let body = format!(
            r#"{{"id":"{ep}","name":"{ep}","device_id":"dfence","driver_id":"test-driver","connection":{{}}}}"#
        );
        let (st, v) = http(port1, "POST", "/api/v1/endpoints", Some(&body)).await;
        assert!(st == 200 || st == 201, "create endpoint {ep}: {v}");
        // 必须有采集任务：无任务的 test-driver 会立即报 Stopped（端点进 RECONNECTING）。
        let tasks = r#"{"tasks":[{"id":"t1","schedule":{"mode":"poll","interval_ms":50},"binding":{"kind":"mesa.resources.v1","config":{"selections":[{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]}}}]}"#;
        let (st, v) = http(port1, "PUT", &format!("/api/v1/tasks/{ep}"), Some(tasks)).await;
        assert_eq!(st, 200, "put tasks {ep}: {v}");
        let (st, v) = http(port1, "POST", &format!("/api/v1/endpoints/{ep}/start"), None).await;
        assert_eq!(st, 200, "start {ep}: {v}");
    }
    for ep in ["ep-fenced", "ep-control"] {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let (st, v) = http(port1, "GET", &format!("/api/v1/endpoints/{ep}/state"), None).await;
            assert_eq!(st, 200);
            if v["state"] == "RUNNING" {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{ep} not running: {v}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    // 崩溃（非优雅）：落盘即事实。
    mesad1.kill().await.unwrap();
    let _ = mesad1.wait().await;

    // ---- 模拟"停止意图未持久化"的崩溃现场 ----
    // 1) 断言前置：DB 里两个端点都还是 desired_running=true（若这里是 false，
    //    本用例就退化成空断言）。
    {
        let store = mesa_config_store::ConfigStore::open(&db).unwrap();
        for ep in ["ep-fenced", "ep-control"] {
            assert!(
                store.get_endpoint(ep).unwrap().unwrap().desired_running,
                "前置：{ep} 的 desired_running 必须仍为 true"
            );
        }
        assert!(
            store.stop_intent_unpersisted().unwrap().is_empty(),
            "前置：围栏必须为空"
        );
    }
    // 2) 直接写入围栏（等价于 Stop 落盘失败时门写入的那条持久化事实）。
    //    用裸 SQL 而不是 store API：本用例要能在**修复前**的代码上编译并失败。
    {
        let raw = rusqlite::Connection::open(&db).unwrap();
        raw.execute(
            "INSERT INTO meta(key,value) VALUES('stop_intent_unpersisted',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [r#"["ep-fenced"]"#],
        )
        .unwrap();
    }

    // ---- Phase 2：同目录重启 ----
    let port2 = free_port().await;
    let mut mesad2 = spawn_mesad(&dir, port2, _staged.path());
    wait_ready(port2).await;

    // 对照组：未命中围栏的端点必须被正常恢复（证明恢复逻辑本身有效）。
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (st, v) = http(port2, "GET", "/api/v1/endpoints/ep-control/state", None).await;
        assert_eq!(st, 200);
        if v["state"] == "RUNNING" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "对照组 ep-control 必须被自动恢复: {v}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // 命中围栏：5s 内**任何时刻**都不得出现 RUNNING（不得被自动拉起）。
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < until {
        let (st, v) = http(port2, "GET", "/api/v1/endpoints/ep-fenced/state", None).await;
        assert_eq!(st, 200, "{v}");
        assert_ne!(
            v["state"], "RUNNING",
            "S2 失败：命中围栏的端点被自动拉起了（用户明确要停的端点）: {v}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // 持久化意图仍是 true（残余风险本身没变），只是被围栏挡住了。
    {
        let store = mesa_config_store::ConfigStore::open(&db).unwrap();
        assert!(
            store.get_endpoint("ep-fenced").unwrap().unwrap().desired_running,
            "围栏不改变 DB 里的旧值（它只阻止自动恢复）"
        );
        assert_eq!(
            store.stop_intent_unpersisted().unwrap(),
            vec!["ep-fenced".to_string()],
            "围栏必须仍在（等待人工确认）"
        );
    }

    // ---- 人工确认：显式 POST /start 成功，并清除围栏 ----
    let (st, v) = http(port2, "POST", "/api/v1/endpoints/ep-fenced/start", None).await;
    assert_eq!(st, 200, "人工确认后必须允许显式启动: {v}");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (st, v) = http(port2, "GET", "/api/v1/endpoints/ep-fenced/state", None).await;
        assert_eq!(st, 200);
        if v["state"] == "RUNNING" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "显式启动后必须进入 RUNNING: {v}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    {
        let store = mesa_config_store::ConfigStore::open(&db).unwrap();
        assert!(
            store.stop_intent_unpersisted().unwrap().is_empty(),
            "显式启动成功（人工确认）后围栏必须清除"
        );
    }

    mesad2.kill().await.unwrap();
    let _ = mesad2.wait().await;
    let _ = std::fs::remove_dir_all(&dir);
}
