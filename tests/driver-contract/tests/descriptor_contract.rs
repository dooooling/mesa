#![allow(clippy::approx_constant)]
//! Descriptor Contract 测试（V2.1 §4, §13, Milestone A）。
//! 覆盖 contract version、唯一性、default 类型、visible_if 引用等。

mod common;

use mesa_driver_sdk::Driver;

use mesa_core_types::{
    AccessMode, DataType, DriverDescriptor, FieldDescriptor, FieldType, LocalizedText,
    OutputDescriptor, OutputTypeSpec, ResourceDescriptor, ResourceSelectionMethod,
    SchemaDescriptor,
};

fn synthetic_descriptor() -> DriverDescriptor {
    use mesa_core_types::capability::{ControlCatalog, DriverCapabilities};
    use mesa_core_types::schema::{Condition, ConditionOp, FieldValidation, UiHints};
    // 覆盖全部 12 种 FieldType 的合成 Schema
    let fields = vec![
        FieldDescriptor {
            key: "str_field".into(),
            label: "String".into(),
            description: None,
            field_type: FieldType::String,
            required: true,
            default: Some(serde_json::json!("default_str")),
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "int_field".into(),
            label: "Integer".into(),
            description: None,
            field_type: FieldType::Integer,
            required: false,
            default: Some(serde_json::json!(42)),
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "num_field".into(),
            label: "Number".into(),
            description: None,
            field_type: FieldType::Number,
            required: false,
            default: Some(serde_json::json!(3.14)),
            validation: FieldValidation {
                min: Some(0.0),
                max: Some(100.0),
                ..Default::default()
            },
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "bool_field".into(),
            label: "Boolean".into(),
            description: None,
            field_type: FieldType::Boolean,
            required: false,
            default: Some(serde_json::json!(true)),
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "enum_field".into(),
            label: "Enum".into(),
            description: None,
            field_type: FieldType::Enum,
            required: true,
            default: Some(serde_json::json!("a")),
            validation: FieldValidation {
                enum_options: Some(vec!["a".into(), "b".into(), "c".into()]),
                ..Default::default()
            },
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "secret_field".into(),
            label: "Secret".into(),
            description: None,
            field_type: FieldType::Secret,
            required: true,
            default: None,
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "duration_field".into(),
            label: "Duration".into(),
            description: None,
            field_type: FieldType::Duration,
            required: false,
            default: Some(serde_json::json!(1000)),
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "host_field".into(),
            label: "Host".into(),
            description: None,
            field_type: FieldType::Host,
            required: true,
            default: Some(serde_json::json!("127.0.0.1")),
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "port_field".into(),
            label: "Port".into(),
            description: None,
            field_type: FieldType::Port,
            required: true,
            default: Some(serde_json::json!(502)),
            validation: FieldValidation {
                min: Some(1.0),
                max: Some(65535.0),
                ..Default::default()
            },
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "url_field".into(),
            label: "URL".into(),
            description: None,
            field_type: FieldType::Url,
            required: false,
            default: Some(serde_json::json!("opc.tcp://127.0.0.1:4840")),
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "file_field".into(),
            label: "File".into(),
            description: None,
            field_type: FieldType::File,
            required: false,
            default: Some(serde_json::json!("/tmp/test.csv")),
            validation: FieldValidation::default(),
            ui: UiHints::default(),
        },
        FieldDescriptor {
            key: "cert_field".into(),
            label: "CertificateRef".into(),
            description: None,
            field_type: FieldType::CertificateRef,
            required: false,
            default: Some(serde_json::json!("cert-123")),
            validation: FieldValidation::default(),
            ui: UiHints {
                visible_if: Some(Condition {
                    field: "bool_field".into(),
                    op: ConditionOp::Eq,
                    value: serde_json::json!(true),
                }),
                ..Default::default()
            },
        },
    ];
    let conn = SchemaDescriptor { fields };
    let resources = vec![ResourceDescriptor {
        id: "res1".into(),
        label: LocalizedText::new("Res1"),
        parameters: SchemaDescriptor::default(),
        outputs: vec![OutputDescriptor {
            id: "value".into(),
            label: LocalizedText::new("Value"),
            type_spec: OutputTypeSpec::Fixed {
                data_type: DataType::F64,
            },
            unit: None,
            access: AccessMode::Read,
        }],
        modes: vec![],
    }];
    DriverDescriptor {
        contract_major: mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR,
        contract_minor: mesa_core_types::DESCRIPTOR_CONTRACT_MINOR,
        identity: mesa_core_types::descriptor::DriverIdentity {
            driver_id: "synthetic".into(),
            name: "Synthetic".into(),
            version: "0.0.1".into(),
        },
        connection: conn,
        resources,
        controls: ControlCatalog::default(),
        resource_selection_methods: vec![ResourceSelectionMethod::Manual],
        capabilities: DriverCapabilities::default(),
        events: Default::default(),
    }
}

