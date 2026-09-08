//! Stop-during-reconnect lifecycle gate：重连退避中显式 Stop 必须有界、结果显式。
//!
//! 诚实契约（review P1）：本测试观察的是 crash 之后、teardown 已结束、
//! snapshot 处于 RECONNECTING（退避间隙）的窗口，此时 Stop 取消退避 sleep
//! 并等待 endpoint 任务结束。它锁的是“重连中 Stop 有界显式”，**不是**
//! “teardown 进行中与 failure 竞争的真 race”——后者需要进程活着但 stalled
//! 的确定性复现手段，当前没有（不造平台相关的 suspend 原语、不新增驱动
//! 故障种类），故不在此冒充覆盖。真 race 的三段式已有各自 Gate：
//! unresponsive 标记（fault_tolerance 会话级）、有界 drain + fail-closed
//! 超时码（endpoint in-crate）、本测试的有界显式 Stop。
//!
//! 触发器用 simulator 现有 `crash_after_batches` 故障（确定性进程死亡），
//! 不新增驱动故障种类。不改生产语义、不放宽 timeout/断言。
//!
//! 第二个测试（`stop_waits_for_inflight_commit_then_drains_exact`）是真
//! in-flight race 的确定性 Gate：test-only writer 栅栏暂停 commit，并发
//! Stop 必须等待（不得提前返回、不得超时误判），放行后成功 + DB/Hub 精确 +
//! Stop 后无新写。它冻结的是 `reader_done → 无新生产 → in-flight 完成 →
//! 队列排空 → drain ACK → Stop 返回` 这条关系。

mod common;
mod event_common;

use std::time::Duration;

use event_common::{rows_of, tmp_db};
use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_event_store::{EVENT_HUB_CAPACITY, EventHub, EventServices, EventStore, StoreFaults};

fn drivers_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("drivers")
}

/// Stop 错误码冻结集合：`stop_endpoint` 的 `Err(String)` 必须是以下精确码之一
/// 开头（`EventDrainError::code` / manager Join 映射），禁止空消息或未定义码。
fn assert_frozen_stop_code(msg: &str) {
    const FROZEN: [&str; 7] = [
        "EVENT_DRAIN_TIMEOUT",
        "EVENT_DRAIN_FAILED",
        "EVENT_SEQUENCE_REGRESSION",
        "EVENT_ID_COLLISION",
        "EVENT_STORE_UNAVAILABLE",
        "EVENT_RECORD_INVALID",
        "EVENT_STREAM_CLOSED",
    ];
    assert!(
        FROZEN.iter().any(|c| msg.starts_with(c)),
        "Stop Err 必须是冻结码集合成员，实际: {msg:?}",
    );
}

/// 重连退避中显式 Stop：40s 内必须返回显式结论，之后端点不得再运行。
/// data 任务触发 crash（2 批即死），event 任务保证 ingress/drain 路径被执行到。
#[tokio::test]
async fn stop_while_reconnecting_is_bounded_and_explicit() {
    common::init_log();
    let db = tmp_db("stop-reconnect");
    let _ = std::fs::remove_file(&db);
    let store = std::sync::Arc::new(EventStore::open(&db).unwrap());
    let services = EventServices::new(store.clone(), EventHub::new(EVENT_HUB_CAPACITY));
    let mgr = std::sync::Arc::new(MesaManager::discover(&drivers_dir()));
    mgr.set_event_services(std::sync::Arc::clone(&services));
    let snapshot = mgr.snapshot();
    let ep = "hd-stop-reconnect";

    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: ep.into(),
        driver_id: "simulator".into(),
        // 第 2 个 data 批后进程退出（50ms poll → 约 100ms 后死亡）。
        connection_json: r#"{"faults":{"crash_after_batches":2}}"#.into(),
        tasks: vec![common::poll_task(
            "d",
            50,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![EventTask {
            id: "cnt".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(50),
            binding: DriverBinding {
                kind: GENERIC_EVENT_BINDING_KIND.into(),
                config: serde_json::to_value(&binding).unwrap(),
            },
        }],
    })
    .unwrap();

    // 存活证据：行落盘（crash 前数据确实流动，不是起不来）。
    common::wait_until(30, || !rows_of(&store, ep).is_empty()).await;
    // 死亡证据：crash 后 snapshot 必现 RECONNECTING 窗口（Lost→退避间隙，
    // 50ms 轮询必抓到；注意是 RECONNECTING 而非 FAILED——后者只用于配置失败）。
    common::wait_until(
        60,
        || matches!(snapshot.endpoint(ep), Some(ref s) if s.state == "RECONNECTING"),
    )
    .await;

    // 重连退避中显式 Stop：40s（≈2× teardown 最坏预算）内必须返回，
    // 结论显式（Ok 布尔或冻结码 Err），不 hang、不静默。
    let res = tokio::time::timeout(Duration::from_secs(40), mgr.stop_endpoint(ep))
        .await
        .expect("重连中 Stop 必须有界返回");
    match &res {
        Ok(was_running) => eprintln!("stop outcome: Ok({was_running})"),
        Err(msg) => {
            assert_frozen_stop_code(msg);
            eprintln!("stop outcome: Err({msg})");
        }
    }
    assert!(
        !mgr.is_running(ep),
        "Stop 返回后端点不得再运行，实际 outcome={res:?}"
    );
    let _ = std::fs::remove_file(&db);
}

