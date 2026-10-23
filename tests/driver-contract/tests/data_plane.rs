//! §21 数据面语义：质量注入（DataBatch Semantics 扩展）与背压（Backpressure）。
//!
//! 一期必需补充：`quality_code` 全链路穿透（驱动 → IPC → Core → REST）。
//! 此前项级码断言止于驱动层（`drivers/sinumerik-nck/src/lib.rs:1985`），
//! 没有任何用例证明同一个码经 IPC/Core 后仍是原值——本套件补上这条链。

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mesa_core_types::{DataBatch, PointDefinition, PointValue, Quality, Value};
use mesa_driver_manager::session::Session;
use mesa_driver_manager::snapshot::Snapshot;
use tower::ServiceExt;

use common::*;

const H: u32 = 1;

/// 质量注入（附录 A.4）：静态 BAD、GOOD→BAD→GOOD 转换按批次数精确生效。
#[tokio::test]
async fn quality_injection_static_and_transition() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    let descriptors = configure_tasks(
        &session,
        H,
        1,
        &[poll_task(
            "t",
            30,
            serde_json::json!([
                // 静态 BAD（quality 参数随 selection parameters 透传）
                {"resource_id":"constant","parameters":{"value":1,"quality":"BAD"},"outputs":[{"output":"value","point_key":"q.bad"}]},
                // 第 3 批起坏，第 6 批起恢复
                {"resource_id":"counter","parameters":{"bad_after_batches":3,"good_again_after":6},"outputs":[{"output":"value","point_key":"q.cycle"}]}
            ]),
        )],
    )
    .await;
    apply_point_map(&session, H, 1, sequential_ids(&descriptors, 10)).await;
    start_connection(&session, H, 1).await;

    // id 分配：q.bad=10, q.cycle=11（按描述符顺序）
    let mut bad_static = Vec::new();
    let mut cycle = Vec::new();
    for _ in 0..8 {
        let b = recv_batch(&mut events, 3).await;
        for pv in &b.values {
            match pv.point_id {
                10 => bad_static.push(pv.quality),
                11 => cycle.push((b.sequence, pv.quality)),
                other => panic!("unexpected point {other}"),
            }
        }
    }
    assert!(
        bad_static.len() >= 5 && bad_static.iter().all(|q| *q == Quality::Bad),
        "static BAD point must always report BAD, got {bad_static:?}"
    );
    assert!(
        cycle.len() >= 6,
        "must observe several batches of the cycling point"
    );
    // 转换序列：第 1-2 批 GOOD，第 3-5 批 BAD，第 6 批起恢复 GOOD
    assert_eq!(cycle[0].1, Quality::Good);
    assert_eq!(cycle[1].1, Quality::Good);
    assert_eq!(cycle[2].1, Quality::Bad);
    assert_eq!(cycle[3].1, Quality::Bad);
    assert_eq!(cycle[4].1, Quality::Bad);
    assert_eq!(
        cycle[5].1,
        Quality::Good,
        "quality must recover after good_again_after"
    );

    teardown(&mut session, Some(cancel));
}

