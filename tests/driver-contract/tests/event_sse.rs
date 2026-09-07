//! SSE Live/Replay 契约测试（PR7 v1.1 §18）。
//!
//! 不引入 HTTP 客户端依赖：裸 TCP 发 GET，自行按 `\n\n` 切 SSE 帧。
//! 覆盖：live-only 默认、after_seq/Last-Event-ID/max 规则、replay→live
//! 无重复、Lagged DB catch-up、非法游标 400、无 Store 503。

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{EventBatch, EventRecord, Value};
use mesa_event_store::{CommitRequest, EVENT_HUB_CAPACITY, EventHub, EventServices, EventStore};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use common::*;

fn tmp_db(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "mesa-event-sse-{}-{}-{tag}.db",
        std::process::id(),
        mesa_core_types::now_unix_ns()
    ));
    p
}

fn seed_record(id: &str, n: i32) -> EventRecord {
    EventRecord {
        event_id: id.into(),
        category: "alarm".into(),
        kind: "alarm.condition".into(),
        source: "Channel1".into(),
        severity: 700,
        code: None,
        message: None,
        message_locale: None,
        occurred_at_ns: None,
        condition: None,
        correlation_id: None,
        attributes: BTreeMap::from([("n".into(), Value::I32(n))]),
    }
}

async fn commit(
    store: &Arc<EventStore>,
    hub: &Arc<EventHub>,
    endpoint: &str,
    id: &str,
    n: i32,
    seq: u64,
) -> i64 {
    // ingress 语义：commit 成功后才 publish（commit-then-publish）
    let res = store
        .commit_batch(CommitRequest {
            endpoint_id: endpoint.into(),
            batch: EventBatch {
                connection_handle: 1,
                stream_epoch: 0xE200,
                sequence: seq,
                timestamp_ns: 1_700_000_000_000_000_000,
                events: vec![seed_record(id, n)],
                mono_ns: None,
            },
            received_at_ns: 1_700_000_000_000_000_001,
        })
        .await
        .unwrap();
    assert_eq!(res.inserted.len(), 1);
    let stored = res.inserted.into_iter().next().unwrap();
    let db_seq = stored.seq;
    hub.publish(&stored);
    db_seq
}

struct TestServer {
    port: u16,
    _handle: tokio::task::JoinHandle<()>,
}

async fn serve(store: Arc<EventStore>, hub: Arc<EventHub>) -> TestServer {
    serve_with_services(EventServices::new(store, hub)).await
}

async fn serve_with_services(services: Arc<EventServices>) -> TestServer {
    let drivers_dir = repo_root().join("drivers");
    let cfg = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    #[allow(deprecated)]
    let state = mesa_core_api::AppState::new(mgr, cfg, drivers_dir.to_string_lossy().to_string());
    state.set_event_services(services);
    let app = mesa_core_api::router(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestServer {
        port,
        _handle: handle,
    }
}

struct SseClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    // 必须持有写半部：drop 即 FIN 半关闭，hyper 会因此杀掉未完成的
    // SSE 响应连接（头已发 + 随即 FIN 正是本故障的症状）。
    _wr: tokio::net::tcp::OwnedWriteHalf,
}

#[derive(Debug)]
struct SseFrame {
    id: Option<String>,
    event: Option<String>,
    data: String,
}

impl SseClient {
    async fn connect(port: u16, path: &str, extra_headers: &[(&str, &str)]) -> (u16, Self) {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (rd, mut wr) = stream.into_split();
        let mut req =
            format!("GET {path} HTTP/1.1\r\nhost: localhost\r\naccept: text/event-stream\r\n");
        for (k, v) in extra_headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        wr.write_all(req.as_bytes()).await.unwrap();
        // NOTE: wr 不在此 drop——见 _wr 字段注释（半关闭杀 SSE 连接）。
        let mut reader = BufReader::new(rd);
        // 状态行 + 头
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let status: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        loop {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line == "\n" || line.is_empty() {
                break;
            }
        }
        (status, Self { reader, _wr: wr })
    }

