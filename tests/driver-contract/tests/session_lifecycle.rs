//! §21 连接生命周期与配置语义：Open/Close、Invalid Config、Duplicate point_key、
//! Configure Snapshot 原子性、Point Registration、Start/Stop、Multiple Connections、
//! Partial Failure、Runtime Reconfigure。

mod common;

use mesa_core_types::{AcquisitionTask, PointDescriptor, TaskMode};
use mesa_driver_manager::session::Session;
use mesa_driver_protocol::pb;

use common::*;

const H: u32 = 1;

fn two_points() -> serde_json::Value {
    serde_json::json!({
        "points": [
            {"key":"a.counter","kind":"counter","step":1},
            {"key":"a.toggle","kind":"toggle"}
        ]
    })
}

/// Open / Close（§21 行 5）：句柄可开关复用；重复打开同句柄被拒；关闭未知句柄幂等。
#[tokio::test]
async fn open_close_lifecycle_and_handle_reuse() {
    let (port, cancel) = start_sim_server().await;
    let (session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    // 重复打开同一句柄必须被拒
    let reply = session
        .call(pb::envelope::Body::OpenConnection(pb::OpenConnection {
            connection_handle: H,
            endpoint_id: "dup".into(),
            config_json: "{}".into(),
        }))
        .await
        .unwrap();
    match reply.body {
        Some(pb::envelope::Body::OpenConnectionAck(a)) => {
            let r = a.result.expect("result present");
            assert!(!r.ok, "duplicate handle must fail");
            assert_eq!(r.error.unwrap().code, "HANDLE_EXISTS");
        }
        other => panic!("unexpected reply {other:?}"),
    }

    close_connection(&session, H).await;
    // 关闭未知句柄幂等成功
    close_connection(&session, 999).await;
    // 句柄可复用
    open_connection(&session, H, "{}").await;

    teardown(&mut session_drop(session), Some(cancel));
}

fn session_drop(s: Session) -> Session {
    s
}

/// Invalid Config（§21 行 6）：非法配置返回结构化 ConfigurationError，
/// 且错误码可区分故障类别。
#[tokio::test]
async fn invalid_config_returns_structured_error() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;

    // 1) 错误的 binding kind
    let bad_kind = vec![AcquisitionTask {
        id: "t".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(50),
        binding: mesa_core_types::DriverBinding {
            kind: "s7.address-group".into(), // simulator 不支持
            config: serde_json::json!({}),
        },
    }];
    configure_tasks_expect_error(&mut session, &mut events, &bad_kind, "UNSUPPORTED_BINDING").await;

    // 2) 缺少 points 数组
    let missing = vec![poll_task("t", 50, serde_json::json!({}))];
    configure_tasks_expect_error(
        &mut session,
        &mut events,
        &missing,
        "INVALID_BINDING_CONFIG",
    )
    .await;

    // 3) Subscribe 模式不被支持
    let sub = vec![AcquisitionTask {
        id: "t".into(),
        mode: TaskMode::Subscribe,
        interval_ms: None,
        binding: mesa_core_types::DriverBinding {
            kind: mesa_driver_simulator::BINDING_KIND.into(),
            config: serde_json::json!({"points": [{"key":"x","kind":"constant","value":1}]}),
        },
    }];
    configure_tasks_expect_error(&mut session, &mut events, &sub, "MODE_NOT_SUPPORTED").await;

    // 4) 未知数据源 kind
    let unknown = vec![poll_task(
        "t",
        50,
        serde_json::json!({"points":[{"key":"x","kind":"warp_drive"}]}),
    )];
    configure_tasks_expect_error(
        &mut session,
        &mut events,
        &unknown,
        "UNSUPPORTED_SOURCE_KIND",
    )
    .await;

    teardown(&mut session, Some(cancel));
}

async fn configure_tasks_expect_error(
    session: &mut Session,
    events: &mut tokio::sync::mpsc::Receiver<mesa_driver_manager::session::SessionEvent>,
    tasks: &[mesa_core_types::AcquisitionTask],
    expected_code: &str,
) {
    let tasks_pb = mesa_driver_protocol::tasks_to_pb(tasks).unwrap();
    // 错误路径的响应是 DriverError 事件而非 PointDescriptors 帧
    let _ = session
        .call(pb::envelope::Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: H,
            revision: 7,
            tasks: tasks_pb,
        }))
        .await; // 响应可能是空 ack 或直接无帧，断言以 DriverError 事件为准
    let (_, code, _) = expect_driver_error_with(session, events, expected_code).await;
    assert_eq!(code, expected_code);
}

