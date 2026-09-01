//! ResourceSelection 契约（V2.1 §15, Milestone C）
//! 验证 mesa.resources.v1 通用绑定与 Legacy 兼容、point_key 唯一、资源/输出存在等。

mod common;

use mesa_core_types::{
    AcquisitionTask, DriverBinding, GenericBinding, ResourceSelection, SelectedOutput, TaskMode,
};
use mesa_driver_sdk::Driver;
use serde_json::json;

// 辅助：构造通用任务
fn generic_task(id: &str, selections: Vec<ResourceSelection>) -> AcquisitionTask {
    AcquisitionTask {
        id: id.into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: mesa_core_types::GENERIC_BINDING_KIND.into(),
            config: serde_json::to_value(GenericBinding { selections }).unwrap(),
        },
    }
}

#[tokio::test]
async fn simulator_generic_single_point_ok() {
    let mut conn = mesa_driver_simulator::SimulatorDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    // 使用 Driver trait 的 open_connection 返回 SimConnection，需通过 trait 对象调用 configure
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "counter".into(),
            parameters: json!({}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "sim.counter".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "sim.counter");
}

#[tokio::test]
async fn simulator_generic_duplicate_point_key_rejected() {
    let mut conn = mesa_driver_simulator::SimulatorDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let task = generic_task(
        "t1",
        vec![
            ResourceSelection {
                resource_id: "counter".into(),
                parameters: json!({}),
                outputs: vec![SelectedOutput {
                    output: "value".into(),
                    point_key: "dup".into(),
                }],
            },
            ResourceSelection {
                resource_id: "sine".into(),
                parameters: json!({}),
                outputs: vec![SelectedOutput {
                    output: "value".into(),
                    point_key: "dup".into(),
                }],
            },
        ],
    );
    let err = conn.configure(1, vec![task]).await.unwrap_err();
    assert!(
        err.code == "DUPLICATE_POINT_KEY" || err.code == "INVALID_BINDING_CONFIG",
        "expected duplicate rejection, got {}",
        err.code
    );
}

#[tokio::test]
async fn s7_generic_memory_ok() {
    let mut conn = mesa_driver_s7::S7Driver
        .open_connection("ep1", r#"{"host":"127.0.0.1"}"#)
        .await
        .unwrap();
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "memory".into(),
            parameters: json!({"address":"DB10.DBD0","data_type":"REAL"}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "motor.speed".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "motor.speed");
}

#[tokio::test]
async fn focas_generic_status_ok() {
    let mut conn = mesa_driver_focas2::FocasDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "status".into(),
            parameters: json!({"address":"status","data_type":"U32"}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "cnc.status".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "cnc.status");
}

#[tokio::test]
async fn opcua_generic_node_ok() {
    let mut conn = mesa_driver_opcua::OpcUaDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "node".into(),
            parameters: json!({"node_id":"ns=2;i=2","data_type":"U32"}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "opc.counter".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "opc.counter");
}

#[tokio::test]
async fn legacy_still_works_for_all_drivers() {
    // Simulator legacy
    let mut sim = mesa_driver_simulator::SimulatorDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let legacy_sim = AcquisitionTask {
        id: "t1".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: mesa_driver_simulator::BINDING_KIND.into(),
            config: json!({"points":[{"key":"a","kind":"counter"}]}),
        },
    };
    assert!(sim.configure(1, vec![legacy_sim]).await.is_ok());

    // S7 legacy
    let mut s7 = mesa_driver_s7::S7Driver
        .open_connection("ep1", r#"{"host":"127.0.0.1"}"#)
        .await
        .unwrap();
    let legacy_s7 = AcquisitionTask {
        id: "t1".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: mesa_driver_s7::BINDING_KIND.into(),
            config: json!({"items":[{"key":"a","address":"DB10.DBD0","data_type":"REAL"}]}),
        },
    };
    assert!(s7.configure(1, vec![legacy_s7]).await.is_ok());

    // FOCAS legacy
    let mut focas = mesa_driver_focas2::FocasDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let legacy_focas = AcquisitionTask {
        id: "t1".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: mesa_driver_focas2::BINDING_KIND.into(),
            config: json!({"items":[{"key":"a","address":"status","data_type":"U32"}]}),
        },
    };
    assert!(focas.configure(1, vec![legacy_focas]).await.is_ok());

    // OPC UA legacy
    let mut opcua = mesa_driver_opcua::OpcUaDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let legacy_opcua = AcquisitionTask {
        id: "t1".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: mesa_driver_opcua::BINDING_POLL.into(),
            config: json!({"nodes":[{"key":"a","node_id":"ns=2;i=2","data_type":"U32"}]}),
        },
    };
    assert!(opcua.configure(1, vec![legacy_opcua]).await.is_ok());
}

#[test]
fn generic_binding_structure_validation() {
    // 空 resource_id 拒绝
    let bad = GenericBinding {
        selections: vec![ResourceSelection {
            resource_id: "".into(),
            parameters: json!({}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "k1".into(),
            }],
        }],
    };
    assert!(mesa_core_types::validate_selections_structure(&bad.selections).is_err());

    // point_key 重复拒绝
    let dup = GenericBinding {
        selections: vec![
            ResourceSelection {
                resource_id: "counter".into(),
                parameters: json!({}),
                outputs: vec![SelectedOutput {
                    output: "value".into(),
                    point_key: "dup".into(),
                }],
            },
            ResourceSelection {
                resource_id: "sine".into(),
                parameters: json!({}),
                outputs: vec![SelectedOutput {
                    output: "value".into(),
                    point_key: "dup".into(),
                }],
            },
        ],
    };
    assert!(mesa_core_types::validate_selections_structure(&dup.selections).is_err());
}
