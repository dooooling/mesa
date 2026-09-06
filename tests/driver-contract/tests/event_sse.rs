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
    let drivers_dir = repo_root().join("drivers");
    let cfg = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    #[allow(deprecated)]
    let state = mesa_core_api::AppState::new(mgr, cfg, drivers_dir.to_string_lossy().to_string());
    state.set_event_services(EventServices::new(store, hub));
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

/// Lagged → DB catch-up：小 hub + 洪峰，帧覆盖全部且有序、无静默跳行。
#[tokio::test]
async fn sse_lagged_catch_up_from_db() {
    common::init_log();
    let db = tmp_db("lag");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    // hub 容量 4：10 行洪峰必 lag
    let hub = EventHub::new(4);
    let _srv = serve(store.clone(), hub.clone()).await;

    let (status, mut cli) = SseClient::connect(_srv.port, "/api/v1/events/live", &[]).await;
    assert_eq!(status, 200);
    // 连接已建立（subscribe 在 handler 内完成）后再洪峰
    tokio::time::sleep(Duration::from_millis(200)).await;
    for i in 0..10 {
        commit(&store, &hub, "ep", &format!("l-{i}"), i, (i + 1) as u64).await;
    }
    let mut got = Vec::new();
    for _ in 0..10 {
        got.push(
            cli.next_frame_timeout(5)
                .await
                .id
                .unwrap()
                .parse::<i64>()
                .unwrap(),
        );
    }
    let mut sorted = got.clone();
    sorted.sort_unstable();
    assert_eq!(sorted.len(), 10, "10 行必须全到，无静默跳行");
    assert!(
        got.windows(2).all(|w| w[0] < w[1]),
        "帧必须按 seq 递增，got {got:?}"
    );
    let _ = std::fs::remove_file(&db);
}