/// Backpressure（§21 行 17 / §12）端到端观测：
/// 消费停滞超过"出站队列 + Core 事件队列"总吸收量后，批次开始被丢弃，
/// sequence 缺口成为丢弃的可观测证据；恢复消费后数据继续流动，
/// 控制面不受数据拥塞影响。合并的精确语义由 SDK 单元测试确定性覆盖。
#[tokio::test]
async fn backpressure_coalesces_and_keeps_control_responsive() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    // burst 注入保证产出速率与 OS 定时器精度无关（burst 为 GenericBinding
    // 顶层参数，与 selections 同级）：
    // 20ms tick × burst 200 ≈ 10000 批/s，700ms 停滞 ≈ 7000 批
    // 远超 出站队列(256) + Core 事件队列(512) 的总吸收量。
    let task = {
        let mut t = poll_task(
            "fast",
            20,
            serde_json::json!([
                {"resource_id":"counter","parameters":{"step":1},"outputs":[{"output":"value","point_key":"p.0"}]},
                {"resource_id":"counter","parameters":{"step":1},"outputs":[{"output":"value","point_key":"p.1"}]},
                {"resource_id":"counter","parameters":{"step":1},"outputs":[{"output":"value","point_key":"p.2"}]},
                {"resource_id":"random","parameters":{},"outputs":[{"output":"value","point_key":"p.3"}]}
            ]),
        );
        // burst 与 selections 同级（GenericBinding 顶层扩展参数）
        if let serde_json::Value::Object(ref mut m) = t.binding.config {
            m.insert("burst".into(), serde_json::json!(200));
        }
        t
    };
    let descriptors = configure_tasks(&session, H, 1, &[task]).await;
    apply_point_map(&session, H, 1, sequential_ids(&descriptors, 1)).await;
    start_connection(&session, H, 9).await;

    // 故意停滞消费：溢出丢弃必然发生
    tokio::time::sleep(Duration::from_millis(700)).await;

    // 控制面在数据洪峰下必须仍然响应（Control 消息不排队在数据合并路径上）
    let meta = tokio::time::timeout(Duration::from_secs(2), session.metadata()).await;
    assert!(
        meta.is_ok(),
        "control plane must stay responsive under data flood"
    );
    assert_eq!(meta.unwrap().unwrap().0, "test-driver");

    // 恢复消费：批次继续到达且出现 sequence 缺口（丢弃/合并的可观测证据）。
    // 积压按序排队，缺口位于事件队列容量(~512)之后，需扫描足够深。
    let mut gaps = 0u32;
    let mut last_seq = 0u64;
    let scanned_start = std::time::Instant::now();
    while scanned_start.elapsed() < Duration::from_secs(20) && gaps == 0 {
        let b = recv_batch(&mut events, 5).await;
        if last_seq > 0 && b.sequence > last_seq + 1 {
            gaps += 1;
        }
        last_seq = b.sequence;
    }
    assert!(
        gaps > 0,
        "sequence gaps must be observable after consumer stall"
    );

    teardown(&mut session, Some(cancel));
}

// ---------------------------------------------------------------------------
// quality_code 全链路（一期必需）
//
// 判据：设备原生码必须原样穿过 IPC → Core → REST，不得被 Core 的 BAD 兜底
// （`DEVICE_ERROR`）或断线覆盖（`COMMUNICATION_LOST`）替换；GOOD 行必须**没有**
// 该键（`skip_serializing_if`）。
// ---------------------------------------------------------------------------

/// 最小 REST 装配：真实 `AppState` + `router`（`Snapshot` 是唯一真值源）。
///
/// 不起驱动子进程：批次由用例经真实 IPC 收到后显式交给 `Snapshot::apply_batch`
/// ——这正是生产侧的唯一写入入口（`endpoint::apply_batch_logged` 内部同一句，
/// `crates/driver-manager/src/endpoint.rs:1136`），因此本用例覆盖的链路是
/// 驱动（SDK 服务端）→ TCP IPC → Core 解码 → Snapshot → REST 序列化。
async fn rest_app() -> (axum::Router, Arc<Snapshot>) {
    let drivers_dir = repo_root().join("drivers");
    let cfg = Arc::new(mesa_config_store::ConfigStore::open_in_memory().unwrap());
    let mgr = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
    let state = mesa_core_api::AppState::try_new_with_control(
        mgr,
        cfg,
        drivers_dir.to_string_lossy().to_string(),
        false,
    )
    .unwrap();
    let snapshot = state.snapshot.clone();
    (mesa_core_api::router(state), snapshot)
}

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn point_def(point_id: u32, key: &str) -> PointDefinition {
    PointDefinition {
        point_id,
        point_key: key.into(),
        data_type: mesa_core_types::DataType::F64,
        unit: None,
        source_label: None,
        display_name: None,
    }
}

