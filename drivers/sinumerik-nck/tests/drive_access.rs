//! 驱动入口与对象身份回归（原生模型）：两条入口靠 `module` 显式区分，
//! 编号不得互换、不得静默忽略；解码仍按显式 `decode` 严格投影。
//!
//! 行为变化（有意为之，V2.0 §4/§6）：原生寻址下用户**可以**直接给出任意
//! column（这是能力，不是缺陷）；因此“改列就会读到别名参数”旧限制不再由
//! 驱动拦截，而由模板来源说明与使用者的显式输入负责。本测试断言的是
//! **模板不会自己改列/改入口**，以及显式入口必须被真实编码进请求。
use mesa_core_types::{AcquisitionTask, DataType, Value};
use mesa_driver_sdk::DriverConnection;
use mesa_driver_sinumerik_nck::{
    AddressCompiler, DefinitionCatalog, NativeNckAddress, NckConnConfig, NckConnection,
    NckDataKind, PresetRegistry, TargetRef, decode_value, encode_var_spec, resolve_native,
};
use serde_json::json;

/// 由模板 + 已确认的 area_no/line 构造原生地址（模板不预填这两项）。
fn address_from_preset(
    area: &str,
    block: &str,
    variable: &str,
    module: u64,
    area_no: u64,
    line: u64,
) -> NativeNckAddress {
    let reg = PresetRegistry::embedded();
    let prefix = format!("828d.{area}.{block}.{variable}.");
    let t = reg
        .for_family("828d")
        .into_iter()
        .find(|t| {
            t.id.starts_with(&prefix)
                && t.parameters.get("module").and_then(|v| v.as_u64()) == Some(module)
        })
        .cloned()
        .unwrap_or_else(|| panic!("缺模板 {area}/{block}/{variable}/module{module}"));
    let mut p = t.parameters.clone();
    p.insert("area_no".into(), json!(area_no));
    p.insert("line".into(), json!(line));
    NativeNckAddress::from_parameters(&p).expect("模板+输入应组成合法地址")
}

/// 同一候选参数的两条入口：column 相同、module 不同、线缆字节必须不同。
#[test]
fn two_entries_differ_only_by_explicit_module() {
    let nc = address_from_preset("V", "M", "r0080", 26, 6, 1);
    let drive = address_from_preset("V", "M", "r0080", 130, 6, 1);
    assert_eq!(nc.column, drive.column, "同一参数的列必须一致");
    assert_eq!(nc.module, 26);
    assert_eq!(drive.module, 130);
    let nc_wire = encode_var_spec(&resolve_native(nc).unwrap().wire);
    let dr_wire = encode_var_spec(&resolve_native(drive).unwrap().wire);
    assert_eq!(nc_wire, vec![0x12, 8, 0x82, 0xa6, 0, 80, 0, 1, 0x1a, 1]);
    assert_eq!(dr_wire, vec![0x12, 8, 0x82, 0xa6, 0, 80, 0, 1, 0x82, 1]);
    assert_ne!(nc_wire, dr_wire, "两条入口不得产生同一请求");
}

/// source_label 必须区分两条入口（同列不同 module ≠ 同一地址身份）。
#[test]
fn source_label_distinguishes_entries() {
    let nc = address_from_preset("V", "M", "r0080", 26, 6, 1);
    let drive = address_from_preset("V", "M", "r0080", 130, 6, 1);
    assert_ne!(nc.source_label(), drive.source_label());
    assert!(nc.source_label().contains("/26/"));
    assert!(drive.source_label().contains("/130/"));
}

/// 任意 module 号是**显式输入**，不会被静默忽略或替换。
#[test]
fn arbitrary_module_is_honored_verbatim() {
    let a = NativeNckAddress::from_parameters(
        json!({"area":"V","area_no":6,"module":200,"column":80,"line":1,"count":1,
               "unit_mode":"current","decode":"F32_BE","transport_size":"0x07"})
        .as_object()
        .unwrap(),
    )
    .unwrap();
    let w = resolve_native(a).unwrap().wire;
    assert_eq!(w.module, 200, "module 必须原样编码，不做白名单替换");
}

