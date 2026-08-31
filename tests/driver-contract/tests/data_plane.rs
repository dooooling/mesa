//! §21 数据面语义：质量注入（DataBatch Semantics 扩展）与背压（Backpressure）。

mod common;

use std::time::Duration;

use mesa_core_types::Quality;
use mesa_driver_manager::session::Session;

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
            serde_json::json!({
                "points": [
                    // 静态 BAD
                    {"key":"q.bad","kind":"constant","value":1,"quality":"BAD"},
                    // 第 3 批起坏，第 6 批起恢复
                    {"key":"q.cycle","kind":"counter","bad_after_batches":3,"good_again_after":6}
                ]
            }),
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
    // burst 注入保证产出速率与 OS 定时器精度无关：
    // 20ms tick × burst 200 ≈ 10000 批/s，700ms 停滞 ≈ 7000 批
    // 远超 出站队列(256) + Core 事件队列(1024) 的总吸收量。
    let descriptors = configure_tasks(
        &session,
        H,
        1,
        &[poll_task(
            "fast",
            20,
            serde_json::json!({
                "burst": 200,
                "points": [
                    {"key":"p.0","kind":"counter","step":1},
                    {"key":"p.1","kind":"counter","step":1},
                    {"key":"p.2","kind":"counter","step":1},
                    {"key":"p.3","kind":"random"}
                ]
            }),
        )],
    )
    .await;
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
    assert_eq!(meta.unwrap().unwrap().0, "simulator");

    // 恢复消费：批次继续到达且出现 sequence 缺口（丢弃/合并的可观测证据）。
    // 积压按序排队，缺口位于事件队列容量(~1024)之后，需扫描足够深。
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