/// T1 + T2（一期必需）：设备原生码穿透 IPC/Core/REST；GOOD 行无该键。
#[tokio::test]
async fn quality_code_native_survives_ipc_and_rest_and_good_row_has_no_key() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    let descriptors = configure_tasks(
        &session,
        H,
        1,
        &[poll_task(
            "t",
            30,
            serde_json::json!([
                // 设备原生码：NCK 的 S7 item 返回码形态（0x05）。故意不用 0/1，
                // 以免与 Core 兜底值巧合相等而让断言失去分辨力。
                {"resource_id":"constant","parameters":{"value":0,"quality":"BAD","quality_code":5},"outputs":[{"output":"value","point_key":"qc.native"}]},
                // 同任务的 GOOD 点（无码）
                {"resource_id":"constant","parameters":{"value":1},"outputs":[{"output":"value","point_key":"qc.good"}]}
            ]),
        )],
    )
    .await;
    let ids = sequential_ids(&descriptors, 10);
    apply_point_map(&session, H, 1, ids.clone()).await;
    start_connection(&session, H, 1).await;

    // ① 驱动 → IPC：批次里必须是驱动填的那个码本身（不是 0、不是 1、不是符号标记）。
    let batch = recv_batch(&mut events, 5).await;
    let bad = batch
        .values
        .iter()
        .find(|pv| pv.point_id == 10)
        .expect("native code point 必须在本批");
    assert_eq!(bad.quality, Quality::Bad);
    assert_eq!(bad.quality_code, Some(5), "IPC 解码后必须仍是设备原生码 5");
    let good = batch
        .values
        .iter()
        .find(|pv| pv.point_id == 11)
        .expect("good point 必须在本批");
    assert_eq!(good.quality, Quality::Good);
    assert_eq!(good.quality_code, None);

    // ② IPC → Core：走 Core 真实写入入口（生产为 endpoint::apply_batch_logged）。
    let (app, snapshot) = rest_app().await;
    snapshot.register_points(
        "ep1",
        &[point_def(10, "qc.native"), point_def(11, "qc.good")],
    );
    snapshot.apply_batch(&batch, "ep1");

    // ③ Core → REST：十进制字符串，逐字同一个码。
    let (status, latest) = get_json(app, "/api/v1/points/latest").await;
    assert_eq!(status, StatusCode::OK);
    let rows = latest["points"].as_array().expect("points 必须是数组");
    let bad_row = rows
        .iter()
        .find(|r| r["point_id"] == 10)
        .expect("native code row 必须在 REST 响应里");
    assert_eq!(bad_row["quality"], "BAD");
    assert_eq!(
        bad_row["quality_code"].as_str(),
        Some("5"),
        "REST 必须输出同一个设备原生码（十进制字符串）: {bad_row}"
    );
    assert_eq!(bad_row["value_origin"], "CURRENT");

    // T2：GOOD 行必须"键不存在"，不是 null、不是空串。
    let good_row = rows
        .iter()
        .find(|r| r["point_id"] == 11)
        .expect("good row 必须在 REST 响应里");
    assert_eq!(good_row["quality"], "GOOD");
    assert!(
        good_row.get("quality_code").is_none(),
        "GOOD 行不得出现 quality_code 键（skip_serializing_if）: {good_row}"
    );

    teardown(&mut session, Some(cancel));
}

/// T3（一期必需）：会话丢失时 Core 写入合成码 `COMMUNICATION_LOST` ⇒ REST 可见。
///
/// Core 侧写入点是 `Snapshot::mark_communication_lost`（生产调用点
/// `crates/driver-manager/src/endpoint.rs:443`，Lost 分支）；本用例验证该合成码
/// 写入后经 `/points/latest` 一定可观测，且不伪造新采样（值/时间戳保持旧值）。
#[tokio::test]
async fn communication_lost_code_is_visible_in_rest() {
    let (app, snapshot) = rest_app().await;
    snapshot.register_points("ep1", &[point_def(1, "k1")]);
    snapshot.apply_batch(
        &DataBatch {
            connection_handle: 1,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: mesa_core_types::now_unix_ns(),
            values: vec![PointValue::good(1, Value::F64(1.5))],
            mono_ns: None,
        },
        "ep1",
    );

    // 断线前：GOOD 行无码（同一响应形态）。
    let (_, before) = get_json(app.clone(), "/api/v1/points/latest").await;
    assert!(
        before["points"][0].get("quality_code").is_none(),
        "GOOD 行不得有码: {before}"
    );
    let ts_before = before["points"][0]["timestamp_ns"].clone();

    snapshot.mark_communication_lost("ep1");

    let (status, after) = get_json(app, "/api/v1/points/latest").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["points"][0]["quality"], "BAD");
    assert_eq!(
        after["points"][0]["quality_code"], "COMMUNICATION_LOST",
        "断线合成码必须出现在 REST 响应里: {after}"
    );
    // 断线不生成虚假采样：值与时间戳保持断线前的旧值。
    assert_eq!(after["points"][0]["value"], 1.5);
    assert_eq!(after["points"][0]["timestamp_ns"], ts_before);
}
