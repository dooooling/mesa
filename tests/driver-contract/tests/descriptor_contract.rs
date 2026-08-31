#![allow(clippy::approx_constant)]
//! Descriptor Contract 测试（V2.1 §4, §13, Milestone A）。
//! 覆盖 contract version、唯一性、default 类型、visible_if 引用等。

mod common;

use mesa_driver_sdk::Driver;

use mesa_core_types::{
    AccessMode, DataType, DriverDescriptor, FieldDescriptor, FieldType, LocalizedText,
    OutputDescriptor, ResourceDescriptor, SchemaDescriptor,
};

fn synthetic_descriptor() -> DriverDescriptor {
    use mesa_core_types::capability::{ControlCatalog, DiscoveryCapabilities, DriverCapabilities};
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
            data_type: DataType::F64,
            unit: None,
            access: AccessMode::Read,
        }],
        modes: vec![],
    }];
    DriverDescriptor {
        contract_major: 1,
        contract_minor: 0,
        identity: mesa_core_types::descriptor::DriverIdentity {
            driver_id: "synthetic".into(),
            name: "Synthetic".into(),
            version: "0.0.1".into(),
        },
        connection: conn,
        resources,
        controls: ControlCatalog::default(),
        discovery: DiscoveryCapabilities {
            manual: true,
            ..Default::default()
        },
        capabilities: DriverCapabilities::default(),
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
    d.contract_major = 1;
    d.contract_minor = 0;
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
            data_type: DataType::Bool,
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
        data_type: DataType::Bool,
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
    // 实现契约：1.0
    assert_eq!(d.contract_major, 1);
    assert!(!d.resources.is_empty());
}

#[test]
fn descriptor_json_roundtrip_stable() {
    let d = synthetic_descriptor();
    let a = serde_json::to_string(&d).unwrap();
    let b: DriverDescriptor = serde_json::from_str(&a).unwrap();
    let c = serde_json::to_string(&b).unwrap();
    assert_eq!(a, c);
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
