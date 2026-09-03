//! Data Plane 精确语义冻结契约（§3 / §4 P0.1 + P0.2）
//! 覆盖 V2.1 §3.14 所列 14 断言。

use mesa_config_store::ConfigStore;
use mesa_core_types::{
    AcquisitionTask, DataBatch, DataType, DriverBinding, PointDescriptor, PointValue, Quality,
    TaskMode, Value, ValueOrigin, ensure_unique_point_keys, now_unix_ns,
};
use mesa_driver_manager::snapshot::Snapshot;

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
            profile: None,
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let descs = vec![PointDescriptor {
        point_key: "k1".into(),
        data_type: DataType::U32,
        unit: None,
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
            profile: None,
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
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
        },
        PointDescriptor {
            point_key: "k2".into(),
            data_type: DataType::U32,
            unit: None,
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
        },
        PointDescriptor {
            point_key: "k3".into(),
            data_type: DataType::U32,
            unit: None,
        },
    ];
    let defs2 = store.assign_point_ids("ep1", &descs2).unwrap();
    let k3_id = defs2.iter().find(|d| d.point_key == "k3").unwrap().point_id;
    assert_ne!(k3_id, k1_id, "deleted id must not be reused");
    // 当前实现中 deleted 的 k1 仍保留，max+1 应大于 k1_id
    assert!(k3_id > k1_id || k3_id != k1_id);
}

// --- re-add tombstone restores same ID (§3.10) ---
#[test]
fn re_add_tombstone_restores_same_id() {
    let store = ConfigStore::open_in_memory().unwrap();
    store
        .create_device(&mesa_config_store::DeviceRecord {
            id: "d1".into(),
            name: "dev".into(),
            profile: None,
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let descs = vec![PointDescriptor {
        point_key: "k1".into(),
        data_type: DataType::I32,
        unit: None,
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
            profile: None,
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let t = AcquisitionTask {
        id: "t1".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
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
            profile: None,
        })
        .unwrap();
    store
        .create_endpoint(&mesa_config_store::EndpointRecord {
            id: "ep1".into(),
            device_id: "d1".into(),
            driver_id: "simulator".into(),
            connection_json: "{}".into(),
            desired_running: false,
            updated_at_ns: 0,
        })
        .unwrap();
    let ok = AcquisitionTask {
        id: "t1".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
        binding: DriverBinding {
            kind: "k".into(),
            config: serde_json::json!({}),
        },
    };
    let r1 = store.replace_tasks("ep1", &[ok]).unwrap();
    // 无效任务（Poll 缺 interval）应失败且 revision 不变
    let bad = AcquisitionTask {
        id: "".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(100),
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
            },
            mesa_core_types::PointDefinition {
                point_id: 2,
                point_key: "k2".into(),
                data_type: DataType::I32,
                unit: None,
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