#[test]
fn synthetic_descriptor_covers_all_field_types_and_validates() {
    let d = synthetic_descriptor();
    d.validate().expect("synthetic must be valid");
    // 序列化往返
    let json = serde_json::to_string(&d).unwrap();
    assert!(json.len() < 256 * 1024, "must be <256KiB");
    let back: DriverDescriptor = serde_json::from_str(&json).unwrap();
    assert_eq!(back.connection.fields.len(), 12);
    back.validate().unwrap();
}

#[test]
fn contract_version_must_be_present() {
    let mut d = synthetic_descriptor();
    d.contract_major = mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR;
    d.contract_minor = mesa_core_types::DESCRIPTOR_CONTRACT_MINOR;
    assert!(d.validate().is_ok());
}

#[test]
fn field_key_unique_enforced() {
    let mut d = synthetic_descriptor();
    d.connection
        .fields
        .push(FieldDescriptor::new("str_field", "dup", FieldType::String));
    assert!(
        d.validate().is_err(),
        "duplicate field key must be rejected"
    );
}

#[test]
fn resource_id_unique_enforced() {
    let mut d = synthetic_descriptor();
    d.resources.push(ResourceDescriptor {
        id: "res1".into(),
        label: LocalizedText::new("dup"),
        parameters: SchemaDescriptor::default(),
        outputs: vec![OutputDescriptor {
            id: "value".into(),
            label: LocalizedText::new("v"),
            type_spec: OutputTypeSpec::Fixed {
                data_type: DataType::Bool,
            },
            unit: None,
            access: AccessMode::Read,
        }],
        modes: vec![],
    });
    assert!(d.validate().is_err());
}

#[test]
fn output_id_unique_enforced() {
    let mut d = synthetic_descriptor();
    d.resources[0].outputs.push(OutputDescriptor {
        id: "value".into(),
        label: LocalizedText::new("dup"),
        type_spec: OutputTypeSpec::Fixed {
            data_type: DataType::Bool,
        },
        unit: None,
        access: AccessMode::Read,
    });
    assert!(d.validate().is_err());
}

#[test]
fn enum_option_unique_enforced() {
    let mut d = synthetic_descriptor();
    // 找到 enum_field 并注入重复 option
    for f in &mut d.connection.fields {
        if f.key == "enum_field" {
            f.validation.enum_options = Some(vec!["a".into(), "a".into()]);
        }
    }
    assert!(d.validate().is_err());
}

#[test]
fn default_value_type_must_match() {
    let mut d = synthetic_descriptor();
    for f in &mut d.connection.fields {
        if f.key == "int_field" {
            f.default = Some(serde_json::json!("not a number"));
        }
    }
    assert!(d.validate().is_err());
}

#[test]
fn visible_if_reference_must_exist() {
    let mut d = synthetic_descriptor();
    for f in &mut d.connection.fields {
        if f.key == "cert_field" {
            f.ui.visible_if = Some(mesa_core_types::schema::Condition {
                field: "nonexistent".into(),
                op: mesa_core_types::schema::ConditionOp::Eq,
                value: serde_json::json!(true),
            });
        }
    }
    assert!(d.validate().is_err());
}

#[test]
fn simulator_descriptor_is_valid_and_small() {
    let driver = mesa_driver_simulator::SimulatorDriver;
    let d = driver.descriptor();
    d.validate().expect("simulator descriptor must be valid");
    let json = serde_json::to_string(&d).unwrap();
    assert!(json.len() < 256 * 1024);
    // 实现契约：2.0（常量唯一真值，禁止魔数）
    assert_eq!(d.contract_major, mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR);
    assert!(!d.resources.is_empty());
}

