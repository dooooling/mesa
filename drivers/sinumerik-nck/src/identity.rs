//! 轴与驱动对象的有界只读浏览。临时Browse连接不保留跨请求缓存。
//! 根目录纯本地，列表按实例编号分页，展开时仅重新核对选中对象。
//!
//! 身份探针是**驱动内建诊断程序**（`identity_spec`），不查变量目录、
//! 不依赖模板文件存在，也不构成“允许采集哪些地址”的白名单。
use crate::identity_spec::{
    AXIS_CTRLOUT_MODULE_NR, AXIS_CTRLOUT_TYPE, AXIS_NAME, AXIS_SPIND_ASSIGN, DRIVE_OBJECT_NAME,
    DRIVE_OBJECT_NAME_M130, IdentityProbeSpec, probes_for_family,
};
use crate::presets::{PresetRegistry, leaf_node_with};
use crate::{NckClient, NckConnConfig, decode_value};
use mesa_core_types::{ErrorKind, Value};
use mesa_driver_protocol::pb::BrowseNode;
use mesa_driver_sdk::SdkDriverError;

pub const ROOT: &str = "nck://identity";
const AXES: &str = "nck://identity/axes";
const OBJECTS: &str = "nck://identity/objects";
const MAX_INSTANCE: u16 = 31;

/// 身份实例页的**驱动侧**上限（不依赖调用方传对 `limit`）。
///
/// 为什么要独立设限：轴实例每项最多 5 次身份读取（轴名/驱动号/输出类型/主轴
/// 分配/对象名），若沿用 Browse 的通用 `limit`（Web 默认 50），一次请求就会
/// 在 15 秒预算内顺序读约 `31 × 5` 项——分页等于没有生效。这里把每页钉在
/// 个位数，使单次请求的读取量可观测、有上界；后续页由 `next_cursor` 驱动
/// （Web 已有「下一页」入口）。
const IDENTITY_PAGE_MAX: usize = 8;
type Page = (Vec<BrowseNode>, Option<String>);

struct AxisIdentity {
    index: u16,
    name: String,
    drive: Option<u16>,
    output_type: u32,
    object_name: Option<String>,
    spindle: Option<u32>,
    notes: Vec<String>,
}

/// 该系列是否有已核对的探针集合（未取得真实证据的系列不得套用 828D 探针）。
pub fn supported(family: &str) -> bool {
    probes_for_family(family).is_some()
}

/// 分支节点：**明确不可选**（IPC 1.8 选择契约），前端无需靠空信封形态猜测。
fn folder(id: &str, label: &str) -> BrowseNode {
    let selection = mesa_driver_protocol::browse_selection::not_selectable();
    BrowseNode {
        id: id.into(),
        label: label.into(),
        kind: "folder".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: true,
        binding_json: "{}".into(),
        selection_mode: selection.mode as i32,
        selection_payload_json: selection.payload_json,
        diagnostics: selection.diagnostics,
    }
}

pub fn root_node() -> BrowseNode {
    folder(ROOT, "轴与驱动对象（实时只读核对）")
}

/// 诊断节点：不可选，但**不给理由**——它本身就是「为什么看不到内容」的说明，
/// 再叠一条红色阻断只会误导（用户本来也不该在这里选用任何东西）。
fn diagnostic(id: &str, message: String) -> BrowseNode {
    let selection = mesa_driver_protocol::browse_selection::not_selectable();
    BrowseNode {
        id: id.into(),
        label: message,
        kind: "diagnostic".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: "{}".into(),
        selection_mode: selection.mode as i32,
        selection_payload_json: selection.payload_json,
        diagnostics: selection.diagnostics,
    }
}

fn device_rejected(error: &SdkDriverError) -> bool {
    error.code.starts_with("S7_JOB_") || error.code.starts_with("S7_ITEM_")
}

