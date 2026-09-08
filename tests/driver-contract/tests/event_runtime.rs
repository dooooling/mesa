//! Event Runtime 契约测试（Event Plane V1 PR6）。
//!
//! 覆盖 PR6 的全部 Gate：Simulator 作为 Reference Event Driver，经 SDK 三队列 +
//! IPC 1.3 + Core 独立 event_rx，把冻结契约真正跑起来。生产 Endpoint/Store/Web
//! 均不在本轮（PR7 起），E2E 断言止于 Core Session 收到 EventBatch。
//!
//! 约定：每个测试独立起进程内服务；数据面仍需先走 Open → Configure → Apply
//! （Simulator run() 要求数据计划存在），事件配置在 Start 之前下发。

mod common;
mod event_common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use event_common::{rows_of, tmp_db};
use mesa_core_types::{
    AcquisitionTask, ConditionTransition, DriverBinding, DriverMetadata, EventBatch, EventTask,
    PointDescriptor, PointMap, TaskMode,
};
use mesa_driver_manager::event_ingress::run_event_ingress;
use mesa_driver_manager::session::{EVENT_BATCH_CAPACITY, Session, SessionError};
use mesa_driver_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR, pb, read_envelope, write_envelope};
use mesa_driver_sdk::{
    DataSink, Driver, DriverConnection, SdkDriverError, SdkFaults, serve_with_faults,
};
use mesa_driver_simulator::{
    EVENT_BINDING_KIND, SIM_ALARM_CONDITION_ID, SIM_EVENT_STREAM_ALARM, SIM_EVENT_STREAM_COUNTER,
};
use mesa_event_store::{EventHub, EventServices, EventStore};
use tokio_util::sync::CancellationToken;

use common::*;

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// 构造 Simulator 事件任务（binding 语义由 Simulator 解释）。
fn sim_event_task(id: &str, stream: &str, mode: TaskMode, interval_ms: Option<u64>) -> EventTask {
    EventTask {
        id: id.into(),
        mode,
        interval_ms,
        binding: DriverBinding {
            kind: EVENT_BINDING_KIND.into(),
            config: serde_json::json!({"stream": stream}),
        },
    }
}

/// 最小数据任务（Simulator run() 的前置要求，与事件断言无关）。
fn mini_data_task() -> AcquisitionTask {
    poll_task(
        "t1",
        50,
        serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
    )
}

/// 完整闭环 + 事件配置：Open → Configure → Apply → ConfigureEvents → Start。
/// 事件必须在 Start 之前配置（运行中连接对象不在 entries，配不进去——这正是
/// Stop → Configure → Start 流程的强制体现）。
async fn configure_start_with_events(
    session: &Session,
    handle: u32,
    revision: u64,
    epoch: u64,
    event_tasks: &[EventTask],
) {
    open_connection(session, handle, "{}").await;
    let descriptors = configure_tasks(session, handle, revision, &[mini_data_task()]).await;
    let map = sequential_ids(&descriptors, 11);
    apply_point_map(session, handle, revision, map).await;
    session
        .configure_events(handle, revision, event_tasks)
        .await
        .expect("configure_events must succeed");
    start_connection(session, handle, epoch).await;
}

/// 在 secs 内收一个 EventBatch；超时或流终止都 panic（各测试按需自捕）。
/// 注意收的是带 Core 侧 epoch 门的 [`EventReceiver`]：StopAck 后的旧 epoch
/// 在此不可见（P0 barrier），测试断言的正是这道门。
async fn recv_event_batch(
    erx: &mut mesa_driver_manager::session::EventReceiver,
    secs: u64,
) -> EventBatch {
    tokio::time::timeout(Duration::from_secs(secs), erx.recv())
        .await
        .expect("timed out waiting for event batch")
        .expect("event stream must stay alive")
}

/// 报警四态的期望跃迁序列。
fn expected_transitions() -> Vec<ConditionTransition> {
    vec![
        ConditionTransition::Raised,
        ConditionTransition::Updated,
        ConditionTransition::Acknowledged,
        ConditionTransition::Cleared,
    ]
}

// ---------------------------------------------------------------------------
// Gate 1/2/14：lifecycle + 永不合并 + IPC 元数据保留
// ---------------------------------------------------------------------------