#[test]
fn s7_descriptor_is_valid() {
    let d = mesa_driver_s7::S7Driver.descriptor();
    d.validate().expect("s7 descriptor must be valid");
    assert_eq!(d.identity.driver_id, "s7");
    assert!(serde_json::to_string(&d).unwrap().len() < 256 * 1024);
    assert!(d.resources.iter().any(|r| r.id == "memory"));
}

#[test]
fn focas_descriptor_is_valid() {
    let d = mesa_driver_focas2::FocasDriver.descriptor();
    d.validate().expect("focas2 descriptor must be valid");
    assert_eq!(d.identity.driver_id, "focas2");
    assert!(serde_json::to_string(&d).unwrap().len() < 256 * 1024);
    // 验收：1 Resource 多 Outputs
    let dyn_res = d
        .resources
        .iter()
        .find(|r| r.id == "dynamic")
        .expect("dynamic");
    assert!(dyn_res.outputs.len() >= 4, "dynamic must have >=4 outputs");
    assert!(d.resources.iter().any(|r| r.id == "pmc"));
}

#[test]
fn opcua_descriptor_is_valid() {
    let d = mesa_driver_opcua::OpcUaDriver.descriptor();
    d.validate().expect("opcua descriptor must be valid");
    assert_eq!(d.identity.driver_id, "opcua");
    assert!(serde_json::to_string(&d).unwrap().len() < 256 * 1024);
    // Browse 是 resource_selection_methods 的唯一真值（capabilities 不再重复）
    assert!(
        d.resource_selection_methods
            .contains(&ResourceSelectionMethod::Browse)
    );
    assert!(
        d.resource_selection_methods
            .contains(&ResourceSelectionMethod::Manual)
    );
}

#[test]
fn sinumerik_nck_descriptor_is_valid_and_read_only() {
    // ADR 0001：原生 NCK 独立驱动（`sinumerik` 已退役）；V1 只读，不伪造 subscribe。
    let d = mesa_driver_sinumerik_nck::SinumerikNckDriver.descriptor();
    d.validate()
        .expect("sinumerik-nck descriptor must be valid");
    assert_eq!(d.identity.driver_id, "sinumerik-nck");
    assert!(serde_json::to_string(&d).unwrap().len() < 256 * 1024);
    assert!(d.capabilities.poll, "NCK V1 must poll");
    assert!(!d.capabilities.subscribe, "NCK 不伪造 subscribe");
    assert!(!d.capabilities.write, "V1 只读：不得声明 write");
    assert!(!d.capabilities.events, "NCK V1 无事件：不得声明 events");
    assert!(d.controls.commands.is_empty(), "V1 只读：无控制目录");
    assert!(d.resources.iter().any(|r| r.id == "variable"));
}

#[test]
fn descriptor_json_roundtrip_stable() {
    let d = synthetic_descriptor();
    let a = serde_json::to_string(&d).unwrap();
    let b: DriverDescriptor = serde_json::from_str(&a).unwrap();
    let c = serde_json::to_string(&b).unwrap();
    assert_eq!(a, c);
}

fn from_parameter_resource(
    options: Vec<&str>,
    mapping: Vec<(&str, DataType)>,
) -> ResourceDescriptor {
    let mut f = FieldDescriptor::new("data_type", "Data Type", FieldType::Enum).required(true);
    f.validation.enum_options = Some(options.into_iter().map(|s| s.to_string()).collect());
    ResourceDescriptor {
        id: "memory".into(),
        label: LocalizedText::new("Memory"),
        parameters: SchemaDescriptor::new(vec![f]),
        outputs: vec![OutputDescriptor {
            id: "value".into(),
            label: LocalizedText::new("Value"),
            type_spec: OutputTypeSpec::FromParameter {
                parameter: "data_type".into(),
                mapping: mapping
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            },
            unit: None,
            access: AccessMode::Read,
        }],
        modes: vec![],
    }
}

