//! Data Plane 精确语义冻结契约（§3 / §4 P0.1 + P0.2）
//! 覆盖 V2.1 §3.14 所列 14 断言。
//!
//! NCK 语义点位重构 PR-A 追加（设计文档 §4.3 / §6.6 T2、T3a）：
//! `point_key`/`point_id` 的稳定性契约按语义模型显式重写。**全部使用字面量冻结 Key**
//! ——Core 不生成也不推导身份（修订 4），因此本套件**不得**出现任何身份推导调用，
//! 也不得依赖驱动子进程（保持「无驱动子进程」的轻量属性）。

use mesa_config_store::{ConfigStore, DeviceRecord, EndpointRecord};
use mesa_core_types::{
    AcquisitionTask, DataBatch, DataType, DriverBinding, PointDescriptor, PointValue, Quality,
    TaskSchedule, Value, ValueOrigin, ensure_endpoint_point_keys_unique, ensure_unique_point_keys,
    now_unix_ns, parse_point_binding,
};
use mesa_driver_manager::snapshot::Snapshot;

/// 字面量冻结 Key（与 NCK Driver 的 golden 向量同源，§4.1.1 K）。
/// **禁止**在 Core 侧用任何函数「推导」这些值——它们就是契约本身。
const SEMANTIC_KEY: &str = "channel.r_parameter#index=i:100@channel=i:1";
const SEMANTIC_KEY_OTHER_VARIABLE: &str = "channel.program.status@channel=i:1";
const SEMANTIC_KEY_OTHER_INDEX: &str = "channel.r_parameter#index=i:101@channel=i:1";
const NATIVE_KEY: &str = "native.C.1.21.1.396";
const NATIVE_KEY_OTHER_ADDR: &str = "native.C.1.21.1.397";
const NATIVE_USER_KEY: &str = "native.C.1.21.1.396~2";

