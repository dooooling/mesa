//! Data Task 绑定门禁的**唯一实现**。
//!
//! 背景（设计文档 §1.2、P2 护栏）：`gate_data_tasks`（REST 保存门禁）与
//! bootstrap 创建门禁此前各自**复制粘贴**了一份「Task Binding Kind 白名单」，
//! 一旦两份分叉就会出现「保存通过、bootstrap 拒绝」的双真值缺陷。本模块把
//! 「Task Binding Kind 路由 + 绑定解析 + Endpoint-wide point_key 唯一性」收敛为
//! 同一组函数，两个调用点只允许**调用**，不得再各写一份判定。
//!
//! 阶段划分与调用顺序（顺序本身是契约，见 `gate_data_tasks` 的注释）：
//!
//! 1. [`route_data_task_kinds`]：**Descriptor 无关**。按 Task Binding Kind 路由；
//!    `nck.points.v1` 走信封层校验（§2.4）。这一步失败必须先于 Endpoint/Descriptor
//!    查询返回，以保持既有「kind 不合法 ⇒ 400」而非 404/503 的顺序。
//! 2. [`parse_generic_bindings`]：解析 `mesa.resources.v1` 绑定。必须在 Descriptor
//!    取到之后调用，以保持既有「descriptor 不可用 ⇒ 503」优先于
//!    `INVALID_BINDING_CONFIG` 的顺序（`mesa.resources.v1` 行为逐字不变）。
//! 3. [`cross_kind_point_key_issues`]：Endpoint-wide `point_key` 唯一性（跨 Task、
//!    **跨 kind**）。同 kind 内部（`mesa.resources.v1`）的重复仍由
//!    `validate_task_set_against` 负责，本函数只补「nck 侧内部 + 跨 kind」的缺口。

use mesa_core_types::{
    AcquisitionTask, GENERIC_BINDING_KIND, GenericBinding, NCK_POINTS_BINDING_KIND, PointBinding,
    ValidationIssue, ensure_endpoint_point_keys_unique, parse_point_binding,
};

/// 允许的 Data Task **Task Binding Kind** 集合（路由唯一真值，§2.1-1）。
///
/// 注意这是**任务层** kind；点位层的 **Point Kind**（`PointSelection.kind`）**不在**
/// 此处枚举——Core 不建立 Point Kind 白名单（修订 1）。
pub(crate) const DATA_BINDING_KINDS: &[&str] = &[GENERIC_BINDING_KIND, NCK_POINTS_BINDING_KIND];

/// 允许的 kind 列表（用于错误消息，保证两处调用点措辞一致）。
pub(crate) fn allowed_kinds_text() -> String {
    DATA_BINDING_KINDS.join(" 或 ")
}

fn issue(path: String, code: &str, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue {
        path,
        code: code.into(),
        message: message.into(),
    }
}

/// 阶段 1：Task Binding Kind 路由 + `nck.points.v1` 信封层校验（Descriptor 无关）。
///
/// 返回 `(issues, nck_bindings)`。`issues` 非空即应直接 400；否则 `nck_bindings`
/// 为按 Task 下标解析出的点位信封（供阶段 3 做跨 kind 唯一性判定）。
///
/// `root` 为任务数组的路径前缀：`tasks`（REST 保存）或 `acquisition.tasks`（bootstrap）。
pub(crate) fn route_data_task_kinds(
    tasks: &[AcquisitionTask],
    root: &str,
) -> (Vec<ValidationIssue>, Vec<(usize, PointBinding)>) {
    let mut issues = Vec::new();
    let mut nck = Vec::new();
    for (i, task) in tasks.iter().enumerate() {
        let kind = task.binding.kind.as_str();
        if !DATA_BINDING_KINDS.contains(&kind) {
            issues.push(issue(
                format!("{root}[{i}].binding.kind"),
                "UNSUPPORTED_BINDING",
                format!(
                    "binding kind `{kind}` 不再支持，仅允许 {}",
                    allowed_kinds_text()
                ),
            ));
            continue;
        }
        if kind == NCK_POINTS_BINDING_KIND {
            let envelope_root = format!("{root}[{i}].binding.config");
            let (point_issues, parsed) = parse_point_binding(&task.binding.config, &envelope_root);
            issues.extend(point_issues);
            if let Some(binding) = parsed {
                nck.push((i, binding));
            }
        }
    }
    (issues, nck)
}

/// 阶段 2：解析 `mesa.resources.v1` 绑定（`config` → [`GenericBinding`]）。
///
/// 返回 `(issues, parsed)`；`issues` 非空即应直接 400。
pub(crate) fn parse_generic_bindings(
    tasks: &[AcquisitionTask],
    root: &str,
) -> (Vec<ValidationIssue>, Vec<(usize, GenericBinding)>) {
    let mut issues = Vec::new();
    let mut parsed = Vec::new();
    for (i, task) in tasks.iter().enumerate() {
        if task.binding.kind != GENERIC_BINDING_KIND {
            continue;
        }
        match GenericBinding::from_json(&task.binding.config) {
            Ok(binding) => parsed.push((i, binding)),
            Err(e) => issues.push(issue(
                format!("{root}[{i}].binding"),
                "INVALID_BINDING_CONFIG",
                e,
            )),
        }
    }
    (issues, parsed)
}

/// 阶段 3：跨 Task、**跨 Task Binding Kind** 的 Endpoint-wide `point_key` 唯一性。
///
/// `mesa.resources.v1` 已占用的 Key 作为 `occupied` 传入，`nck.points.v1` 的 Key
/// 逐个判重。`mesa.resources.v1` **内部**的重复不在此处报告（由
/// `validate_task_set_against` 负责），以免同一缺陷被报两次。
pub(crate) fn cross_kind_point_key_issues(
    resources: &[(usize, GenericBinding)],
    nck: &[(usize, PointBinding)],
    root: &str,
) -> Vec<ValidationIssue> {
    if nck.is_empty() {
        return Vec::new();
    }
    // 已占用：resources 的 selections[i].outputs[j].point_key。
    let occupied: Vec<(String, String)> = resources
        .iter()
        .flat_map(|(ti, b)| {
            b.selections.iter().enumerate().flat_map(move |(si, sel)| {
                let base = format!("{root}[{ti}].selections[{si}].outputs");
                sel.outputs
                    .iter()
                    .map(move |out| (out.point_key.clone(), base.clone()))
            })
        })
        .collect();
    let occupied_ref: Vec<(&str, &str)> = occupied
        .iter()
        .map(|(k, p)| (k.as_str(), p.as_str()))
        .collect();

    let entries: Vec<(String, String)> = nck
        .iter()
        .flat_map(|(ti, b)| {
            let root = root.to_string();
            b.points.iter().enumerate().map(move |(pi, p)| {
                (
                    p.point_key.clone(),
                    format!("{root}[{ti}].binding.config.points[{pi}]"),
                )
            })
        })
        .collect();
    let entries_ref: Vec<(&str, &str)> = entries
        .iter()
        .map(|(k, p)| (k.as_str(), p.as_str()))
        .collect();

    ensure_endpoint_point_keys_unique(&entries_ref, &occupied_ref)
}