#[test]
fn output_type_spec_from_parameter_validates_and_resolves() {
    let r = from_parameter_resource(
        vec!["REAL", "BOOL"],
        vec![("REAL", DataType::F32), ("BOOL", DataType::Bool)],
    );
    r.validate().expect("mapping 全覆盖必须通过");
    let out = &r.outputs[0];
    let params = |v: &str| {
        [("data_type".to_string(), serde_json::json!(v))]
            .into_iter()
            .collect::<serde_json::Map<String, serde_json::Value>>()
    };
    assert_eq!(out.type_spec.resolve(&params("REAL")), Some(DataType::F32));
    assert_eq!(out.type_spec.resolve(&params("BOOL")), Some(DataType::Bool));
    // 未知键 → None（Driver 在 Configure 时拒绝，不得静默定类型）
    assert_eq!(out.type_spec.resolve(&params("NOPE")), None);
}

#[test]
fn output_type_spec_mapping_missing_option_rejected() {
    // mapping 漏掉 BOOL → 定义期拒绝
    let r = from_parameter_resource(vec!["REAL", "BOOL"], vec![("REAL", DataType::F32)]);
    assert!(r.validate().is_err());
}

#[test]
fn output_type_spec_mapping_extra_key_rejected() {
    // mapping 多出 NOPE → 定义期拒绝
    let r = from_parameter_resource(
        vec!["REAL"],
        vec![("REAL", DataType::F32), ("NOPE", DataType::Bool)],
    );
    assert!(r.validate().is_err());
}

#[test]
fn output_type_spec_parameter_must_exist_and_be_enum() {
    // 引用不存在的参数 → 拒绝
    let mut r = from_parameter_resource(vec!["REAL"], vec![("REAL", DataType::F32)]);
    if let OutputTypeSpec::FromParameter { parameter, .. } = &mut r.outputs[0].type_spec {
        *parameter = "nope".into();
    }
    assert!(r.validate().is_err());
    // 参数非 Enum → 拒绝
    let mut r2 = from_parameter_resource(vec!["REAL"], vec![("REAL", DataType::F32)]);
    r2.parameters.fields[0].field_type = FieldType::String;
    assert!(r2.validate().is_err());
    // 类型参数 optional → 拒绝（缺参 selection 会 schema 合法但 resolve 出 None）
    let mut r3 = from_parameter_resource(vec!["REAL"], vec![("REAL", DataType::F32)]);
    r3.parameters.fields[0].required = false;
    assert!(r3.validate().is_err());
}

#[test]
fn s7_and_opcua_descriptors_declare_from_parameter() {
    // S7 memory.value ← data_type（9 选项全覆盖）
    let s7 = mesa_driver_s7::S7Driver.descriptor();
    let mem = s7.resources.iter().find(|r| r.id == "memory").unwrap();
    match &mem.outputs[0].type_spec {
        OutputTypeSpec::FromParameter { parameter, mapping } => {
            assert_eq!(parameter, "data_type");
            assert_eq!(mapping.len(), 9);
            assert_eq!(mapping["REAL"], DataType::F32);
            assert_eq!(mapping["BOOL"], DataType::Bool);
            assert_eq!(mapping["DINT"], DataType::I32);
            assert_eq!(mapping["STRING"], DataType::String);
        }
        other => panic!("s7 memory.value 必须为 FromParameter，实际 {other:?}"),
    }
    // OPC UA node.value ← data_type（10 选项全覆盖，见 CANONICAL_DATA_TYPES）
    let opc = mesa_driver_opcua::OpcUaDriver.descriptor();
    let node = opc.resources.iter().find(|r| r.id == "node").unwrap();
    match &node.outputs[0].type_spec {
        OutputTypeSpec::FromParameter { parameter, mapping } => {
            assert_eq!(parameter, "data_type");
            assert_eq!(mapping.len(), 10);
            assert_eq!(mapping["DOUBLE"], DataType::F64);
            assert_eq!(mapping["BOOL"], DataType::Bool);
            assert_eq!(mapping["UINT32"], DataType::U32);
            assert_eq!(mapping["DATETIME"], DataType::DateTime);
        }
        other => panic!("opcua node.value 必须为 FromParameter，实际 {other:?}"),
    }
    // NCK catalog 变量 → DriverResolved
    let nck = mesa_driver_sinumerik_nck::SinumerikNckDriver.descriptor();
    let var = nck.resources.iter().find(|r| r.id == "variable").unwrap();
    assert_eq!(var.outputs[0].type_spec, OutputTypeSpec::DriverResolved);
}

