//! 通道 / 主轴探测树 + 「未知 parent 报错」的驱动契约测试（离线回环，不连真机）。
//!
//! 为什么单独成文件：`identity_browse.rs` 覆盖轴/对象树与变量叶载荷，本文件覆盖
//! 本轮新增的两棵树与一条**行为纠正**（未知 parent 不再静默返回空页/根）。
//! 判据不同：这里最关心「凭什么认为这个实例存在」，而不是「变量叶载荷长什么样」。
//!
//! 全部响应由 `NckFixture` 按**完整请求字节**回放：未登记的地址按项拒绝
//! （`S7_ITEM_0x0A`），因此"设备明确拒绝"与"驱动凭空生成"在测试里不可能混淆。
use std::collections::HashMap;

use mesa_driver_sdk::DriverConnection;
use mesa_driver_sinumerik_nck::{
    DefinitionCatalog, NckConnConfig, NckConnection, NckFixture, NckFixtureState, PresetRegistry,
    TargetKind,
};
use serde_json::{Value, json};

const ROOT: &str = "nck://identity";
const AXES: &str = "nck://identity/axes";
const OBJECTS: &str = "nck://identity/objects";
const CHANNELS: &str = "nck://identity/channels";
const SPINDLES: &str = "nck://identity/spindles";

fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|part| u8::from_str_radix(part, 16).unwrap())
        .collect()
}

/// 通道探针的线缆地址：C 区 `area_unit = 2<<5 | 通道号`，column 20070、line 1。
///
/// 这里刻意**独立**构造字节（不调用驱动的 `resolve`）：测试要钉住的是"驱动到底
/// 把实例号放进了哪一段"，用被测代码生成期望值就变成了自证。
fn channel_spec(channel: u8) -> Vec<u8> {
    vec![
        0x12,
        0x08,
        0x82,
        0x40 | channel,
        0x4e,
        0x66,
        0x00,
        0x01,
        0x1a,
        0x01,
    ]
}

/// 主轴分配探针：A 区 `area_unit = 3<<5 | 轴号`，column 35000、line 1。
fn spindle_spec(axis: u8) -> Vec<u8> {
    vec![
        0x12,
        0x08,
        0x82,
        0x60 | axis,
        0x88,
        0xb8,
        0x00,
        0x01,
        0x1a,
        0x01,
    ]
}

/// 轴名探针：N 区、column 10000、line = 轴号。
fn axis_name_spec(axis: u8) -> Vec<u8> {
    vec![0x12, 0x08, 0x82, 0x01, 0x27, 0x10, 0x00, axis, 0x1a, 0x01]
}

/// 轴驱动模块号探针：A 区 `area_unit = 3<<5 | 轴号`、column 30110、line 1。
fn axis_module_spec(axis: u8) -> Vec<u8> {
    vec![
        0x12,
        0x08,
        0x82,
        0x60 | axis,
        0x75,
        0x9e,
        0x00,
        0x01,
        0x1a,
        0x01,
    ]
}

/// 轴输出类型探针：A 区、column 30130、line 1。
fn axis_type_spec(axis: u8) -> Vec<u8> {
    vec![
        0x12,
        0x08,
        0x82,
        0x60 | axis,
        0x75,
        0xb2,
        0x00,
        0x01,
        0x1a,
        0x01,
    ]
}

/// 驱动对象名探针（**module 26**，轴路径核对入口）：
/// V 区 `area_unit = 5<<5 | 驱动号`、column 199、25×U16BE。
fn drive_name_spec(drive: u8) -> Vec<u8> {
    vec![
        0x12,
        0x08,
        0x82,
        0xa0 | drive,
        0x00,
        0xc7,
        0x00,
        0x01,
        0x1a,
        0x19,
    ]
}

fn utf8_fixed(text: &str, len: usize) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.resize(len, 0);
    bytes
}

