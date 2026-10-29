//! Revision 2 removes the unsupported encoder measurement claim while preserving revision 1 immutably.

use serde_json::Value;

const R1_JSON: &str = include_str!("../definitions/828d/r1.json");
const R2_JSON: &str = include_str!("../definitions/828d/r2.json");
const CITED_EVIDENCE_FILE: &str = "docs/evidence/nck-live-identity-probe.txt";

fn find_definition<'a>(json: &'a str, id: &str) -> &'a Value {
    let value: &'a Value = Box::leak(Box::new(
        serde_json::from_str(json).expect("definition JSON must be valid"),
    ));
    value["definitions"]
        .as_array()
        .expect("definitions array")
        .iter()
        .find(|d| d["variable_id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("missing definition {id}"))
}

#[test]
fn revision_one_remains_immutable_historical_evidence() {
    let def = find_definition(R1_JSON, "axis.encoder.module_number");
    assert_eq!(def["revision"], 1);
    assert_eq!(def["semantics_status"], "verified");
    assert!(def["source"].as_array().unwrap().iter().any(|s| {
        s["kind"] == "measurement" && s["reference"] == CITED_EVIDENCE_FILE
    }));
}

#[test]
fn current_revision_removes_unsupported_measurement_claim() {
    let def = find_definition(R2_JSON, "axis.encoder.module_number");
    assert_eq!(def["revision"], 2);
    assert_eq!(def["semantics_status"], "candidate");
    assert_eq!(def["category"], "machine_data");
    assert!(!def["source"].as_array().unwrap().iter().any(|s| {
        s["kind"] == "measurement" && s["reference"] == CITED_EVIDENCE_FILE
    }));
    assert!(def["source"].as_array().unwrap().iter().any(|s| {
        s["kind"] == "identity_probe"
    }));
}

#[test]
fn revision_two_categories_are_persisted_in_definition_data() {
    let catalog: Value = serde_json::from_str(R2_JSON).expect("r2.json must be valid");
    let defs = catalog["definitions"].as_array().expect("definitions array");
    assert_eq!(defs.len(), 42);
    for def in defs {
        assert_eq!(def["revision"], 2);
        assert!(matches!(
            def["category"].as_str(),
            Some("cnc_variable" | "system_variable" | "machine_data" | "drive_parameter" | "unclassified")
        ));
    }
}