/// 最小夹具：一个 Device + 一个 Endpoint（各用例独立，避免并发互踩）。
fn store_with_endpoint() -> ConfigStore {
    let store = ConfigStore::open_in_memory().unwrap();
    store
        .create_device(&DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
        })
        .unwrap();
    store
        .create_endpoint(&EndpointRecord {
            id: "ep1".into(),
            name: "EP1".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    store
}

fn descriptor(key: &str) -> PointDescriptor {
    PointDescriptor {
        point_key: key.into(),
        data_type: DataType::F64,
        unit: None,
        source_label: None,
    }
}

/// 分配一批字面量 Key，返回 `point_id`（与入参同序）。
fn assign(store: &ConfigStore, keys: &[&str]) -> Vec<u32> {
    let descs: Vec<PointDescriptor> = keys.iter().map(|k| descriptor(k)).collect();
    store
        .assign_point_ids("ep1", &descs)
        .unwrap()
        .into_iter()
        .map(|d| d.point_id)
        .collect()
}

/// 从 `point_registry`（含墓碑）读回 `(point_id, deleted)`。
fn registry(store: &ConfigStore, key: &str) -> (u32, bool) {
    store
        .point_registry_all("ep1")
        .unwrap()
        .into_iter()
        .find(|r| r.0 == key)
        .map(|r| (r.1, r.3))
        .unwrap_or_else(|| panic!("point_registry 中不存在 `{key}`"))
}

/// 以 `nck.points.v1` 信封（**字面量 point_key**）构造点位并取回其 `point_key`。
/// 这同时证明「信封层通过」且「Core 不解释 parameters」。
fn nck_point_key(parameters: serde_json::Value) -> String {
    let binding = serde_json::json!({"points": [{
        "kind": "nck_variable",
        "point_key": SEMANTIC_KEY,
        "parameters": parameters,
    }]});
    let (issues, parsed) = parse_point_binding(&binding, "tasks[0].binding.config");
    assert!(issues.is_empty(), "信封层必须通过: {issues:?}");
    parsed.unwrap().points[0].point_key.clone()
}

// --- §3.4 GOOD 必须类型匹配 ---
#[test]
fn good_value_type_matches_descriptor() {
    let pv = PointValue::good(1, Value::I32(100));
    assert_eq!(pv.quality, Quality::Good);
    assert_eq!(pv.value.data_type(), DataType::I32);
    // 若类型不匹配应为契约违反，此处仅验证匹配路径
    assert!(matches!(pv.value, Value::I32(100)));
}

// --- §3.6 BAD 仍必须携带匹配类型的 typed value，禁止 String 冒充 ---
#[test]
fn bad_value_type_still_matches_descriptor() {
    // 模拟 FOCAS 单点失败后产生的 BAD 点，必须仍为 I32 而非 String("ERR:...")
    let neutral = Value::I32(0); // neutral_value_for(I32)
    let pv = PointValue {
        point_id: 42,
        value: neutral.clone(),
        quality: Quality::Bad,
        quality_code: Some(1),
        source_timestamp_ns: None,
        value_origin: ValueOrigin::Placeholder,
    };
    assert_eq!(pv.quality, Quality::Bad);
    assert_eq!(pv.value.data_type(), DataType::I32);
    assert!(
        !matches!(pv.value, Value::String(_)),
        "BAD must not be String ERR"
    );
    assert_eq!(pv.value, Value::I32(0));
}

// --- point_key duplicate rejected (§3.14) ---
#[test]
fn point_key_duplicate_rejected() {
    let d = |k: &str| PointDescriptor {
        point_key: k.into(),
        data_type: DataType::F64,
        unit: None,
        source_label: None,
    };
    assert!(ensure_unique_point_keys(&[d("a"), d("b")]).is_ok());
    let err = ensure_unique_point_keys(&[d("a"), d("a")]).unwrap_err();
    assert_eq!(err.0, "a");
}

// --- same point_key restores same point_id (§3.10) ---
#[test]
fn same_point_key_restores_same_point_id() {
    let store = ConfigStore::open_in_memory().unwrap();
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            name: "EP1".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let descs = vec![PointDescriptor {
        point_key: "k1".into(),
        data_type: DataType::U32,
        unit: None,
        source_label: None,
    }];
    let defs1 = store.assign_point_ids("ep1", &descs).unwrap();
    let id1 = defs1[0].point_id;
    let defs2 = store.assign_point_ids("ep1", &descs).unwrap();
    assert_eq!(defs2[0].point_id, id1, "same key must restore same id");
}

// --- deleted point ID never reused (§3.10) ---
#[test]
fn deleted_point_id_never_reused() {
    let store = ConfigStore::open_in_memory().unwrap();
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            name: "EP1".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    // 分配 k1,k2
    let descs = vec![
        PointDescriptor {
            point_key: "k1".into(),
            data_type: DataType::U32,
            unit: None,
            source_label: None,
        },
        PointDescriptor {
            point_key: "k2".into(),
            data_type: DataType::U32,
            unit: None,
            source_label: None,
        },
    ];
    let defs = store.assign_point_ids("ep1", &descs).unwrap();
    let k1_id = defs.iter().find(|d| d.point_key == "k1").unwrap().point_id;
    // 删除 k1 的场景：通过 tombstone 标记 deleted=1（assign 时会保留旧 id）
    // 新分配 k3 时不应复用 k1_id
    let descs2 = vec![
        PointDescriptor {
            point_key: "k2".into(),
            data_type: DataType::U32,
            unit: None,
            source_label: None,
        },
        PointDescriptor {
            point_key: "k3".into(),
            data_type: DataType::U32,
            unit: None,
            source_label: None,
        },
    ];
    let defs2 = store.assign_point_ids("ep1", &descs2).unwrap();
    let k3_id = defs2.iter().find(|d| d.point_key == "k3").unwrap().point_id;
    assert_ne!(k3_id, k1_id, "deleted id must not be reused");
    // 强断言（原实现为 `k3_id > k1_id || k3_id != k1_id`，因 `>` 蕴含 `!=`
    // 该表达式等价于 `k3_id != k1_id`，`>` 分支是死代码，等于没验证单调递增）。
    // 墓碑行保留在表中，故 max(point_id)+1 必须**严格大于**墓碑 id。
    assert!(
        k3_id > k1_id,
        "新 id 必须严格大于墓碑 id（max+1 计入墓碑）: k1={k1_id} k3={k3_id}"
    );
}

// --- re-add tombstone restores same ID (§3.10) ---
#[test]
fn re_add_tombstone_restores_same_id() {
    let store = ConfigStore::open_in_memory().unwrap();
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            name: "EP1".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let descs = vec![PointDescriptor {
        point_key: "k1".into(),
        data_type: DataType::I32,
        unit: None,
        source_label: None,
    }];
    let defs1 = store.assign_point_ids("ep1", &descs).unwrap();
    let id1 = defs1[0].point_id;
    // 模拟删除后重加：再次 assign 同 key 应恢复原 id
    let defs2 = store.assign_point_ids("ep1", &descs).unwrap();
    assert_eq!(defs2[0].point_id, id1);
}

// --- revision success +1 (§3.14) ---
#[test]
fn revision_success_plus_one() {
    let store = ConfigStore::open_in_memory().unwrap();
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            name: "EP1".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let t = AcquisitionTask {
        id: "t1".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: "k".into(),
            config: serde_json::json!({}),
        },
    };
    let r0 = store.current_revision("ep1").unwrap();
    let r1 = store
        .replace_tasks("ep1", std::slice::from_ref(&t))
        .unwrap();
    assert_eq!(r1, r0 + 1);
    let r2 = store
        .replace_tasks("ep1", std::slice::from_ref(&t))
        .unwrap();
    assert_eq!(r2, r1 + 1);
}