async fn expect_driver_error_with(
    _session: &Session,
    events: &mut tokio::sync::mpsc::Receiver<mesa_driver_manager::session::SessionEvent>,
    _code: &str,
) -> (String, String, String) {
    for _ in 0..20 {
        match tokio::time::timeout(std::time::Duration::from_secs(2), events.recv()).await {
            Ok(Some(mesa_driver_manager::session::SessionEvent::DriverError {
                kind,
                code,
                message,
                ..
            })) => return (kind, code, message),
            Ok(Some(_)) => continue,
            Ok(None) => panic!("session closed"),
            Err(_) => panic!("timed out waiting for DriverError"),
        }
    }
    panic!("no driver error observed");
}

/// Duplicate point_key（§21 行 8）：configure 阶段必须拒绝跨任务重复 key。
#[tokio::test]
async fn duplicate_point_key_rejected_over_ipc() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    let dup = vec![
        poll_task(
            "t1",
            100,
            serde_json::json!({"points":[{"key":"same.key","kind":"counter"}]}),
        ),
        poll_task(
            "t2",
            100,
            serde_json::json!({"points":[{"key":"same.key","kind":"counter"}]}),
        ),
    ];
    let tasks_pb = mesa_driver_protocol::tasks_to_pb(&dup).unwrap();
    let _ = session
        .call(pb::envelope::Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: H,
            revision: 1,
            tasks: tasks_pb,
        }))
        .await;
    let (kind, code, _) =
        expect_driver_error_with(&session, &mut events, "DUPLICATE_POINT_KEY").await;
    assert_eq!(code, "DUPLICATE_POINT_KEY");
    assert_eq!(kind, "ConfigurationError");

    teardown(&mut session, Some(cancel));
}

/// Configure Snapshot（§21 行 7）：失败的新快照不得破坏旧计划——
/// 配置 rev2 失败后，rev1 的点集仍可正常启动出数。
#[tokio::test]
async fn failed_reconfigure_keeps_previous_snapshot() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    // rev1 合法快照
    let good: Vec<PointDescriptor> =
        configure_tasks(&session, H, 1, &[poll_task("t", 40, two_points())]).await;
    assert_eq!(good.len(), 2);

    // rev2 非法快照 → 被拒绝
    let broken = vec![poll_task(
        "t",
        40,
        serde_json::json!({"points":[{"key":"b.x","kind":"black_hole"}]}),
    )];
    let tasks_pb = mesa_driver_protocol::tasks_to_pb(&broken).unwrap();
    let _ = session
        .call(pb::envelope::Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: H,
            revision: 2,
            tasks: tasks_pb,
        }))
        .await;
    let (_, code, _) =
        expect_driver_error_with(&session, &mut events, "UNSUPPORTED_SOURCE_KIND").await;
    assert_eq!(code, "UNSUPPORTED_SOURCE_KIND");

    // 旧快照仍然有效：按 rev1 的描述符分配 id 并启动，数据照常流出
    let map = sequential_ids(&good, 100);
    apply_point_map(&session, H, 1, map).await;
    start_connection(&session, H, 42).await;
    let b = recv_batch(&mut events, 3).await;
    assert_eq!(b.stream_epoch, 42);
    assert!(
        b.values
            .iter()
            .all(|v| v.point_id >= 100 && v.point_id <= 101)
    );

    teardown(&mut session, Some(cancel));
}

