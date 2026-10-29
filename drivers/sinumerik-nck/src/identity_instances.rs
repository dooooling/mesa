//! 通道 / 主轴**实例探测树**（`nck://identity/channels` / `nck://identity/spindles`）。
//!
//! 为什么与 `identity.rs`（轴与驱动对象）分成两个文件：那两棵树回答"这台机床有
//! 哪些轴 / 驱动对象"，本模块回答"有哪些通道 / 主轴"。它们的读法、成节点条件与
//! 异常处理**都不一样**（轴/对象按名称与输出类型核对；通道按 `AXCONF_MACHAX_USED`
//! 逐通道读；主轴只能从逐轴的主轴分配号派生），混写会掩盖判据。
//!
//! 三条不可妥协的判据：
//!
//! 1. **不凭"请求成功"认定对象存在**。四类结果必须分开：
//!    - 有效实例（读通且数据有效）⇒ 成节点，`selection_mode=SEMANTIC`；
//!    - 无效实例（设备明确拒绝，`S7_JOB_*`/`S7_ITEM_*`）⇒ **不成节点**
//!      （拒绝就是"这台机床上没有这个实例"）；
//!    - 通信/协议失败 ⇒ 不成节点，但**必须留诊断**（`IDENTITY_INSTANCE_READ_FAILED`）：
//!      "读不到"与"不存在"混为一谈，就会把一次断线显示成"没有通道"；
//!    - 无法判定（值解释不出结论、载荷构造不出）⇒ 不成节点 + `IDENTITY_PENDING_REVIEW`。
//!
//! 2. **主轴只能由真实读数派生**：绝不按"预设主轴编号 1..n"凭空生成节点，
//!    只认 `SPIND_ASSIGN_TO_MACHAX` 的非零分配号；多轴指向同一主轴时**去重**为
//!    一个节点，并把来源轴记在标签里（用户要能看出"这个主轴号是谁给的"）。
//!
//! 3. **枚举中断必须可见**：主轴集合是**跨轴派生**的，某个轴读失败会让集合
//!    可能不完整，此时必须给诊断，而不是安静地少列一个主轴。
//!
//! 载荷（`selection_payload_json`）由 [`crate::presets::instance_coordinate_payload`]
//! 从**真实定义目录**构造：身份键与模板都来自"需要该坐标"的定义，绝不手写拼接。

use std::collections::BTreeMap;

use mesa_driver_protocol::browse_selection as wire;
use mesa_driver_protocol::pb::{BrowseDiagnosticSeverity, BrowseNode};

use crate::browse_selection::{
    CODE_INSTANCE_READ_FAILED, CODE_PENDING_REVIEW, CODE_SPINDLE_ASSIGN_INVALID,
};
use crate::compiler::POINT_KIND_VARIABLE;
use crate::identity::{
    IDENTITY_PAGE_MAX, MAX_INSTANCE, Page, as_u32, device_rejected, instance_cursor, limit_size,
    local_page, matches, note, read,
};
use crate::identity_spec::{AXIS_SPIND_ASSIGN, CHANNEL_MACHAX_USED, IdentityProbeSpec, ProbeSlot};
use crate::presets::instance_coordinate_payload;
use crate::targets::TargetKind;
use crate::{NckClient, SdkDriverError};

/// 通道树根（根节点下的一个分支）。
pub const CHANNELS: &str = "nck://identity/channels";
/// 主轴树根（根节点下的一个分支）。
pub const SPINDLES: &str = "nck://identity/spindles";