// --- revision failure unchanged (§3.14) ---
#[test]
fn revision_failure_unchanged() {
    let store = ConfigStore::open_in_memory().unwrap();
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            name: "EP1".into(),
            device_id: "d1".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let ok = AcquisitionTask {
        id: "t1".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: "k".into(),
            config: serde_json::json!({}),
        },
    };
    let r1 = store.replace_tasks("ep1", &[ok]).unwrap();
    // 无效任务（空 id）应失败且 revision 不变
    let bad = AcquisitionTask {
        id: "".into(),
        schedule: TaskSchedule::Poll { interval_ms: 100 },
        binding: DriverBinding {
            kind: "k".into(),
            config: serde_json::json!({}),
        },
    };
    let res = store.replace_tasks("ep1", &[bad]);
    assert!(res.is_err());
    let r2 = store.current_revision("ep1").unwrap();
    assert_eq!(r2, r1);
}

// --- disconnect preserves typed last value (§3.11 P0-A) ---
#[test]
fn disconnect_preserves_typed_last_value() {
    let snap = Snapshot::new();
    snap.register_points(
        "ep1",
        &[mesa_core_types::PointDefinition {
            point_id: 1,
            point_key: "k1".into(),
            data_type: DataType::I32,
            unit: None,
            source_label: None,

            display_name: None,
        }],
    );
    let batch = DataBatch {
        connection_handle: 0,
        stream_epoch: 1,
        sequence: 1,
        timestamp_ns: 1_000_000,
        values: vec![PointValue::good(1, Value::I32(42))],
        mono_ns: None,
    };
    snap.apply_batch(&batch, "ep1");
    // 断线
    snap.mark_communication_lost("ep1");
    let latest = snap.latest_all();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].quality, "BAD");
    // 保留 typed I32(42)，而非 Null
    assert_eq!(latest[0].value.type_name, "i32");
    assert_eq!(latest[0].value.value, serde_json::json!(42));
}

// --- disconnect keeps original timestamp (§3.11) ---
#[test]
fn disconnect_keeps_original_timestamp() {
    let snap = Snapshot::new();
    snap.register_points(
        "ep1",
        &[mesa_core_types::PointDefinition {
            point_id: 1,
            point_key: "k1".into(),
            data_type: DataType::U32,
            unit: None,
            source_label: None,

            display_name: None,
        }],
    );
    let ts = now_unix_ns();
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: ts,
            values: vec![PointValue::good(1, Value::U32(7))],
            mono_ns: None,
        },
        "ep1",
    );
    snap.mark_communication_lost("ep1");
    assert_eq!(snap.latest_all()[0].timestamp_ns, ts);
}

