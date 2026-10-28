//! `axis.encoder.module_number` 的测量声明撤回状态守卫。
//!
//! ## 为什么需要这个测试
//!
//! 审计已判定该定义的 `measurement` 声明**不成立**：
//! 它指向 `docs/evidence/nck-live-identity-probe.txt`，而该文件（128 行 / 13 小节）
//! 没有编码器驱动号一节；同时常量 `AXIS_ENC_MODULE_NR` 从未进入执行路径
//! （`identity.rs` 的 use 清单不含它，轴读取序列只有 5 次读）。
//!
//! 但**审计判定不等于定义已撤回**。定义目录的 `(variable_id, revision)`
//! 不可原地修改（`definition.rs` 模块文档 §3.1 硬约束），且当前**不存在撤回机制**：
//! `DIGESTS.lock` 必须与定义 1:1（`MissingLockEntry` / `UnexpectedLockEntry`），
//! 也没有 supersede / deprecate 语义。要移除这条声明，必须发布新 revision 或走
//! 经评审的定义修订。
//!
//! 在那种修订落地之前，生效定义**仍然携带**这条不成立的声明。本测试把这一事实
//! 固定成可执行断言，目的有三个：
//!
//! 1. **不允许它被遗忘**：任何人读这张测试表都会看到生产定义里有一条无证据的声明。
//! 2. **不允许被误读成已修复**：审计记录里写「撤回待定」时，这里断言的是相反的事实。
//! 3. **修复时强制同步**：一旦有人真切地删掉了该声明，本测试**必然失败**，
//!    从而强制同时更新审计记录与证据矩阵里的 `unresolved` 项，而不是让两边脱节。
//!
//! 注意：本测试**不是**在认可这条声明。它断言的是「脏状态仍然存在」，
//! 属于「正向断言已知缺陷」的守卫写法（同 `block_wire_golden.rs` 的
//! `unobserved_blocks_must_not_get_golden_bytes`）。

use serde_json::Value;

const DEFINITION_JSON: &str = include_str!("../definitions/828d/r1.json");

/// 该测量声明指向的证据文件——审计已确认其中不含编码器驱动号一节。
const CITED_EVIDENCE_FILE: &str = "docs/evidence/nck-live-identity-probe.txt";

fn find_definition<'a>(defs: &'a [Value], variable_id: &str) -> &'a Value {
    defs.iter()
        .find(|d| d.get("variable_id").and_then(Value::as_str) == Some(variable_id))
        .unwrap_or_else(|| panic!("定义目录中找不到 `{variable_id}`"))
}

/// 生效定义仍然携带无证据的测量声明——审计「撤回待定」期间的事实守卫。
#[test]
fn effective_definition_still_carries_the_unsupported_measurement_claim() {
    let catalog: Value = serde_json::from_str(DEFINITION_JSON).expect("r1.json 必须是合法 JSON");
    let defs = catalog["definitions"]
        .as_array()
        .expect("r1.json 顶层必须有 definitions 数组");
    let def = find_definition(defs, "axis.encoder.module_number");

    // ① 仍是已发布 rev1：说明没有通过「发布新 revision」的方式撤回。
    assert_eq!(
        def.get("revision").and_then(Value::as_u64),
        Some(1),
        "该定义应仍为 revision=1。若已升版，说明定义修订已经发生，\
         必须同步更新审计记录与证据矩阵的 unresolved 项，并改写本测试"
    );

    // ② 语义状态仍是 verified：这正是「目录会把它当作已确认」的直接原因。
    assert_eq!(
        def.get("semantics_status").and_then(Value::as_str),
        Some("verified"),
        "该定义应仍为 semantics_status=verified。若已降级，说明修订已发生，\
         必须同步更新审计记录并改写本测试"
    );

    // ③ 无证据的 measurement 条目仍在 source[] 里。
    let sources = def
        .get("source")
        .and_then(Value::as_array)
        .expect("该定义必须有 source 数组");
    let offending: Vec<&Value> = sources
        .iter()
        .filter(|s| {
            s.get("kind").and_then(Value::as_str) == Some("measurement")
                && s.get("reference").and_then(Value::as_str) == Some(CITED_EVIDENCE_FILE)
        })
        .collect();

    assert_eq!(
        offending.len(),
        1,
        "该定义应仍带 1 条指向 `{CITED_EVIDENCE_FILE}` 的 measurement 声明。\
         若已移除，说明撤回已落地——请同时更新审计记录（撤回状态）、\
         证据矩阵 unresolved 项，并改写本测试"
    );
}

/// 撤回状态必须仍被登记为「待定」，而不是「已完成」。
///
/// 这条断言防止一种具体的自欺：审计文档里写成「已撤销」，
/// 而生产定义原封不动。只要本测试还在，就不能这么写。
#[test]
fn retraction_is_recorded_as_pending_not_done() {
    let catalog: Value = serde_json::from_str(DEFINITION_JSON).expect("r1.json 必须是合法 JSON");
    let defs = catalog["definitions"]
        .as_array()
        .expect("r1.json 顶层必须有 definitions 数组");
    let def = find_definition(defs, "axis.encoder.module_number");

    // 只要上面那条「仍携带声明」的三个条件成立，撤回就是「待定」而非「完成」。
    let still_rev1 = def.get("revision").and_then(Value::as_u64) == Some(1);
    let still_verified = def.get("semantics_status").and_then(Value::as_str) == Some("verified");
    let still_carries = def
        .get("source")
        .and_then(Value::as_array)
        .map(|sources| {
            sources.iter().any(|s| {
                s.get("kind").and_then(Value::as_str) == Some("measurement")
                    && s.get("reference").and_then(Value::as_str) == Some(CITED_EVIDENCE_FILE)
            })
        })
        .unwrap_or(false);

    assert!(
        still_rev1 && still_verified && still_carries,
        "撤回状态判定与实际定义不一致：rev1={still_rev1} verified={still_verified} \
         仍携带声明={still_carries}。若定义已被修订，撤回即为「已完成」，\
         必须把审计记录与证据矩阵改为完成状态"
    );
}