#[test]
fn definition_rejects_bad_defaults_and_loose_rules() {
    // Enum default 不在 options → 拒绝
    let mut f = FieldDescriptor::new("m", "M", FieldType::Enum).required(false);
    f.validation.enum_options = Some(vec!["a".into()]);
    f.default = Some(serde_json::json!("z"));
    assert!(
        SchemaDescriptor::new(vec![f])
            .validate_definition()
            .is_err()
    );
    // Enum 无 options → 拒绝
    let f2 = FieldDescriptor::new("m", "M", FieldType::Enum).required(false);
    assert!(
        SchemaDescriptor::new(vec![f2])
            .validate_definition()
            .is_err()
    );
    // 非 Enum 带 options → 拒绝
    let mut f3 = FieldDescriptor::new("s", "S", FieldType::String).required(false);
    f3.validation.enum_options = Some(vec!["a".into()]);
    assert!(
        SchemaDescriptor::new(vec![f3])
            .validate_definition()
            .is_err()
    );
    // Integer default 越界 → 拒绝
    let mut f4 = FieldDescriptor::new("n", "N", FieldType::Integer).required(false);
    f4.validation.min = Some(1.0);
    f4.validation.max = Some(10.0);
    f4.default = Some(serde_json::json!(20));
    assert!(
        SchemaDescriptor::new(vec![f4])
            .validate_definition()
            .is_err()
    );
    // min > max → 拒绝
    let mut f5 = FieldDescriptor::new("n", "N", FieldType::Integer).required(false);
    f5.validation.min = Some(10.0);
    f5.validation.max = Some(1.0);
    assert!(
        SchemaDescriptor::new(vec![f5])
            .validate_definition()
            .is_err()
    );
    // String 带 min → 拒绝；Integer 带 pattern → 拒绝
    let mut f6 = FieldDescriptor::new("s", "S", FieldType::String).required(false);
    f6.validation.min = Some(1.0);
    assert!(
        SchemaDescriptor::new(vec![f6])
            .validate_definition()
            .is_err()
    );
    let mut f7 = FieldDescriptor::new("n", "N", FieldType::Integer).required(false);
    f7.validation.pattern = Some(".*".into());
    assert!(
        SchemaDescriptor::new(vec![f7])
            .validate_definition()
            .is_err()
    );
    // default 不匹配 pattern → 拒绝
    let mut f8 = FieldDescriptor::new("c", "C", FieldType::String).required(false);
    f8.validation.pattern = Some("^ABC$".into());
    f8.default = Some(serde_json::json!("XYZ"));
    assert!(
        SchemaDescriptor::new(vec![f8])
            .validate_definition()
            .is_err()
    );
    // 非法 regex → 拒绝
    let mut f9 = FieldDescriptor::new("c", "C", FieldType::String).required(false);
    f9.validation.pattern = Some("([".into());
    assert!(
        SchemaDescriptor::new(vec![f9])
            .validate_definition()
            .is_err()
    );
}

