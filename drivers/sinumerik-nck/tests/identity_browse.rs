//! 每次重建连接模拟管理面临时Browse；断言实际请求，防止隐式全量扫描和拒绝码丢失。
use mesa_driver_sdk::DriverConnection;
use mesa_driver_sinumerik_nck::{NckConnConfig, NckConnection, PresetRegistry};
use mesa_driver_sinumerik_nck::{NckFixture, NckFixtureState};
use serde_json::{Value, json};

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
    // 身份浏览只用内建探针；模板仅用于变量叶预填（可选）。
    NckConnection::with_presets(cfg, PresetRegistry::embedded())
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

/// 轴页必须是**可选语义叶**，且实例只来自真实读数。
///
/// 这条直接钉住联动选择缺口的修复：轴节点不能只是不可选文件夹，否则前端
/// 无法从载荷得到 `target.axis`，`axis.position.*` 永远选不到实际轴。
#[tokio::test]
async fn axis_page_returns_selectable_axis_instances_from_actual_reads() {
    let fixture = NckFixture::spawn(recorded_state()).await;
    let (axes, next) = connection(&fixture).browse(AXES, "", "", 50).await.unwrap();
    // 证据夹具里只有 8 个真实轴；不得按 1..31 预设凭空生成第 9 个。
    assert_eq!(axes.len(), 8, "轴实例只能来自实际读到的轴");
    assert_eq!(next.as_deref(), Some("9"), "8 个轴之后游标指向 9");
    assert!(
        !axes.iter().any(|node| node.id == "nck://identity/axis/9"),
        "不得生成未读到的轴"
    );

    let axis1 = axes
        .iter()
        .find(|node| node.id == "nck://identity/axis/1")
        .expect("轴1必须存在");
    assert_eq!(axis1.selection_mode, 2, "轴节点必须是可选语义叶");
    assert!(!axis1.has_children, "轴节点只承担选择，不得同时可下钻");
    let payload: Value = serde_json::from_str(&axis1.selection_payload_json).unwrap();
    assert_eq!(payload["parameters"]["target"]["axis"], 1);
    assert_eq!(payload["identity"]["keys"][0]["path"], "axis");
    let applies = payload["identity"]["applies_to_variables"]
        .as_array()
        .expect("applies_to_variables 必须由定义目录算出");
    assert!(
        applies
            .iter()
            .any(|value| value == "axis.position.actual_bcs"),
        "轴载荷必须适用于 axis.position.actual_bcs：{applies:?}"
    );

    // MB1（轴6）在证据里是 output_type=0 的未确认形态：轴本身仍可选，
    // 但标签必须如实标出"未确认有效驱动输出"，不能伪装成已确认驱动对象。
    let mb1 = axes
        .iter()
        .find(|node| node.id == "nck://identity/axis/6")
        .expect("MB1（轴6）必须存在");
    assert_eq!(mb1.selection_mode, 2, "MB1 仍是真实轴，可选");
    assert!(mb1.label.contains("MB1"), "{}", mb1.label);
    assert!(
        mb1.label.contains("未确认有效驱动输出"),
        "MB1 未确认形态必须可见：{}",
        mb1.label
    );
}

#[tokio::test]
async fn fresh_requests_browse_root_pages_axis_object_and_leaf_pages_without_full_rescan() {
    let fixture = NckFixture::spawn(recorded_state()).await;
    // 身份根下四棵树（轴 / 通道 / 主轴 / 驱动对象），逐页翻完：分页语义是
    // "游标 = 下一个分支下标"，因此四页各一项，最后一页没有后续游标。
    for (cursor, expected) in [
        ("", AXES),
        ("1", "nck://identity/channels"),
        ("2", "nck://identity/spindles"),
        ("3", OBJECTS),
    ] {
        let (root, next) = connection(&fixture)
            .browse(ROOT, "", cursor, 1)
            .await
            .unwrap();
        assert_eq!(root.len(), 1, "游标 `{cursor}` 只返回一项");
        assert_eq!(root[0].id, expected, "游标 `{cursor}`");
        let expected_next = if expected == OBJECTS {
            None
        } else {
            Some((cursor.parse::<usize>().unwrap_or(0) + 1).to_string())
        };
        assert_eq!(next, expected_next, "游标 `{cursor}` 的后续游标");
    }
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
    // PR-B：浏览叶是**语义载荷**，已核对的实例编号出现在 `target` 里（不再是原生
    // `area_no`）；`line`/实例仍未确认的字段一律不出现（不按变量名猜行索引）。
    assert_eq!(binding["kind"], "nck_variable");
    // 身份核对出的实例编号出现在语义 `target` 里；入口身份由 `entry` 表达
    // （轴路径经 module 26 的 p0199 核对 ⇒ `entry=nc_variable`）。
    assert_eq!(binding["parameters"]["target"]["drive_object"]["number"], 4);
    assert_eq!(
        binding["parameters"]["target"]["drive_object"]["entry"], "nc_variable",
        "轴路径只列 module 26 入口模板"
    );
    assert!(
        binding["parameters"].get("access_path").is_none(),
        "新模型不再有访问路径字段，入口由 module 显式表达"
    );
    // 语义载荷不得含任何原生字段（用户不面对协议字段）。
    for native in [
        "area_no",
        "line",
        "module",
        "column",
        "decode",
        "transport_size",
    ] {
        assert!(
            binding["parameters"].get(native).is_none(),
            "语义载荷不得含原生字段 `{native}`：{}",
            leaves[0].binding_json
        );
    }
    // 身份块给出模板/键集合/预览 Key（前端只做通用替换，不推导身份）。
    let preview = binding["identity"]["preview_point_key"].as_str().unwrap();
    assert!(preview.starts_with("drive.parameter."), "{preview}");
    assert!(
        preview.ends_with("@drive_object=o:{entry=s:nc_variable,number=i:4}"),
        "{preview}"
    );
    assert_eq!(
        binding["identity"]["template"],
        format!(
            "{}@drive_object={{drive_object}}",
            binding["parameters"]["variable_id"].as_str().unwrap()
        )
    );
    assert_eq!(binding["identity"]["keys"][0]["path"], "drive_object");
    assert!(
        leaves[0].label.contains("module26入口编号4"),
        "叶标签必须标出编号出处：{}",
        leaves[0].label
    );
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
    // 对象路径的编号由 module 130 入口核对，因此**只能**给出 module 130 模板；
    // 不得用它生成 module 26 采集地址（两条入口编号在实机上并不相同）。
    assert_eq!(leaves.len(), 1, "对象路径只列 module 130 模板");
    let binding: serde_json::Value = serde_json::from_str(&leaves[0].binding_json).unwrap();
    assert_eq!(binding["parameters"]["target"]["drive_object"]["number"], 6);
    assert_eq!(
        binding["parameters"]["target"]["drive_object"]["entry"], "drive_object",
        "入口身份由 `entry` 显式表达（module 130）"
    );
    assert!(binding["parameters"].get("access_path").is_none());
    assert_eq!(
        binding["identity"]["preview_point_key"],
        "drive.parameter.r0080@drive_object=o:{entry=s:drive_object,number=i:6}"
    );
    assert!(
        leaves[0].label.contains("module130入口编号6"),
        "叶标签必须标出编号出处：{}",
        leaves[0].label
    );
    assert_eq!(
        fixture.received_specs(),
        vec![bytes("12 08 82 a6 00 c7 00 01 82 19")]
    );
}

