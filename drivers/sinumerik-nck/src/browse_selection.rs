//! Browse 选择契约（IPC 1.8）的 **NCK 侧**：稳定诊断码 + 「准入拒绝」→ wire 的桥。
//!
//! 为什么准入判定归驱动：`scope`、定义的 `family`、语义载荷与身份块都只存在于
//! 驱动 / 内嵌定义目录一侧。IPC 1.8 之前，前端只能从 `kind` / `binding_json`
//! 的**形态**反推「这个叶能不能选、为什么不能选」——那套反推规则正是本轮要
//! 消除的耦合（`BrowseNode.selection_mode` + `selection_payload_json` +
//! `diagnostics`）。
//!
//! 分工：
//! - **契约构造器**在 `mesa_driver_protocol::browse_selection`（与厂商无关）；
//! - 本模块只放 NCK 私有码与「`CompileError` → wire 拒绝」的桥。
//!
//! 纪律（三条，缺一即为回归）：
//! - 判定只读**权威数据**（定义目录里的 `scope` / `family` / 身份模板），
//!   不猜、不补默认值、不从变量名或标签推断；
//! - 拒绝一律落在 `NOT_SELECTABLE` + 结构化诊断（稳定码 + 可读文案），
//!   **不得**只把理由拼进 `label`——那正是前端需要"猜"的根源；
//! - 码与文案**复用** `CompileError` 的既有稳定码表：同一事实只有一个码
//!   （例如 scope 不可采集在 Browse 与 Configure 两处都是
//!   `DEFINITION_SCOPE_NOT_COLLECTABLE`）。Core 与通用 UI 只把它当不透明字符串。
use mesa_driver_protocol::browse_selection as wire;

use crate::compiler::CompileError;

/// 语义载荷不可用：内嵌定义目录装载失败、或模板没有对应的定义（`GoldenRef`
/// 查不到）。此时叶仍可浏览，但**不允许**回落到旧 `variable` 原生采集入口。
pub const CODE_SEMANTIC_PAYLOAD_UNAVAILABLE: &str = "SEMANTIC_PAYLOAD_UNAVAILABLE";

/// 身份预览 Key（**仅展示**）：提交前仍必须走服务端 Preview 重新确认。
pub const CODE_IDENTITY_PREVIEW: &str = "IDENTITY_PREVIEW";

/// 身份尚未确定（target/arguments 不完整，预览 Key 为 null）。
/// **不阻断**选用，但必须显式提示（进语义表单补全后才可能预览成功）。
pub const CODE_IDENTITY_UNRESOLVED: &str = "IDENTITY_UNRESOLVED";

/// 准入拒绝 → wire 形态：码与文案都取自 [`CompileError`]。
///
/// 载荷照旧随 `selection_payload_json` 给出：旧客户端按 1.7 的形态消费它，
/// 新客户端只认 `NOT_SELECTABLE` + ERROR 诊断——载荷**不构成**可选许可。
pub fn rejected(payload_json: String, err: &CompileError) -> wire::Selection {
    wire::rejected(payload_json, err.code(), err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_driver_protocol::pb::BrowseSelectionMode;

    /// 拒绝必须同时给出「不可选」与「ERROR 级诊断」，且码表与 Configure 同源。
    #[test]
    fn compile_error_maps_to_not_selectable_with_same_code() {
        let err = CompileError::DefinitionScopeNotCollectable {
            variable_id: "channel.program.status".into(),
            revision: 1,
            scope: "identity",
        };
        let sel = rejected(r#"{"kind":"nck_variable"}"#.into(), &err);
        assert_eq!(sel.mode, BrowseSelectionMode::NotSelectable);
        assert_eq!(sel.payload_json, r#"{"kind":"nck_variable"}"#);
        assert_eq!(sel.diagnostics.len(), 1);
        assert_eq!(sel.diagnostics[0].code, "DEFINITION_SCOPE_NOT_COLLECTABLE");
        assert_eq!(
            sel.diagnostics[0].code,
            err.code(),
            "Browse 诊断码必须与 Configure 拒绝码同源"
        );
        assert_eq!(sel.diagnostics[0].severity, 3, "必须是 ERROR 级");
        assert!(sel.diagnostics[0].message.contains("scope=`identity`"));
    }

    /// family 不一致同样落到同一个桥：码取 `UNSUPPORTED_FAMILY`。
    #[test]
    fn family_mismatch_uses_unsupported_family_code() {
        let err = CompileError::UnsupportedFamily {
            definition_family: "840d-sl".into(),
            device_family: "828d".into(),
        };
        let sel = rejected(String::new(), &err);
        assert_eq!(sel.diagnostics[0].code, "UNSUPPORTED_FAMILY");
    }
}
