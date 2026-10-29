//! 驱动侧**导航分类**投影的验收（CNC 变量 / 系统变量 / 机床数据 / 驱动参数 / 未分类）。
//!
//! ## 这个文件守的边界
//!
//! 分类是驱动侧为了前端建树临时定的**导航规则**，不是手册确认的事实。因此这里
//! 不只测「分了什么」，还测「分类敢不敢自证」：每行必须带
//! 最新 revision 的 `category_basis = "definition"`，且历史 `r1.json` 里绝不能出现分类字段
//! （定义已冻结，分类不得伪装成定义的一部分）。
//!
//! ## 只读性
//!
//! 全部用例不建连接、不连机床：目录与分类都是纯计算路径（`catalog_category.rs`
//! 不引用设备访问模块）。

use std::collections::{BTreeMap, BTreeSet};

use mesa_driver_sdk::Driver;
use mesa_driver_sinumerik_nck::{
    CATEGORY_BASIS_NAVIGATION_RULE, CatalogCategory, DefinitionCatalog, SinumerikNckDriver,
    classify_category,
};
use serde_json::Value;

fn driver() -> SinumerikNckDriver {
    SinumerikNckDriver
}

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_repo_file(rel: &str) -> String {
    std::fs::read_to_string(repo_path(rel)).unwrap_or_else(|e| panic!("读取 `{rel}` 失败：{e}"))
}

/// `r1.json` 的原始定义（比对基准；只读，绝不改写）。
fn raw_definitions() -> BTreeMap<String, Value> {
    let doc: Value = serde_json::from_str(&read_repo_file(
        "drivers/sinumerik-nck/definitions/828d/r1.json",
    ))
    .expect("r1.json 必须是合法 JSON");
    doc["definitions"]
        .as_array()
        .expect("r1.json 必须含 definitions 数组")
        .iter()
        .map(|d| {
            (
                d["variable_id"].as_str().expect("variable_id").to_string(),
                d.clone(),
            )
        })
        .collect()
}

async fn catalog_json() -> Value {
    let raw = driver()
        .definitions(&serde_json::json!({ "limit": 500 }).to_string())
        .await
        .expect("目录查询应成功");
    serde_json::from_str(&raw).expect("目录响应必须是合法 JSON")
}

/// 分类码 → 该桶里的 variable_id 集合（分类是覆盖 42 条的分桶，不是标签集合）。
async fn partition() -> BTreeMap<String, BTreeSet<String>> {
    let v = catalog_json().await;
    let rows = v["definitions"].as_array().expect("definitions 必须是数组");
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        let id = row["variable_id"]
            .as_str()
            .expect("variable_id")
            .to_string();
        let category = row["category"]
            .as_str()
            .unwrap_or_else(|| panic!("{id} 缺 `category`"))
            .to_string();
        out.entry(category).or_default().insert(id);
    }
    out
}

const CATEGORY_CODES: [&str; 5] = [
    "cnc_variable",
    "system_variable",
    "machine_data",
    "drive_parameter",
    "unclassified",
];

// ---------------------------------------------------------------------------
// 1. 全覆盖 + 取值合法 + 自证依据
// ---------------------------------------------------------------------------

/// 42 条**逐条**都有 `category`，且取值只能是五个码之一；`category_basis` 恒为
/// `"definition"`（最新 revision 已将 category 纳入摘要锁，不再使用投影期导航规则）。
#[tokio::test]
async fn every_definition_declares_a_legal_navigation_category() {
    let v = catalog_json().await;
    let rows = v["definitions"].as_array().expect("definitions 必须是数组");
    assert_eq!(rows.len(), 42, "目录必须如实返回全部 42 条");

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for row in rows {
        let id = row["variable_id"].as_str().expect("variable_id");
        let category = row["category"]
            .as_str()
            .unwrap_or_else(|| panic!("{id} 缺 `category`"));
        assert!(
            CATEGORY_CODES.contains(&category),
            "{id} 的 category `{category}` 不在五个合法码内"
        );
        let basis = row["category_basis"]
            .as_str()
            .unwrap_or_else(|| panic!("{id} 缺 `category_basis`"));
        assert_eq!(
            basis, "definition",
            "{id} 的最新 revision 必须由正式定义字段提供 category_basis=definition"
        );
        for forbidden in ["manual", "verified", "authoritative"] {
            assert_ne!(
                basis, forbidden,
                "{id} 不得把导航分类伪装成 {forbidden} 级证据"
            );
        }
        seen.insert(category);
    }
    // 四个「已决」分类码都有成员；`unclassified` 当前为空（由单独用例解释）。
    assert_eq!(
        seen,
        [
            "cnc_variable",
            "system_variable",
            "machine_data",
            "drive_parameter"
        ]
        .into_iter()
        .collect::<BTreeSet<&str>>(),
        "本轮出现的分类码必须恰为四个已决分类（unclassified 见 unclassified_bucket_is_empty_today）"
    );
}

