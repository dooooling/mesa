//! SINUMERIK Manager 级 Data E2E（PR11 Final Gate）。
//!
//! 生产路径（缺一不可，全部真实）：
//! ```text
//! drivers/sinumerik/driver.toml（discovery）
//! → 真驱动子进程 mesa-driver-sinumerik（token/stdin/liveness 同生产）
//! → SDK wire（Hello/握手/Open/Configure/Apply/Start/Stop/Close）
//! → Configure（canonical 校验）→ Core 风格 PointMap → DataSink 盖戳
//! → DataBatch（wire）→ Stop
//! ```
//! 数据来自驱动 Fake 路径的确定性 fixture（`sinumerik_shaped_fake` 种子，
//! `MESA_ALLOW_FAKE_NATIVE=1` 门控），无需真机；锁的是整条真实路径，
//! 不是 fixture 本体（本体由驱动单测覆盖）。
//!
//! 注意：改过驱动代码后须先 `cargo build --workspace`（旧二进制静默失效）。

mod common;

use std::path::PathBuf;

use mesa_core_types::{
    AcquisitionTask, DriverBinding, GenericBinding, Quality, ResourceSelection, SelectedOutput,
    TaskMode, Value,
};
use mesa_driver_manager::manifest::DiscoveredDriver;
use mesa_driver_manager::process::DriverProcess;
use mesa_driver_manager::session::Session;

const SIEMENS_NS: &str = "http://www.siemens.com/sinumerik";

/// 允许测试显式使用 Fake 驱动（子进程继承本进程环境）。
fn allow_fake() {
    unsafe {
        std::env::set_var("MESA_ALLOW_FAKE_NATIVE", "1");
    }
}

fn fake_connection_json() -> String {
    r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840","use_native":false}"#.into()
}

/// 手工构造指向已构建二进制的 DiscoveredDriver（跳过目录扫描，聚焦行为本身）。
fn sinumerik_discovered() -> DiscoveredDriver {
    let exe: PathBuf = common::sinumerik_exe();
    DiscoveredDriver {
        manifest: mesa_driver_manager::manifest::DriverManifest {
            id: "sinumerik".into(),
            name: "SINUMERIK".into(),
            version: "0.0.0".into(), // 测试桩版本，仅用于 Hello 展示
            executable: exe.file_name().unwrap().to_string_lossy().to_string(),
            protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
            protocol_minor: mesa_driver_protocol::PROTOCOL_MINOR,
            sdk: None,
            os: None,
            arch: None,
        },
        manifest_dir: PathBuf::new(),
        executable_path: Some(exe),
        platform_ok: true,
        platform_reason: None,
        protocol_ok: true,
    }
}

fn poll_speed_task() -> AcquisitionTask {
    AcquisitionTask {
        id: "t-speed".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: mesa_core_types::GENERIC_BINDING_KIND.into(),
            config: serde_json::to_value(GenericBinding {
                selections: vec![ResourceSelection {
                    resource_id: "node".into(),
                    parameters: serde_json::json!({
                        "node_id": format!("nsu={SIEMENS_NS};s=Speed"),
                        "data_type": "F64",
                    }),
                    outputs: vec![SelectedOutput {
                        output: "value".into(),
                        point_key: "spindle.speed".into(),
                    }],
                }],
            })
            .unwrap(),
        },
    }
}

#[tokio::test]
async fn sinumerik_poll_e2e_through_real_subprocess() {
    common::init_log();
    allow_fake();
    let mut process = DriverProcess::spawn(&sinumerik_discovered())
        .await
        .expect("spawn sinumerik");
    let (mut session, mut events, _) = Session::connect_retry(process.port, &process.token)
        .await
        .expect("handshake with real sinumerik binary");

    // 驱动身份（wire）：sinumerik，不是 simulator/opcua。
    let (driver_id, _, _) = session.metadata().await.expect("metadata");
    assert_eq!(driver_id, "sinumerik");

    // 建连（Fake fixture 路径）→ Probe 走 wire 确认 SINUMERIK（high）。
    common::open_connection(&session, 1, &fake_connection_json()).await;
    let report = session.probe(1).await.expect("probe over wire");
    assert!(report.reachable);
    assert_eq!(report.family.as_deref(), Some("SINUMERIK"));
    assert_eq!(report.model_confidence.as_deref(), Some("high"));

    // Configure（canonical 校验）→ Core 风格 PointMap → Start。
    let descriptors = common::configure_tasks(&session, 1, 1, &[poll_speed_task()]).await;
    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].point_key, "spindle.speed");
    let map = common::sequential_ids(&descriptors, 5001);
    common::apply_point_map(&session, 1, 1, map).await;
    common::start_connection(&session, 1, 7).await;

    // DataBatch（wire）：SDK 盖戳 handle/epoch，值来自 fixture（Speed=1500.0）。
    let batch = common::recv_batch(&mut events, 10).await;
    assert_eq!(batch.connection_handle, 1);
    assert_eq!(batch.stream_epoch, 7);
    assert_eq!(batch.values.len(), 1);
    assert_eq!(batch.values[0].point_id, 5001);
    assert_eq!(batch.values[0].value, Value::F64(1500.0));
    assert_eq!(batch.values[0].quality, Quality::Good);

    // Stop → Close → 子进程回收（孤儿防护不断言此处，只保证干净退出）。
    assert!(common::stop_connection(&session, 1).await);
    common::close_connection(&session, 1).await;
    common::teardown(&mut session, None);
    process.terminate().await;
}

#[tokio::test]
async fn sinumerik_point_id_stable_across_stop_start() {
    // 真实路径 Stop → Start（新 epoch）：同一 canonical 同一 point_id。
    common::init_log();
    allow_fake();
    let mut process = DriverProcess::spawn(&sinumerik_discovered())
        .await
        .expect("spawn sinumerik");
    let (mut session, mut events, _) = Session::connect_retry(process.port, &process.token)
        .await
        .expect("handshake");

    common::open_connection(&session, 1, &fake_connection_json()).await;
    let descriptors = common::configure_tasks(&session, 1, 1, &[poll_speed_task()]).await;
    let map = common::sequential_ids(&descriptors, 6001);
    common::apply_point_map(&session, 1, 1, map).await;

    common::start_connection(&session, 1, 11).await;
    let first = common::recv_batch(&mut events, 10).await;
    assert_eq!(first.stream_epoch, 11);
    assert_eq!(first.values[0].point_id, 6001);

    assert!(common::stop_connection(&session, 1).await);
    common::start_connection(&session, 1, 12).await;
    let second = common::recv_batch(&mut events, 10).await;
    assert_eq!(second.stream_epoch, 12);
    assert_eq!(
        second.values[0].point_id, 6001,
        "Stop→Start 后 point_id 不漂"
    );

    assert!(common::stop_connection(&session, 1).await);
    common::close_connection(&session, 1).await;
    common::teardown(&mut session, None);
    process.terminate().await;
}