/// Raised → Updated → Acknowledged → Cleared 四条独立 occurrence 全部收到：
/// 同一 condition_id、event_id 互异、顺序正确；每批 header（handle/epoch/
/// timestamp/mono）完整保留——Event ≠ Point，生命周期不被覆盖。
#[tokio::test]
async fn event_lifecycle_four_records_received_with_metadata() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    const HANDLE: u32 = 7;
    const EPOCH: u64 = 0xE000_0001;
    configure_start_with_events(
        &session,
        HANDLE,
        1,
        EPOCH,
        &[sim_event_task(
            "al",
            SIM_EVENT_STREAM_ALARM,
            TaskMode::Subscribe,
            None,
        )],
    )
    .await;
    let mut erx = session.take_event_batches().unwrap();

    let mut records = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while records.len() < 4 && std::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(std::time::Instant::now());
        let b = tokio::time::timeout(remain, erx.recv())
            .await
            .expect("alarm cycle must complete")
            .expect("stream alive");
        // IPC 元数据（Gate 14）：一批不差
        assert_eq!(b.connection_handle, HANDLE);
        assert_eq!(b.stream_epoch, EPOCH);
        assert!(b.timestamp_ns > 0, "publish 时间必须由 SDK 盖戳");
        assert!(b.mono_ns.is_some(), "mono 埋点必须存在（IPC latency 可测）");
        records.extend(b.events);
    }
    assert_eq!(records.len(), 4, "四态必须各一条，got {records:?}");

    let transitions: Vec<ConditionTransition> = records
        .iter()
        .map(|r| {
            r.condition
                .as_ref()
                .expect("alarm 必带 condition")
                .transition
        })
        .collect();
    assert_eq!(transitions, expected_transitions());
    // 同一 condition，多条 occurrence
    for r in &records {
        assert_eq!(
            r.condition.as_ref().unwrap().condition_id,
            SIM_ALARM_CONDITION_ID
        );
        assert_eq!(r.category, "alarm");
    }
    let mut ids: Vec<&str> = records.iter().map(|r| r.event_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4, "event_id 必须互异（去重键），got {ids:?}");

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 3：sequence 严格递增、无缺口、无合并
// ---------------------------------------------------------------------------

/// Counter 流 6 批：sequence 恰为 1..=6（可靠队列无合并无丢弃），
/// 每批单记录且 event_id 互异——Latest-Wins 永远不许碰事件。
#[tokio::test]
async fn event_sequence_strictly_increasing_no_coalescing() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    configure_start_with_events(
        &session,
        7,
        1,
        0xE000_0002,
        &[sim_event_task(
            "cnt",
            SIM_EVENT_STREAM_COUNTER,
            TaskMode::Poll,
            Some(20),
        )],
    )
    .await;
    let mut erx = session.take_event_batches().unwrap();

    let mut batches = Vec::new();
    for _ in 0..6 {
        batches.push(recv_event_batch(&mut erx, 5).await);
    }
    let mut ids = Vec::new();
    for (i, b) in batches.iter().enumerate() {
        assert_eq!(b.sequence, i as u64 + 1, "sequence 必须从 1 连续");
        assert_eq!(b.events.len(), 1, "每 publish 一批 exactly 一条（不合并）");
        assert_eq!(b.events[0].kind, "counter.tick");
        ids.push(b.events[0].event_id.clone());
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 6, "6 条记录必须互异");

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// P0-2 单 handle 多 publisher：sequence 到达顺序必须严格 1,2,3...
// ---------------------------------------------------------------------------

/// 同一 connection 配两个 counter EventTask（10ms 高频并发 publish）：
/// 收到的 12 批 sequence 必须恰为 1..=12（分配→入队原子化），且 event_id
/// 全互异（task id 已纳入 ID 构造，PR7 去重键安全）。
#[tokio::test]
async fn event_single_handle_multi_publisher_stays_ordered() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, 7, "{}").await;
    let descriptors = configure_tasks(&session, 7, 1, &[mini_data_task()]).await;
    apply_point_map(&session, 7, 1, sequential_ids(&descriptors, 11)).await;
    session
        .configure_events(
            7,
            1,
            &[
                sim_event_task("cnt-a", SIM_EVENT_STREAM_COUNTER, TaskMode::Poll, Some(10)),
                sim_event_task("cnt-b", SIM_EVENT_STREAM_COUNTER, TaskMode::Poll, Some(10)),
            ],
        )
        .await
        .unwrap();
    start_connection(&session, 7, 0xE000_0701).await;
    let mut erx = session.take_event_batches().unwrap();

    let mut seqs = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..12 {
        let b = recv_event_batch(&mut erx, 5).await;
        assert_eq!(b.stream_epoch, 0xE000_0701);
        seqs.push(b.sequence);
        ids.extend(b.events.iter().map(|e| e.event_id.clone()));
    }
    assert_eq!(
        seqs,
        (1..=12u64).collect::<Vec<_>>(),
        "双 publisher 并发下 wire 顺序必须等于序号顺序"
    );
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 12, "双任务 event_id 必须互异，got {ids:?}");

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// P0 Start race：首批永不丢失（PR7 式早消费）
// ---------------------------------------------------------------------------

