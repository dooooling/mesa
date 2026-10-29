//! sidecar 元数据（`definitions/828d/r1.meta.json`）的驱动侧验收。
//!
//! ## 这一层在验什么
//!
//! 数据本身（单位/页码/可用性判定）由 `tools/ci/check_definition_meta.py`（M1–M8）
//! 机械校验；本文件只验**驱动侧接线**的四件事：
//!
//! 1. 42 条定义投影后**都**带上 `unit` / `manual_location` / `index_rule` /
//!    `availability` 四块，且内容与 sidecar **逐字一致**（不是重新拼装出来的）；
//! 2. sidecar 的 `binds_to` 与 `r1.json` 的 git blob **摘要一致**（含 Windows
//!    CRLF 工作树的归一），且内嵌副本与磁盘文件逐字一致；
//! 3. sidecar 摘要不符 / 形状非法 / 与目录条目对不上 ⇒ **装载即失败**
//!    （fail-closed：整份目录不可用，绝不「少返回几个字段」）；
//! 4. `availability.state` 只允许四态，`basis` 非空。
//!
//! ## 只读性
//!
//! 全部用例不建连接、不连机床、不启动 mesad：目录投影是纯计算路径。

use mesa_driver_sdk::Driver;
use mesa_driver_sinumerik_nck::{
    AVAILABILITY_STATES, DefinitionCatalog, DefinitionMeta, EMBEDDED_DEFINITIONS_828D_R1,
    EMBEDDED_DEFINITIONS_828D_R1_META, META_BLOCK_KEYS, META_DEFINITION_FILE, META_FORMAT_VERSION,
    SinumerikNckDriver, git_blob_sha1,
};
use serde_json::{Value, json};

/// `r1.json` 的冻结 git blob：本变更内一字节不许动。
const R1_FROZEN_BLOB: &str = "3c0abe67cbcb64b525cddec1584e6e1747e0f062";

fn sidecar() -> Value {
    serde_json::from_str(EMBEDDED_DEFINITIONS_828D_R1_META).expect("内嵌 sidecar 必须是合法 JSON")
}

fn repo_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn sidecar_by_id(doc: &Value) -> std::collections::BTreeMap<String, Value> {
    doc["entries"]
        .as_array()
        .expect("sidecar 必须含 entries 数组")
        .iter()
        .map(|e| {
            (
                e["variable_id"].as_str().expect("variable_id").to_string(),
                e.clone(),
            )
        })
        .collect()
}

fn catalog() -> DefinitionCatalog {
    DefinitionCatalog::embedded().expect("内嵌定义目录必须通过装载期自检")
}

/// 42 条定义的目录投影（一次取满一页）。
async fn projected() -> Vec<Value> {
    let body = SinumerikNckDriver
        .definitions(&json!({ "limit": 500 }).to_string())
        .await
        .expect("目录查询应成功");
    let doc: Value = serde_json::from_str(&body).expect("目录响应必须是合法 JSON");
    doc["definitions"]
        .as_array()
        .expect("definitions 必须是数组")
        .clone()
}

// ---------------------------------------------------------------------------
// 1. 绑定摘要
// ---------------------------------------------------------------------------