    /// 读一帧（跳过 `:ping` 等注释块；返回 None 表连接关闭）。
    async fn next_frame(&mut self) -> Option<SseFrame> {
        let mut id = None;
        let mut event = None;
        let mut data = Vec::new();
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await.ok()?;
            if n == 0 {
                return None;
            }
            let t = line.trim_end_matches(['\r', '\n']);
            if t.is_empty() {
                if data.is_empty() && id.is_none() && event.is_none() {
                    continue; // 纯注释块，继续等下一帧
                }
                return Some(SseFrame {
                    id,
                    event,
                    data: data.join("\n"),
                });
            }
            if t.starts_with(':') {
                continue;
            }
            // HTTP chunked 帧头（hex 长度行）：SSE body 经 chunked 传输，
            // 长度行与块尾 `\r\n` 都不是帧内容，显式跳过（不靠"恰好落空行"）。
            if !t.is_empty() && t.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            if let Some(v) = t.strip_prefix("id:") {
                id = Some(v.trim().to_string());
            } else if let Some(v) = t.strip_prefix("event:") {
                event = Some(v.trim().to_string());
            } else if let Some(v) = t.strip_prefix("data:") {
                // axum data() 单行（JSON 无换行）；多行 data 追加
                let v = v.strip_prefix(' ').unwrap_or(v);
                data.push(v.to_string());
            }
        }
    }

    async fn next_frame_timeout(&mut self, secs: u64) -> SseFrame {
        tokio::time::timeout(Duration::from_secs(secs), self.next_frame())
            .await
            .expect("SSE frame timeout")
            .expect("stream alive")
    }
}

/// 普通 JSON GET（裸 TCP；`GET /api/v1/events/stats` 等诊断接口用）。
/// 返回 `(status, body_json)`。
async fn get_json(port: u16, path: &str) -> (u16, serde_json::Value) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut buf)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&buf);
    let status: u16 = text
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    // 最后一个 `\r\n\r\n` 之后是 body（chunked 传输时先解码）
    let body = text.split("\r\n\r\n").last().unwrap_or("");
    let body = if text.contains("transfer-encoding: chunked") {
        let mut out = String::new();
        let mut rest = body;
        loop {
            let end = rest.find("\r\n").unwrap_or(rest.len());
            let size = usize::from_str_radix(rest[..end].trim(), 16).unwrap_or(0);
            if size == 0 {
                break;
            }
            rest = &rest[end + 2..];
            out.push_str(&rest[..size.min(rest.len())]);
            rest = &rest[size.min(rest.len())..];
            rest = rest.strip_prefix("\r\n").unwrap_or(rest);
        }
        out
    } else {
        body.to_string()
    };
    (status, serde_json::from_str(&body).unwrap())
}

/// live-only 默认：无游标不灌历史，首帧即新提交行。
#[tokio::test]
async fn sse_live_only_by_default() {
    common::init_log();
    let db = tmp_db("live");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    for i in 0..3 {
        commit(&store, &hub, "ep", &format!("old-{i}"), i, (i + 1) as u64).await;
    }
    let _srv = serve(store.clone(), hub.clone()).await;

    let (status, mut cli) = SseClient::connect(_srv.port, "/api/v1/events/live", &[]).await;
    assert_eq!(status, 200);
    // 连接后再提交：首帧必须是新行（旧 3 行不得重灌）
    let s4 = commit(&store, &hub, "ep", "new-3", 3, 4).await;
    let f = cli.next_frame_timeout(5).await;
    assert_eq!(f.id.as_deref(), Some(s4.to_string()).as_deref());
    assert_eq!(f.event.as_deref(), Some("mesa-event"));
    let v: serde_json::Value = serde_json::from_str(&f.data).unwrap();
    assert_eq!(v["event"]["event_id"], "new-3");
    let _ = std::fs::remove_file(&db);
}

/// replay → live：after_seq 回放旧行，新提交行续上，无重复、无遗漏。
#[tokio::test]
async fn sse_replay_then_live_no_duplicates() {
    common::init_log();
    let db = tmp_db("replay");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let mut seqs = Vec::new();
    for i in 0..4 {
        seqs.push(commit(&store, &hub, "ep", &format!("r-{i}"), i, (i + 1) as u64).await);
    }
    let _srv = serve(store.clone(), hub.clone()).await;

    let (status, mut cli) = SseClient::connect(
        _srv.port,
        &format!("/api/v1/events/live?after_seq={}", seqs[0]),
        &[],
    )
    .await;
    assert_eq!(status, 200);
    // 回放 seq[1..=3]
    for expect in &seqs[1..] {
        let f = cli.next_frame_timeout(5).await;
        assert_eq!(f.id.as_deref(), Some(expect.to_string()).as_deref());
    }
    // live 续上，无重复
    let s5 = commit(&store, &hub, "ep", "r-4", 4, 5).await;
    let f = cli.next_frame_timeout(5).await;
    assert_eq!(f.id.as_deref(), Some(s5.to_string()).as_deref());
    let _ = std::fs::remove_file(&db);
}