/// PR7 的真实消费方式：先 take EventReceiver 并常驻 recv，再 Start。
/// 若 SDK 在 Ack 入队前放行 run，首批会撞上两道门（SDK writer 门 / Core
/// EventReceiver 门）被当 stale 丢弃。连续 3 轮，每轮首批必须 epoch 对、
/// sequence == 1——seq=1 的 occurrence 永不丢失。
#[tokio::test]
async fn event_first_batch_never_lost_before_start_ack() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, 7, "{}").await;
    let descriptors = configure_tasks(&session, 7, 1, &[mini_data_task()]).await;
    apply_point_map(&session, 7, 1, sequential_ids(&descriptors, 11)).await;
    session
        .configure_events(
            7,
            1,
            &[sim_event_task(
                "cnt",
                SIM_EVENT_STREAM_COUNTER,
                TaskMode::Poll,
                Some(10),
            )],
        )
        .await
        .unwrap();
    // 早消费：Start 之前就 take 并开始等（PR7 EventIngress 常驻形态）
    let mut erx = session.take_event_batches().unwrap();

    for round in 0..3u64 {
        let epoch = 0xE000_0800 + round;
        start_connection(&session, 7, epoch).await;
        let b = recv_event_batch(&mut erx, 5).await;
        assert_eq!(b.stream_epoch, epoch, "round {round}: 首批 epoch 必须正确");
        assert_eq!(b.sequence, 1, "round {round}: 首批 sequence 必须为 1");
        assert!(stop_connection(&session, 7).await);
    }

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 4/8：新 epoch 从 1 重来 + 多次启停无状态泄漏（有界）
// ---------------------------------------------------------------------------

/// 3 次 Stop → Start：每次首批 sequence 均为 1（序号不跨 epoch 继承，
/// SDK registry 在 Stop 时清理，无泄漏、无串扰）。
#[tokio::test]
async fn event_epoch_restart_and_bounded_across_cycles() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, 7, "{}").await;
    let descriptors = configure_tasks(&session, 7, 1, &[mini_data_task()]).await;
    apply_point_map(&session, 7, 1, sequential_ids(&descriptors, 11)).await;
    session
        .configure_events(
            7,
            1,
            &[sim_event_task(
                "al",
                SIM_EVENT_STREAM_ALARM,
                TaskMode::Subscribe,
                None,
            )],
        )
        .await
        .unwrap();
    let mut erx = session.take_event_batches().unwrap();

    for cycle in 0..3u64 {
        let epoch = 0xE000_0100 + cycle;
        start_connection(&session, 7, epoch).await;
        let b = recv_event_batch(&mut erx, 5).await;
        assert_eq!(b.stream_epoch, epoch);
        assert_eq!(b.sequence, 1, "新 epoch 序号从 1 重来（cycle {cycle}）");
        assert!(stop_connection(&session, 7).await);
    }

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 5：Stop barrier——StopAck 后旧 epoch 事件为 0
// ---------------------------------------------------------------------------

/// Counter 20ms 跑两批后 Stop：800ms 内旧 epoch 不得再出现；
/// 再 Start 新 epoch 恢复正常（流本身没死，只是旧 epoch 被门掉）。
#[tokio::test]
async fn event_stop_barrier_drops_stale_epoch() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    const OLD: u64 = 0xE000_0201;
    const NEW: u64 = 0xE000_0202;
    configure_start_with_events(
        &session,
        7,
        1,
        OLD,
        &[sim_event_task(
            "cnt",
            SIM_EVENT_STREAM_COUNTER,
            TaskMode::Poll,
            Some(20),
        )],
    )
    .await;
    let mut erx = session.take_event_batches().unwrap();
    recv_event_batch(&mut erx, 5).await;
    recv_event_batch(&mut erx, 5).await;

    assert!(stop_connection(&session, 7).await);
    // StopAck 之后：排空 800ms，旧 epoch 一批都不许出现
    let deadline = std::time::Instant::now() + Duration::from_millis(800);
    while std::time::Instant::now() < deadline {
        match erx.try_recv() {
            Ok(b) => panic!("stale event survived StopAck: {b:?}"),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                panic!("stream must not die on clean stop")
            }
        }
    }
    assert!(
        !session.event_stream_failed(),
        "干净 Stop 不得触发 fail-closed"
    );

    // 新 epoch 恢复：首批 seq=1
    start_connection(&session, 7, NEW).await;
    let b = recv_event_batch(&mut erx, 5).await;
    assert_eq!(b.stream_epoch, NEW);
    assert_eq!(b.sequence, 1);

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 6/7：Event/Data 隔离 + Control 优先
// ---------------------------------------------------------------------------