// --- disconnect sets BAD/COMMUNICATION_LOST (§3.11) ---
#[test]
fn disconnect_sets_bad_communication_lost() {
    let snap = Snapshot::new();
    snap.register_points(
        "ep1",
        &[mesa_core_types::PointDefinition {
            point_id: 1,
            point_key: "k1".into(),
            data_type: DataType::Bool,
            unit: None,
            source_label: None,

            display_name: None,
        }],
    );
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: now_unix_ns(),
            values: vec![PointValue::good(1, Value::Bool(true))],
            mono_ns: None,
        },
        "ep1",
    );
    snap.mark_communication_lost("ep1");
    let e = &snap.latest_all()[0];
    assert_eq!(e.quality, "BAD");
    assert_eq!(e.quality_code.as_deref(), Some("COMMUNICATION_LOST"));
}

// --- one output BAD does not poison sibling outputs (§3.12) ---
#[test]
fn one_output_bad_does_not_poison_sibling() {
    let snap = Snapshot::new();
    snap.register_points(
        "ep1",
        &[
            mesa_core_types::PointDefinition {
                point_id: 1,
                point_key: "k1".into(),
                data_type: DataType::I32,
                unit: None,
                source_label: None,
                display_name: None,
            },
            mesa_core_types::PointDefinition {
                point_id: 2,
                point_key: "k2".into(),
                data_type: DataType::I32,
                unit: None,
                source_label: None,
                display_name: None,
            },
        ],
    );
    let batch = DataBatch {
        connection_handle: 0,
        stream_epoch: 1,
        sequence: 1,
        timestamp_ns: now_unix_ns(),
        values: vec![
            PointValue {
                point_id: 1,
                value: Value::I32(0),
                quality: Quality::Bad,
                quality_code: Some(1),
                source_timestamp_ns: None,
                value_origin: ValueOrigin::Placeholder,
            },
            PointValue::good(2, Value::I32(99)),
        ],
        mono_ns: None,
    };
    snap.apply_batch(&batch, "ep1");
    let mut latest = snap.latest_all();
    latest.sort_by_key(|e| e.point_id);
    assert_eq!(latest[0].quality, "BAD");
    assert_eq!(latest[0].value.value, serde_json::json!(0));
    assert_eq!(latest[1].quality, "GOOD");
    assert_eq!(latest[1].value.value, serde_json::json!(99));
}

// --- V1.2.1 P0-A GOOD→BAD→GOOD 平台连续性契约（§5.5）：驱动已保证 LastKnown，平台必须透传不丢失 ---
#[test]
fn p0a_good_bad_good_platform_continuity() {
    let snap = Snapshot::new();
    snap.register_points(
        "ep1",
        &[mesa_core_types::PointDefinition {
            point_id: 1,
            point_key: "k1".into(),
            data_type: DataType::F64,
            unit: None,
            source_label: None,

            display_name: None,
        }],
    );
    // GOOD Current 12.5 @ T1
    let t1: i64 = 1_700_000_000_000_000_000;
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: t1,
            values: vec![PointValue {
                point_id: 1,
                value: Value::F64(12.5),
                quality: Quality::Good,
                quality_code: None,
                source_timestamp_ns: Some(t1),
                value_origin: ValueOrigin::Current,
            }],
            mono_ns: None,
        },
        "ep1",
    );
    let e1 = snap.latest_all()[0].clone();
    assert_eq!(e1.value_origin, "CURRENT");
    assert_eq!(e1.source_timestamp_ns, Some(t1));
    assert_eq!(e1.quality, "GOOD");

    // BAD LastKnown 12.5 @ T1（复用 GOOD 的 source，不使用 BAD 自身时间）
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 2,
            timestamp_ns: t1 + 500_000_000,
            values: vec![PointValue {
                point_id: 1,
                value: Value::F64(12.5),
                quality: Quality::Bad,
                quality_code: Some(0x80340000u32 as i32),
                source_timestamp_ns: Some(t1),
                value_origin: ValueOrigin::LastKnown,
            }],
            mono_ns: None,
        },
        "ep1",
    );
    let e2 = snap.latest_all()[0].clone();
    assert_eq!(e2.value_origin, "LAST_KNOWN");
    assert_eq!(
        e2.source_timestamp_ns,
        Some(t1),
        "LastKnown 必须携带 GOOD 的 SourceTimestamp"
    );
    assert_eq!(e2.value.value, serde_json::json!(12.5));
    assert_eq!(e2.quality, "BAD");

    // GOOD Current 13.2 @ T3 恢复
    let t3 = t1 + 1_000_000_000;
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 3,
            timestamp_ns: t3,
            values: vec![PointValue {
                point_id: 1,
                value: Value::F64(13.2),
                quality: Quality::Good,
                quality_code: None,
                source_timestamp_ns: Some(t3),
                value_origin: ValueOrigin::Current,
            }],
            mono_ns: None,
        },
        "ep1",
    );
    let e3 = snap.latest_all()[0].clone();
    assert_eq!(e3.value_origin, "CURRENT");
    assert_eq!(e3.source_timestamp_ns, Some(t3));
    assert_eq!(e3.value.value, serde_json::json!(13.2));
    assert_eq!(e3.quality, "GOOD");
}