/// 只有成功解码的空名称代表表尾；作业/单项拒绝与本地解码失败均保留为错误。
///
/// `target` 填入探针的可变槽位（`ProbeSlot::TargetIndex`）；探针是内建常量，
/// 不查目录、不依赖模板。
async fn read(
    client: &mut NckClient,
    spec: &IdentityProbeSpec,
    target: u16,
) -> Result<Value, SdkDriverError> {
    let (_, item) = spec
        .resolve(target)
        .map_err(|e| SdkDriverError::configuration("IDENTITY_ADDRESS", e.to_string()))?;
    let results = client.read_vars(&[item]).await?;
    let result = results.first().ok_or_else(|| {
        SdkDriverError::new(
            ErrorKind::Protocol,
            "IDENTITY_RESPONSE_INVALID",
            "缺少读取结果",
        )
    })?;
    if result.return_code != 0xff {
        return Err(SdkDriverError::new(
            ErrorKind::Address,
            format!("S7_ITEM_0x{:02X}", result.return_code),
            format!(
                "身份探针{}被设备拒绝，原始单项码0x{:02X}",
                spec.name, result.return_code
            ),
        ));
    }
    let bytes = result.data.as_deref().ok_or_else(|| {
        SdkDriverError::new(
            ErrorKind::Protocol,
            "IDENTITY_RESPONSE_INVALID",
            "响应类型或长度不符，不能当作未定义对象",
        )
    })?;
    decode_value(bytes, spec.decode)
        .map_err(|e| SdkDriverError::new(ErrorKind::Protocol, "IDENTITY_DECODE", e.to_string()))
}

fn as_string(value: Value) -> Result<String, SdkDriverError> {
    match value {
        Value::String(name) => Ok(name),
        _ => Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "IDENTITY_TYPE",
            "身份名称必须是字符串",
        )),
    }
}

fn as_u32(value: Value) -> Result<u32, SdkDriverError> {
    match value {
        Value::U32(number) => Ok(number),
        _ => Err(SdkDriverError::new(
            ErrorKind::Protocol,
            "IDENTITY_TYPE",
            "轴分配必须是无符号整数",
        )),
    }
}