/// 事件洪峰（10ms）下数据面仍推进、控制 RPC（metadata）仍及时完成：
/// Event/Data 公平交替、Control 恒优先——Alarm 风暴不饿死任何一路。
#[tokio::test]
async fn event_flood_neither_starves_data_nor_control() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    configure_start_with_events(
        &session,
        7,
        1,
        0xE000_0301,
        &[sim_event_task(
            "cnt",
            SIM_EVENT_STREAM_COUNTER,
            TaskMode::Poll,
            Some(10),
        )],
    )
    .await;
    let mut erx = session.take_event_batches().unwrap();

    // 数据面：5s 内至少 3 批（20ms 级数据任务，洪峰下不得停滞）
    let mut data_batches = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while data_batches < 3 && std::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remain, events.recv()).await {
            Ok(Some(mesa_driver_manager::session::SessionEvent::Batch(_))) => data_batches += 1,
            Ok(Some(_)) => continue,
            Ok(None) => panic!("session closed"),
            Err(_) => break,
        }
    }
    assert_eq!(data_batches, 3, "事件洪峰不得饿死数据面");

    // 事件面同步推进
    for _ in 0..3 {
        recv_event_batch(&mut erx, 5).await;
    }

    // 控制面：洪峰中 metadata RPC 必须及时完成（Control 恒优先）
    let (driver_id, _, _) = tokio::time::timeout(Duration::from_secs(8), session.metadata())
        .await
        .expect("control RPC must not starve under event flood")
        .expect("metadata ok");
    assert_eq!(driver_id, "simulator");

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 9：溢出 fail-closed（Core 侧）
// ---------------------------------------------------------------------------

/// 不消费 event_rx + 10ms 洪峰：通道槽位必满 → 流终止（fail-closed）：
/// `event_stream_failed()` 置位、overflow 计数 ≥1、排空已有缓冲后 recv 到 None，
/// 而不是静默丢弃后继续。填满时长由容量公式推导（`EVENT_BATCH_CAPACITY` /
/// 100/s + 余量），不是魔法数字。
#[tokio::test]
async fn event_overflow_terminates_stream_fail_closed() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    configure_start_with_events(
        &session,
        7,
        1,
        0xE000_0401,
        &[sim_event_task(
            "cnt",
            SIM_EVENT_STREAM_COUNTER,
            TaskMode::Poll,
            Some(10),
        )],
    )
    .await;
    // 故意不 take_event_batches：消费端已死，队列必满。
    // 填满时长 = 容量 / 到达率 + 余量（10ms 间隔即 100/s；余量覆盖调度抖动）。
    let fill_secs = EVENT_BATCH_CAPACITY as u64 / 100 + 5;
    tokio::time::sleep(Duration::from_secs(fill_secs)).await;

    assert!(session.event_stream_failed(), "溢出必须置位 fail-closed");
    assert!(session.event_overflow_drops() >= 1, "溢出必须计数可见");

    // 取走接收端：排空残留缓冲后必须到 None（流终止），不能永远阻塞
    let mut erx = session.take_event_batches().unwrap();
    let mut drained = 0u32;
    loop {
        match tokio::time::timeout(Duration::from_secs(2), erx.recv()).await {
            Ok(Some(_)) => drained += 1,
            Ok(None) => break,
            Err(_) => panic!("terminated stream must close, not hang"),
        }
    }
    assert!(drained > 0, "终止前缓冲的批次仍可排空，got {drained}");

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 10：多连接隔离
// ---------------------------------------------------------------------------