/// 分类是**投影期附加字段**：`r1.json` 本身绝不能出现分类字段（定义已冻结）。
#[tokio::test]
async fn frozen_definitions_carry_no_category_field() {
    for (id, raw) in raw_definitions() {
        for forbidden in ["category", "category_basis"] {
            assert!(
                raw.get(forbidden).is_none(),
                "{id} 在 r1.json 里不得出现 `{forbidden}`（分类是驱动侧导航规则，不是定义字段）"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. 分桶结果（42 条逐条钉住）
// ---------------------------------------------------------------------------

/// 每个桶的成员**逐条钉住**：分类若被静默改动，这里立刻失败。
#[tokio::test]
async fn category_partition_is_pinned_entry_by_entry() {
    let got = partition().await;

    let expect = |code: &str| -> BTreeSet<String> {
        got.get(code)
            .cloned()
            .unwrap_or_else(|| panic!("分类 `{code}` 在输出里不存在"))
    };

    // CNC 变量：NC 变量清单（OPI 入口）里的通道/轴/主轴/方式组运行数据，15 条。
    let cnc: BTreeSet<String> = [
        "channel.program.status",
        "mode_group.op_mode",
        "channel.program.name.main",
        "channel.program.name.active",
        "channel.program.line_number",
        "channel.program.block",
        "axis.position.commanded_mcs",
        "axis.position.actual_bcs",
        "channel.feed.actual",
        "channel.feed.commanded",
        "channel.feed.override",
        "spindle.speed.actual",
        "spindle.speed.commanded",
        "spindle.speed.override",
        "channel.tool.number_active",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(expect("cnc_variable"), cnc, "CNC 变量桶必须恰为 15 条");

    // 系统变量：本轮只有 R 参数（系统变量手册把 Rn/$R[n] 定义为「系统变量」）。
    let sys: BTreeSet<String> = ["channel.r_parameter"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        expect("system_variable"),
        sys,
        "系统变量桶本轮只有 R 参数一条（其余定义都不是可编程系统变量）"
    );

    // 机床数据：符号 + MD 编号双证据的 6 条身份定义。
    let md: BTreeSet<String> = [
        "axis.name",
        "axis.ctrlout.module_number",
        "axis.encoder.module_number",
        "axis.ctrlout.type",
        "channel.machine_axis_assignment",
        "axis.spindle_assignment",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(expect("machine_data"), md, "机床数据桶必须恰为 6 条");

    // 驱动参数：19 条 S120 参数 + 1 条驱动对象名（p0199），共 20 条。
    let drive: BTreeSet<String> = [
        "drive.parameter.r0021",
        "drive.parameter.r0027",
        "drive.motor.temperature",
        "drive.parameter.r0033",
        "drive.parameter.r0031",
        "drive.parameter.r0062",
        "drive.parameter.r0063",
        "drive.parameter.r0064",
        "drive.parameter.r0077",
        "drive.parameter.r0078",
        "drive.parameter.r0079",
        "drive.parameter.r0080",
        "drive.parameter.r0081",
        "drive.parameter.r0082",
        "drive.parameter.r0103",
        "drive.parameter.p0105",
        "drive.parameter.r0106",
        "drive.parameter.r0107",
        "drive.parameter.r0108",
        "drive.object.name",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(expect("drive_parameter"), drive, "驱动参数桶必须恰为 20 条");

    // 15 + 1 + 6 + 20 + 0 = 42：分桶是全覆盖的，没有条目被丢在四类之外。
    let total: usize = got.values().map(BTreeSet::len).sum();
    assert_eq!(total, 42, "五个桶必须恰好覆盖 42 条定义");
}

/// `unclassified` 判定：**当前为空**，并说明为什么不是「偷偷塞满」。
///
/// 三条最容易被误判的边界条目，各自都有定义内证据支撑，因此**不该**进这个桶：
/// - `channel.r_parameter`：`golden_ref` 是 RP 块 `rpa`，系统变量手册把 R 参数
///   定义为「系统变量 Rn」，归 `system_variable`；
/// - `channel.machine_axis_assignment`：`channel.` 前缀但符号 `AXCONF_MACHAX_USED`
///   + 列号 20070 证明是机床数据，归 `machine_data`；
/// - `drive.object.name`：无 manual 来源，但 `golden_ref` 有 `drive_object` 入口、
///   参数号 `p0199` 是驱动参数，归 `drive_parameter`。
///
/// 待分类桶的真正用途是**失败关闭**：未来定义未登记、或表与定义漂移时落这里
/// （见下方反例用例），而不是「拿不准就随便丢」。
#[tokio::test]
async fn unclassified_bucket_is_empty_today() {
    let got = partition().await;
    let unclassified = got.get("unclassified").cloned().unwrap_or_default();
    assert!(
        unclassified.is_empty(),
        "当前 42 条都有显式规则且证据锚点成立，unclassified 必须为空；实际 {unclassified:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. 分类必须跟随定义内证据（不许凭空分类）
// ---------------------------------------------------------------------------

/// 每条分类都能在定义自身的证据里找到依据：
/// - `drive_parameter` ⇒ `golden_ref` 含 `drive_object` 入口（26/130 两条入口不可折叠）；
/// - `machine_data` ⇒ `golden_ref.variable` 是机床数据符号；
/// - `cnc_variable` / `system_variable` ⇒ `source` 引的是 NC 变量清单（OPI 入口）。
#[tokio::test]
async fn categories_are_backed_by_definition_evidence() {
    let v = catalog_json().await;
    let rows = v["definitions"].as_array().expect("definitions 必须是数组");
    let md_symbols = [
        "MDSA_AXCONF_MACHAX_NAME_TAB",
        "MDCA_CTRLOUT_MODULE_NR",
        "MDCA_ENC_MODULE_NR",
        "MDCA_CTRLOUT_TYPE",
        "AXCONF_MACHAX_USED",
        "SPIND_ASSIGN_TO_MACHAX",
    ];

    for row in rows {
        let id = row["variable_id"].as_str().expect("variable_id");
        let category = row["category"].as_str().expect("category");
        let golden: Vec<&str> = row["golden_ref"]
            .as_array()
            .expect("golden_ref")
            .iter()
            .map(|g| g["variable"].as_str().expect("golden variable"))
            .collect();
        let entries: Vec<&str> = row["golden_ref"]
            .as_array()
            .expect("golden_ref")
            .iter()
            .map(|g| g["entry"].as_str().expect("entry"))
            .collect();

        match category {
            "drive_parameter" => assert!(
                entries.contains(&"drive_object"),
                "{id} 归驱动参数，但 golden_ref 里没有 drive_object 入口"
            ),
            "machine_data" => assert!(
                golden.iter().any(|g| md_symbols.contains(g)),
                "{id} 归机床数据，但 golden_ref 里没有机床数据符号：{golden:?}"
            ),
            "cnc_variable" | "system_variable" => {
                let has_nc_manual = row["source"].as_array().expect("source").iter().any(|s| {
                    s["kind"].as_str() == Some("manual")
                        && s["reference"]
                            .as_str()
                            .is_some_and(|r| r.contains("NC variables"))
                });
                assert!(
                    has_nc_manual,
                    "{id} 归 {category}，但 source 里没有 NC 变量清单的手册依据"
                );
            }
            other => panic!("{id} 的分类 `{other}` 未在本用例的检查范围内"),
        }
    }
}

// ---------------------------------------------------------------------------
// 4. 反例：无法归类的条目不得被勉强塞进四类之一
// ---------------------------------------------------------------------------

/// 未经登记的 ID（形状再像 `channel.*`）也**不许**猜测归类，只落 `unclassified`。
#[test]
fn unregistered_definition_cannot_be_forced_into_a_category() {
    let catalog = DefinitionCatalog::embedded().expect("内嵌目录必须通过装载期自检");
    let mut def = catalog
        .lookup("channel.feed.actual", 1)
        .expect("内嵌目录必须含 channel.feed.actual")
        .clone();

    // 形状与既有 CNC 变量完全一致，仅 ID 未登记。
    def.variable_id = "channel.feed.unknown".into();
    let a = classify_category(&def);
    assert_eq!(a.category, CatalogCategory::Unclassified);
    assert_eq!(
        a.basis, CATEGORY_BASIS_NAVIGATION_RULE,
        "落 unclassified 时依据仍是 navigation_rule，不得改成 definition"
    );

    // 证据锚点失效（手册来源被移除）时同样不沿用表里分类。
    def.variable_id = "channel.feed.actual".into();
    def.source
        .retain(|s| s.kind != mesa_driver_sinumerik_nck::SourceKind::Manual);
    assert_eq!(
        classify_category(&def).category,
        CatalogCategory::Unclassified,
        "定义自身不再能证明它引 NC 变量清单时，不得仍报 cnc_variable"
    );
}