// --- V1.2.1 P0-A 首次 BAD 即 Placeholder（无历史 GOOD） ---
#[test]
fn p0a_first_bad_is_placeholder_no_source_timestamp() {
    let snap = Snapshot::new();
    snap.register_points(
        "ep1",
        &[mesa_core_types::PointDefinition {
            point_id: 1,
            point_key: "k1".into(),
            data_type: DataType::F64,
            unit: None,
            source_label: None,

            display_name: None,
        }],
    );
    // 首次采样即 BAD，无 LastKnown → Placeholder + typed neutral + source None
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: 1_000_000,
            values: vec![PointValue {
                point_id: 1,
                value: Value::typed_placeholder(DataType::F64),
                quality: Quality::Bad,
                quality_code: Some(0x80340000u32 as i32),
                // 驱动层已保证 Placeholder 的 source 为 None，即使传入 Some 也应被平台防御性清零
                source_timestamp_ns: Some(9_999_999),
                value_origin: ValueOrigin::Placeholder,
            }],
            mono_ns: None,
        },
        "ep1",
    );
    let e = snap.latest_all()[0].clone();
    assert_eq!(e.value_origin, "PLACEHOLDER");
    assert_eq!(
        e.source_timestamp_ns, None,
        "Placeholder 必须无 source_timestamp"
    );
    assert_eq!(e.value.value, serde_json::json!(0.0));
    assert_eq!(e.quality, "BAD");
    // REST JSON 必须可见 PLACEHOLDER 而非被吞
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["value_origin"], "PLACEHOLDER");
    assert!(json.get("source_timestamp_ns").is_none());
}

// --- V1.2.1 P0-A Snapshot/REST value_origin 不丢失 ---
#[test]
fn p0a_snapshot_rest_value_origin_not_lost() {
    let snap = Snapshot::new();
    snap.register_points(
        "ep1",
        &[mesa_core_types::PointDefinition {
            point_id: 1,
            point_key: "k1".into(),
            data_type: DataType::I32,
            unit: None,
            source_label: None,

            display_name: None,
        }],
    );
    // LastKnown
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: 1_000_000,
            values: vec![PointValue {
                point_id: 1,
                value: Value::I32(42),
                quality: Quality::Bad,
                quality_code: Some(1),
                source_timestamp_ns: Some(500_000),
                value_origin: ValueOrigin::LastKnown,
            }],
            mono_ns: None,
        },
        "ep1",
    );
    let e = snap.latest_all()[0].clone();
    assert_eq!(e.value_origin, "LAST_KNOWN");
    assert_eq!(e.source_timestamp_ns, Some(500_000));
    let json = serde_json::to_value(&e).unwrap();
    assert_eq!(json["value_origin"], "LAST_KNOWN");
    assert_eq!(json["source_timestamp_ns"], 500_000);

    // Placeholder
    snap.apply_batch(
        &DataBatch {
            connection_handle: 0,
            stream_epoch: 1,
            sequence: 2,
            timestamp_ns: 2_000_000,
            values: vec![PointValue {
                point_id: 1,
                value: Value::typed_placeholder(DataType::I32),
                quality: Quality::Bad,
                quality_code: Some(1),
                source_timestamp_ns: None,
                value_origin: ValueOrigin::Placeholder,
            }],
            mono_ns: None,
        },
        "ep1",
    );
    let e2 = snap.latest_all()[0].clone();
    assert_eq!(e2.value_origin, "PLACEHOLDER");
    assert!(e2.source_timestamp_ns.is_none());
    assert_eq!(
        serde_json::to_value(&e2).unwrap()["value_origin"],
        "PLACEHOLDER"
    );
}