/// 双 handle 各自 epoch/sequence 独立：handle/epoch 盖戳正确、序号各从 1 起。
#[tokio::test]
async fn event_multi_connection_isolation() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    for (handle, epoch) in [(7u32, 0xE000_0507u64), (8u32, 0xE000_0508u64)] {
        open_connection(&session, handle, "{}").await;
        let descriptors = configure_tasks(&session, handle, 1, &[mini_data_task()]).await;
        apply_point_map(&session, handle, 1, sequential_ids(&descriptors, 11)).await;
        session
            .configure_events(
                handle,
                1,
                &[sim_event_task(
                    "cnt",
                    SIM_EVENT_STREAM_COUNTER,
                    TaskMode::Poll,
                    Some(30),
                )],
            )
            .await
            .unwrap();
        start_connection(&session, handle, epoch).await;
    }
    let mut erx = session.take_event_batches().unwrap();

    let mut seen7 = Vec::new();
    let mut seen8 = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    while (seen7.len() < 3 || seen8.len() < 3) && std::time::Instant::now() < deadline {
        let remain = deadline.saturating_duration_since(std::time::Instant::now());
        let b = tokio::time::timeout(remain, erx.recv())
            .await
            .expect("both connections must deliver")
            .expect("stream alive");
        match (b.connection_handle, b.stream_epoch) {
            (7, 0xE000_0507) => seen7.push(b.sequence),
            (8, 0xE000_0508) => seen8.push(b.sequence),
            other => panic!("handle/epoch 串扰: {other:?}"),
        }
    }
    assert!(seen7.len() >= 3 && seen8.len() >= 3);
    // 各自从 1 连续（首批即 1：dequeue 顺序不影响各自序号起点）
    assert_eq!(seen7[0], 1, "handle 7 序号独立从 1 起");
    assert_eq!(seen8[0], 1, "handle 8 序号独立从 1 起");

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 11：空任务零成本（兼容路径）+ Gate 13：坏任务整个 revision 失败
// ---------------------------------------------------------------------------

/// 空 EventTask：直接成功、无 RPC，数据流不受影响。
#[tokio::test]
async fn event_empty_tasks_are_noop() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, 7, "{}").await;
    let descriptors = configure_tasks(&session, 7, 1, &[mini_data_task()]).await;
    apply_point_map(&session, 7, 1, sequential_ids(&descriptors, 11)).await;
    session
        .configure_events(7, 1, &[])
        .await
        .expect("empty tasks must succeed without RPC");
    start_connection(&session, 7, 0xE000_0601).await;

    // 数据面正常
    let b = recv_batch(&mut events, 5).await;
    assert_eq!(b.connection_handle, 7);

    teardown(&mut session, Some(server_cancel));
}

/// 好 + 坏（未知 stream）混装：整个 revision 失败（精确码），原子不残留；
/// 随后纯好 revision 成功。
#[tokio::test]
async fn event_invalid_task_fails_whole_revision() {
    init_log();
    let (port, server_cancel) = start_sim_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, 7, "{}").await;
    let descriptors = configure_tasks(&session, 7, 1, &[mini_data_task()]).await;
    apply_point_map(&session, 7, 1, sequential_ids(&descriptors, 11)).await;

    let bad = sim_event_task("bad", "sim.events.nope", TaskMode::Subscribe, None);
    let good = sim_event_task("al", SIM_EVENT_STREAM_ALARM, TaskMode::Subscribe, None);
    let err = session
        .configure_events(7, 2, &[good.clone(), bad])
        .await
        .expect_err("mixed revision must fail");
    assert!(
        matches!(err, SessionError::Driver { ref code, .. } if code == "UNKNOWN_EVENT_STREAM"),
        "必须精确报 UNKNOWN_EVENT_STREAM，got {err}"
    );

    // 原子性：失败 revision 无残留，纯好 revision 照常成功并可 Start 收事件
    session.configure_events(7, 3, &[good]).await.unwrap();
    start_connection(&session, 7, 0xE000_0602).await;
    let mut erx = session.take_event_batches().unwrap();
    let b = recv_event_batch(&mut erx, 5).await;
    assert_eq!(b.events[0].category, "alarm");

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// Gate 12：老 Driver（默认 configure_events）立即精确拒绝，不过 RPC 超时
// ---------------------------------------------------------------------------

/// 未覆盖 `configure_events` 的驱动 = 老 Driver：非空任务秒级返回
/// `EVENT_NOT_SUPPORTED` 精确码，而不是发未知 RPC 干等 10s 超时。
struct LegacyConn;

