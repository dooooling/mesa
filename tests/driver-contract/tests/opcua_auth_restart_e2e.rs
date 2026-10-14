//! A21（C1 §9.4 / V1.1 §11）：**含 Secret 的 OPC UA Endpoint 进程级重启恢复**。
//!
//! 本套件证明的是「重启后驱动重新完成了**真实认证**并产出**新的采集周期数据**」，
//! 而不是只看 Endpoint 显示 RUNNING：
//!
//! 1. fixture 是**要求 UserNameIdentityToken** 的 OPC UA 服务器
//!    （`FixtureEventServer::start_with_credentials`，已剔除匿名策略）；
//! 2. 建 Endpoint 时提交明文口令 → Core 写 SecretStore 并把 connection 换成 marker；
//! 3. 启动后必须采到 `ns=0;i=2258`（ServerStatus CurrentTime）的 GOOD/CURRENT 值；
//! 4. **非优雅 kill** 整个 mesad → 同一 db 目录拉起第二个进程；
//! 5. 断言恢复后：仍是 RUNNING、`epoch` 增长（新数据流）、`point_id` 不变，
//!    并且出现 `timestamp_ns > 重启前` 的新采样——这才是「真的又采到了」；
//! 6. 负例：同一 fixture 上换**错误口令**的 Endpoint 必须拿不到数据
//!    （RECONNECTING + CONNECT_FAILED），反证 fixture 没有静默放行匿名。
//!    注意该负例只证明「错误凭据取不到数」，不声称错误码已精确映射为
//!    `BadIdentityTokenRejected`（实测透出的是 Connect 超时，见文件内 NOTE）。
//!
//! 若 (3) 或 (6) 任一失效，就说明该 fixture 实际上允许匿名——那么本套件的
//! 「认证恢复」结论不成立，必须先修 fixture。

mod common;

#[allow(dead_code)] // 各测试目标取用子集，未用 helper 属正常
#[path = "../../../drivers/opcua/tests/support/event_server.rs"]
mod event_server;

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use common::*;
use event_server::FixtureEventServer;

/// ServerStatus_CurrentTime（每个 tick 变化，故每次采样都是新值）。
const CURRENT_TIME_NODE: &str = "nsu=http://opcfoundation.org/UA/;i=2258";