async fn axis(client: &mut NckClient, index: u16) -> Result<Option<AxisIdentity>, SdkDriverError> {
    let name = as_string(read(client, &AXIS_NAME, index).await?)?;
    if name.is_empty() {
        return Ok(None);
    }
    let number = as_u32(read(client, &AXIS_CTRLOUT_MODULE_NR, index).await?)?;
    let drive = (1..=u32::from(MAX_INSTANCE))
        .contains(&number)
        .then_some(number as u16);
    let output_type = as_u32(read(client, &AXIS_CTRLOUT_TYPE, index).await?)?;
    let spindle = as_u32(read(client, &AXIS_SPIND_ASSIGN, index).await?)?;
    let mut notes = Vec::new();
    let object_name = if let Some(drive) = drive.filter(|_| output_type == 1) {
        // NC 变量入口（module 26）读驱动对象名；与 module 130 入口编号不同，
        // 因此这里只核对该入口自身的对象身份，不做跨入口换算。
        match read(client, &DRIVE_OBJECT_NAME, drive).await {
            Ok(value) => {
                let name = as_string(value)?;
                (!name.is_empty()).then_some(name)
            }
            Err(error) if device_rejected(&error) => {
                notes.push(format!(
                    "驱动{drive}名称读取被拒绝：{}；对象身份未确认",
                    error.code
                ));
                None
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    Ok(Some(AxisIdentity {
        index,
        name,
        drive,
        output_type,
        object_name,
        spindle: (spindle > 0 && output_type == 1).then_some(spindle),
        notes,
    }))
}

fn axis_node(axis: &AxisIdentity) -> BrowseNode {
    let identity = match axis.spindle {
        Some(number) => format!("{}（主轴{number}）", axis.name),
        None => axis.name.clone(),
    };
    let mut node = folder(
        &format!("{ROOT}/axis/{}", axis.index),
        &format!(
            "{identity} · 轴{} → NC驱动{} · {} → {}",
            axis.index,
            axis.drive
                .map(|n| n.to_string())
                .unwrap_or_else(|| "未确认".into()),
            if axis.output_type == 1 {
                "已分配驱动输出"
            } else {
                "未确认有效驱动输出"
            },
            axis.object_name.as_deref().unwrap_or("未确认对象")
        ),
    );
    node.has_children = axis.object_name.is_some();
    if !axis.notes.is_empty() {
        node.label
            .push_str(&format!(" · {}", axis.notes.join("；")));
    }
    node
}

fn object_node(unit: u16, name: &str) -> BrowseNode {
    folder(
        &format!("{ROOT}/object/{unit}"),
        &format!("{name} · 候选对象入口编号{unit} · 轴关联须按名称核对"),
    )
}

/// 变量叶：可选模板 + **已确认实例**填充 `area_no`；
/// `line` 仍留给使用者确认（不按变量名猜行索引语义）。
///
/// `entry` 是**核对该编号时实际使用的探针**。实例编号只能用于它被核对过的
/// 那条入口：`module 26`（NC 变量）与 `module 130`（驱动对象）的编号在实机上
/// 并不相同，把经 A 入口核对的编号填进 B 入口的模板，等于凭空制造一个未经
/// 证明的地址归属。因此这里按 `entry.module` 做第二重过滤。
///
/// 模板缺失时返回空集合——身份核对结果照常显示，不会被模板缺失阻断。
fn variable_nodes(
    presets: &PresetRegistry,
    family: &str,
    entry: &IdentityProbeSpec,
    unit: u16,
    label: &str,
) -> Vec<BrowseNode> {
    presets
        .for_family(family)
        .into_iter()
        // 身份节点代表「本入口的驱动对象」，因此只列驱动区（area=V）模板。
        // 这是**浏览作用域**选择，不是地址准入：手动填写任意 area 都不受影响。
        .filter(|t| t.parameters.get("area").and_then(|v| v.as_str()) == Some("V"))
        // 入口过滤：只列 module 与本次核对入口一致的模板（编号不跨入口复用）。
        .filter(|t| {
            t.parameters.get("module").and_then(|v| v.as_u64()) == Some(u64::from(entry.module))
        })
        .map(|t| {
            let mut node = leaf_node_with(t, &[("area_no", serde_json::Value::from(unit))]);
            // 标签显式标出编号出处，避免使用者误以为两条入口编号等价。
            node.label = format!(
                "{label} · module{}入口编号{unit} · {}",
                entry.module, node.label
            );
            node
        })
        .collect()
}

fn matches(node: &BrowseNode, filter: &str) -> bool {
    node.kind == "diagnostic"
        || filter.is_empty()
        || node.label.contains(filter)
        || node.id.contains(filter)
}

fn limit_size(limit: u32) -> usize {
    if limit == 0 {
        50
    } else {
        limit.min(500) as usize
    }
}

fn local_page(
    mut nodes: Vec<BrowseNode>,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> Result<Page, SdkDriverError> {
    nodes.retain(|node| matches(node, filter));
    let start = if cursor.is_empty() {
        0
    } else {
        cursor.parse::<usize>().map_err(|_| {
            SdkDriverError::configuration("IDENTITY_CURSOR", "变量页游标必须是非负整数")
        })?
    }
    .min(nodes.len());
    let end = (start + limit_size(limit)).min(nodes.len());
    Ok((
        nodes[start..end].to_vec(),
        (end < nodes.len()).then(|| end.to_string()),
    ))
}

fn instance_cursor(cursor: &str) -> Result<u16, SdkDriverError> {
    let index = if cursor.is_empty() {
        1
    } else {
        cursor
            .parse::<u16>()
            .map_err(|_| SdkDriverError::configuration("IDENTITY_CURSOR", "实例游标必须是1..31"))?
    };
    if !(1..=MAX_INSTANCE).contains(&index) {
        return Err(SdkDriverError::configuration(
            "IDENTITY_CURSOR",
            "实例游标必须是1..31",
        ));
    }
    Ok(index)
}

/// 列表游标记录下一个实例编号，不依赖上次临时进程。过滤只处理本页已读实例，
/// 即使无匹配节点仍返回后续游标，避免一次搜索隐式扫描整个控制器。
async fn instance_page(
    client: &mut NckClient,
    axes: bool,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> Result<Page, SdkDriverError> {
    let mut index = instance_cursor(cursor)?;
    let mut nodes = Vec::new();
    // 双重收敛：调用方 limit 与服务端上限取小；并以实例总数封顶。
    let page = limit_size(limit)
        .min(IDENTITY_PAGE_MAX)
        .min(usize::from(MAX_INSTANCE));
    for _ in 0..page {
        let result = if axes {
            axis(client, index)
                .await
                .map(|axis| axis.map(|value| axis_node(&value)))
        } else {
            // 驱动对象列表走**显式驱动对象入口**（module 130）；它与 NC 变量
            // 入口（module 26）编号不同，两者不得互相换算。
            read(client, &DRIVE_OBJECT_NAME_M130, index)
                .await
                .and_then(as_string)
                .map(|name| (!name.is_empty()).then(|| object_node(index, &name)))
        };
        match result {
            Ok(Some(node)) => {
                if matches(&node, filter) {
                    nodes.push(node);
                }
            }
            Ok(None) => return Ok((nodes, None)),
            Err(error) if device_rejected(&error) => {
                // 拒绝不是确认的表尾；诊断不受过滤词影响，避免悄悄隐藏枚举中断。
                nodes.push(diagnostic(
                    &format!(
                        "{ROOT}/note/{}-{index}",
                        if axes { "axis" } else { "object" }
                    ),
                    format!(
                        "{}编号{index}读取被拒绝：{}；后续未枚举，目录完整性待确认",
                        if axes { "轴" } else { "对象" },
                        error.code
                    ),
                ));
                return Ok((nodes, None));
            }
            Err(error) => return Err(error),
        }
        if index == MAX_INSTANCE {
            return Ok((nodes, None));
        }
        index += 1;
    }
    Ok((nodes, Some(index.to_string())))
}

enum Target {
    Axes,
    Objects,
    Axis(u16),
    Object(u16),
}

fn target(parent: &str) -> Result<Option<Target>, SdkDriverError> {
    if parent == AXES {
        return Ok(Some(Target::Axes));
    }
    if parent == OBJECTS {
        return Ok(Some(Target::Objects));
    }
    for (prefix, is_axis) in [
        ("nck://identity/axis/", true),
        ("nck://identity/object/", false),
    ] {
        if let Some(value) = parent.strip_prefix(prefix) {
            if value.is_empty() {
                return Err(SdkDriverError::configuration(
                    "IDENTITY_ADDRESS",
                    "对象编号不能为空",
                ));
            }
            let index = instance_cursor(value)?;
            return Ok(Some(if is_axis {
                Target::Axis(index)
            } else {
                Target::Object(index)
            }));
        }
    }
    Ok(None)
}

/// 单次请求仅读取目标页或选中身份；不遍历另一入口，不推断两个入口编号相同。
pub async fn browse(
    cfg: &NckConnConfig,
    presets: &PresetRegistry,
    parent: &str,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> Result<Page, SdkDriverError> {
    if parent == ROOT {
        return local_page(
            vec![
                folder(AXES, "机床轴（NC驱动入口）"),
                folder(OBJECTS, "驱动对象（候选入口，按名称核对）"),
            ],
            filter,
            cursor,
            limit,
        );
    }
    let Some(target) = target(parent)? else {
        return Ok((Vec::new(), None));
    };
    if matches!(target, Target::Axes | Target::Objects) {
        instance_cursor(cursor)?;
    }
    let mut client = NckClient::connect(cfg).await?;
    match target {
        Target::Axes => instance_page(&mut client, true, filter, cursor, limit).await,
        Target::Objects => instance_page(&mut client, false, filter, cursor, limit).await,
        Target::Axis(index) => match axis(&mut client, index).await {
            Ok(Some(axis)) => {
                // 身份复核被拒绝后不存在可翻页的变量列表；旧游标不能跳过诊断。
                if !axis.notes.is_empty() {
                    return Ok((vec![diagnostic(parent, axis.notes.join("；"))], None));
                }
                let nodes = if let Some(drive) = axis.drive.filter(|_| axis.object_name.is_some()) {
                    // 轴路径的驱动编号来自 MD30110，且已用 **module 26**
                    // （`DRIVE_OBJECT_NAME`）核对过对象名，故只列该入口模板。
                    variable_nodes(presets, &cfg.family, &DRIVE_OBJECT_NAME, drive, &axis.name)
                } else {
                    Vec::new()
                };
                local_page(nodes, filter, cursor, limit)
            }
            Ok(None) => Ok((Vec::new(), None)),
            Err(error) if device_rejected(&error) => Ok((
                vec![diagnostic(
                    parent,
                    format!("轴{index}读取被拒绝：{}；身份未确认", error.code),
                )],
                None,
            )),
            Err(error) => Err(error),
        },
        Target::Object(unit) => match read(&mut client, &DRIVE_OBJECT_NAME_M130, unit).await {
            Ok(value) => {
                let name = as_string(value)?;
                local_page(
                    if name.is_empty() {
                        Vec::new()
                    } else {
                        // 对象路径的编号由 **module 130**（`DRIVE_OBJECT_NAME_M130`）
                        // 核对，只列该入口模板；不得据此生成 module 26 采集地址。
                        variable_nodes(presets, &cfg.family, &DRIVE_OBJECT_NAME_M130, unit, &name)
                    },
                    filter,
                    cursor,
                    limit,
                )
            }
            Err(error) if device_rejected(&error) => Ok((
                vec![diagnostic(
                    parent,
                    format!(
                        "对象{unit}读取被拒绝：{}；不是已确认的未定义对象",
                        error.code
                    ),
                )],
                None,
            )),
            Err(error) => Err(error),
        },
    }
}