#[async_trait::async_trait]
impl DriverConnection for LegacyConn {
    async fn configure(
        &mut self,
        _r: u64,
        _t: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        Ok(vec![])
    }
    async fn apply_point_map(&mut self, _m: PointMap) -> Result<(), SdkDriverError> {
        Ok(())
    }
    async fn run(
        &mut self,
        _s: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError> {
        shutdown.cancelled().await;
        Ok(())
    }
}

struct LegacyDriver;

#[async_trait::async_trait]
impl Driver for LegacyDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: "legacy-stub".into(),
            name: "Legacy Stub".into(),
            version: "0.0.1".into(),
            protocol_major: 1,
            protocol_minor: 2,
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        _config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        Ok(Box::new(LegacyConn))
    }
}

async fn start_legacy_server() -> (u16, CancellationToken) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind legacy stub");
    let port = listener.local_addr().unwrap().port();
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        let _ = serve_with_faults(
            LegacyDriver,
            listener,
            TOKEN.into(),
            c,
            Some(SdkFaults::new()),
        )
        .await;
    });
    (port, cancel)
}

#[tokio::test]
async fn event_legacy_driver_rejects_immediately_with_precise_code() {
    init_log();
    let (port, server_cancel) = start_legacy_server().await;
    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();
    open_connection(&session, 7, "{}").await;

    // 10s 内必须返回（若实现发了未知 RPC 干等，这里会超时 panic 而不是精确码）
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        session.configure_events(
            7,
            1,
            &[sim_event_task(
                "e1",
                SIM_EVENT_STREAM_COUNTER,
                TaskMode::Poll,
                Some(50),
            )],
        ),
    )
    .await
    .expect("legacy rejection must be immediate, not a timeout")
    .expect_err("legacy driver must reject event tasks");
    assert!(
        matches!(err, SessionError::Driver { ref code, .. } if code == "EVENT_NOT_SUPPORTED"),
        "必须精确报 EVENT_NOT_SUPPORTED，got {err}"
    );

    teardown(&mut session, Some(server_cancel));
}

// ---------------------------------------------------------------------------
// 精确批量 Gates（hardening）：数量代替"时间×推测速率"。
// 同文件 Gate 9（sim 计时洪峰）保留作 session 级集成覆盖；这里用脚本化
// fake driver（无 timer）证明精确分界：容量内全活、容量+1 必死、500 精确
// 落盘。所有等待都是"条件必成立"（TCP 必达 + reader 必 pump），
// 超时只在真 bug 时触发，不赌 runner 快慢。
// ---------------------------------------------------------------------------

/// 精确批次构造：单事件一批，sequence = i，event_id 唯一，epoch 统一。
fn exact_batch(handle: u32, epoch: u64, seq: u64) -> pb::EventBatchMsg {
    pb::EventBatchMsg {
        connection_handle: handle,
        stream_epoch: epoch,
        sequence: seq,
        timestamp_ns: mesa_core_types::now_unix_ns(),
        mono_ns: None,
        events: vec![pb::EventRecordMsg {
            event_id: format!("exact:{seq}"),
            category: "message".into(),
            kind: "exact.tick".into(),
            source: "exact".into(),
            severity: 0,
            code: None,
            message: Some(format!("exact tick {seq}")),
            message_locale: None,
            occurred_at_ns: None,
            condition: None,
            correlation_id: None,
            attributes: vec![],
        }],
    }
}