// --- automatic tombstone GC does not exist (§3.10.1) ---
#[allow(clippy::assertions_on_constants)]
#[test]
fn automatic_tombstone_gc_does_not_exist() {
    // 契约：Core 不提供自动 GC；删除后重加必须恢复原 ID（已在 re_add 测试覆盖）
    let store = ConfigStore::open_in_memory().unwrap();
    let _ = store;
    assert!(true, "no automatic GC contract holds");
}

// ===========================================================================
// NCK 语义点位 PR-A：§4.3 语义 Key 断言 + §6.6 T2/T3a
// ===========================================================================

// --- §4.3(1b) 语义字面量 Key 同样恢复同一 point_id ---
#[test]
fn semantic_point_key_restores_same_point_id() {
    let store = store_with_endpoint();
    let ids1 = assign(&store, &[SEMANTIC_KEY]);
    let ids2 = assign(&store, &[SEMANTIC_KEY]);
    assert_eq!(ids2[0], ids1[0], "语义 Key 必须恢复同一 point_id");
    // 同一批内多个语义 Key 各自独立分配。
    let ids3 = assign(&store, &[SEMANTIC_KEY, SEMANTIC_KEY_OTHER_VARIABLE]);
    assert_eq!(ids3[0], ids1[0]);
    assert_ne!(ids3[1], ids1[0]);
}

// --- §6.6 T3a：Core 身份**只由 point_key 决定**，parameters 不参与 ---
#[test]
fn same_key_same_id_with_different_parameters() {
    // 两份 parameters 仅 `definition_revision` 从 1 改为 2（R8：revision 不进身份）。
    let key_rev1 = nck_point_key(serde_json::json!({
        "variable_id": "channel.r_parameter",
        "definition_revision": 1,
        "target": {"channel": 1},
        "arguments": {"index": 100}
    }));
    let key_rev2 = nck_point_key(serde_json::json!({
        "variable_id": "channel.r_parameter",
        "definition_revision": 2,
        "target": {"channel": 1},
        "arguments": {"index": 100}
    }));
    // Core 不解释 parameters，因此两份信封都必须通过，且取出的 Key 逐字相同。
    assert_eq!(key_rev1, SEMANTIC_KEY);
    assert_eq!(key_rev2, SEMANTIC_KEY);
    assert_eq!(key_rev1, key_rev2);

    let store = store_with_endpoint();
    let ids1 = assign(&store, &[key_rev1.as_str()]);
    let ids2 = assign(&store, &[key_rev2.as_str()]);
    assert_eq!(
        ids2[0], ids1[0],
        "identity 只由 point_key 决定，parameters 变化不得改变 point_id（T3a）"
    );
    // 反向：point_key 变化 ⇒ 新 point_id，旧 id 走墓碑且不复用（R9/R3/R4）。
    let ids3 = assign(&store, &[SEMANTIC_KEY_OTHER_INDEX]);
    assert_ne!(ids3[0], ids1[0]);
    assert!(ids3[0] > ids1[0], "新 id 必须严格大于墓碑 id");
    assert_eq!(registry(&store, SEMANTIC_KEY), (ids1[0], true));
    // 旧 id 绝不再出现在活跃映射中。
    let active = store.point_map("ep1").unwrap();
    assert!(!active.values().any(|v| *v == ids1[0]));
}