/// 通道枚举探针：沿用 [`CHANNEL_MACHAX_USED`] 的 wire 常量（C 区、module 26、
/// column 20070、U8、transport `0x09`），只把**实例槽位**从 `line` 换成 `area_no`。
///
/// 为什么必须换槽位：C 区的 `area_no` 就是**通道号**（定义目录里 `channel.*` 的
/// `addressing.area_no.source = target/channel` 是同一事实），而 `line` 是通道内的
/// **位置**。现有常量把 `TargetIndex` 放在 `line`，是按"通道 1 的位置 1..n"设计的
/// （真机证据 `docs/evidence/nck-live-identity-probe.txt` 只读了通道 1 的位置 1..8）。
/// 若直接沿用它去"枚举 31 个通道"，实际会重复读通道 1 的 31 个位置——每个位置都
/// 读得通，于是把"位置存在"当成"通道存在"，凭空生成不存在的通道节点。
///
/// 换槽位后 `channel=1` 的请求字节与既有常量**逐字节相同**（`area_unit = 0x41`、
/// `line = 1`），因此不推翻任何已核对证据，只是把 `channel > 1` 的地址指向真正的
/// 通道 N。
///
/// NOTE(待真机确认): 通道 2..31 的 `AXCONF_MACHAX_USED` 读数尚无真机记录
/// （现有证据只覆盖通道 1 的位置 1..8）。"通道 N 的位置 1 是否映射到有效机床轴"
/// 这条成节点判据因此需要真机复核；在复核之前它只做**保守判定**：
/// 读到 0（位置 1 未映射任何机床轴）时不成节点、只给 `IDENTITY_PENDING_REVIEW`。
fn channel_probe() -> IdentityProbeSpec {
    IdentityProbeSpec {
        area_no: ProbeSlot::TargetIndex,
        line: ProbeSlot::Fixed(1),
        ..CHANNEL_MACHAX_USED
    }
}

/// 通道节点：**可选语义叶**，载荷只给 `channel` 坐标（由真实定义目录构造）。
///
/// 载荷构造不出来时返回 `None`：调用方必须转成 `IDENTITY_PENDING_REVIEW` 诊断，
/// 绝不能给一个空载荷冒充"可选"。
fn channel_node(family: &str, number: u16, machine_axis: u32) -> Option<BrowseNode> {
    let payload = instance_coordinate_payload(family, TargetKind::Channel, u32::from(number))?;
    let selection = wire::semantic(payload.clone(), Vec::new());
    Some(BrowseNode {
        id: format!("{CHANNELS}/{number}"),
        // 标签写清"凭什么认为这个通道存在"：位置 1 映射到的机床轴号来自本次读数。
        label: format!("通道{number} · 位置1→机床轴{machine_axis}"),
        kind: POINT_KIND_VARIABLE.into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: payload.clone(),
        selection_mode: selection.mode as i32,
        selection_payload_json: payload,
        diagnostics: selection.diagnostics,
    })
}