/// Stop 等待 in-flight commit 的确定性 Gate（main CI #103 真因冻结），
/// 且是旧实现的真回归测试（old 必红 / new 必绿），不是"没撞到就不红"的采样.
///
/// 用 test-only writer 栅栏把一个 commit 暂停在"已发送、未执行"状态，
/// 此时并发 Stop 必须**等待**（500ms 内不得返回——提前返回或超时误判都是 bug），
/// 然后继续卡住直到 Stop 发起后约 12s 才放行：
///
/// ```text
/// 旧实现（drain timer 5s，起于 shutdown_ingress ≈ Stop 后 0.5s）：
///     timer 在约 5.5s 先赢 → 确定性返回 Err(EVENT_DRAIN_TIMEOUT) → 测试红
/// 新实现（drain timer 15s，可组合预算）：
///     放行（12s）恒早于 timer（起于 ≥0s，15s 后才赢）→ drain 成功 → 测试绿
/// ```
///
/// 判别力推导：旧实现失败 ⟺ shutdown_ingress 在 Stop 后 7s 内开始。
/// 健康驱动（simulator 响应 Shutdown）约 0.5s 内到达，14× 裕度；
/// 均匀调度膨胀下 sleep 与 pre 同比拉伸，关系保持。新实现通过是无条件的
/// （放行时刻恒早于新 timer，与 pre 无关）。仅当 teardown 前置阶段全部
/// 病态拉满（pre ≥ 7s）时判别力退化——超出任何 timing Gate 的覆盖范围，
/// 届时测试按"Stop 有界"继续断言，不误报。
///
/// 放行后 Stop 成功，且 DB/Hub 精确、Stop 后无新写。冻结的关系：
/// `reader_done → 无新生产 → in-flight 完成 → 队列排空 → drain ACK → Stop 返回`。
///
/// 时序全部由栅栏握手决定，不赌调度：`entered() >= 1` 之前 Stop 尚未发起；
/// 栅栏关闭期间 Stop 不可能完成（drain 需要该 commit 的 reply）。
#[tokio::test]
async fn stop_waits_for_inflight_commit_then_drains_exact() {
    common::init_log();
    let db = tmp_db("stop-gate");
    let _ = std::fs::remove_file(&db);
    // 故障器仅提供栅栏能力，不注入失败（new(0, false) 永不失败）。
    let faults = std::sync::Arc::new(StoreFaults::new(0, false));
    let store = std::sync::Arc::new(
        EventStore::open_with_faults(&db, std::sync::Arc::clone(&faults)).unwrap(),
    );
    let hub = EventHub::new(EVENT_HUB_CAPACITY);
    let services = EventServices::new(store.clone(), std::sync::Arc::clone(&hub));
    let mgr = std::sync::Arc::new(MesaManager::discover(&drivers_dir()));
    mgr.set_event_services(std::sync::Arc::clone(&services));
    let ep = "hd-stop-gate";

    // Hub 精确送达的见证者：start 之前订阅（broadcast 迟到者收不到旧消息，
    // 订阅必须在生产之前）。结尾一次性 try_recv 排空：本测试总量远小于 Hub
    // 容量（256），Lagged 不可能；比后台排空任务少一个 abort 间隙，无误红窗口。
    let mut hub_rx = hub.subscribe();

    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: ep.into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![],
        event_tasks: vec![EventTask {
            id: "cnt".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(50),
            binding: DriverBinding {
                kind: GENERIC_EVENT_BINDING_KIND.into(),
                config: serde_json::to_value(&binding).unwrap(),
            },
        }],
    })
    .unwrap();

    // commit 流动证据（栅栏布防前生产正常，不是起不来）。
    common::wait_until(30, || rows_of(&store, ep).len() >= 2).await;

    // 布防 → 等 writer 到达栅栏（commit 已发送、执行中被暂停；entered 计数
    // 让"暂停已生效"可观测，不赌"commit 已经发出来了"）。
    let gate = faults.arm_commit_gate();
    common::wait_until(10, || gate.entered() >= 1).await;

    // 并发 Stop：栅栏关闭期间必须等待，不得提前返回（成功或超时都是 bug）。
    let stop_fut = mgr.stop_endpoint(ep);
    tokio::pin!(stop_fut);
    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut stop_fut)
            .await
            .is_err(),
        "栅栏关闭时 Stop 必须等待 in-flight commit，不得提前返回",
    );

    // 判别 hold：继续卡住到 Stop 发起后约 12s。旧 5s timer（起于
    // shutdown_ingress）必在此期间先赢 → 旧代码确定性 TIMEOUT 红；
    // 新 15s timer 不可能赢 → 放行后确定性成功绿。见函数头推导。
    tokio::time::sleep(Duration::from_secs(12)).await;

    // 放行 → Stop 成功（有界）。
    gate.release();
    let res = tokio::time::timeout(Duration::from_secs(30), stop_fut)
        .await
        .expect("放行后 Stop 必须有界返回");
    assert_eq!(res, Ok(true), "放行后 Stop 必须干净成功，实际: {res:?}");

    // 精确：DB 行连续 + persisted 计数 == 行数。
    let rows = rows_of(&store, ep);
    let mut ns: Vec<u64> = rows
        .iter()
        .map(|r| {
            let v: serde_json::Value = serde_json::from_str(&r.attributes_json).unwrap();
            v["value"]["U64"].as_u64().unwrap()
        })
        .collect();
    ns.sort_unstable();
    let max = *ns.last().unwrap();
    assert_eq!(ns.len() as u64, max, "无丢失无重复");
    for w in ns.windows(2) {
        assert_eq!(w[1], w[0] + 1, "n 必须连续");
    }
    {
        use std::sync::atomic::Ordering;
        assert_eq!(
            services
                .diagnostics
                .ingress_persisted_events_total
                .load(Ordering::Relaxed),
            rows.len() as u64,
            "persisted 必须等于行数",
        );
    }

    // 精确：Hub 收到的 == DB 落盘的（commit-then-publish 原子性）。
    // Stop 后 2s 静默窗口：无新生产；然后排空 Hub 比较；同时验证 Stop 后无新写。
    tokio::time::sleep(Duration::from_secs(2)).await;
    let quiet_rows = rows_of(&store, ep).len();
    {
        use std::sync::atomic::Ordering;
        let quiet_persisted = services
            .diagnostics
            .ingress_persisted_events_total
            .load(Ordering::Relaxed);
        assert_eq!(quiet_rows, rows.len(), "Stop 后 DB 不得再出新行");
        assert_eq!(
            quiet_persisted,
            rows.len() as u64,
            "Stop 后 persisted 不得再涨"
        );
    }
    let mut got: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    loop {
        match hub_rx.try_recv() {
            Ok(ev) => {
                got.insert(ev.event_id.clone());
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(e) => panic!("Hub 排空不得失败（总量远小于容量），实际: {e:?}"),
        }
    }
    let want: std::collections::BTreeSet<String> =
        rows.iter().map(|r| r.event_id.clone()).collect();
    assert_eq!(got, want, "Hub 必须恰好收到 DB 全部行（不多不少）");
    let _ = std::fs::remove_file(&db);
}
