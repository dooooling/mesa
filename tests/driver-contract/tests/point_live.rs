//! Point Live Stream 契约（STATE STREAM，不是 EVENT STREAM）。
//!
//! 不引入 HTTP 客户端依赖：裸 TCP 发 GET，自行切 SSE 帧（与 event_sse.rs 同手法）。
//! 覆盖：首帧 snapshot、空库、DataBatch→delta、同点合并、多点合并、
//! 断线 BAD、改名、删除 resync、重连 fresh snapshot、无 id/seq。
//!
//! 语义冻结：Snapshot.latest 是唯一真值；SSE 只发通知的最终值；
//! Lagged/结构变化即 Resync（不补 backlog）；不断言 Event Plane 任何行为。

mod common;

use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{
    DataBatch, DataType, PointDefinition, PointValue, Quality, Value, ValueOrigin,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use common::*;

#[derive(Debug)]
struct PointFrame {
    event: Option<String>,
    data: String,
}

struct PointSseClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    _wr: tokio::net::tcp::OwnedWriteHalf,
}

impl PointSseClient {
    async fn connect(port: u16, path: &str) -> (u16, Self) {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (rd, mut wr) = stream.into_split();
        let req =
            format!("GET {path} HTTP/1.1\r\nhost: localhost\r\naccept: text/event-stream\r\n\r\n");
        wr.write_all(req.as_bytes()).await.unwrap();
        let mut reader = BufReader::new(rd);
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

    async fn next_frame(&mut self) -> Option<PointFrame> {
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
                if data.is_empty() && event.is_none() {
                    continue;
                }
                return Some(PointFrame {
                    event,
                    data: data.join("\n"),
                });
            }
            if t.starts_with(':') {
                continue;
            }
            if !t.is_empty() && t.chars().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            if t.starts_with("id:") {
                // Point SSE 绝不能发 id（state stream 无 seq）。
                panic!("point live frame must not carry id: {t}");
            } else if let Some(v) = t.strip_prefix("event:") {
                event = Some(v.trim().to_string());
            } else if let Some(v) = t.strip_prefix("data:") {
                let v = v.strip_prefix(' ').unwrap_or(v);
                data.push(v.to_string());
            }
        }
    }

    async fn next_frame_timeout(&mut self, secs: u64) -> PointFrame {
        tokio::time::timeout(Duration::from_secs(secs), self.next_frame())
            .await
            .expect("point SSE frame timeout")
            .expect("stream alive")
    }
}

struct PointTestServer {
    port: u16,
    _handle: tokio::task::JoinHandle<()>,
    snapshot: Arc<mesa_driver_manager::Snapshot>,
}

