//! Profile 契约（V2.1 §10, Milestone I）

use mesa_core_types::{DeviceProfile, expand_preset};

#[test]
fn profile_validate_and_expand() {
    let json = r#"{
        "profile_version":1,
        "id":"test-profile",
        "version":"1.0.0",
        "vendor":"Test",
        "family":"Fam",
        "model":"Mod",
        "driver_id":"simulator",
        "match_rules":[{"field":"driver_id","op":"eq","value":"simulator"}],
        "connection_defaults":{"seed":1},
        "rate_classes":{"normal":1000},
        "presets":[
            {"id":"basic","label":{"default":"Basic"},"selections":[
                {"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k1"}]}
            ]}
        ]
    }"#;
    let p: DeviceProfile = serde_json::from_str(json).unwrap();
    p.validate().unwrap();
    assert_eq!(p.presets.len(), 1);
    let sels = expand_preset(&p.presets[0]);
    assert_eq!(sels.len(), 1);
    assert_eq!(sels[0].resource_id, "counter");
}

#[test]
fn load_profiles_from_drivers_dir() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
    let profiles = mesa_driver_manager::profile::load_profiles(&dir);
    assert!(profiles.iter().any(|p| p.id == "simulator-basic"));
    assert!(profiles.iter().any(|p| p.id == "fanuc-0i-f-plus"));
    assert!(profiles.iter().any(|p| p.id == "s7-1200"));
    // 每个 profile 必须校验通过
    for p in &profiles {
        p.validate().expect("profile must be valid");
        assert!(p.presets.iter().any(|pr| !pr.selections.is_empty()));
    }
}