/// 裸 TCP HTTP/1.1（与 mesad_restart.rs 同款；此处复制而非提公共，
/// 避免改动既有套件的行为）。
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
    // NOTE: 写半部必须持有到响应读完（提前 shutdown 即 FIN 半关闭，
    // hyper 会杀掉未完成的响应）。
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
        let lower = t.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().ok();
        }
    }
    let body_bytes = match content_length {
        Some(0) => Vec::new(),
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
            // mesad 的开机恢复在 serve 之前完成：REST 就绪即恢复已执行。
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

/// 启动 mesad 子进程。
///
/// `pki_dir` 必须显式给出：OPC UA 驱动侧需要 client PKI 目录
/// （`MESA_OPCUA_PKI_DIR`），否则 mesad 的相对默认路径会落到测试进程的
/// CWD（`tests/driver-contract/`）并污染源码树。
fn spawn_mesad(
    dir: &std::path::Path,
    port: u16,
    drivers_dir: &std::path::Path,
    pki_dir: &std::path::Path,
) -> tokio::process::Child {
    tokio::process::Command::new(mesad_exe())
        .arg("--db")
        .arg(dir.join("mesa.db"))
        .arg("--drivers-dir")
        .arg(drivers_dir)
        .arg("--http-port")
        .arg(port.to_string())
        .env("MESA_OPCUA_PKI_DIR", pki_dir)
        .kill_on_drop(true)
        .spawn()
        .expect("spawn mesad")
}

fn poll_task_json(point_key: &str) -> String {
    format!(
        r#"{{"tasks":[{{"id":"t1","schedule":{{"mode":"poll","interval_ms":200}},"binding":{{"kind":"mesa.resources.v1","config":{{"selections":[{{"resource_id":"node","parameters":{{"node_id":"{CURRENT_TIME_NODE}","data_type":"DATETIME"}},"outputs":[{{"output":"value","point_key":"{point_key}"}}]}}]}}}}}}]}}"#
    )
}

/// 在 `points/latest` 里找指定 point_key，返回该行（不存在则 None）。
fn find_point(v: &serde_json::Value, key: &str) -> Option<serde_json::Value> {
    v["points"]
        .as_array()?
        .iter()
        .find(|p| p["key"] == key)
        .cloned()
}

#[tokio::test]
async fn authenticated_opcua_endpoint_recovers_secret_and_new_data_after_restart() {
    common::init_log();

    let dir = std::env::temp_dir().join(format!(
        "mesa-opcua-auth-restart-{}-{}",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pki_dir = dir.join("driver-pki");
    std::fs::create_dir_all(&pki_dir).unwrap();

    // 要求用户名/口令的 fixture（不含匿名策略）。
    let mut fixture = FixtureEventServer::start_with_credentials("mesa", "s3cr3t").await;
    let url = fixture.endpoint_url();

    let _staged = common::staged_drivers_with_test_driver();

    // ---- Phase 1：建档 + 明文口令 + 启动 + 采到数据
    let port1 = free_port().await;
    let mut mesad1 = spawn_mesad(&dir, port1, _staged.path(), &pki_dir);
    wait_ready(port1).await;

    let (st, _) = http(
        port1,
        "POST",
        "/api/v1/devices",
        Some(r#"{"id":"d-auth","name":"AuthRestart"}"#),
    )
    .await;
    assert_eq!(st, 201, "create device");

    // 明文口令：Core 必须写入 SecretStore 并把落库值换成 marker。
    let endpoint_body = format!(
        r#"{{"id":"ep-auth","name":"Auth","device_id":"d-auth","driver_id":"opcua","connection":{{"endpoint_url":"{url}","username":"mesa","password":"s3cr3t"}}}}"#
    );
    let (st, v) = http(port1, "POST", "/api/v1/endpoints", Some(&endpoint_body)).await;
    assert!(st == 200 || st == 201, "create endpoint, got {st}: {v}");

    // 落库值必须是 marker，且 GET 不回显明文。
    let (st, ep) = http(port1, "GET", "/api/v1/endpoints/ep-auth", None).await;
    assert_eq!(st, 200);
    let stored = ep["connection"].to_string();
    assert!(
        stored.contains("secret_set"),
        "落库连接必须是 Secret marker，实际 {stored}"
    );
    assert!(
        !stored.contains("s3cr3t"),
        "REST 不得回显明文口令，实际 {stored}"
    );

    let (st, v) = http(
        port1,
        "PUT",
        "/api/v1/tasks/ep-auth",
        Some(&poll_task_json("k.time")),
    )
    .await;
    assert_eq!(st, 200, "put tasks: {v}");
    let (st, v) = http(port1, "POST", "/api/v1/endpoints/ep-auth/start", None).await;
    assert_eq!(st, 200, "start: {v}");

    // 必须真的采到 GOOD/CURRENT 值（证明认证通过且读到了设备数据）。
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let (t1, epoch1, point_id1) = loop {
        let (st, latest) = http(port1, "GET", "/api/v1/points/latest", None).await;
        assert_eq!(st, 200);
        if let Some(p) = find_point(&latest, "k.time") {
            let good = p["quality"] == "GOOD" && p["value_origin"] == "CURRENT";
            if good {
                let (st, state) = http(port1, "GET", "/api/v1/endpoints/ep-auth/state", None).await;
                assert_eq!(st, 200);
                break (
                    p["timestamp_ns"].as_i64().expect("timestamp_ns"),
                    state["epoch"].as_u64().expect("epoch"),
                    p["point_id"].as_u64().expect("point_id"),
                );
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "phase1 未在 40s 内采到 GOOD/CURRENT 数据（fixture 认证可能未生效）"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(
        find_point(
            &http(port1, "GET", "/api/v1/points/latest", None).await.1,
            "k.time"
        )
        .unwrap()["type"],
        "datetime_ns",
        "CurrentTime 必须是 DATETIME 类型"
    );
    assert_ne!(epoch1, 0, "phase1 必须记录到非零 epoch");

    // 负例：错误口令必须**连不上**，用它反证 fixture 确实强制了口令
    //（否则上面的"认证成功"可能只是匿名通道放行）。
    //
    // NOTE: 实测该端点的 detail 为 `[Timeout/Connect] 等待连接超时`（status=None）
    // 而非预期的 `BadIdentityTokenRejected`：服务端拒绝身份后客户端并未把该
    // status 透出到 detail，最终表现为连接等待超时。因此本断言只证明
    // 「错误凭据拿不到数据」，**不**声称错误码已精确映射——那是 OPC UA
    // 错误透传的可诊断性缺口，属独立问题，未在本轮改动。
    let bad_body = format!(
        r#"{{"id":"ep-bad","name":"Bad","device_id":"d-auth","driver_id":"opcua","connection":{{"endpoint_url":"{url}","username":"mesa","password":"wrong-password"}}}}"#
    );
    let (st, _) = http(port1, "POST", "/api/v1/endpoints", Some(&bad_body)).await;
    assert!(st == 200 || st == 201, "create bad endpoint, got {st}");
    let (st, _) = http(
        port1,
        "PUT",
        "/api/v1/tasks/ep-bad",
        Some(&poll_task_json("k.time.bad")),
    )
    .await;
    assert_eq!(st, 200);
    let (st, _) = http(port1, "POST", "/api/v1/endpoints/ep-bad/start", None).await;
    assert_eq!(st, 200);

    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        let (st, state) = http(port1, "GET", "/api/v1/endpoints/ep-bad/state", None).await;
        assert_eq!(st, 200);
        let detail = state["detail"].as_str().unwrap_or_default().to_string();
        if detail.contains("CONNECT_FAILED") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "错误口令必须认证失败（detail 应含 CONNECT_FAILED），实际 state={state}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // 错误口令的端点不得产出 GOOD 数据。
    let (_, latest) = http(port1, "GET", "/api/v1/points/latest", None).await;
    if let Some(p) = find_point(&latest, "k.time.bad") {
        assert_ne!(
            p["quality"], "GOOD",
            "认证失败的端点不得产出 GOOD 数据：{p}"
        );
    }

    // ---- 非优雅 kill → 同 db 目录重启
    mesad1.kill().await.unwrap();
    let _ = mesad1.wait().await;

    let port2 = free_port().await;
    assert_ne!(port1, port2);
    let mut mesad2 = spawn_mesad(&dir, port2, _staged.path(), &pki_dir);
    wait_ready(port2).await;

    // 恢复：desired_running=true 的端点必须重新认证并再次采到数据。
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let (t2, epoch2, point_id2) = loop {
        let (st, state) = http(port2, "GET", "/api/v1/endpoints/ep-auth/state", None).await;
        assert_eq!(st, 200);
        if state["state"] == "RUNNING" {
            let (st, latest) = http(port2, "GET", "/api/v1/points/latest", None).await;
            assert_eq!(st, 200);
            if let Some(p) = find_point(&latest, "k.time") {
                let fresh = p["quality"] == "GOOD"
                    && p["value_origin"] == "CURRENT"
                    && p["timestamp_ns"].as_i64().unwrap_or(i64::MIN) > t1;
                if fresh {
                    break (
                        p["timestamp_ns"].as_i64().unwrap(),
                        state["epoch"].as_u64().unwrap(),
                        p["point_id"].as_u64().unwrap(),
                    );
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "重启后 60s 内未恢复出新采集数据（只回到 RUNNING 不算通过），state={state:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    assert!(
        epoch2 > epoch1,
        "重启必须产生新的 stream epoch（epoch1={epoch1} epoch2={epoch2}）"
    );
    assert_eq!(
        point_id2, point_id1,
        "重启后 point_id 必须稳定（PointMap 持久化）"
    );
    assert!(t2 > t1, "新采样时间戳必须晚于重启前（t1={t1} t2={t2}）");

    // 不产生重复 Endpoint。
    let (_, eps) = http(port2, "GET", "/api/v1/endpoints", None).await;
    let auth_count = eps["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["id"] == "ep-auth")
        .count();
    assert_eq!(auth_count, 1, "重启不得产生重复 Endpoint");

    // 口令仍不得泄露（REST / 错误 detail 都不含明文）。
    let (_, ep2) = http(port2, "GET", "/api/v1/endpoints/ep-auth", None).await;
    assert!(
        !ep2.to_string().contains("s3cr3t"),
        "重启后 REST 仍不得回显明文口令"
    );
    let (_, bad_state) = http(port2, "GET", "/api/v1/endpoints/ep-bad/state", None).await;
    assert!(
        !bad_state.to_string().contains("wrong-password"),
        "认证失败详情不得回显口令"
    );

    mesad2.kill().await.unwrap();
    let _ = mesad2.wait().await;
    fixture.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}