async fn serve_points() -> PointTestServer {
    let drivers_dir = repo_root().join("drivers");
    let cfg = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    #[allow(deprecated)]
    let state = mesa_core_api::AppState::new(mgr, cfg, drivers_dir.to_string_lossy().to_string());
    let snapshot = state.snapshot.clone();
    let app = mesa_core_api::router(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    PointTestServer {
        port,
        _handle: handle,
        snapshot,
    }
}

fn defs() -> Vec<PointDefinition> {
    vec![
        PointDefinition {
            point_id: 1,
            point_key: "a".into(),
            data_type: DataType::F64,
            unit: None,
            source_label: Some("src.a".into()),
            display_name: None,
        },
        PointDefinition {
            point_id: 2,
            point_key: "b".into(),
            data_type: DataType::F64,
            unit: None,
            source_label: None,
            display_name: None,
        },
    ]
}

fn batch_vals(vals: Vec<(u32, f64, Quality)>, ts: i64) -> DataBatch {
    DataBatch {
        connection_handle: 1,
        stream_epoch: 1,
        sequence: 1,
        timestamp_ns: ts,
        values: vals
            .into_iter()
            .map(|(pid, v, q)| PointValue {
                point_id: pid,
                value: Value::F64(v),
                quality: q,
                quality_code: None,
                source_timestamp_ns: None,
                value_origin: ValueOrigin::Current,
            })
            .collect(),
        mono_ns: None,
    }
}

fn apply(snap: &Arc<mesa_driver_manager::Snapshot>, ep: &str, b: &DataBatch) {
    snap.apply_batch(b, ep);
}

fn points_of(frame: &PointFrame) -> Vec<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(&frame.data).unwrap();
    v.get("points")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn point_live_empty_snapshot_ok() {
    // 空库首帧即 {points:[]} 合法 snapshot。
    let srv = serve_points().await;
    let (status, mut cli) = PointSseClient::connect(srv.port, "/api/v1/points/live").await;
    assert_eq!(status, 200);
    let f = cli.next_frame_timeout(5).await;
    assert_eq!(f.event.as_deref(), Some("mesa-points-snapshot"));
    assert_eq!(points_of(&f).len(), 0);
}

#[tokio::test]
async fn point_live_batch_then_delta_with_final_value() {
    let srv = serve_points().await;
    srv.snapshot.register_points("ep1", &defs());
    let (status, mut cli) = PointSseClient::connect(srv.port, "/api/v1/points/live").await;
    assert_eq!(status, 200);
    // 首帧 snapshot（尚无值，register 不发 Upsert 但首帧全量；points 为空）。
    let f0 = cli.next_frame_timeout(5).await;
    assert_eq!(f0.event.as_deref(), Some("mesa-points-snapshot"));
    // 同一点 200ms 内 3 次变化 → delta 只带最终值（latest-wins）。
    let b1 = batch_vals(vec![(1, 1.0, Quality::Good)], 100);
    let b2 = batch_vals(vec![(1, 2.0, Quality::Good)], 200);
    let b3 = batch_vals(vec![(1, 3.0, Quality::Good)], 300);
    apply(&srv.snapshot, "ep1", &b1);
    apply(&srv.snapshot, "ep1", &b2);
    apply(&srv.snapshot, "ep1", &b3);
    let f1 = cli.next_frame_timeout(5).await;
    assert_eq!(f1.event.as_deref(), Some("mesa-points-delta"));
    let pts = points_of(&f1);
    assert_eq!(pts.len(), 1);
    assert_eq!(pts[0]["point_id"], 1);
    assert_eq!(pts[0]["value"], 3.0);
    // delta 行是完整 LatestEntry（source_label 同在，非 patch）。
    assert_eq!(pts[0]["source_label"], "src.a");
}

#[tokio::test]
async fn point_live_multi_point_single_frame() {
    let srv = serve_points().await;
    srv.snapshot.register_points("ep1", &defs());
    let (_, mut cli) = PointSseClient::connect(srv.port, "/api/v1/points/live").await;
    let _ = cli.next_frame_timeout(5).await;
    let b = batch_vals(vec![(1, 1.0, Quality::Good), (2, 2.0, Quality::Good)], 100);
    apply(&srv.snapshot, "ep1", &b);
    let f = cli.next_frame_timeout(5).await;
    assert_eq!(f.event.as_deref(), Some("mesa-points-delta"));
    let pts = points_of(&f);
    assert_eq!(pts.len(), 2);
}

#[tokio::test]
async fn point_live_comm_lost_and_rename_are_delta() {
    let srv = serve_points().await;
    srv.snapshot.register_points("ep1", &defs());
    let b = batch_vals(vec![(1, 1.0, Quality::Good)], 100);
    apply(&srv.snapshot, "ep1", &b);
    let (_, mut cli) = PointSseClient::connect(srv.port, "/api/v1/points/live").await;
    // 首帧 snapshot 已含 GOOD 值。
    let f0 = cli.next_frame_timeout(5).await;
    assert_eq!(f0.event.as_deref(), Some("mesa-points-snapshot"));
    assert_eq!(points_of(&f0).len(), 1);
    // 断线 → delta BAD（不重发全量）。
    srv.snapshot.mark_communication_lost("ep1");
    let f1 = cli.next_frame_timeout(5).await;
    assert_eq!(f1.event.as_deref(), Some("mesa-points-delta"));
    assert_eq!(points_of(&f1)[0]["quality"], "BAD");
    // 改名 → delta 带新 display_name。
    srv.snapshot
        .update_display_name("ep1", 1, Some("新名".into()));
    let f2 = cli.next_frame_timeout(5).await;
    assert_eq!(f2.event.as_deref(), Some("mesa-points-delta"));
    assert_eq!(points_of(&f2)[0]["display_name"], "新名");
}

#[tokio::test]
async fn point_live_remove_endpoint_resyncs_and_reconnect_fresh() {
    let srv = serve_points().await;
    srv.snapshot.register_points("ep1", &defs());
    let b = batch_vals(vec![(1, 1.0, Quality::Good)], 100);
    apply(&srv.snapshot, "ep1", &b);
    let (_, mut cli) = PointSseClient::connect(srv.port, "/api/v1/points/live").await;
    let f0 = cli.next_frame_timeout(5).await;
    assert_eq!(points_of(&f0).len(), 1);
    // 删除 endpoint → Resync 全量（旧行消失）。
    srv.snapshot.remove_endpoint("ep1");
    let f1 = cli.next_frame_timeout(5).await;
    assert_eq!(f1.event.as_deref(), Some("mesa-points-snapshot"));
    assert_eq!(points_of(&f1).len(), 0);
    // 重连即新连接 + fresh snapshot（无 replay/seq）。
    let (_, mut cli2) = PointSseClient::connect(srv.port, "/api/v1/points/live").await;
    let g0 = cli2.next_frame_timeout(5).await;
    assert_eq!(g0.event.as_deref(), Some("mesa-points-snapshot"));
    assert_eq!(points_of(&g0).len(), 0);
}