#[test]
fn definition_enforces_port_duration_intrinsics() {
    // Port default -1 → 拒绝（内禀 1..=65535）；Duration 负数 → 拒绝
    let f = FieldDescriptor::new("port", "Port", FieldType::Port)
        .required(false)
        .default_value(serde_json::json!(-1));
    assert!(
        SchemaDescriptor::new(vec![f])
            .validate_definition()
            .is_err()
    );
    let f2 = FieldDescriptor::new("timeout_ms", "Timeout", FieldType::Duration)
        .required(false)
        .default_value(serde_json::json!(-5));
    assert!(
        SchemaDescriptor::new(vec![f2])
            .validate_definition()
            .is_err()
    );
    // min/max 不得放宽内禀：Port min 0 / max 99999 → 拒绝；Duration min -1 → 拒绝
    let mut f3 = FieldDescriptor::new("port", "Port", FieldType::Port).required(false);
    f3.validation.min = Some(0.0);
    assert!(
        SchemaDescriptor::new(vec![f3])
            .validate_definition()
            .is_err()
    );
    let mut f4 = FieldDescriptor::new("port", "Port", FieldType::Port).required(false);
    f4.validation.max = Some(99999.0);
    assert!(
        SchemaDescriptor::new(vec![f4])
            .validate_definition()
            .is_err()
    );
    let mut f5 = FieldDescriptor::new("timeout_ms", "Timeout", FieldType::Duration).required(false);
    f5.validation.min = Some(-1.0);
    assert!(
        SchemaDescriptor::new(vec![f5])
            .validate_definition()
            .is_err()
    );
    // instance 期：port -1 → OUT_OF_RANGE（类型对，值越界）
    let schema = SchemaDescriptor::new(vec![
        FieldDescriptor::new("port", "Port", FieldType::Port).required(true),
    ]);
    let issues = schema.validate_instance("connection", &serde_json::json!({"port": -1}));
    assert!(issues.iter().any(|i| i.code == "OUT_OF_RANGE"));
}

#[test]
fn resource_parameters_reject_secret() {
    let r = ResourceDescriptor {
        id: "r".into(),
        label: LocalizedText::new("R"),
        parameters: SchemaDescriptor::new(vec![
            FieldDescriptor::new("password", "Password", FieldType::Secret).required(true),
        ]),
        outputs: vec![OutputDescriptor {
            id: "value".into(),
            label: LocalizedText::new("V"),
            type_spec: OutputTypeSpec::Fixed {
                data_type: DataType::Bool,
            },
            unit: None,
            access: AccessMode::Read,
        }],
        modes: vec![],
    };
    assert!(r.validate().is_err());
}

#[test]
fn resource_selection_methods_reject_duplicates() {
    let mut d = synthetic_descriptor();
    d.resource_selection_methods = vec![
        ResourceSelectionMethod::Manual,
        ResourceSelectionMethod::Manual,
    ];
    assert!(d.validate().is_err());
}

#[test]
fn resource_selection_methods_replaces_discovery_bools() {
    // methods 为唯一真值；序列化为 snake_case 字符串数组（可扩展枚举）
    let d = mesa_driver_opcua::OpcUaDriver.descriptor();
    let json = serde_json::to_value(&d.resource_selection_methods).unwrap();
    assert_eq!(json, serde_json::json!(["manual", "browse"]));
    let back: Vec<ResourceSelectionMethod> = serde_json::from_value(json).unwrap();
    assert_eq!(
        back,
        vec![
            ResourceSelectionMethod::Manual,
            ResourceSelectionMethod::Browse
        ]
    );
}