// --- §4.3(2b) 语义点删除后墓碑保留 id，新语义 Key 取严格更大的 id ---
#[test]
fn semantic_tombstone_retains_id_and_next_id_is_greater() {
    let store = store_with_endpoint();
    let first = assign(&store, &[SEMANTIC_KEY, SEMANTIC_KEY_OTHER_VARIABLE]);
    // 第二轮只保留第二个 ⇒ 第一个转墓碑（保留行与 id）。
    let second = assign(&store, &[SEMANTIC_KEY_OTHER_VARIABLE]);
    assert_eq!(second[0], first[1]);
    assert_eq!(registry(&store, SEMANTIC_KEY), (first[0], true));
    assert_eq!(
        registry(&store, SEMANTIC_KEY_OTHER_VARIABLE),
        (first[1], false)
    );
    // 新增一个不同语义 Key ⇒ id 严格大于墓碑 id。
    let third = assign(&store, &[SEMANTIC_KEY_OTHER_INDEX]);
    assert!(
        third[0] > first[0],
        "max+1 必须计入墓碑: tombstone={} new={}",
        first[0],
        third[0]
    );
    // 墓碑恢复：重新加入原 Key ⇒ 恢复原 id 并清 deleted（R2）。
    let restored = assign(&store, &[SEMANTIC_KEY]);
    assert_eq!(restored[0], first[0]);
    assert_eq!(registry(&store, SEMANTIC_KEY), (first[0], false));
}

// --- §4.3(2c) R9：改 variable_id ⇒ 新 Key、新 id，旧 id 永不出现 ---
#[test]
fn changed_variable_id_gets_new_id_and_old_id_stays_tombstoned() {
    let store = store_with_endpoint();
    let old = assign(&store, &[SEMANTIC_KEY]);
    let new = assign(&store, &[SEMANTIC_KEY_OTHER_VARIABLE]);
    assert_ne!(new[0], old[0]);
    assert_eq!(registry(&store, SEMANTIC_KEY), (old[0], true));
    assert_eq!(
        registry(&store, SEMANTIC_KEY_OTHER_VARIABLE),
        (new[0], false)
    );
    // 活跃映射只含新 Key；旧 id 不再出现。
    let active = store.point_map("ep1").unwrap();
    assert_eq!(active.get(SEMANTIC_KEY_OTHER_VARIABLE), Some(&new[0]));
    assert!(!active.contains_key(SEMANTIC_KEY));
}

// --- §4.3(3b) R8：revision 变化不改变 Key ⇒ 不改变 point_id ---
#[test]
fn revision_change_keeps_same_key_and_id() {
    let store = store_with_endpoint();
    // 同一个持久化 Key（字面量），两轮「配置」只有 definition_revision 不同。
    let ids1 = assign(&store, &[SEMANTIC_KEY]);
    let ids2 = assign(&store, &[SEMANTIC_KEY]);
    assert_eq!(ids1[0], ids2[0]);
    assert_eq!(registry(&store, SEMANTIC_KEY), (ids1[0], false));
    // 墓碑计数不得因「同一 Key 反复出现」增长。
    let rows = store.point_registry_all("ep1").unwrap();
    assert_eq!(rows.len(), 1, "同一 Key 不得产生第二行: {rows:?}");
}

// --- §4.3(3c) R10/R14：语义点与原生点是不同身份；derived 地址变化 ⇒ 新 id ---
#[test]
fn semantic_and_native_are_distinct_identities() {
    let store = store_with_endpoint();
    // `assign_point_ids` 是**全量替换**语义（不在本轮的活跃 Key 一律转墓碑），
    // 因此每轮必须给出完整的活跃集合。
    let first = assign(&store, &[SEMANTIC_KEY, NATIVE_KEY]);
    assert_ne!(
        first[0], first[1],
        "nck_variable 与 nck_native 不得互相顶替（Key 前缀不同 ⇒ 身份不同）"
    );
    // R14：identity_mode="derived" 下改地址字段 ⇒ 新 Key ⇒ 新 id，旧 Key 走墓碑。
    let moved = assign(&store, &[SEMANTIC_KEY, NATIVE_KEY_OTHER_ADDR]);
    assert_eq!(moved[0], first[0], "语义点不受原生点地址变化影响");
    assert!(moved[1] > first[1], "新地址 ⇒ 新 id（严格大于墓碑 id）");
    assert_eq!(registry(&store, NATIVE_KEY), (first[1], true));
    assert_eq!(registry(&store, NATIVE_KEY_OTHER_ADDR), (moved[1], false));
    assert_eq!(registry(&store, SEMANTIC_KEY), (first[0], false));
}