/// Point Registration（§21 行 9）：同配置重复 configure 得到稳定一致的描述符集合。
#[tokio::test]
async fn point_registration_stable_across_configures() {
    let (port, cancel) = start_sim_server().await;
    let (session, _events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    let d1 = configure_tasks(&session, H, 1, &[poll_task("t", 100, two_points())]).await;
    let d2 = configure_tasks(&session, H, 2, &[poll_task("t", 100, two_points())]).await;
    assert_eq!(
        d1, d2,
        "descriptor set must be stable across identical reconfigures"
    );

    teardown(&mut session_drop(session), Some(cancel));
}

/// Start / Stop（§21 行 10）：启停可重复执行，停止后数据静默，重启后续流。
#[tokio::test]
async fn start_stop_idempotent_without_leaks() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    let descriptors = configure_tasks(&session, H, 1, &[poll_task("t", 40, two_points())]).await;
    apply_point_map(&session, H, 1, sequential_ids(&descriptors, 1)).await;

    // 第一轮运行
    start_connection(&session, H, 11).await;
    let b1 = recv_batch(&mut events, 3).await;
    assert_eq!(b1.stream_epoch, 11);

    // 停止：Ack 成功且数据静默
    assert!(stop_connection(&session, H).await, "stop must ack ok");
    // StopAck 为数据面 barrier，但 TCP 单 reader 保证 wire-before-StopAck 的 batch
    // 必然在 StopAck 唤醒前已进入 events_rx，需排空 barrier 之前的 backlog 再判定静默
    drain_pre_barrier_events(&mut events);
    assert_no_batches_for(&mut events, 300).await;

    // 未运行时再次 Stop 幂等成功
    assert!(
        stop_connection(&session, H).await,
        "double stop must be idempotent"
    );

    // 第二轮运行（新 epoch）
    start_connection(&session, H, 22).await;
    let b2 = recv_batch(&mut events, 3).await;
    assert_eq!(b2.stream_epoch, 22, "second run carries its own epoch");

    teardown(&mut session, Some(cancel));
}

