//! ResourceSelection 契约（V2.1 §15, Milestone C；Foundation-2 单路径收口）
//! 验证 mesa.resources.v1 通用绑定、point_key 唯一、资源/输出存在等。
//! legacy binding 已删除（ADR 0003），此处不再覆盖任何 legacy kind。

mod common;

use mesa_core_types::{
    AcquisitionTask, DriverBinding, GenericBinding, ResourceSelection, SelectedOutput, TaskSchedule,
};
use mesa_driver_sdk::Driver;
use serde_json::json;

// 辅助：构造通用任务
fn generic_task(id: &str, selections: Vec<ResourceSelection>) -> AcquisitionTask {
    AcquisitionTask {
        id: id.into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
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
    // canonical 结构化参数（address 字符串只属于 legacy，不进 generic）
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "memory".into(),
            parameters: json!({"area":"DB","db":10,"offset":0,"data_type":"REAL"}),
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
    // canonical：resource status 无参数（address/data_type 只属于 legacy）
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "status".into(),
            parameters: json!({}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "cnc.status".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "cnc.status");
    assert_eq!(descs[0].data_type, mesa_core_types::DataType::U32);
}

#[tokio::test]
async fn opcua_generic_node_ok() {
    let mut conn = mesa_driver_opcua::OpcUaDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    // canonical data_type（旧 "U32" 拼写只属于 legacy，不进 generic）
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "node".into(),
            parameters: json!({"node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"UINT32"}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "opc.counter".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "opc.counter");
    assert_eq!(descs[0].data_type, mesa_core_types::DataType::U32);

    // legacy ns= 索引形态 fail-closed（与 sinumerik 同口径）。
    let bad = generic_task(
        "t2",
        vec![ResourceSelection {
            resource_id: "node".into(),
            parameters: json!({"node_id":"ns=2;i=2","data_type":"UINT32"}),
            outputs: vec![SelectedOutput {
                output: "value".into(),
                point_key: "k".into(),
            }],
        }],
    );
    let err = conn.configure(2, vec![bad]).await.unwrap_err();
    assert_eq!(err.code, "INVALID_ADDRESS");
}

#[tokio::test]
async fn legacy_kinds_rejected_for_all_drivers() {
    // Foundation-2：legacy kind 已删除，非 generic 即 UNSUPPORTED_BINDING。
    // 被删 kind 字符串集中在此测试（生产 Driver 不再 export 相关常量）。
    const LEGACY_KINDS: &[&str] = &[
        "simulator.points",
        "s7.address-group",
        "focas.data-block",
        "opcua.node-group",
        "opcua.subscription",
        "opcua.browse",
        "simulator.events",
    ];
    let mut sim = mesa_driver_simulator::SimulatorDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let legacy_sim = AcquisitionTask {
        id: "t1".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: LEGACY_KINDS[0].into(),
            config: json!({"points":[{"key":"a","kind":"counter"}]}),
        },
    };
    assert_eq!(
        sim.configure(1, vec![legacy_sim]).await.unwrap_err().code,
        "UNSUPPORTED_BINDING"
    );

    // S7
    let mut s7 = mesa_driver_s7::S7Driver
        .open_connection("ep1", r#"{"host":"127.0.0.1"}"#)
        .await
        .unwrap();
    let legacy_s7 = AcquisitionTask {
        id: "t1".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: LEGACY_KINDS[1].into(),
            config: json!({"items":[{"key":"a","address":"DB10.DBD0","data_type":"REAL"}]}),
        },
    };
    assert_eq!(
        s7.configure(1, vec![legacy_s7]).await.unwrap_err().code,
        "UNSUPPORTED_BINDING"
    );

    // FOCAS
    let mut focas = mesa_driver_focas2::FocasDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    let legacy_focas = AcquisitionTask {
        id: "t1".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: LEGACY_KINDS[2].into(),
            config: json!({"items":[{"key":"a","address":"status","data_type":"U32"}]}),
        },
    };
    assert_eq!(
        focas
            .configure(1, vec![legacy_focas])
            .await
            .unwrap_err()
            .code,
        "UNSUPPORTED_BINDING"
    );

    // OPC UA legacy kinds
    let mut opcua = mesa_driver_opcua::OpcUaDriver
        .open_connection("ep1", "{}")
        .await
        .unwrap();
    for kind in [LEGACY_KINDS[3], LEGACY_KINDS[4], LEGACY_KINDS[5]] {
        let legacy_opcua = AcquisitionTask {
            id: "t1".into(),
            schedule: TaskSchedule::Poll { interval_ms: 100 },
            binding: DriverBinding {
                kind: kind.into(),
                config: json!({"nodes":[{"key":"a","node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"U32"}]}),
            },
        };
        assert_eq!(
            opcua
                .configure(1, vec![legacy_opcua])
                .await
                .unwrap_err()
                .code,
            "UNSUPPORTED_BINDING",
            "{kind}"
        );
    }
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