/// 模板不会自己改列或改入口：r0080 的两条模板列都是 80。
#[test]
fn templates_never_rewrite_column_or_entry() {
    let reg = PresetRegistry::embedded();
    let hits: Vec<_> = reg
        .for_family("828d")
        .into_iter()
        .filter(|t| t.id.contains(".r0080."))
        .collect();
    assert_eq!(hits.len(), 2, "两条入口各一个模板");
    for t in hits {
        assert_eq!(
            t.parameters.get("column").and_then(|v| v.as_u64()),
            Some(80)
        );
        assert!(t.parameters.contains_key("module"));
        assert!(!t.parameters.contains_key("access_path"));
    }
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

/// 两条驱动入口（module 26 / 130）与 `p0199` 对象名的语义点位可直接 Configure，
/// 且 `p0199` 是 50 字节 String（`count=25 × UTF16_BE`）。
///
/// PR-B 起走**语义信封**（`nck.points.v1`）：点位由定义目录的
/// `drive.parameter.r0080` / `drive.object.name` + `target.drive_object{number,entry}`
/// 描述；`entry` 显式决定 module 26/130，**不允许折叠**（旧 `mesa.resources.v1`
/// 信封整体拒绝）。
#[tokio::test]
async fn drive_entry_points_configure_with_declared_types() {
    let catalog = DefinitionCatalog::embedded().expect("内嵌定义目录");
    let compiler = AddressCompiler::new(&catalog, "828d");
    let point = |variable_id: &str, number: u32, entry: &str| {
        let def = catalog.lookup(variable_id, 1).unwrap();
        let target_json = json!({"drive_object": {"number": number, "entry": entry}});
        let target = TargetRef::from_json(&target_json).unwrap();
        let arguments = serde_json::Map::new();
        let key = def.canonical_identity(&target, &arguments).unwrap();
        let params = json!({
            "variable_id": variable_id,
            "definition_revision": 1,
            "target": target_json,
            "arguments": {},
        });
        // 顺带证明「Key 逐字相等 ⇒ 编译成功」在真实定义上成立。
        compiler
            .compile_point(
                mesa_driver_sinumerik_nck::POINT_KIND_VARIABLE,
                params.as_object().unwrap(),
                &key,
            )
            .expect("语义点位必须可编译");
        json!({"kind": "nck_variable", "point_key": key, "parameters": params})
    };
    let points_json = json!([
        point("drive.parameter.r0080", 6, "nc_variable"),
        point("drive.parameter.r0080", 6, "drive_object"),
        point("drive.object.name", 6, "drive_object"),
    ]);
    let task: AcquisitionTask = serde_json::from_value(
        json!({"id":"paths","schedule":{"mode":"poll","interval_ms":5000},
        "binding":{"kind":"nck.points.v1","config":{"points":points_json}}}),
    )
    .unwrap();
    let cfg = NckConnConfig::from_json(
        &json!({"family":"828d","host":"127.0.0.1","local_tsap":256,"remote_tsap":258}),
    )
    .unwrap();
    let connection = NckConnection::with_presets(cfg, PresetRegistry::embedded());
    let points = connection.configure(1, vec![task]).await.unwrap();
    assert_eq!(points.len(), 3);
    assert_eq!(points[2].data_type, DataType::String);
    assert_eq!(points[2].unit, None, "单位只是使用者注释，不伪造工程单位");
    // source_label 是原生地址标识，不含变量名，但能区分两条入口。
    assert!(points[0].source_label.as_deref().unwrap().contains("/26/"));
    assert!(points[1].source_label.as_deref().unwrap().contains("/130/"));
    // 两条入口的身份不同（entry 进身份 ⇒ 不同 point_key）。
    assert_ne!(points[0].point_key, points[1].point_key);
    assert!(points[0].point_key.contains("entry=s:nc_variable"));
    assert!(points[1].point_key.contains("entry=s:drive_object"));
}