/// 两条身份入口的实例编号不得交叉复用（审计 P1-A 回归）。
///
/// 轴路径用 module 26 核对编号、对象路径用 module 130 核对编号；若把两者
/// 混填，浏览器就会给出一个"编号来自另一条入口"的采集地址——地址结构合法、
/// 却带着未经证明的归属。
#[tokio::test]
async fn identity_entries_never_cross_fill_instance_numbers() {
    let fixture = NckFixture::spawn(recorded_state()).await;

    let (axis_leaves, _) = connection(&fixture)
        .browse("nck://identity/axis/4", "", "", 200)
        .await
        .unwrap();
    assert!(!axis_leaves.is_empty(), "轴 4 应有模板叶");
    for node in &axis_leaves {
        let b: serde_json::Value = serde_json::from_str(&node.binding_json).unwrap();
        assert_eq!(b["parameters"]["target"]["drive_object"]["number"], 4);
        assert_eq!(
            b["parameters"]["target"]["drive_object"]["entry"], "nc_variable",
            "轴入口不得给出 module 130 模板：{}",
            node.id
        );
    }

    let (object_leaves, _) = connection(&fixture)
        .browse("nck://identity/object/6", "", "", 200)
        .await
        .unwrap();
    assert!(!object_leaves.is_empty(), "对象 6 应有模板叶");
    for node in &object_leaves {
        let b: serde_json::Value = serde_json::from_str(&node.binding_json).unwrap();
        assert_eq!(b["parameters"]["target"]["drive_object"]["number"], 6);
        assert_eq!(
            b["parameters"]["target"]["drive_object"]["entry"], "drive_object",
            "对象入口不得给出 module 26 模板：{}",
            node.id
        );
    }
}

/// 身份实例页必须有**驱动侧**上限，不依赖调用方传对 limit（审计 P1-B 回归）。
///
/// Web 默认 `limit=50`，而实例总数上限 31；若照单全收，一次请求就会顺序读
/// 约 `31 × 5` 项身份数据，分页形同虚设。
#[tokio::test]
async fn identity_instance_page_is_bounded_regardless_of_client_limit() {
    let fixture = NckFixture::spawn(recorded_state()).await;
    // 客户端按 Web 默认值要 50 个；服务端仍必须只读一页的量。
    let (axes, next) = connection(&fixture).browse(AXES, "", "", 50).await.unwrap();
    assert!(
        axes.len() <= 8,
        "单页实例数必须受驱动侧上限约束，实际 {}",
        axes.len()
    );
    assert_eq!(
        next.as_deref(),
        Some("9"),
        "受上限约束后必须返回后续游标，否则界面无法翻页"
    );
    let reads = fixture.received_specs();
    assert!(
        reads.len() <= 8 * 5,
        "单页读取量必须可观测有上界，实际 {}",
        reads.len()
    );
    assert!(
        reads.len() < 31 * 5,
        "不得在一次请求里扫描全部实例（实际 {}）",
        reads.len()
    );
    // 后续页从游标继续，而不是重扫。
    reset_reads(&fixture);
    let (_, next2) = connection(&fixture)
        .browse(AXES, "", next.as_deref().unwrap(), 50)
        .await
        .unwrap();
    assert!(next2.is_none(), "游标 9 起已到表尾");
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