/// 消费者 kill → 重连恢复（PR10 commit 6 §12）：断线期间提交的行经 DB
/// replay 精确补齐（无重复、无遗漏），之后 live 续上。hub 无订阅者时在途
/// 通知可丢——恢复的唯一真相是 DB + Last-Event-ID 游标。
#[tokio::test]
async fn sse_consumer_kill_reconnects_with_last_event_id() {
    common::init_log();
    let db = tmp_db("kill");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let mut seqs = Vec::new();
    for i in 0..3 {
        seqs.push(commit(&store, &hub, "ep", &format!("k-{i}"), i, (i + 1) as u64).await);
    }
    let _srv = serve(store.clone(), hub.clone()).await;

    // 首连：after_seq=0 回放全部 3 行，记住末帧游标
    let (status, mut cli) =
        SseClient::connect(_srv.port, "/api/v1/events/live?after_seq=0", &[]).await;
    assert_eq!(status, 200);
    let mut last = 0i64;
    for expect in &seqs {
        let f = cli.next_frame_timeout(5).await;
        assert_eq!(f.id.as_deref(), Some(expect.to_string()).as_deref());
        last = expect.to_string().parse().unwrap();
    }
    // kill 消费者（drop 即关连接），断线期间提交 3 行（hub 无人听）
    drop(cli);
    let mut missed = Vec::new();
    for i in 3..6 {
        missed.push(commit(&store, &hub, "ep", &format!("k-{i}"), i, (i + 1) as u64).await);
    }
    // 重连带 Last-Event-ID：必须精确补齐断线 3 行（不多不少），随后 live 续上
    let (status, mut cli2) = SseClient::connect(
        _srv.port,
        "/api/v1/events/live",
        &[("Last-Event-ID", &last.to_string())],
    )
    .await;
    assert_eq!(status, 200);
    for expect in &missed {
        let f = cli2.next_frame_timeout(5).await;
        assert_eq!(f.id.as_deref(), Some(expect.to_string()).as_deref());
        let v: serde_json::Value = serde_json::from_str(&f.data).unwrap();
        assert!(v["event"]["event_id"].as_str().unwrap().starts_with("k-"));
    }
    let s7 = commit(&store, &hub, "ep", "k-6", 6, 7).await;
    let f = cli2.next_frame_timeout(5).await;
    assert_eq!(f.id.as_deref(), Some(s7.to_string()).as_deref());
    let _ = std::fs::remove_file(&db);
}

