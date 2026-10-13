//! 驱动入口与对象身份回归：固定独立向量，拒绝把入口编号互换或替代参数改名。
use mesa_core_types::{AcquisitionTask, DataType, Value};
use mesa_driver_sdk::DriverConnection;
use mesa_driver_sinumerik_nck::{
    CatalogRegistry, NckArea, NckConnConfig, NckConnection, NckDataKind, NckVariableRef,
    build_tree, decode_value, encode_var_spec, resolve_wire,
};
use serde_json::json;

fn catalog() -> mesa_driver_sinumerik_nck::NckCatalog {
    CatalogRegistry::load_dir(&format!("{}/catalog", env!("CARGO_MANIFEST_DIR")))
        .unwrap()
        .resolve("828d")
        .unwrap()
}

#[test]
fn access_path_changes_only_declared_entry_and_preserves_old_addresses() {
    let catalog = catalog();
    let mut parameters = json!({"area":"V","area_no":6,"block":"M","variable":"r0080","line":1});
    let old = NckVariableRef::from_parameters(parameters.as_object().unwrap()).unwrap();
    let def = catalog.lookup(old.area, "M", "r0080").unwrap();
    assert_eq!(
        encode_var_spec(&resolve_wire(&old, def).unwrap().wire),
        vec![0x12, 8, 0x82, 0xa6, 0, 80, 0, 1, 0x1a, 1]
    );
    parameters["access_path"] = json!("drive_object");
    let direct = NckVariableRef::from_parameters(parameters.as_object().unwrap()).unwrap();
    assert_eq!(
        encode_var_spec(&resolve_wire(&direct, def).unwrap().wire),
        vec![0x12, 8, 0x82, 0xa6, 0, 80, 0, 1, 0x82, 1]
    );
    assert_ne!(old.canonical_key(), direct.canonical_key());
    assert!(!old.canonical_key().contains("access_path"));
    parameters["area"] = json!("C");
    parameters["block"] = json!("S");
    parameters["variable"] = json!("progStatus");
    let reference = NckVariableRef::from_parameters(parameters.as_object().unwrap()).unwrap();
    assert!(
        resolve_wire(
            &reference,
            catalog.lookup(NckArea::Channel, "S", "progStatus").unwrap()
        )
        .is_err()
    );
    parameters["access_path"] = json!("auto");
    assert!(NckVariableRef::from_parameters(parameters.as_object().unwrap()).is_err());
    parameters =
        json!({"area":"V","area_no":6,"block":"M","variable":"r0080","line":1,"column":27});
    let reference = NckVariableRef::from_parameters(parameters.as_object().unwrap()).unwrap();
    assert!(
        resolve_wire(&reference, def).is_err(),
        "不能把r0080的标签改列读r0027"
    );
    parameters["column"] = json!(80);
    parameters["module"] = json!(130);
    assert!(
        NckVariableRef::from_parameters(parameters.as_object().unwrap()).is_err(),
        "任意模块号不能被静默忽略"
    );
}

#[test]
fn object_name_status_and_axis_name_decode_recorded_values_strictly() {
    let name: Vec<u8> = "SERVO_3.3:8"
        .encode_utf16()
        .chain(std::iter::repeat_n(0, 14))
        .flat_map(u16::to_be_bytes)
        .collect();
    assert_eq!(name.len(), 50);
    assert_eq!(
        decode_value(&name, NckDataKind::Utf16Be).unwrap(),
        Value::String("SERVO_3.3:8".into())
    );
    assert!(decode_value(&[0xd8, 0], NckDataKind::Utf16Be).is_err());
    assert!(decode_value(&[0, 65, 0], NckDataKind::Utf16Be).is_err());
    assert_eq!(
        decode_value(&[0, 11], NckDataKind::I16Be).unwrap(),
        Value::I32(11)
    );
    assert_eq!(
        decode_value(&[0xff, 0xff], NckDataKind::I16Be).unwrap(),
        Value::I32(-1)
    );
    assert_eq!(
        decode_value(&[0, 105], NckDataKind::U16Be).unwrap(),
        Value::U32(105)
    );
    let axis = [b'M', b'Z', b'1', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(
        decode_value(&axis, NckDataKind::Utf8Fixed16).unwrap(),
        Value::String("MZ1".into())
    );
    assert_eq!(
        NckDataKind::Utf8Fixed16.core_array_type(),
        DataType::StringArray
    );
    assert_eq!(NckDataKind::Utf16Be.core_array_type(), DataType::String);
}

#[tokio::test]
async fn browse_bindings_preserve_path_and_full_object_name_projection() {
    let catalog = catalog();
    let (nodes, _) = build_tree(&catalog, None, "nck://V/M", "r0080", "", 50);
    assert_eq!(nodes.len(), 2);
    assert_ne!(nodes[0].id, nodes[1].id);
    let mut selections = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        let mut binding: serde_json::Value = serde_json::from_str(&node.binding_json).unwrap();
        binding["parameters"]["area_no"] = json!(6);
        binding["parameters"]["line"] = json!(1);
        binding["outputs"] = json!([{"output":"value","point_key":format!("path{i}")}]);
        selections.push(binding);
    }
    let (names, _) = build_tree(&catalog, None, "nck://V/M", "p0199", "", 50);
    let mut binding: serde_json::Value = serde_json::from_str(&names[1].binding_json).unwrap();
    assert_eq!(binding["parameters"]["count"], 25);
    assert_eq!(binding["parameters"]["access_path"], "drive_object");
    binding["parameters"]["area_no"] = json!(6);
    binding["parameters"]["line"] = json!(1);
    binding["outputs"] = json!([{"output":"value","point_key":"object_name"}]);
    selections.push(binding);
    let task: AcquisitionTask = serde_json::from_value(
        json!({"id":"paths","schedule":{"mode":"poll","interval_ms":5000},
        "binding":{"kind":"mesa.resources.v1","config":{"selections":selections}}}),
    )
    .unwrap();
    let cfg = NckConnConfig::from_json(
        &json!({"family":"828d","host":"127.0.0.1","local_tsap":256,"remote_tsap":258}),
    )
    .unwrap();
    let connection = NckConnection::with_catalog(cfg, catalog);
    let points = connection.configure(1, vec![task]).await.unwrap();
    assert_eq!(points.len(), 3);
    assert_eq!(points[2].data_type, DataType::String);
}