/// 脚本化 fake driver：Hello → Welcome → 应答 ConfigureEventTasks /
// StartConnection → 按波次精确发射（无 timer，数量即真相）。
/// `second_wave` 非空时等测试放行再发射（512 存活 / 513 死亡分界的执行手段）；
/// 发射后读到 EOF 退出（teardown 关 socket 即清理，无残留任务）。
async fn start_exact_fake(
    first_wave: Vec<pb::EventBatchMsg>,
    second_wave: Vec<pb::EventBatchMsg>,
    release: Option<tokio::sync::oneshot::Receiver<()>>,
) -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let (mut rd, mut wr) = sock.into_split();
        // Driver 先发言 Hello（§14.3），token 与 Core 期望一致。
        write_envelope(
            &mut wr,
            &pb::Envelope {
                msg_id: 1,
                body: Some(pb::envelope::Body::Hello(pb::Hello {
                    driver_id: "exact".into(),
                    driver_version: "0.0.0".into(),
                    protocol_major: PROTOCOL_MAJOR,
                    protocol_minor: PROTOCOL_MINOR,
                    sdk_version: "test".into(),
                    platform: std::env::consts::OS.into(),
                    instance_id: "exact-1".into(),
                    session_token: TOKEN.into(),
                })),
            },
        )
        .await
        .unwrap();
        // Welcome 读掉（握手完成；token 由 Core 校验）。
        let _ = read_envelope(&mut rd).await;
        let mut release = release;
        loop {
            let req = match read_envelope(&mut rd).await {
                Ok(e) => e,
                Err(_) => return,
            };
            match req.body {
                Some(pb::envelope::Body::ConfigureEventTasks(c)) => {
                    write_envelope(
                        &mut wr,
                        &pb::Envelope {
                            msg_id: req.msg_id,
                            body: Some(pb::envelope::Body::EventConfigApplied(
                                pb::EventConfigApplied {
                                    connection_handle: c.connection_handle,
                                    revision: c.revision,
                                    result: Some(pb::GenericResult {
                                        ok: true,
                                        error: None,
                                    }),
                                },
                            )),
                        },
                    )
                    .await
                    .unwrap();
                }
                Some(pb::envelope::Body::StartConnection(c)) => {
                    write_envelope(
                        &mut wr,
                        &pb::Envelope {
                            msg_id: req.msg_id,
                            body: Some(pb::envelope::Body::StartConnectionAck(
                                pb::StartConnectionAck {
                                    connection_handle: c.connection_handle,
                                    result: Some(pb::GenericResult {
                                        ok: true,
                                        error: None,
                                    }),
                                },
                            )),
                        },
                    )
                    .await
                    .unwrap();
                    for (i, b) in first_wave.iter().enumerate() {
                        write_envelope(
                            &mut wr,
                            &pb::Envelope {
                                msg_id: 9000 + i as u64,
                                body: Some(pb::envelope::Body::EventBatch(b.clone())),
                            },
                        )
                        .await
                        .unwrap();
                    }
                    if !second_wave.is_empty() {
                        // 等测试确认第一波全部到达后再发射分界批。
                        if let Some(rx) = release.take() {
                            let _ = rx.await;
                        }
                        for (i, b) in second_wave.iter().enumerate() {
                            write_envelope(
                                &mut wr,
                                &pb::Envelope {
                                    msg_id: 9500 + i as u64,
                                    body: Some(pb::envelope::Body::EventBatch(b.clone())),
                                },
                            )
                            .await
                            .unwrap();
                        }
                    }
                    // 发射后读到 EOF 退出。
                    loop {
                        if read_envelope(&mut rd).await.is_err() {
                            return;
                        }
                    }
                }
                Some(pb::envelope::Body::Ping(_)) => {
                    write_envelope(
                        &mut wr,
                        &pb::Envelope {
                            msg_id: req.msg_id,
                            body: Some(pb::envelope::Body::Pong(pb::Pong {})),
                        },
                    )
                    .await
                    .unwrap();
                }
                _ => {}
            }
        }
    });
    port
}

/// 精确事件任务（binding 内容 fake 直接忽略；Core 只做透传）。
fn exact_task() -> EventTask {
    EventTask {
        id: "exact".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(1000),
        binding: DriverBinding {
            kind: "exact-test".into(),
            config: serde_json::json!({}),
        },
    }
}

