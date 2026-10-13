//! 每次重建连接模拟管理面临时Browse；断言实际请求，防止隐式全量扫描和拒绝码丢失。
use mesa_driver_sdk::DriverConnection;
use mesa_driver_sinumerik_nck::{CatalogRegistry, NckConnConfig, NckConnection};
use mesa_driver_sinumerik_nck::{NckFixture, NckFixtureState};
use serde_json::json;

const ROOT: &str = "nck://identity";
const AXES: &str = "nck://identity/axes";
const OBJECTS: &str = "nck://identity/objects";

fn bytes(value: &str) -> Vec<u8> {
    value
        .split_whitespace()
        .map(|part| u8::from_str_radix(part, 16).unwrap())
        .collect()
}

fn recorded_state() -> NckFixtureState {
    let evidence: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/evidence/nck-drive-object-readback.json"
    ))
    .unwrap();
    let mut items = std::collections::HashMap::new();
    let mut jobs = std::collections::HashMap::new();
    for key in ["mapping_samples", "object_identity_samples"] {
        for row in evidence[key].as_array().unwrap() {
            let spec = bytes(row["request_spec_hex"].as_str().unwrap());
            let code =
                u16::from_str_radix(row["code"].as_str().unwrap().trim_start_matches("0x"), 16)
                    .unwrap();
            if row["status"] == "JOB_REJECTED" {
                jobs.insert(spec, code);
            } else {
                items.insert(
                    spec,
                    (
                        code as u8,
                        row["transport_size"].as_u64().unwrap() as u8,
                        bytes(row["data_hex"].as_str().unwrap()),
                    ),
                );
            }
        }
    }
    // 此处显式给出主轴编号与表尾向量，不从待测地址/解码器生成响应。
    for index in 1..=8 {
        items.insert(
            vec![0x12, 8, 0x82, 0x60 + index, 0x88, 0xb8, 0, 1, 0x1a, 1],
            (0xff, 9, vec![if index == 4 { 1 } else { 0 }]),
        );
    }
    items.insert(
        bytes("12 08 82 01 27 10 00 09 1a 01"),
        (0xff, 9, vec![0; 16]),
    );
    NckFixtureState {
        recorded_items: Some(items),
        recorded_jobs: jobs,
        ..Default::default()
    }
}

fn connection(fixture: &NckFixture) -> NckConnection {
    let cfg = NckConnConfig::from_json(&json!({"host":"127.0.0.1","port":fixture.addr.port(),
        "family":"828d","local_tsap":256,"remote_tsap":258,"max_read_items":1}))
    .unwrap();
    let catalog = CatalogRegistry::load_dir(&format!("{}/catalog", env!("CARGO_MANIFEST_DIR")))
        .unwrap()
        .resolve("828d")
        .unwrap();
    NckConnection::with_catalog(cfg, catalog)
}

fn reset_reads(fixture: &NckFixture) {
    fixture
        .state
        .lock()
        .unwrap()
        .received
        .lock()
        .unwrap()
        .clear();
}

#[tokio::test]
async fn fresh_requests_browse_root_pages_axis_object_and_leaf_pages_without_full_rescan() {
    let fixture = NckFixture::spawn(recorded_state()).await;
    let (root, next) = connection(&fixture).browse(ROOT, "", "", 1).await.unwrap();
    assert_eq!(root[0].id, AXES);
    assert_eq!(next.as_deref(), Some("1"));
    let (root, next) = connection(&fixture).browse(ROOT, "", "1", 1).await.unwrap();
    assert_eq!(root[0].id, OBJECTS);
    assert!(next.is_none());
    assert!(
        fixture.received_specs().is_empty(),
        "根目录和翻页不能访问控制器"
    );

    let (axes, next) = connection(&fixture).browse(AXES, "", "", 2).await.unwrap();
    assert_eq!(axes.len(), 2);
    assert_eq!(next.as_deref(), Some("3"));
    assert_eq!(fixture.received_specs().len(), 10);
    reset_reads(&fixture);
    let (axes, next) = connection(&fixture).browse(AXES, "", "3", 2).await.unwrap();
    assert_eq!(axes[0].id, "nck://identity/axis/3");
    assert!(axes[1].label.contains("主轴1"));
    assert_eq!(next.as_deref(), Some("5"));
    let reads = fixture.received_specs();
    assert_eq!(reads.len(), 10);
    assert!(reads.iter().all(|s| s[8] == 0x1a));
    assert!(
        reads
            .iter()
            .filter(|s| s[3] == 1)
            .all(|s| matches!(s[7], 3 | 4))
    );

    reset_reads(&fixture);
    let (objects, next) = connection(&fixture)
        .browse(OBJECTS, "", "", 2)
        .await
        .unwrap();
    assert_eq!(objects.len(), 2);
    assert_eq!(next.as_deref(), Some("3"));
    assert_eq!(
        fixture.received_specs(),
        vec![
            bytes("12 08 82 a1 00 c7 00 01 82 19"),
            bytes("12 08 82 a2 00 c7 00 01 82 19")
        ]
    );
    reset_reads(&fixture);
    connection(&fixture)
        .browse(OBJECTS, "", "3", 2)
        .await
        .unwrap();
    assert_eq!(
        fixture.received_specs(),
        vec![
            bytes("12 08 82 a3 00 c7 00 01 82 19"),
            bytes("12 08 82 a4 00 c7 00 01 82 19")
        ]
    );

    reset_reads(&fixture);
    let (leaves, next) = connection(&fixture)
        .browse("nck://identity/axis/4", "", "", 1)
        .await
        .unwrap();
    assert!(next.is_some());
    let binding: serde_json::Value = serde_json::from_str(&leaves[0].binding_json).unwrap();
    assert_eq!(binding["parameters"]["area_no"], 4);
    assert_eq!(binding["parameters"]["access_path"], "nc_variable");
    assert_eq!(fixture.received_specs().len(), 5);
    assert!(fixture.received_specs().iter().all(|s| s[8] == 0x1a));
    reset_reads(&fixture);
    connection(&fixture)
        .browse("nck://identity/axis/4", "", next.as_deref().unwrap(), 1)
        .await
        .unwrap();
    assert_eq!(fixture.received_specs().len(), 5, "变量页仅复核当前轴");

    reset_reads(&fixture);
    let (leaves, _) = connection(&fixture)
        .browse("nck://identity/object/6", "r0080", "", 100)
        .await
        .unwrap();
    let binding: serde_json::Value = serde_json::from_str(&leaves[0].binding_json).unwrap();
    assert_eq!(binding["parameters"]["area_no"], 6);
    assert_eq!(binding["parameters"]["access_path"], "drive_object");
    assert_eq!(
        fixture.received_specs(),
        vec![bytes("12 08 82 a6 00 c7 00 01 82 19")]
    );
}