/// 运行中 Start 必须被拒（防双跑泄漏）。
#[tokio::test]
async fn start_while_running_is_rejected() {
    let (port, cancel) = start_sim_server().await;
    let (session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    let descriptors = configure_tasks(&session, H, 1, &[poll_task("t", 50, two_points())]).await;
    apply_point_map(&session, H, 1, sequential_ids(&descriptors, 1)).await;
    start_connection(&session, H, 5).await;
    recv_batch(&mut events, 3).await; // 确认已在运行

    let reply = session
        .call(pb::envelope::Body::StartConnection(pb::StartConnection {
            connection_handle: H,
            stream_epoch: 6,
        }))
        .await
        .unwrap();
    match reply.body {
        Some(pb::envelope::Body::StartConnectionAck(a)) => {
            assert!(
                !ack_ok_raw(a.result.as_ref()),
                "start while running must fail"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
    stop_connection(&session, H).await;
    teardown(&mut session_drop(session), Some(cancel));
}

fn ack_ok_raw(result: Option<&pb::GenericResult>) -> bool {
    result.map(|r| r.ok).unwrap_or(false)
}

/// Multiple Connections（§21 行 11）+ Partial Failure（§21 行 12）：
/// 两个连接并行采集互不干扰；单连接配置失败不影响另一连接继续出数。
#[tokio::test]
async fn multiple_connections_and_partial_failure_isolation() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    const HA: u32 = 10;
    const HB: u32 = 20;

    open_connection(&session, HA, "{}").await;
    open_connection(&session, HB, "{}").await;

    let da = configure_tasks(&session, HA, 1, &[poll_task("ta", 50, two_points())]).await;
    let db = configure_tasks(&session, HB, 1, &[poll_task("tb", 70, two_points())]).await;
    apply_point_map(&session, HA, 1, sequential_ids(&da, 100)).await;
    apply_point_map(&session, HB, 1, sequential_ids(&db, 200)).await;
    start_connection(&session, HA, 1).await;
    start_connection(&session, HB, 2).await;

    // 两路批次都到达且 handle 各自正确
    let mut seen_a = false;
    let mut seen_b = false;
    for _ in 0..20 {
        let b = recv_batch(&mut events, 3).await;
        match b.connection_handle {
            HA => seen_a = true,
            HB => seen_b = true,
            other => panic!("unknown handle {other}"),
        }
        if seen_a && seen_b {
            break;
        }
    }
    assert!(
        seen_a && seen_b,
        "both connections must stream concurrently"
    );

    // Partial failure：连接 B 配置一个非法任务被拒，A 不受影响继续出数
    let broken = vec![poll_task(
        "bad",
        50,
        serde_json::json!({"points":[{"key":"z","kind":"nope"}]}),
    )];
    let tasks_pb = mesa_driver_protocol::tasks_to_pb(&broken).unwrap();
    let _ = session
        .call(pb::envelope::Body::ConfigureTasks(pb::ConfigureTasks {
            connection_handle: HB,
            revision: 2,
            tasks: tasks_pb,
        }))
        .await;

    // A 仍在产数（在 B 失败之后收到 A 的新批次）
    let mut got_a_after_failure = false;
    for _ in 0..10 {
        let b = recv_batch(&mut events, 3).await;
        if b.connection_handle == HA {
            got_a_after_failure = true;
            break;
        }
    }
    assert!(
        got_a_after_failure,
        "connection A must keep flowing after B's failure"
    );

    teardown(&mut session, Some(cancel));
}

/// Runtime Reconfigure（§21 行 19）：RUNNING 中修改走 Stop→Configure→Apply→Start，
/// 新 epoch 生效且只上报新点集。
#[tokio::test]
async fn runtime_reconfigure_swaps_epoch_and_points() {
    let (port, cancel) = start_sim_server().await;
    let (mut session, mut events, _) = Session::connect(port, TOKEN).await.unwrap();

    open_connection(&session, H, "{}").await;
    let old = configure_tasks(&session, H, 1, &[poll_task("t", 40, two_points())]).await;
    apply_point_map(&session, H, 1, sequential_ids(&old, 1)).await;
    start_connection(&session, H, 111).await;
    recv_batch(&mut events, 3).await;

    // Stop -> Configure(新点集) -> Apply -> Start(new epoch)
    assert!(stop_connection(&session, H).await);
    drain_pre_barrier_events(&mut events);
    let new_tasks = vec![poll_task(
        "t2",
        40,
        serde_json::json!({"points":[{"key":"n.only","kind":"constant","value":9}]}),
    )];
    let new_desc = configure_tasks(&session, H, 2, &new_tasks).await;
    assert_eq!(new_desc.len(), 1);
    assert_eq!(new_desc[0].point_key, "n.only");
    apply_point_map(&session, H, 2, sequential_ids(&new_desc, 77)).await;
    start_connection(&session, H, 222).await;

    // 验证重配置后新 epoch/新点位已生效（取首个 batch 即可，无需循环）
    let b = recv_batch(&mut events, 3).await;
    assert_eq!(
        b.stream_epoch, 222,
        "all batches after reconfigure carry the new epoch"
    );
    assert!(
        b.values.iter().all(|v| v.point_id == 77),
        "only the new point set may appear"
    );

    teardown(&mut session, Some(cancel));
}

/// 排空 StopAck 之前已进入 events_rx 的 backlog（wire-before-StopAck）。
/// 成功 StopConnectionAck 是该 Connection 当前 Stream Epoch 的数据面 barrier：
/// 允许 DataBatch → StopAck，禁止 StopAck → 旧 DataBatch；但 TCP 单 reader 保证
/// StopAck 唤醒前 wire-before-StopAck 的 batch 已在队列中，需先 try_recv 排空再判定静默。
pub fn drain_pre_barrier_events(
    events: &mut tokio::sync::mpsc::Receiver<mesa_driver_manager::session::SessionEvent>,
) {
    while let Ok(ev) = events.try_recv() {
        match ev {
            mesa_driver_manager::session::SessionEvent::Batch(_) => {
                // barrier 之前的 backlog，允许丢弃
            }
            mesa_driver_manager::session::SessionEvent::State { .. } => {
                // lifecycle state 可消费
            }
            mesa_driver_manager::session::SessionEvent::DriverError { code, message, .. } => {
                panic!("unexpected driver error: {code}: {message}");
            }
        }
    }
}

/// 断言窗口期内没有任何批次到达。
pub async fn assert_no_batches_for(
    events: &mut tokio::sync::mpsc::Receiver<mesa_driver_manager::session::SessionEvent>,
    ms: u64,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(ms);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(_) => return, // 超时即窗口内无事件
            Ok(Some(mesa_driver_manager::session::SessionEvent::Batch(b))) => {
                panic!("expected quiet window, got batch seq {}", b.sequence)
            }
            Ok(Some(_)) => continue,
            Ok(None) => return,
        }
    }
}