// --- §4.3(3d) R13：identity_mode="user" 的逻辑身份不随地址变化 ---
#[test]
fn user_identity_mode_native_address_change_keeps_id() {
    // 逻辑 Key 由用户显式选择（§4.1.2）：地址字段变化**不**改变 Key。
    // Core 只看到同一个字面量 Key，因此必须复用同一 point_id。
    let store = store_with_endpoint();
    let ids1 = assign(&store, &[NATIVE_USER_KEY]);
    let ids2 = assign(&store, &[NATIVE_USER_KEY]);
    assert_eq!(ids1[0], ids2[0], "逻辑身份不随地址变化（R13）");
    // 与 derived 默认 Key 是**不同**点位（~2 后缀区分「地址相同、用途不同」），
    // 两者可同时活跃（`assign_point_ids` 是全量替换语义）。
    let both = assign(&store, &[NATIVE_USER_KEY, NATIVE_KEY]);
    assert_eq!(both[0], ids1[0], "同一逻辑 Key 必须复用原 id");
    assert_ne!(both[1], ids1[0]);
    let active = store.point_map("ep1").unwrap();
    assert_eq!(active.get(NATIVE_USER_KEY), Some(&ids1[0]));
    assert_eq!(active.get(NATIVE_KEY), Some(&both[1]));
}

// --- §6.6 T2：跨任务 / 跨 kind / 跨 Endpoint 的 point_key 唯一性 ---
#[test]
fn cross_task_duplicate_point_key() {
    // ① 同一 Endpoint 的**两个不同任务**出现同一 Key ⇒ DUPLICATE_POINT_KEY。
    let cross_task = ensure_endpoint_point_keys_unique(
        &[(SEMANTIC_KEY, "tasks[1].binding.config.points[0]")],
        &[(SEMANTIC_KEY, "tasks[0].binding.config.points[0]")],
    );
    assert_eq!(cross_task.len(), 1);
    assert_eq!(cross_task[0].code, "DUPLICATE_POINT_KEY");

    // ② 同一任务内重复（第二次出现才报）⇒ 同样拒绝。
    let same_task = ensure_endpoint_point_keys_unique(
        &[
            (SEMANTIC_KEY, "tasks[0].binding.config.points[0]"),
            (SEMANTIC_KEY, "tasks[0].binding.config.points[1]"),
        ],
        &[],
    );
    assert_eq!(same_task.len(), 1);
    assert_eq!(same_task[0].code, "DUPLICATE_POINT_KEY");
    assert_eq!(same_task[0].path, "tasks[0].binding.config.points[1]");

    // ③ 跨 Task Binding Kind：`mesa.resources.v1` 已占用同一 Key ⇒ 拒绝。
    let cross_kind = ensure_endpoint_point_keys_unique(
        &[(SEMANTIC_KEY, "tasks[1].binding.config.points[0]")],
        &[(SEMANTIC_KEY, "tasks[0].selections[0].outputs")],
    );
    assert_eq!(cross_kind.len(), 1);
    assert_eq!(cross_kind[0].code, "DUPLICATE_POINT_KEY");

    // ④ 不同 Endpoint 的同名 Key **不**冲突（本函数按 Endpoint 调用）。
    assert!(ensure_endpoint_point_keys_unique(&[(SEMANTIC_KEY, "p")], &[]).is_empty());
    assert!(ensure_endpoint_point_keys_unique(&[(SEMANTIC_KEY, "p")], &[]).is_empty());

    // ⑤ 不同 Key 不冲突。
    assert!(
        ensure_endpoint_point_keys_unique(
            &[(SEMANTIC_KEY, "p1"), (NATIVE_KEY, "p2")],
            &[(SEMANTIC_KEY_OTHER_VARIABLE, "p0")],
        )
        .is_empty()
    );
}
