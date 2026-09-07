//! Stop-vs-failure lifecycle gate：driver 死亡后显式 Stop 必须有界、结果显式。
//!
//! 背景：heartbeat 标记 unresponsive 会跳过 graceful Shutdown post，而 Stop
//! barrier 的 ingress drain 是 5s fail-closed（`EVENT_DRAIN_TIMEOUT`）。当显式
//! Stop 与 failure/session-loss 竞争时，`stop_endpoint` 必须在预算内返回显式
//! 结论（`Ok(was_running)` 或带精确码的 `Err`），不能 hang、不能静默成功。
//! 本文件只锁该契约，不改生产语义、不放宽任何 timeout/断言。
//!
//! 触发器用 simulator 现有 `crash_after_batches` 故障（确定性进程死亡），
//! 不新增驱动故障种类。

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

/// driver 死亡后显式 Stop：40s 内必须返回显式结论，之后端点不得再运行。
/// data 任务触发 crash（2 批即死），event 任务保证 ingress/drain 路径被执行到。
#[tokio::test]
async fn stop_after_driver_death_is_bounded_and_explicit() {
    common::init_log();
    let db = tmp_db("stop-death");
    let _ = std::fs::remove_file(&db);
    let store = std::sync::Arc::new(EventStore::open(&db).unwrap());
    let services = EventServices::new(store.clone(), EventHub::new(EVENT_HUB_CAPACITY));
    let mgr = std::sync::Arc::new(MesaManager::discover(&drivers_dir()));
    mgr.set_event_services(std::sync::Arc::clone(&services));
    let snapshot = mgr.snapshot();
    let ep = "hd-stop-death";

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
    common::wait_until(30, || rows_of(&store, ep).len() >= 1).await;
    // 死亡证据：crash 后 snapshot 必现 RECONNECTING 窗口（Lost→退避间隙，
    // 50ms 轮询必抓到；注意是 RECONNECTING 而非 FAILED——后者只用于配置失败）。
    common::wait_until(
        60,
        || matches!(snapshot.endpoint(ep), Some(ref s) if s.state == "RECONNECTING"),
    )
    .await;

    // 与 failure 竞争的显式 Stop：40s（≈2× teardown 最坏预算）内必须返回，
    // 结论显式（Ok 布尔或带码 Err），不 hang、不静默。
    let res = tokio::time::timeout(Duration::from_secs(40), mgr.stop_endpoint(ep))
        .await
        .expect("Stop 与 failure 竞争时必须有界返回");
    match &res {
        Ok(was_running) => eprintln!("stop outcome: Ok({was_running})"),
        Err(msg) => {
            assert!(!msg.trim().is_empty(), "Err 必须带精确码，不能空消息");
            eprintln!("stop outcome: Err({msg})");
        }
    }
    assert!(
        !mgr.is_running(ep),
        "Stop 返回后端点不得再运行，实际 outcome={res:?}"
    );
    let _ = std::fs::remove_file(&db);
}