fn utf16be_fixed(text: &str, chars: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(chars * 2);
    for unit in text.encode_utf16().take(chars) {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    bytes.resize(chars * 2, 0);
    bytes
}

/// 契约场景里的 8 个真实轴（名称/驱动/输出类型/主轴分配来自真机证据形态）。
///
/// 轴6 = MB1、output_type=0：这是"轴名读到了，但驱动对象身份未确认"的形态，
/// 轴本身仍是真实实例；前端必须能选到它，同时标签如实说明未确认。
fn contract_axis_items() -> Vec<(Vec<u8>, ItemVector)> {
    let axes = [
        ("MX1", 1u8, 1u8, 0u8),
        ("MY1", 2, 1, 1),
        ("MZ1", 3, 1, 0),
        ("MSP1", 4, 1, 1),
        ("MA1", 5, 1, 0),
        ("MB1", 6, 0, 2),
        ("MC1", 7, 1, 0),
        ("MQ1", 8, 1, 0),
    ];
    let mut items = Vec::new();
    for (position, (name, drive, output_type, spindle)) in axes.iter().enumerate() {
        let index = (position + 1) as u8;
        items.push((axis_name_spec(index), (0xff, 9, utf8_fixed(name, 16))));
        items.push((axis_module_spec(index), (0xff, 9, vec![*drive])));
        items.push((axis_type_spec(index), (0xff, 9, vec![*output_type])));
        items.push((spindle_spec(index), (0xff, 9, vec![*spindle])));
        if *output_type == 1 {
            items.push((
                drive_name_spec(*drive),
                (0xff, 6, utf16be_fixed(&format!("SERVO_{drive}"), 25)),
            ));
        }
    }
    items
}

/// 单项现场向量：返回码 + transport + 载荷。
type ItemVector = (u8, u8, Vec<u8>);

/// 现场向量：请求字节 → (返回码, transport, 载荷)。未登记的请求一律按项拒绝。
fn state(items: Vec<(Vec<u8>, ItemVector)>) -> NckFixtureState {
    NckFixtureState {
        recorded_items: Some(items.into_iter().collect::<HashMap<_, _>>()),
        ..Default::default()
    }
}

fn connection(fixture: &NckFixture) -> NckConnection {
    let cfg = NckConnConfig::from_json(&json!({
        "host": "127.0.0.1", "port": fixture.addr.port(),
        "family": "828d", "local_tsap": 256, "remote_tsap": 258, "max_read_items": 1
    }))
    .unwrap();
    NckConnection::with_presets(cfg, PresetRegistry::embedded())
}

fn ids(nodes: &[mesa_driver_protocol::pb::BrowseNode]) -> Vec<String> {
    nodes.iter().map(|node| node.id.clone()).collect()
}

// ---------------------------------------------------------------------------
// 1. 通道树：四类结果必须分开
// ---------------------------------------------------------------------------

/// 通道枚举的**唯一**合法性判据是"逐通道读 `AXCONF_MACHAX_USED[1]` 的结果"，
/// 因此四类结果必须各自落到不同处理上：
///
/// | 结果 | 处理 |
/// |---|---|
/// | 读通且位置1映射到机床轴 | 成节点，`selection_mode=2` |
/// | 设备明确拒绝（无效实例） | **不成节点**，也不产生诊断噪声 |
/// | 通信/协议失败 | 不成节点 + `IDENTITY_INSTANCE_READ_FAILED` 诊断 |
/// | 读通但无法判定（值 0 / 形状异常） | 不成节点 + `IDENTITY_PENDING_REVIEW` 诊断 |
#[tokio::test]
async fn channels_tree_separates_valid_rejected_unreadable_and_undecidable() {
    let fixture = NckFixture::spawn(state(vec![
        // 通道1：读通且位置1→机床轴1 ⇒ 有效通道。
        (channel_spec(1), (0xff, 9, vec![1])),
        // 通道3：读通但位置1=0 ⇒ 无法判定（既不是"不存在"，也不是"有效"）。
        (channel_spec(3), (0xff, 9, vec![0])),
        // 通道4：读通但 transport 与期望不符 ⇒ 通信/协议失败。
        (channel_spec(4), (0xff, 7, vec![1])),
        // 通道2 刻意不登记 ⇒ 设备按项拒绝（S7_ITEM_0x0A）= 无效实例。
    ]))
    .await;
    let (nodes, next) = connection(&fixture)
        .browse(CHANNELS, "", "", 4)
        .await
        .unwrap();

    // 无效实例：不成节点（否则"31 个通道"会凭空出现）。
    assert!(
        !ids(&nodes).contains(&format!("{CHANNELS}/2")),
        "被设备拒绝的通道不得成节点：{:?}",
        ids(&nodes)
    );

    // 有效通道：可选语义节点，标签写明依据（凭什么认为它存在）。
    let channel1 = nodes
        .iter()
        .find(|node| node.id == format!("{CHANNELS}/1"))
        .expect("通道1 必须成节点");
    assert_eq!(channel1.selection_mode, 2, "有效通道必须声明 SEMANTIC 可选");
    assert_eq!(channel1.kind, "nck_variable");
    assert!(
        channel1.label.contains("位置1→机床轴1"),
        "标签必须写明读数依据：{}",
        channel1.label
    );

    // 无法判定：只有诊断节点，稳定码 + WARNING（不是红色阻断，也不是静默）。
    let pending = nodes
        .iter()
        .find(|node| node.id == format!("{CHANNELS}/note/3"))
        .expect("通道3 必须留诊断");
    assert_eq!(pending.kind, "diagnostic");
    assert_eq!(pending.selection_mode, 3, "诊断节点不可选");
    assert_eq!(pending.diagnostics.len(), 1);
    assert_eq!(pending.diagnostics[0].code, "IDENTITY_PENDING_REVIEW");
    assert_eq!(pending.diagnostics[0].severity, 2, "无法判定是警告级");
    assert!(
        pending.diagnostics[0].message.contains("无法判定"),
        "{}",
        pending.diagnostics[0].message
    );

    // 通信/协议失败：诊断码与"不存在"严格不同，且必须说明枚举中断。
    let failed = nodes
        .iter()
        .find(|node| node.id == format!("{CHANNELS}/note/4"))
        .expect("通道4 必须留诊断");
    assert_eq!(failed.diagnostics[0].code, "IDENTITY_INSTANCE_READ_FAILED");
    assert!(
        failed.diagnostics[0].message.contains("可能不完整"),
        "{}",
        failed.diagnostics[0].message
    );
    assert!(next.is_none(), "通信失败后不得给出后续游标");

    // 有界：4 个实例各一次读，没有隐式全量扫描。
    assert_eq!(fixture.received_specs().len(), 4);
}

/// 通道节点的载荷必须**由需要 `channel` 的定义**构造，而不是手写拼接。
///
/// 具体钉住四件事：
/// ① 坐标只给 `channel`（不绑死任何一条 `variable_id`，否则其它变量用不了）；
/// ② `identity.keys` = `[{path:"channel", required:true}]`；
/// ③ `identity.template` 取自真实定义的原文；
/// ④ `applies_to_variables` 与**目录事实**逐字一致（16 条定义 `targets` 含 channel）。
#[tokio::test]
async fn channel_payload_is_built_from_definitions_that_need_channel() {
    let fixture = NckFixture::spawn(state(vec![(channel_spec(2), (0xff, 9, vec![7]))])).await;
    let (nodes, _) = connection(&fixture)
        .browse(CHANNELS, "", "", 2)
        .await
        .unwrap();
    let node = nodes
        .iter()
        .find(|node| node.id == format!("{CHANNELS}/2"))
        .expect("通道2 必须成节点");

    // 旧客户端继续读 binding_json：两条通路必须同内容。
    assert_eq!(node.selection_payload_json, node.binding_json);
    let payload: Value = serde_json::from_str(&node.selection_payload_json).unwrap();
    assert_eq!(payload["kind"], "nck_variable");
    assert_eq!(payload["parameters"]["target"]["channel"], 2);
    assert_eq!(payload["parameters"]["arguments"], json!({}));
    assert!(
        payload["parameters"].get("variable_id").is_none(),
        "实例节点不得绑死某一条变量（否则其它 channel 变量会被判成变量不一致）：{payload}"
    );
    assert_eq!(
        payload["identity"]["keys"],
        json!([{ "path": "channel", "required": true }])
    );
    assert_eq!(
        payload["identity"]["template"], "channel.feed.actual@channel={channel}",
        "身份模板必须取自真实定义原文"
    );
    assert!(
        payload["identity"]["preview_point_key"].is_null(),
        "单坐标不构成完整身份，不得伪造预览 Key：{payload}"
    );

    // 适用定义 = 目录里"声明 channel 且把它当必填身份键"的定义，逐字比对。
    let catalog = DefinitionCatalog::embedded().unwrap();
    let expected: Vec<String> = catalog
        .for_family("828d")
        .into_iter()
        .filter(|def| def.targets.contains(&TargetKind::Channel))
        .filter(|def| {
            def.identity_keys
                .iter()
                .any(|key| key.path == "channel" && key.required)
        })
        .map(|def| def.variable_id.clone())
        .collect();
    let applies: Vec<String> = payload["identity"]["applies_to_variables"]
        .as_array()
        .expect("applies_to_variables 必须是数组")
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert_eq!(applies, expected, "适用定义必须与目录事实一致");
    assert_eq!(
        applies.len(),
        16,
        "42 条定义中 16 条的 targets 含 channel（本断言是回归钉子）"
    );
}

/// 通道枚举必须落在**通道号段**（`area_no`），且 channel=1 的字节与既有已核对
/// 布局逐字节相同（不推翻证据，只是把 channel>1 指向真正的通道 N）。
#[tokio::test]
async fn channel_probe_addresses_the_channel_number() {
    let fixture = NckFixture::spawn(state(vec![
        (channel_spec(1), (0xff, 9, vec![1])),
        (channel_spec(2), (0xff, 9, vec![2])),
    ]))
    .await;
    connection(&fixture)
        .browse(CHANNELS, "", "", 2)
        .await
        .unwrap();
    let specs = fixture.received_specs();
    // 既有布局推导证据（identity_spec.rs 的 `axis_and_channel_probe_layout_derived`）。
    assert_eq!(specs[0], bytes("12 08 82 41 4e 66 00 01 1a 01"));
    // 通道2 ⇒ area_unit 0x42；若把通道号放进 line 段，这里会是 `... 41 4e 66 00 02 ...`。
    assert_eq!(specs[1], bytes("12 08 82 42 4e 66 00 01 1a 01"));
}

/// 通道页有驱动侧上限，不依赖调用方传对 `limit`（Web 默认 50）。
#[tokio::test]
async fn channel_page_is_bounded_regardless_of_client_limit() {
    let items: Vec<_> = (1..=31u8)
        .map(|channel| (channel_spec(channel), (0xff, 9, vec![1])))
        .collect();
    let fixture = NckFixture::spawn(state(items)).await;
    let (nodes, next) = connection(&fixture)
        .browse(CHANNELS, "", "", 50)
        .await
        .unwrap();
    assert!(
        nodes.len() <= 8,
        "单页实例数必须有上限，实际 {}",
        nodes.len()
    );
    assert_eq!(
        next.as_deref(),
        Some("9"),
        "必须有后续游标，否则界面翻不了页"
    );
    assert_eq!(
        fixture.received_specs().len(),
        8,
        "单页读取量必须可观测有界"
    );

    // 后续页从游标继续，且不会超出 31 的上界。
    let (nodes, next) = connection(&fixture)
        .browse(CHANNELS, "", next.as_deref().unwrap(), 50)
        .await
        .unwrap();
    assert_eq!(nodes[0].id, format!("{CHANNELS}/9"));
    assert!(next.is_some());
}

// ---------------------------------------------------------------------------
// 2. 主轴树：只能由真实读数派生
// ---------------------------------------------------------------------------

/// 主轴节点必须来自 `SPIND_ASSIGN_TO_MACHAX` 的非零分配号：
/// 零分配的轴不生成节点；多轴指向同一主轴时**去重**为一个节点并记录来源轴。
#[tokio::test]
async fn spindles_are_derived_from_axis_assignments_and_deduplicated() {
    let fixture = NckFixture::spawn(state(vec![
        (spindle_spec(1), (0xff, 9, vec![0])),
        (spindle_spec(2), (0xff, 9, vec![1])),
        (spindle_spec(3), (0xff, 9, vec![0])),
        // 轴4 与轴2 指向同一主轴 ⇒ 必须去重成一个节点，来源轴都要记下。
        (spindle_spec(4), (0xff, 9, vec![1])),
        (spindle_spec(6), (0xff, 9, vec![2])),
    ]))
    .await;
    let (nodes, _) = connection(&fixture)
        .browse(SPINDLES, "", "", 50)
        .await
        .unwrap();

    assert_eq!(
        ids(&nodes),
        vec![format!("{SPINDLES}/1"), format!("{SPINDLES}/2")],
        "主轴集合必须恰好是读数给出的两个，且按主轴号升序"
    );
    let spindle1 = &nodes[0];
    assert_eq!(spindle1.selection_mode, 2, "主轴节点必须是可选语义叶");
    assert!(
        spindle1.label.contains("轴2") && spindle1.label.contains("轴4"),
        "去重后必须记录来源轴：{}",
        spindle1.label
    );
    let payload: Value = serde_json::from_str(&spindle1.selection_payload_json).unwrap();
    assert_eq!(payload["parameters"]["target"]["spindle"], 1);
    assert_eq!(
        payload["identity"]["keys"],
        json!([{ "path": "spindle", "required": true }]),
        "主轴节点只提供 spindle 坐标"
    );
    // 适用定义同样来自目录事实（3 条定义 targets 含 spindle）。
    let applies = payload["identity"]["applies_to_variables"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(applies, 3, "42 条定义中 3 条的 targets 含 spindle");

    // 零分配的轴绝不生成节点：主轴 0 不是"零号主轴"。
    assert!(!ids(&nodes).contains(&format!("{SPINDLES}/0")));
    // 有界：主轴集合是跨轴派生的，单次请求最多 31 次读，不多读。
    assert_eq!(fixture.received_specs().len(), 31);
}

/// 轴读数失败时：该轴不参与派生，但**必须**留诊断说明"集合可能不完整"。
///
/// 这里的失败是"单项协议异常"（transport 不符），会话仍然可用 ⇒ 其余轴必须继续读，
/// 否则一个坏轴会连带丢掉后面所有主轴。
#[tokio::test]
async fn spindle_axis_read_failure_keeps_partial_set_with_diagnostic() {
    let fixture = NckFixture::spawn(state(vec![
        (spindle_spec(1), (0xff, 9, vec![1])),
        // 轴2：transport 与期望不符 ⇒ 该轴读不到（不是"该轴不是主轴"）。
        (spindle_spec(2), (0xff, 7, vec![1])),
        (spindle_spec(3), (0xff, 9, vec![2])),
    ]))
    .await;
    let (nodes, _) = connection(&fixture)
        .browse(SPINDLES, "", "", 50)
        .await
        .unwrap();

    // 诊断排在第一位：本地分页按 limit 切分，诊断必须落在第一页才看得见。
    assert_eq!(nodes[0].kind, "diagnostic");
    assert_eq!(
        nodes[0].diagnostics[0].code,
        "IDENTITY_INSTANCE_READ_FAILED"
    );
    assert!(
        nodes[0].diagnostics[0].message.contains("轴2"),
        "必须指出是哪根轴：{}",
        nodes[0].diagnostics[0].message
    );
    assert!(
        nodes[0].diagnostics[0].message.contains("可能不完整"),
        "必须说明集合可能不完整：{}",
        nodes[0].diagnostics[0].message
    );
    // 轴1、轴3 的主轴照常派生（协议异常不中断扫描）。
    assert_eq!(
        ids(&nodes)[1..].to_vec(),
        vec![format!("{SPINDLES}/1"), format!("{SPINDLES}/2")]
    );
}

/// 分配号非法/超范围：既不能当主轴，也不能当"该轴不是主轴"，只能给诊断。
#[tokio::test]
async fn out_of_range_spindle_assignment_is_diagnosed_not_a_node() {
    let fixture = NckFixture::spawn(state(vec![
        // 200 > MAX_INSTANCE(31)：地址模型里不存在这样的主轴实例。
        (spindle_spec(1), (0xff, 9, vec![200])),
        (spindle_spec(2), (0xff, 9, vec![1])),
    ]))
    .await;
    let (nodes, _) = connection(&fixture)
        .browse(SPINDLES, "", "", 50)
        .await
        .unwrap();

    assert!(
        !ids(&nodes).contains(&format!("{SPINDLES}/200")),
        "越界分配号不得成节点：{:?}",
        ids(&nodes)
    );
    let note = nodes
        .iter()
        .find(|node| node.id == format!("{SPINDLES}/note/axis-1"))
        .expect("越界分配号必须留诊断");
    assert_eq!(note.diagnostics[0].code, "IDENTITY_SPINDLE_ASSIGN_INVALID");
    assert!(ids(&nodes).contains(&format!("{SPINDLES}/1")));
}

// ---------------------------------------------------------------------------
// 3. 无效 parent：稳定错误码，绝不伪装成根/空页
// ---------------------------------------------------------------------------

/// `parent` 为空才是根；未知 id 必须报错。
///
/// 旧行为是"未知 parent → 空页"，于是「把字段名写成 `parent_node_id`」这种客户端
/// 错误在链路上完全不可见（请求里 parent 为空 → 静默返回根节点）。本测试同时钉住
/// 正向对照（空 parent 正常返回根）与负向断言（未知 id 报 `UNKNOWN_PARENT`）。
#[tokio::test]
async fn unknown_parent_is_a_stable_error_and_empty_parent_is_the_root() {
    let fixture = NckFixture::spawn(state(vec![])).await;

    // 正向对照：空 parent = 根（模板入口 + 身份入口）。
    let (root, _) = connection(&fixture).browse("", "", "", 10).await.unwrap();
    assert_eq!(root[0].id, "nck://presets");
    assert!(
        ids(&root).contains(&ROOT.to_string()),
        "根必须包含身份入口：{:?}",
        ids(&root)
    );

    // 身份根下四个分支：轴 / 通道 / 主轴 / 驱动对象，都是不可选文件夹。
    let (groups, _) = connection(&fixture).browse(ROOT, "", "", 10).await.unwrap();
    assert_eq!(
        ids(&groups),
        vec![
            AXES.to_string(),
            CHANNELS.to_string(),
            SPINDLES.to_string(),
            OBJECTS.to_string()
        ],
        "身份根必须列出四棵树"
    );
    for node in &groups {
        assert_eq!(node.kind, "folder");
        assert_eq!(node.selection_mode, 3, "分支节点不可选：{}", node.id);
        assert!(node.has_children);
    }

    // 负向：未知 id 一律稳定错误码，且文案带上原始 parent（可排查）。
    for parent in [
        "nck://unknown",
        "nck://identity/nope",
        "nck://identity/channel/1",
        "nck://presets/x",
    ] {
        let error = connection(&fixture)
            .browse(parent, "", "", 10)
            .await
            .unwrap_err();
        assert_eq!(error.code, "UNKNOWN_PARENT", "parent={parent}");
        assert!(
            error.message.contains(parent),
            "错误文案必须回显原始 parent（parent={parent}）：{}",
            error.message
        );
    }

    // 全程不访问控制器：根、分支列表与错误路径都必须是纯本地判定。
    assert!(fixture.received_specs().is_empty());
}

// ---------------------------------------------------------------------------
// 4. 前端契约夹具：树 id 与节点形状由驱动产出，前端测试只消费
// ---------------------------------------------------------------------------

/// 夹具现场：8 个真实轴（含 MB1 未确认形态）+ 3 个通道（其中 1 个"无法判定"）
/// + 2 个主轴（主轴1 由两根轴指向）。
fn contract_scene() -> NckFixtureState {
    let mut items = contract_axis_items();
    items.extend(vec![
        (channel_spec(1), (0xff, 9, vec![1])),
        (channel_spec(2), (0xff, 9, vec![2])),
        // 通道3 位置1=0 ⇒ 无法判定（前端要能看到诊断，而不是"少了一个通道"）。
        (channel_spec(3), (0xff, 9, vec![0])),
        (spindle_spec(2), (0xff, 9, vec![1])),
        (spindle_spec(4), (0xff, 9, vec![1])),
        (spindle_spec(6), (0xff, 9, vec![2])),
    ]);
    state(items)
}

/// 节点 → 与 Core REST 透传层**同形**的 JSON（`core-api::browse_nodes_json` 的字段集）。
///
/// 为什么不直接 serde 序列化 `BrowseNode`：protobuf 类型没有 serde 派生，且前端
/// 消费的是 Core 透传后的 REST 形状（字段名、`selection_mode` 的数值化枚举都在
/// 那一层定死）。这里逐字对齐，夹具才能被前端测试直接当响应体用。
fn node_json(node: &mesa_driver_protocol::pb::BrowseNode) -> Value {
    json!({
        "id": node.id,
        "label": node.label,
        "kind": node.kind,
        "data_type": node.data_type,
        "access": node.access,
        "has_children": node.has_children,
        "binding_json": node.binding_json,
        "selection_mode": node.selection_mode,
        "selection_payload_json": node.selection_payload_json,
        "diagnostics": node
            .diagnostics
            .iter()
            .map(|d| json!({ "code": d.code, "message": d.message, "severity": d.severity }))
            .collect::<Vec<_>>(),
    })
}

/// 前端流程测试消费的**契约夹具**（`apps/mesa-web/src/test/nckIdentityProbeContract.json`）。
///
/// 为什么需要它：前端测试离线运行，不可能拉起驱动；但它又**不能自己编**树 id 与
/// 节点形状——历史教训是 `nck://identity/channels` 曾在测试里被凭空写出来，而驱动
/// 根本没有这棵树，于是前端测试全绿、真机走不通。因此夹具由**驱动**产出：本测试在
/// 回环脚手架上真实浏览一遍，把每个 parent 的节点序列序列化成 JSON，前端测试逐字
/// 消费同一份文件（树 id、`selection_mode`、载荷、诊断码全部来自驱动）。
///
/// 同步机制：文件与驱动实际输出不一致 ⇒ 本测试失败。重新生成：
/// `MESA_UPDATE_IDENTITY_CONTRACT=1 cargo test -p mesa-driver-sinumerik-nck --test identity_probe_trees`
#[tokio::test]
async fn web_contract_fixture_matches_driver_browse_contract() {
    let fixture = NckFixture::spawn(contract_scene()).await;
    let mut pages = serde_json::Map::new();
    // 只导出前端流程真正下钻的那几页；其余树（轴/驱动对象）由本文件与
    // `identity_browse.rs` 的驱动侧断言覆盖。
    for (parent, limit) in [
        ("", 10u32),
        ("nck://presets", 6),
        (ROOT, 10),
        // 轴页是联动选择 axis.* 的实例来源，必须导出；否则前端测试会再次
        // 把"轴选不到"漏掉（本轮修复的直接盲区）。
        (AXES, 8),
        (CHANNELS, 8),
        (SPINDLES, 50),
    ] {
        let (nodes, next) = connection(&fixture)
            .browse(parent, "", "", limit)
            .await
            .unwrap();
        // 与 Core REST 的响应体同形：`{nodes, next_cursor}`——前端拿到的就是它。
        pages.insert(
            parent.to_string(),
            json!({
                "nodes": nodes.iter().map(node_json).collect::<Vec<_>>(),
                "next_cursor": next,
            }),
        );
    }
    let document = json!({
        "note": "由 drivers/sinumerik-nck/tests/identity_probe_trees.rs 的 web_contract_fixture_matches_driver_browse_contract 产出：前端测试只消费，不得手改。",
        "family": "828d",
        "pages": pages,
    });
    let text = format!("{}\n", serde_json::to_string_pretty(&document).unwrap());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/mesa-web/src/test/nckIdentityProbeContract.json");
    if std::env::var("MESA_UPDATE_IDENTITY_CONTRACT").is_ok() {
        std::fs::write(&path, &text).expect("写夹具");
        return;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    // 换行必须归一后再比较：Windows 检出会把仓库里的 LF 转成 CRLF（`core.autocrlf`），
    // 若直接按字节比较，同一份夹具在 Linux 上通过、在 Windows 上失败 —— 那是检出设置的
    // 差异，不是夹具与驱动脱节。生成侧恒写 LF（见上面的 `format!`），故两边都归一即可。
    let normalize = |s: &str| s.replace("\r\n", "\n");
    assert_eq!(
        normalize(&existing),
        normalize(&text),
        "前端契约夹具与驱动实际输出不一致（{}）：用 MESA_UPDATE_IDENTITY_CONTRACT=1 重新生成",
        path.display()
    );
}

/// 主轴页的游标是**结果偏移**（不是轴号），非法游标必须在建连之前失败。
///
/// 为什么单独钉这条：通道/轴/对象树的游标是实例号（1..=31），主轴页却是本地分页的
/// 结果偏移。两套语义混用会让"翻到第 2 页"变成"从轴 2 重新扫描"，或让偏移 0
/// 被当成非法实例号——都是看起来能跑、结果悄悄错的那类缺陷。
#[tokio::test]
async fn spindle_cursor_is_a_result_offset_and_invalid_cursor_fails_before_connecting() {
    let fixture = NckFixture::spawn(state(vec![
        (spindle_spec(2), (0xff, 9, vec![1])),
        (spindle_spec(4), (0xff, 9, vec![2])),
    ]))
    .await;
    let (first, next) = connection(&fixture)
        .browse(SPINDLES, "", "", 1)
        .await
        .unwrap();
    assert_eq!(ids(&first), vec![format!("{SPINDLES}/1")]);
    assert_eq!(
        next.as_deref(),
        Some("1"),
        "游标必须是结果偏移（第 2 项从 1 开始）"
    );
    let (second, next) = connection(&fixture)
        .browse(SPINDLES, "", next.as_deref().unwrap(), 1)
        .await
        .unwrap();
    assert_eq!(ids(&second), vec![format!("{SPINDLES}/2")]);
    assert!(next.is_none());

    // 非法游标：建连前失败，零读取。
    fixture
        .state
        .lock()
        .unwrap()
        .received
        .lock()
        .unwrap()
        .clear();
    let error = connection(&fixture)
        .browse(SPINDLES, "", "bad", 1)
        .await
        .unwrap_err();
    assert_eq!(error.code, "IDENTITY_CURSOR");
    assert!(
        fixture.received_specs().is_empty(),
        "非法游标不得访问控制器"
    );
}