#[tokio::test]
async fn job_8201_preserves_axis_and_object_rejections_instead_of_implying_table_end() {
    let mut state = recorded_state();
    state
        .recorded_jobs
        .insert(bytes("12 08 82 01 27 10 00 02 1a 01"), 0x8201);
    state
        .recorded_jobs
        .insert(bytes("12 08 82 a2 00 c7 00 01 82 19"), 0x8201);
    let fixture = NckFixture::spawn(state).await;
    for parent in [AXES, OBJECTS] {
        let (nodes, next) = connection(&fixture)
            .browse(parent, "", "", 4)
            .await
            .unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].kind, "diagnostic");
        assert!(nodes[1].label.contains("S7_JOB_0x8201"));
        assert!(nodes[1].label.contains("完整性待确认"));
        assert!(next.is_none());
    }
    let (nodes, _) = connection(&fixture)
        .browse("nck://identity/object/2", "r0080", "", 10)
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert!(nodes[0].label.contains("S7_JOB_0x8201"));
}

#[tokio::test]
async fn rejected_axis_drive_name_is_visible_even_when_variable_filter_is_present() {
    let mut state = recorded_state();
    state
        .recorded_jobs
        .insert(bytes("12 08 82 a4 00 c7 00 01 1a 19"), 0x8201);
    let fixture = NckFixture::spawn(state).await;
    let (axes, _) = connection(&fixture).browse(AXES, "", "4", 1).await.unwrap();
    assert_eq!(axes.len(), 1);
    assert!(!axes[0].has_children);
    assert!(axes[0].label.contains("S7_JOB_0x8201"));
    for cursor in ["", "1"] {
        let (nodes, next) = connection(&fixture)
            .browse("nck://identity/axis/4", "r0080", cursor, 10)
            .await
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, "diagnostic");
        assert!(nodes[0].label.contains("S7_JOB_0x8201"));
        assert!(next.is_none());
    }
}

#[tokio::test]
async fn successful_empty_name_is_table_end_but_item_rejection_is_diagnostic() {
    let fixture = NckFixture::spawn(recorded_state()).await;
    let (nodes, next) = connection(&fixture).browse(AXES, "", "9", 4).await.unwrap();
    assert!(nodes.is_empty() && next.is_none());
    let mut state = recorded_state();
    state
        .recorded_items
        .as_mut()
        .unwrap()
        .insert(bytes("12 08 82 01 27 10 00 09 1a 01"), (0x0a, 0, vec![]));
    let rejected = NckFixture::spawn(state).await;
    let (nodes, next) = connection(&rejected)
        .browse(AXES, "nomatch", "9", 4)
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert!(nodes[0].label.contains("S7_ITEM_0x0A"));
    assert!(next.is_none());
}

#[tokio::test]
async fn filters_and_invalid_cursors_do_not_trigger_a_hidden_scan() {
    let fixture = NckFixture::spawn(recorded_state()).await;
    let (nodes, next) = connection(&fixture)
        .browse(OBJECTS, "does-not-match", "", 2)
        .await
        .unwrap();
    assert!(nodes.is_empty());
    assert_eq!(next.as_deref(), Some("3"));
    assert_eq!(fixture.received_specs().len(), 2);
    reset_reads(&fixture);
    for (parent, cursor) in [
        (AXES, "0"),
        (OBJECTS, "32"),
        (OBJECTS, "bad"),
        ("nck://identity/axis/", ""),
        ("nck://identity/object/32", ""),
    ] {
        assert!(
            connection(&fixture)
                .browse(parent, "", cursor, 2)
                .await
                .is_err()
        );
    }
    assert!(fixture.received_specs().is_empty());
}

#[tokio::test]
async fn malformed_successful_response_is_an_error_not_an_empty_directory() {
    let mut state = recorded_state();
    state.recorded_items.as_mut().unwrap().insert(
        bytes("12 08 82 a1 00 c7 00 01 82 19"),
        (0xff, 6, vec![0; 2]),
    );
    let fixture = NckFixture::spawn(state).await;
    let error = connection(&fixture)
        .browse(OBJECTS, "", "", 2)
        .await
        .unwrap_err();
    assert_eq!(error.code, "IDENTITY_RESPONSE_INVALID");
    assert_eq!(fixture.received_specs().len(), 1);
}
