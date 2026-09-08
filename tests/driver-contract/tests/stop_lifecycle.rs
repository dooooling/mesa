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

mod common;
mod event_common;

use std::time::Duration;

use event_common::{rows_of, tmp_db};
use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, TaskMode};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::BuiltinEndpoint;
use mesa_event_store::{EVENT_HUB_CAPACITY, EventHub, EventServices, EventStore};

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