/// sidecar 必须绑定到**当前**这份 `r1.json`，且内嵌副本与磁盘文件逐字一致。
#[test]
fn sidecar_binds_to_the_frozen_r1_blob() {
    let doc = sidecar();
    // 纯 Rust 侧算出的 blob 必须等于 git 的 blob id（CRLF 已归一）。
    let measured = git_blob_sha1(EMBEDDED_DEFINITIONS_828D_R1);
    assert_eq!(
        measured, R1_FROZEN_BLOB,
        "内嵌 r1.json 的 git blob 必须是冻结值"
    );
    assert_eq!(doc["format_version"], json!(META_FORMAT_VERSION));
    assert_eq!(
        doc["binds_to"]["definition_file"],
        json!(META_DEFINITION_FILE),
        "sidecar 只允许绑定同目录的 r1.json"
    );
    assert_eq!(
        doc["binds_to"]["definition_blob_sha1"],
        json!(R1_FROZEN_BLOB),
        "sidecar 声明的绑定摘要必须是 r1.json 的冻结 blob"
    );
    // 内嵌副本 vs 磁盘文件：`include_str!` 走的是编译期快照，两者必须一致，
    // 否则「改文件不重编」会让 CI 与本地看到不同的数据。
    let on_disk = std::fs::read_to_string(repo_path(
        "drivers/sinumerik-nck/definitions/828d/r1.meta.json",
    ))
    .expect("必须能读到 r1.meta.json");
    assert_eq!(
        on_disk.replace("\r\n", "\n"),
        EMBEDDED_DEFINITIONS_828D_R1_META.replace("\r\n", "\n"),
        "内嵌 sidecar 必须与磁盘文件逐字一致"
    );
}

/// CRLF/LF 归一：同一份逻辑数据在两个平台必须算出同一个 blob。
#[test]
fn git_blob_sha1_normalizes_line_endings() {
    assert_eq!(git_blob_sha1("a\r\nb\r\n"), git_blob_sha1("a\nb\n"));
    assert_ne!(git_blob_sha1("a\nb"), git_blob_sha1("a\nc"));
}

// ---------------------------------------------------------------------------
// 2. 装载 + 双向对齐
// ---------------------------------------------------------------------------

/// 内嵌 sidecar 可装载，且与内嵌目录**双向**对齐（42 ↔ 42）。
#[test]
fn embedded_sidecar_covers_the_whole_catalog() {
    let meta = DefinitionMeta::embedded().expect("内嵌 sidecar 必须可装载");
    meta.verify_catalog(&catalog())
        .expect("sidecar 与定义目录必须双向对齐");
    let doc = sidecar();
    assert_eq!(
        doc["entries"].as_array().expect("entries").len(),
        42,
        "sidecar 必须覆盖 42 条定义"
    );
}

// ---------------------------------------------------------------------------
// 3. 投影：四块元数据逐条到位且逐字一致
// ---------------------------------------------------------------------------

/// 42 条定义**逐条**都带上四块元数据，内容与 sidecar 逐字一致。
#[tokio::test]
async fn every_projected_definition_carries_the_four_meta_blocks() {
    let doc = sidecar();
    let by_id = sidecar_by_id(&doc);
    let rows = projected().await;
    assert_eq!(rows.len(), 42, "投影必须是 42 条");

    let mut ready = 0;
    for row in &rows {
        let id = row["variable_id"].as_str().expect("variable_id");
        let src = by_id
            .get(id)
            .unwrap_or_else(|| panic!("{id} 不在 sidecar 中"));
        for key in META_BLOCK_KEYS {
            assert!(
                row.get(key).is_some(),
                "{id} 缺少元数据块 `{key}`（sidecar 缺失即整份目录不可用，不应出现半截投影）"
            );
            assert_eq!(row[key], src[key], "{id} 的 `{key}` 必须逐字取自 sidecar");
        }
        let state = row["availability"]["state"].as_str().expect("state");
        assert!(
            AVAILABILITY_STATES.contains(&state),
            "{id} 的 availability.state=`{state}` 不在 {AVAILABILITY_STATES:?} 内"
        );
        let basis = row["availability"]["basis"]
            .as_array()
            .unwrap_or_else(|| panic!("{id} 的 basis 必须是数组"));
        assert!(!basis.is_empty(), "{id} 的 basis 不能为空");
        if state == "ready" {
            ready += 1;
            assert!(
                row.get("device_read_evidence").is_some(),
                "{id} 判 ready 必须带 device_read_evidence"
            );
        } else {
            // 非 ready 条目**不得**出现证据键：让「没有证据」与「证据为空」不可混淆。
            assert!(
                row.get("device_read_evidence").is_none(),
                "{id} 不是 ready，却带了 device_read_evidence"
            );
        }
    }
    // 当前 42 条**没有**任何一条满足 ready（唯一候选 drive.object.name 缺
    // environment.cnc_software，见 docs/evidence 与校验器 M7）。
    assert_eq!(ready, 0, "当前数据不应有 ready 条目");
}