#[test]
fn validate_instance_covers_required_type_enum_range_pattern_unknown() {
    let schema = SchemaDescriptor::new(vec![
        FieldDescriptor::new("host", "Host", FieldType::Host).required(true),
        {
            let mut f = FieldDescriptor::new("port", "Port", FieldType::Port).required(false);
            f.validation.min = Some(1.0);
            f.validation.max = Some(65535.0);
            f
        },
        {
            let mut f =
                FieldDescriptor::new("threshold", "Threshold", FieldType::Number).required(false);
            f.validation.min = Some(0.0);
            f.validation.max = Some(100.0);
            f
        },
        {
            let mut f = FieldDescriptor::new("mode", "Mode", FieldType::Enum).required(false);
            f.validation.enum_options = Some(vec!["a".into(), "b".into()]);
            f
        },
        {
            let mut f = FieldDescriptor::new("code", "Code", FieldType::String).required(false);
            f.validation.pattern = Some("^[A-Z]{2}[0-9]+$".into());
            f
        },
    ]);
    // 全合法 → 空 issues
    let ok = serde_json::json!({"host":"h","port":102,"mode":"a","code":"AB12"});
    assert!(schema.validate_instance("connection", &ok).is_empty());
    // 缺 required
    let issues = schema.validate_instance("connection", &serde_json::json!({}));
    assert!(
        issues
            .iter()
            .any(|i| i.code == "REQUIRED" && i.path == "connection.host")
    );
    // 类型错
    let issues = schema.validate_instance("connection", &serde_json::json!({"host":1}));
    assert!(issues.iter().any(|i| i.code == "INVALID_TYPE"));
    // 枚举错
    let issues =
        schema.validate_instance("connection", &serde_json::json!({"host":"h","mode":"z"}));
    assert!(issues.iter().any(|i| i.code == "INVALID_ENUM"));
    // 越界（min/max 收窄；Port/Duration 内禀越界同样 OUT_OF_RANGE，
    // 字符串给数值字段才是 INVALID_TYPE）
    let issues = schema.validate_instance(
        "connection",
        &serde_json::json!({"host":"h","threshold":150}),
    );
    assert!(issues.iter().any(|i| i.code == "OUT_OF_RANGE"));
    let issues =
        schema.validate_instance("connection", &serde_json::json!({"host":"h","port":99999}));
    assert!(issues.iter().any(|i| i.code == "OUT_OF_RANGE"));
    let issues =
        schema.validate_instance("connection", &serde_json::json!({"host":"h","port":"102"}));
    assert!(issues.iter().any(|i| i.code == "INVALID_TYPE"));
    // 真 regex（旧伪实现 s.contains 会放过 "ab12"）
    let issues =
        schema.validate_instance("connection", &serde_json::json!({"host":"h","code":"ab12"}));
    assert!(issues.iter().any(|i| i.code == "PATTERN_MISMATCH"));
    // 未知字段
    let issues = schema.validate_instance("connection", &serde_json::json!({"host":"h","typo":1}));
    assert!(issues.iter().any(|i| i.code == "UNKNOWN_FIELD"));
    // 非对象
    let issues = schema.validate_instance("connection", &serde_json::json!(42));
    assert!(issues.iter().any(|i| i.code == "INVALID_TYPE"));
}

#[test]
fn driver_version_identity_toml_metadata_package_agree() {
    // §4.1 门禁：driver.toml.version == DriverMetadata.version == package version；
    // Descriptor/公开行为变化必须同步三处，禁止同一 version 对应不同语义。
    use mesa_driver_sdk::Driver;
    let drivers: Vec<(&str, String)> = vec![
        (
            "simulator",
            mesa_driver_simulator::SimulatorDriver.metadata().version,
        ),
        ("s7", mesa_driver_s7::S7Driver.metadata().version),
        ("focas2", mesa_driver_focas2::FocasDriver.metadata().version),
        ("opcua", mesa_driver_opcua::OpcUaDriver.metadata().version),
        (
            "sinumerik-nck",
            mesa_driver_sinumerik_nck::SinumerikNckDriver
                .metadata()
                .version,
        ),
    ];
    for (dir, meta_version) in drivers {
        let toml_text = std::fs::read_to_string(
            common::repo_root()
                .join("drivers")
                .join(dir)
                .join("driver.toml"),
        )
        .unwrap();
        let toml_version = toml_text
            .lines()
            .find_map(|l| l.strip_prefix("version = \"")?.strip_suffix('"'))
            .expect("driver.toml 必须有 version");
        assert_eq!(
            toml_version, meta_version,
            "{dir}: driver.toml 与 metadata 版本不一致"
        );
        assert_eq!(
            meta_version,
            env!("CARGO_PKG_VERSION"),
            "{dir}: metadata 与 package 版本不一致"
        );
    }
}

#[tokio::test]
async fn manager_lazy_load_descriptor_via_temp_process() {
    // cargo build 需先产出 simulator 二进制（与 subprocess_recovery 同理）
    let mgr = std::sync::Arc::new(mesa_driver_manager::MesaManager::discover(
        &common::repo_root().join("drivers"),
    ));
    // 若环境未编译 simulator，跳过而非失败
    if mgr.find_driver("simulator").is_none() {
        eprintln!("simulator not discovered, skip lazy descriptor test");
        return;
    }
    let desc = mgr
        .get_descriptor("simulator")
        .await
        .expect("lazy descriptor must succeed");
    desc.validate().expect("fetched descriptor must be valid");
    assert_eq!(desc.identity.driver_id, "simulator");
    assert!(desc.contract_major >= 1);
    // 二次命中缓存
    let desc2 = mgr.get_descriptor("simulator").await.unwrap();
    assert_eq!(desc, desc2);
}
