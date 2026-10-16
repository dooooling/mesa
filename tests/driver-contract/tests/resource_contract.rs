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
    let conn = mesa_test_driver::TestDriver
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
                point_key: "test.counter".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "test.counter");
}

#[tokio::test]
async fn simulator_generic_duplicate_point_key_rejected() {
    let conn = mesa_test_driver::TestDriver
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
    let conn = mesa_driver_s7::S7Driver
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
    let conn = mesa_driver_focas2::FocasDriver
        .open_connection("ep1", r#"{"host":"127.0.0.1"}"#)
        .await
        .unwrap();
    // Resource Model Cleanup：machine/status（旧 status/value 已删除）。
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "machine".into(),
            parameters: json!({}),
            outputs: vec![SelectedOutput {
                output: "status".into(),
                point_key: "cnc.status".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].point_key, "cnc.status");
    assert_eq!(descs[0].data_type, mesa_core_types::DataType::U32);
    assert_eq!(
        descs[0].source_label.as_deref(),
        Some("machine.status"),
        "来源必须准确到真实地址"
    );
}

#[tokio::test]
async fn focas_generic_axis_absolute_ok() {
    let conn = mesa_driver_focas2::FocasDriver
        .open_connection("ep1", r#"{"host":"127.0.0.1"}"#)
        .await
        .unwrap();
    // axis/absolute（旧 axis/value、dynamic/position.absolute 已删除）。
    let task = generic_task(
        "t1",
        vec![ResourceSelection {
            resource_id: "axis".into(),
            parameters: json!({"axis": 1}),
            outputs: vec![SelectedOutput {
                output: "absolute".into(),
                point_key: "cnc.x.absolute".into(),
            }],
        }],
    );
    let descs = conn.configure(1, vec![task]).await.unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].data_type, mesa_core_types::DataType::I32);
    assert_eq!(descs[0].source_label.as_deref(), Some("axis[1].absolute"));
}

#[tokio::test]
async fn focas_generic_legacy_shapes_rejected() {
    let conn = mesa_driver_focas2::FocasDriver
        .open_connection("ep1", r#"{"host":"127.0.0.1"}"#)
        .await
        .unwrap();
    // 旧形态无 alias：dynamic、status/value、axis/value 一律拒绝。
    for (resource_id, parameters, output) in [
        ("dynamic", json!({}), "feed"),
        ("status", json!({}), "value"),
        ("axis", json!({"axis": 1}), "value"),
        ("spindle", json!({"spindle": 1}), "value"),
        ("program", json!({}), "value"),
    ] {
        let task = generic_task(
            "t1",
            vec![ResourceSelection {
                resource_id: resource_id.into(),
                parameters,
                outputs: vec![SelectedOutput {
                    output: output.into(),
                    point_key: "k".into(),
                }],
            }],
        );
        let err = conn.configure(1, vec![task]).await.unwrap_err();
        assert!(
            err.code == "UNSUPPORTED_RESOURCE" || err.code == "UNKNOWN_OUTPUT",
            "{resource_id}/{output} 必须拒绝，got {}",
            err.code
        );
    }
}

#[tokio::test]
async fn opcua_generic_node_ok() {
    let conn = mesa_driver_opcua::OpcUaDriver
        .open_connection("ep1", r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840"}"#)
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
        "test-driver.points",
        "s7.address-group",
        "focas.data-block",
        "opcua.node-group",
        "opcua.subscription",
        "opcua.browse",
        "test-driver.events",
    ];
    let sim = mesa_test_driver::TestDriver
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
    let s7 = mesa_driver_s7::S7Driver
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
    let focas = mesa_driver_focas2::FocasDriver
        .open_connection("ep1", r#"{"host":"127.0.0.1"}"#)
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
    let opcua = mesa_driver_opcua::OpcUaDriver
        .open_connection("ep1", r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840"}"#)
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

// ---------------------------------------------------------------------------
// PR-A P9 护栏：Core 零 Siemens 知识
// ---------------------------------------------------------------------------

/// 递归收集目录下的 `.rs` 文件。
fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// P9 护栏（设计文档 §5.1）：`crates/core-types/**` 与 `crates/core-api/**`
/// **不得出现** NCK Driver 侧的 Siemens 专有类型 / 身份契约标识。
///
/// 唯一允许出现的 NCK 标识是**任务层**常量 `nck.points.v1`
/// （`NCK_POINTS_BINDING_KIND`）——它属于 §2.0 术语表的 **Task Binding Kind**，
/// 与点位层 **Point Kind** 是两个层级，故不在此禁令内。
///
/// 扫描口径：只扫**生产代码**（第一个 `#[cfg(test)]` 之前）并剔除行注释。
/// 之所以排除测试代码：`nck_variable`/`nck_native` 恰好是「Core **不**建立
/// Point Kind 白名单」这一结论的**证据字面量**（未知 kind 被放行的用例必须提到它们）。
#[test]
fn core_has_zero_siemens_knowledge() {
    const FORBIDDEN: &[&str] = &[
        // NCK Driver 侧专有类型（§2.4.1 / §2.5）
        "TargetRef",
        "DriveObjectRef",
        "DriveEntry",
        "DriveEntryModule",
        "NativeFieldSource",
        "AddressingRule",
        "VariableDefinition",
        "DefinitionCatalog",
        "AddressCompiler",
        // 身份契约与身份推导（§4.1.1 C/D，归 Driver）
        "identity_template",
        "identity_keys",
        "canonical_identity",
        "identity_mode",
        // 参数层 / Driver 专属错误码（Core 不得产生）
        "POINT_KEY_MISMATCH",
        "UNSUPPORTED_POINT_KIND",
        "UNKNOWN_VARIABLE",
        "UNSUPPORTED_DEFINITION_REVISION",
        "MISSING_TARGET",
        "INVALID_TARGET",
        "INVALID_ARGUMENTS",
        // Point Kind 取值（Core 不得枚举）
        "nck_variable",
        "nck_native",
        // 驱动对象双入口（Core 不应知道 Module 26/130）
        "identity_spec",
        "drive_object",
    ];

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for dir in ["crates/core-types/src", "crates/core-api/src"] {
        collect_rs(&root.join(dir), &mut files);
    }
    assert!(
        files.len() >= 5,
        "必须扫到 Core 源码，实际 {} 个文件（路径基准 {})",
        files.len(),
        root.display()
    );

    let mut violations = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        // 生产代码 = 第一个 `#[cfg(test)]` 之前的部分。
        let prod = text.split("#[cfg(test)]").next().unwrap_or("");
        for (n, line) in prod.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for token in FORBIDDEN {
                if code.contains(token) {
                    violations.push(format!("{}:{}: `{token}`", f.display(), n + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Core 出现 Siemens 专有标识（P9 违规）：\n{}",
        violations.join("\n")
    );
}