/// 诊断契约（PR10 commit 7 §14）：真 ingress 流量后，
/// `GET /events/stats` 键集完整（16 键冻结）且计数值与 DB 自洽
///（persisted == stored_rows == 实际行数，batches ≥ persisted）。
/// dup/gap/collision 等计数器的接线由 lifecycle torture 的诊断增量门
/// 直接锁定；此处锁暴露形状 + 基本自洽。零生产改动。
#[tokio::test]
async fn events_stats_contract_keys_and_values() {
    common::init_log();
    let db = tmp_db("stats");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let services = EventServices::new(store.clone(), hub.clone());

    // 真 ingress 流量：manager + Sim counter（与 pressure 同形状），
    // 计数器落在同一个 services Arc 上，stats 读同一份。
    let drivers_dir = repo_root().join("drivers");
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    mgr.set_event_services(Arc::clone(&services));
    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    mgr.start_endpoint(mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: "hd-stats".into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![],
        event_tasks: vec![mesa_core_types::EventTask {
            id: "cnt".into(),
            mode: mesa_core_types::TaskMode::Poll,
            interval_ms: Some(50),
            binding: mesa_core_types::DriverBinding {
                kind: mesa_core_types::GENERIC_EVENT_BINDING_KIND.into(),
                config: serde_json::to_value(&binding).unwrap(),
            },
        }],
    })
    .unwrap();
    // 等 ≥20 行落盘（真流量证据），再停 endpoint 冻结计数器
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let rows = loop {
        let n = store
            .query_history(&mesa_event_store::EventFilter {
                endpoint_id: Some("hd-stats".into()),
                limit: Some(500),
                ..Default::default()
            })
            .unwrap()
            .0
            .len();
        if n >= 20 {
            break n as u64;
        }
        assert!(std::time::Instant::now() < deadline, "30s 内行数不足 20");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(mgr.stop_endpoint("hd-stats").await, Ok(true));

    // stats 读同一 services：键集冻结 + 值自洽
    let _srv = serve_with_services(Arc::clone(&services)).await;
    let (status, v) = get_json(_srv.port, "/api/v1/events/stats").await;
    assert_eq!(status, 200);
    let obj = v.as_object().expect("stats 必须是 JSON 对象");
    let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "ingress_batch_duplicates_total",
            "ingress_batches_total",
            "ingress_collisions_total",
            "ingress_event_duplicates_total",
            "ingress_gaps_total",
            "ingress_invalid_total",
            "ingress_persisted_events_total",
            "ingress_regressions_total",
            "ingress_store_failures_total",
            "live_clients",
            "retention_purged_total",
            "sse_lagged_total",
            "sse_reconcile_total",
            "sse_replay_frames_total",
            "stored_rows",
            "stored_size_bytes",
        ],
        "stats 键集冻结，增删都必须显式评审"
    );
    assert_eq!(v["ingress_persisted_events_total"], rows);
    assert_eq!(v["stored_rows"], rows);
    assert!(v["ingress_batches_total"].as_u64().unwrap() >= rows);
    assert_eq!(v["ingress_store_failures_total"], 0);
    assert!(v["stored_size_bytes"].as_u64().unwrap() > 0);
    let _ = std::fs::remove_file(&db);
}

/// 游标合并：max(query.after_seq, Last-Event-ID)；非法 header 400。
#[tokio::test]
async fn sse_cursor_max_rule_and_bad_header() {
    common::init_log();
    let db = tmp_db("cursor");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let mut seqs = Vec::new();
    for i in 0..4 {
        seqs.push(commit(&store, &hub, "ep", &format!("c-{i}"), i, (i + 1) as u64).await);
    }
    let _srv = serve(store.clone(), hub.clone()).await;

    // query=seqs[0] + header=seqs[2] → 从 seqs[3] 开始（max 生效）
    let (status, mut cli) = SseClient::connect(
        _srv.port,
        &format!("/api/v1/events/live?after_seq={}", seqs[0]),
        &[("Last-Event-ID", &seqs[2].to_string())],
    )
    .await;
    assert_eq!(status, 200);
    let f = cli.next_frame_timeout(5).await;
    assert_eq!(f.id.as_deref(), Some(seqs[3].to_string()).as_deref());

    // 非法 header → 400（不静默当 absent）
    let (status, _) = SseClient::connect(
        _srv.port,
        "/api/v1/events/live",
        &[("Last-Event-ID", "abc")],
    )
    .await;
    assert_eq!(status, 400);
    let _ = std::fs::remove_file(&db);
}

/// P0-1：多 ingress 全局乱序 → DB 补齐。
/// 两行都已提交（hub 外），但按 row2 → row1 倒序 publish：
/// 无论服务端调度如何交错，输出必须是 1,2 各一次（hub 顺序≠交付顺序）。
#[tokio::test]
async fn sse_hub_reorder_recovers_from_db() {
    common::init_log();
    // 空库连接（high-water=0），再入库、再倒序 publish
    let db = tmp_db("reorder");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let _srv = serve(store.clone(), hub.clone()).await;
    let (status, mut cli) = SseClient::connect(_srv.port, "/api/v1/events/live", &[]).await;
    assert_eq!(status, 200);
    // 空库 high-water=0；两行入库（不 publish），再倒序 publish
    let mut stored = Vec::new();
    for (id, n, seq) in [("o-1", 1, 1u64), ("o-2", 2, 2u64)] {
        let res = store
            .commit_batch(CommitRequest {
                endpoint_id: "ep".into(),
                batch: EventBatch {
                    connection_handle: 1,
                    stream_epoch: 0xE200,
                    sequence: seq,
                    timestamp_ns: 1_700_000_000_000_000_000,
                    events: vec![seed_record(id, n)],
                    mono_ns: None,
                },
                received_at_ns: 1_700_000_000_000_000_001,
            })
            .await
            .unwrap();
        stored.push(res.inserted.into_iter().next().unwrap());
    }
    // 倒序 publish：row2 先，row1 后（同步连发，服务端任何交错下结果必须一致）
    hub.publish(&stored[1]);
    hub.publish(&stored[0]);
    let f1 = cli.next_frame_timeout(5).await;
    let f2 = cli.next_frame_timeout(5).await;
    assert_eq!(f1.id.as_deref(), Some("1"));
    assert_eq!(f2.id.as_deref(), Some("2"));
    let _ = std::fs::remove_file(&db);
}