/// 既有的导航字段语义不受元数据合并影响（`category_basis` 仍恒为导航规则）。
#[tokio::test]
async fn category_basis_stays_navigation_rule() {
    for row in projected().await {
        assert_eq!(
            row["category_basis"], "navigation_rule",
            "合并元数据不得改变 category_basis 的取值"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. fail-closed：任何一条不成立都让整份目录不可用
// ---------------------------------------------------------------------------

/// 摘要不符 ⇒ 装载失败（这是 sidecar 与定义之间唯一的防漂移锚点）。
#[test]
fn sidecar_digest_mismatch_fails_closed() {
    let mut doc = sidecar();
    let real = doc["binds_to"]["definition_blob_sha1"]
        .as_str()
        .expect("blob")
        .to_string();
    let tampered = format!("0{}", &real[1..]);
    doc["binds_to"]["definition_blob_sha1"] = json!(tampered);
    let err = DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1)
        .expect_err("摘要不符必须装载失败");
    let msg = err.to_string();
    assert!(
        msg.contains("definition_blob_sha1") && msg.contains(&real),
        "错误消息必须点明摘要不符：{msg}"
    );
}

/// 形状非法 ⇒ 装载失败（format_version / 绑定文件名 / state / basis）。
#[test]
fn sidecar_shape_violations_fail_closed() {
    let base = sidecar();

    let mut doc = base.clone();
    doc["format_version"] = json!(2);
    assert!(
        DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1).is_err(),
        "未知 format_version 必须拒绝"
    );

    let mut doc = base.clone();
    doc["binds_to"]["definition_file"] = json!("r2.json");
    assert!(
        DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1).is_err(),
        "绑定到非 r1.json 必须拒绝"
    );

    let mut doc = base.clone();
    doc["entries"][0]["availability"]["state"] = json!("verified");
    assert!(
        DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1).is_err(),
        "四态之外的 state 必须拒绝"
    );

    let mut doc = base.clone();
    doc["entries"][0]["availability"]["basis"] = json!([]);
    assert!(
        DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1).is_err(),
        "空 basis 必须拒绝"
    );

    let mut doc = base.clone();
    doc["entries"][0]["unit"] = json!("mm");
    assert!(
        DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1).is_err(),
        "unit 不是对象必须拒绝"
    );
}

/// 少一条 / 多一条 ⇒ 装载能过，但与目录对齐失败（双向检查）。
#[test]
fn sidecar_coverage_mismatch_fails_closed() {
    let base = sidecar();

    // 少一条：42 → 41
    let mut doc = base.clone();
    doc["entries"].as_array_mut().expect("entries").pop();
    let meta = DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1)
        .expect("少一条本身不违反装载期形状规则");
    assert!(
        meta.verify_catalog(&catalog()).is_err(),
        "定义没有对应元数据条目时必须整体失败"
    );

    // 多一条孤儿：把某条复制成另一个不存在的 variable_id
    let mut doc = base.clone();
    let mut orphan = doc["entries"][0].clone();
    orphan["variable_id"] = json!("ghost.variable");
    doc["entries"].as_array_mut().expect("entries").push(orphan);
    let meta = DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1)
        .expect("孤儿条目本身不违反装载期形状规则");
    assert!(
        meta.verify_catalog(&catalog()).is_err(),
        "sidecar 含无定义对应的孤儿条目时必须整体失败"
    );

    // 重复条目：同 (variable_id, revision) 出现两次
    let mut doc = base.clone();
    let dup = doc["entries"][0].clone();
    doc["entries"].as_array_mut().expect("entries").push(dup);
    assert!(
        DefinitionMeta::load(&doc.to_string(), EMBEDDED_DEFINITIONS_828D_R1).is_err(),
        "重复条目必须装载失败"
    );
}