/// 通道页：按通道号 `1..=MAX_INSTANCE` 有界枚举，逐通道读 `AXCONF_MACHAX_USED[1]`。
///
/// 分页语义与轴/对象树一致：游标是**下一个通道号**，每页实例数受
/// [`IDENTITY_PAGE_MAX`] 与调用方 `limit` 双重收敛，因此单次请求的读取量可观测、
/// 有上界（不会一次扫完 31 个通道）。
pub(crate) async fn channels_page(
    client: &mut NckClient,
    family: &str,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> Result<Page, SdkDriverError> {
    let mut index = instance_cursor(cursor)?;
    let page = limit_size(limit)
        .min(IDENTITY_PAGE_MAX)
        .min(usize::from(MAX_INSTANCE));
    let mut nodes = Vec::new();
    for _ in 0..page {
        match read(client, &channel_probe(), index).await {
            Ok(value) => match as_u32(value) {
                // 位置 1 未映射任何机床轴：通道可能存在，但"能用"这件事没有证据。
                Ok(0) => nodes.push(note(
                    format!("{CHANNELS}/note/{index}"),
                    CODE_PENDING_REVIEW,
                    format!(
                        "通道{index}读到了数据但位置1未映射机床轴（值为0）：无法判定该通道是否可用，需人工复核"
                    ),
                    BrowseDiagnosticSeverity::Warning,
                )),
                Ok(machine_axis) => match channel_node(family, index, machine_axis) {
                    Some(node) => {
                        if matches(&node, filter) {
                            nodes.push(node);
                        }
                    }
                    // 载荷构造失败（定义目录不可用，或没有任何定义把 `channel`
                    // 当必填身份键）：坐标事实仍成立，但"能不能选"没有依据，
                    // 因此不给节点、只给诊断（绝不发空载荷冒充可选）。
                    None => nodes.push(note(
                        format!("{CHANNELS}/note/{index}"),
                        CODE_PENDING_REVIEW,
                        format!(
                            "通道{index}读数有效（位置1→机床轴{machine_axis}），但定义目录没有只依赖 channel 的定义：载荷无法构造，需人工复核"
                        ),
                        BrowseDiagnosticSeverity::Warning,
                    )),
                },
                // 读通了但类型/形状不对（不是 U32 语义）：同样属于"无法判定"。
                Err(error) => nodes.push(note(
                    format!("{CHANNELS}/note/{index}"),
                    CODE_PENDING_REVIEW,
                    format!("通道{index}读数形状异常：{}", error.message),
                    BrowseDiagnosticSeverity::Warning,
                )),
            },
            // 设备明确拒绝 = 该通道不存在。不成节点，也不给诊断：
            // 一台只有 1 个通道的机床会被拒绝 30 次，逐条报错只会淹没真正的失败。
            Err(error) if device_rejected(&error) => {}
            // 通信/协议失败（连接断开、响应 transport/长度不符、解码失败）：
            // **不是**"不存在"，必须留诊断。这里终止本页而不是继续：本页后续实例的
            // 读取可靠性已无从保证，而"通道列表可能不完整"这句话必须留给用户看见
            // （通道是逐实例独立判定，中断不影响已经读到的结论，因此已读节点照常返回）。
            Err(error) => {
                nodes.push(note(
                    format!("{CHANNELS}/note/{index}"),
                    CODE_INSTANCE_READ_FAILED,
                    format!(
                        "通道{index}读取失败（{}）：后续通道未枚举，通道列表可能不完整",
                        error.code
                    ),
                    BrowseDiagnosticSeverity::Warning,
                ));
                return Ok((nodes, None));
            }
        }
        if index == MAX_INSTANCE {
            return Ok((nodes, None));
        }
        index += 1;
    }
    Ok((nodes, Some(index.to_string())))
}

/// 主轴节点：**可选语义叶**，载荷只给 `spindle` 坐标；来源轴写进标签。
///
/// `axes` 是该主轴号的全部来源轴（升序）：多轴指向同一主轴时**去重成一个节点**，
/// 但来源必须可见——否则用户无法判断"主轴1 到底是哪根轴"。
fn spindle_node(family: &str, number: u32, axes: &[u16]) -> Option<BrowseNode> {
    let payload = instance_coordinate_payload(family, TargetKind::Spindle, number)?;
    let selection = wire::semantic(payload.clone(), Vec::new());
    let sources = axes
        .iter()
        .map(|axis| format!("轴{axis}"))
        .collect::<Vec<_>>()
        .join("/");
    Some(BrowseNode {
        id: format!("{SPINDLES}/{number}"),
        label: format!("主轴{number} · 来源{sources}"),
        kind: POINT_KIND_VARIABLE.into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: payload.clone(),
        selection_mode: selection.mode as i32,
        selection_payload_json: payload,
        diagnostics: selection.diagnostics,
    })
}

/// 主轴页：逐轴读 `SPIND_ASSIGN_TO_MACHAX`，取**分配号非零**者派生主轴。
///
/// 为什么不按轴号分页：主轴集合是**跨轴**派生的（同一主轴可由多个轴指向、
/// 也可能由编号靠后的轴给出），只读一个轴区间得不到完整集合。因此单次请求内做
/// 一次**有界**扫描（≤ [`MAX_INSTANCE`] 次读，与轴树一页的读取量同量级），
/// 再对结果本地分页（游标是结果偏移，不是轴号——两者的语义不同，不能混用）。
pub(crate) async fn spindles_page(
    client: &mut NckClient,
    family: &str,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> Result<Page, SdkDriverError> {
    // 主轴号 → 来源轴（升序）。BTreeMap 保证节点顺序稳定（主轴号升序）。
    let mut spindles: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    let mut notes: Vec<BrowseNode> = Vec::new();
    // 最近一次**读到数据**的轴号：用于判断失败轴是否落在有效区间内（见下）。
    let mut last_read: Option<u16> = None;
    let mut failed: Vec<(u16, String)> = Vec::new();

    for index in 1..=MAX_INSTANCE {
        match read(client, &AXIS_SPIND_ASSIGN, index).await {
            Ok(value) => match as_u32(value) {
                Ok(0) => last_read = Some(index),
                Ok(number) if (1..=u32::from(MAX_INSTANCE)).contains(&number) => {
                    spindles.entry(number).or_default().push(index);
                    last_read = Some(index);
                }
                // 越界分配号：既不能当主轴（地址会指向不存在的实例），也不能当
                // "该轴不是主轴"（值不是 0），只能如实标诊断。
                Ok(number) => {
                    last_read = Some(index);
                    notes.push(note(
                        format!("{SPINDLES}/note/axis-{index}"),
                        CODE_SPINDLE_ASSIGN_INVALID,
                        format!(
                            "轴{index}的主轴分配号为{number}，超出 1..={MAX_INSTANCE} 的合法范围：该轴不参与主轴派生"
                        ),
                        BrowseDiagnosticSeverity::Warning,
                    ));
                }
                Err(error) => failed.push((index, error.message)),
            },
            // 设备明确拒绝 = 该轴不存在，不参与派生（与通道的"无效实例"同口径）。
            Err(error) if device_rejected(&error) => {}
            Err(error) => {
                failed.push((index, format!("{}：{}", error.code, error.message)));
                // 连接/超时级失败：会话已不可用，继续读只会重复失败，立即停止扫描。
                // 单项协议/解码异常（响应 transport/长度不符、解码失败）不影响会话，
                // 因此**继续**读其余轴——主轴集合是跨轴汇总的，少读一个轴就可能少
                // 一个主轴；缺口由下面的诊断如实说明。
                if matches!(
                    error.kind,
                    mesa_core_types::ErrorKind::Connection | mesa_core_types::ErrorKind::Timeout
                ) {
                    break;
                }
            }
        }
    }

    // 失败轴只要落在"已读到数据的轴号区间内"，就说明枚举中断过，主轴集合可能不完整；
    // 落在表尾之后的拒绝（例如 8 轴机床的轴 9..31）属正常，不制造噪音。
    // 一个轴都没读到时（`last_read` 为 None），所有失败都算缺口。
    let gaps: Vec<u16> = failed
        .iter()
        .map(|(axis, _)| *axis)
        .filter(|axis| match last_read {
            Some(last) => *axis < last,
            None => true,
        })
        .collect();
    if !gaps.is_empty() {
        let detail = failed
            .iter()
            .filter(|(axis, _)| gaps.contains(axis))
            .map(|(axis, reason)| format!("轴{axis}（{reason}）"))
            .collect::<Vec<_>>()
            .join("；");
        notes.push(note(
            format!("{SPINDLES}/note/incomplete"),
            CODE_INSTANCE_READ_FAILED,
            format!("{detail} 读取失败：主轴集合可能不完整，需人工复核"),
            BrowseDiagnosticSeverity::Warning,
        ));
    }

    // 节点顺序：诊断在前 + 主轴节点（升序）。诊断前置是为了让它们落在**第一页**：
    // 本地分页按 `limit` 切分，若把诊断排在末尾，用户翻不到那页就永远看不到
    // "集合可能不完整"这条结论。
    let mut nodes = notes;
    for (number, axes) in &spindles {
        match spindle_node(family, *number, axes) {
            Some(node) => {
                if matches(&node, filter) {
                    nodes.push(node);
                }
            }
            None => nodes.push(note(
                format!("{SPINDLES}/note/{number}"),
                CODE_PENDING_REVIEW,
                format!(
                    "主轴{number}读数有效（来源{}），但定义目录里没有任何定义把 spindle 当必填身份键：载荷无法构造，需人工复核",
                    axes.iter()
                        .map(|axis| format!("轴{axis}"))
                        .collect::<Vec<_>>()
                        .join("/")
                ),
                BrowseDiagnosticSeverity::Warning,
            )),
        }
    }
    // 过滤只作用于实例节点（诊断恒可见），分页游标是结果偏移。
    local_page(nodes, filter, cursor, limit)
}