/// Lagged → DB catch-up（确定性）：replay 期服务端零 hub 轮询。
/// 2000 行初始 replay 在客户端未读时不可能完成（远超 socket 缓冲），
/// 此时 publish 的 10 行必在 hub（cap=4）溢出 → Lagged → DB 补齐。
/// 断言 2010 帧精确升序全覆盖（少一行/错一序即失败）。
#[tokio::test]
async fn sse_lagged_catch_up_from_db() {
    common::init_log();
    let db = tmp_db("lag");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    let hub = EventHub::new(4);
    // 2000 行大载荷直接入库（不 publish）：约 8MB，任何默认 socket 缓冲都
    // 一次吃不完，保证 replay 期服务端不进入 live（即零 hub 轮询）。
    let big = "x".repeat(4096);
    for i in 0..2000 {
        let mut r = seed_record(&format!("bulk-{i}"), i);
        r.message = Some(big.clone());
        store
            .commit_batch(CommitRequest {
                endpoint_id: "ep".into(),
                batch: EventBatch {
                    connection_handle: 1,
                    stream_epoch: 0xE200,
                    sequence: (i + 1) as u64,
                    timestamp_ns: 1_700_000_000_000_000_000,
                    events: vec![r],
                    mono_ns: None,
                },
                received_at_ns: 1_700_000_000_000_000_001,
            })
            .await
            .unwrap();
    }
    let _srv = serve(store.clone(), hub.clone()).await;
    // after_seq=0 → replay (0,2000]；一帧不读（replay 不可能完成）
    let (status, mut cli) =
        SseClient::connect(_srv.port, "/api/v1/events/live?after_seq=0", &[]).await;
    assert_eq!(status, 200);
    // replay 期内 publish 10 行：必在 hub 溢出（cap=4，服务端零轮询）
    for i in 0..10 {
        commit(
            &store,
            &hub,
            "ep",
            &format!("l-{i}"),
            10_000 + i,
            2001 + i as u64,
        )
        .await;
    }
    // 读完全部 2010 帧：精确升序、无遗漏、无重复
    let mut prev = 0i64;
    for expect in 1..=2010i64 {
        let f = cli.next_frame_timeout(30).await;
        let seq: i64 = f.id.unwrap().parse().unwrap();
        assert_eq!(seq, expect, "第 {expect} 帧必须恰为 seq={expect}");
        assert!(seq > prev);
        prev = seq;
    }
    // branch contract（⑧c）：程序级证明 Lagged 分支确实走过，
    // 而非"8MB 超 socket 缓冲"的环境假设；replay 帧计数同步断言。
    let (status, stats) = get_json(_srv.port, "/api/v1/events/stats").await;
    assert_eq!(status, 200);
    assert!(
        stats["sse_lagged_total"].as_u64().unwrap() >= 1,
        "必须至少命中一次 Lagged，stats={stats}"
    );
    assert_eq!(
        stats["sse_replay_frames_total"].as_u64().unwrap(),
        2010,
        "DB replay 帧总数必须恰为 2010，stats={stats}"
    );
    assert_eq!(stats["stored_rows"].as_u64().unwrap(), 2010);
    // P1-1：诊断契约锁形——新增计数键必须存在（本测试无 ingress，
    // ingress_* 为 0；聚合正确性由单测 ingress_cancel_drains_backlog 断言）。
    for key in [
        "ingress_batches_total",
        "ingress_persisted_events_total",
        "ingress_batch_duplicates_total",
        "ingress_event_duplicates_total",
        "ingress_gaps_total",
        "ingress_regressions_total",
        "ingress_collisions_total",
        "ingress_invalid_total",
        "ingress_store_failures_total",
        "retention_purged_total",
        "live_clients",
    ] {
        assert!(
            stats.get(key).and_then(|v| v.as_u64()).is_some(),
            "stats 缺键 {key}：{stats}"
        );
    }
    let _ = std::fs::remove_file(&db);
}