/// SLO 精确证明：500 个 occurrence（公式 RATE×STALL×SAFETY 的 RATE×10s）在
/// 10s budget 内精确推送/持久化：DB=500、无丢失无重复、单 epoch、Hub=500。
/// 不经过 Tokio interval——"可持续 ≥50/s"由数量+budget 证明，不由 timer 证明。
#[tokio::test]
async fn exact_500_events_pipeline_exact_within_budget() {
    init_log();
    const N: u64 = 500;
    const HANDLE: u32 = 7;
    const EPOCH: u64 = 0xE000_0E01;
    let batches: Vec<_> = (1..=N).map(|i| exact_batch(HANDLE, EPOCH, i)).collect();
    let port = start_exact_fake(batches, vec![], None).await;

    let db = tmp_db("exact-slo");
    let _ = std::fs::remove_file(&db);
    let store = Arc::new(EventStore::open(&db).unwrap());
    // Hub 按测试体量精确配额（500 < 1024：结尾排空不断言 Lagged 场景）。
    let services = Arc::new(EventServices::new(store.clone(), EventHub::new(1024)));
    let hub_rx = services.hub.subscribe();

    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();
    session
        .configure_events(HANDLE, 1, &[exact_task()])
        .await
        .expect("configure_events must succeed");
    start_connection(&session, HANDLE, EPOCH).await;
    let erx = session.take_event_batches().unwrap();
    let shutdown = CancellationToken::new();
    let ingress_h = tokio::spawn(run_event_ingress(
        erx,
        "exact-slo".into(),
        Arc::clone(&services),
        shutdown.clone(),
    ));

    // 500 精确落盘（10s budget；条件必成立，超时只在真 bug 时触发）。
    common::wait_until(10, || {
        services
            .diagnostics
            .ingress_persisted_events_total
            .load(std::sync::atomic::Ordering::Relaxed)
            >= N
    })
    .await;

    // DB 精确：500 行、event_id exact:1..=500 连续、单 epoch。
    let rows = rows_of(&store, "exact-slo");
    assert_eq!(rows.len(), N as usize, "DB 必须恰好 500 行");
    let mut ids: Vec<u64> = rows
        .iter()
        .map(|r| {
            r.event_id
                .strip_prefix("exact:")
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, (1..=N).collect::<Vec<_>>(), "event_id 必须精确连续");
    let epochs: BTreeSet<u64> = rows.iter().map(|r| r.stream_epoch).collect();
    assert_eq!(epochs, BTreeSet::from([EPOCH]), "必须单 epoch");
    {
        use std::sync::atomic::Ordering;
        assert_eq!(
            services
                .diagnostics
                .ingress_persisted_events_total
                .load(Ordering::Relaxed),
            N,
        );
    }

    // Hub 精确：恰好收到 DB 全部行（commit-then-publish）。
    let mut got = BTreeSet::new();
    let mut hub_rx = hub_rx;
    while let Ok(ev) = hub_rx.try_recv() {
        got.insert(ev.event_id.clone());
    }
    let want: BTreeSet<String> = rows.iter().map(|r| r.event_id.clone()).collect();
    assert_eq!(got, want, "Hub 必须恰好收到 DB 全部行");

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), ingress_h)
        .await
        .expect("ingress 必须随 cancel 退出")
        .unwrap()
        .expect("ingress 不得 fatal");
    teardown(&mut session, None);
    let _ = std::fs::remove_file(&db);
}

/// 容量精确分界：512 批全部存活（depth 见证到达），第 513 批必触发
/// fail-closed（置位 + 计数 + backlog 完整可排空后流终止）。
/// 数量即真相：不依赖任何速率假设，慢 runner 只是到达得晚，不会误判。
#[tokio::test]
async fn exact_capacity_plus_one_overflow_is_loud() {
    init_log();
    const HANDLE: u32 = 7;
    const EPOCH: u64 = 0xE000_0E02;
    let cap = EVENT_BATCH_CAPACITY as u64;
    let first: Vec<_> = (1..=cap).map(|i| exact_batch(HANDLE, EPOCH, i)).collect();
    let second = vec![exact_batch(HANDLE, EPOCH, cap + 1)];
    let (tx, rx) = tokio::sync::oneshot::channel();
    let port = start_exact_fake(first, second, Some(rx)).await;

    let (mut session, _events, _) = Session::connect(port, TOKEN).await.unwrap();
    session
        .configure_events(HANDLE, 1, &[exact_task()])
        .await
        .expect("configure_events must succeed");
    start_connection(&session, HANDLE, EPOCH).await;

    // 全部到达见证（TCP 必达 + reader 必 pump；慢 runner 只需更久）。
    common::wait_until(30, || session.event_queue_depth_last_sample() >= cap).await;
    assert!(
        !session.event_stream_failed(),
        "容量内 512 批必须全部存活，不得 fail-closed"
    );
    assert_eq!(session.event_queue_high_water(), cap, "高水位必须恰为容量");

    // 放行第 513 批 → 必死。
    tx.send(()).unwrap();
    common::wait_until(10, || session.event_stream_failed()).await;
    assert!(session.event_overflow_drops() >= 1, "溢出必须计数可见");

    // backlog 完整可排空后流终止（恰 512 + None，不能挂不能多不能少）。
    let mut erx = session.take_event_batches().unwrap();
    let mut drained = 0u64;
    loop {
        match tokio::time::timeout(Duration::from_secs(5), erx.recv()).await {
            Ok(Some(_)) => drained += 1,
            Ok(None) => break,
            Err(_) => panic!("terminated stream must close, not hang"),
        }
    }
    assert_eq!(drained, cap, "终止前缓冲必须恰为 512 批");

    teardown(&mut session, None);
}
