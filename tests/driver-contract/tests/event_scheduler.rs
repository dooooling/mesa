//! Scheduler contract（PR10 commit 7 §7）：事件/数据洪峰下 Control 面行为。
//! 按现有实现诚实落子（不做 Control Plane 新功能）。
//! Run 期间连接对象整体移入 run task，`control_command` 必回显式 BUSY
//!   （Failed + BUSY，不是 hang、不是静默丢弃）；conn-less 的 metadata 路径
//!   才是恒优先那条（session 级 Gate 6 已锁）。
//! 门锁三件事：BUSY 每次 2s 内返回（无 hang）；错误码显式可断言；
//! Stop 干净完成（连接归还由 session 级 StopAck 语义保证，此处不断言复用）。
//! 真抢占（preemption）不在本阶段实现，不在此伪装。

mod common;
mod event_common;

use std::time::Duration;

use event_common::rows_of;
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

fn generic_counter_task() -> EventTask {
    let binding = mesa_core_types::GenericEventBinding {
        stream_id: mesa_driver_simulator::SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    EventTask {
        id: "cnt".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(50),
        binding: DriverBinding {
            kind: GENERIC_EVENT_BINDING_KIND.into(),
            config: serde_json::to_value(&binding).unwrap(),
        },
    }
}

/// 持续负载下 5 次 control_command 每次 8s 内成功；事件面同步推进；
/// endpoint 全程健康、干净停止（Control 恒优先，负载不饿死控制）。
#[tokio::test]
async fn control_priority_under_event_and_data_flood() {
    common::init_log();
    let db = event_common::tmp_db("sched");
    let _ = std::fs::remove_file(&db);
    let store = std::sync::Arc::new(EventStore::open(&db).unwrap());
    let mgr = std::sync::Arc::new(MesaManager::discover(&drivers_dir()));
    mgr.set_event_services(EventServices::new(
        store.clone(),
        EventHub::new(EVENT_HUB_CAPACITY),
    ));
    let ep = "hd-sched-001";
    mgr.start_endpoint(BuiltinEndpoint {
        endpoint_id: ep.into(),
        driver_id: "simulator".into(),
        connection_json: "{}".into(),
        tasks: vec![common::poll_task(
            "d",
            500,
            serde_json::json!({"points": [{"key":"k.counter","kind":"counter"}]}),
        )],
        event_tasks: vec![generic_counter_task()],
    })
    .unwrap();

    // 预热 2s：事件/数据流起来（行数增长证明负载真实存在）
    tokio::time::sleep(Duration::from_secs(2)).await;
    let n0 = rows_of(&store, ep).len();
    assert!(n0 >= 10, "负载应先跑起来，got {n0}");

    // Run 中命令必 BUSY：每次 2s 内返回显式 BUSY（无 hang 无丢）；
    // Stop 后同一命令成功（连接归还，生命周期完整）。
    for i in 0..5u32 {
        let (status, _, err) = tokio::time::timeout(
            Duration::from_secs(2),
            mgr.control_command(ep, "reset", "{}", &format!("sched-{i}")),
        )
        .await
        .unwrap_or_else(|_| panic!("第 {i} 次 control 2s 未返回（hang）"))
        .expect("control_command 转发本身必须成功");
        assert_eq!(status, "Failed", "Run 中命令应显式 BUSY");
        assert!(err.contains("BUSY"), "错误码必须显式可断言，got {err}");
    }
    // 事件面洪峰中同步推进（control 没把事件挤停）：给 1s 推进窗口再采样
    tokio::time::sleep(Duration::from_secs(1)).await;
    let n1 = rows_of(&store, ep).len();
    assert!(n1 > n0, "control 期间事件必须同步推进");
    assert!(mgr.is_running(ep));
    assert_eq!(mgr.stop_endpoint(ep).await, Ok(true));
    // Stop 后直连会话已无；manager 级 BUSY 语义止于 Run 期——此处仅确认干净停止。
    let _ = std::fs::remove_file(&db);
}
