//! 轴与驱动对象的有界只读浏览。临时Browse连接不保留跨请求缓存。
//! 根目录纯本地，列表按实例编号分页，展开时仅重新核对选中对象。
use crate::{
    NckArea, NckCatalog, NckClient, NckConnConfig, NckReadItem, NckVariableRef, decode_value,
    resolve_wire,
};
use mesa_core_types::{ErrorKind, Value};
use mesa_driver_protocol::pb::BrowseNode;
use mesa_driver_sdk::SdkDriverError;

pub const ROOT: &str = "nck://identity";
const AXES: &str = "nck://identity/axes";
const OBJECTS: &str = "nck://identity/objects";
const MAX_INSTANCE: u16 = 31;
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

pub fn supported(catalog: &NckCatalog) -> bool {
    catalog
        .lookup(NckArea::Nck, "M", "MDSA_AXCONF_MACHAX_NAME_TAB")
        .is_ok()
        && catalog.lookup(NckArea::FeedDrive, "M", "p0199").is_ok()
}

fn folder(id: &str, label: &str) -> BrowseNode {
    BrowseNode {
        id: id.into(),
        label: label.into(),
        kind: "folder".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: true,
        binding_json: "{}".into(),
    }
}

pub fn root_node() -> BrowseNode {
    folder(ROOT, "轴与驱动对象（实时只读核对）")
}

fn diagnostic(id: &str, message: String) -> BrowseNode {
    BrowseNode {
        id: id.into(),
        label: message,
        kind: "diagnostic".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: false,
        binding_json: "{}".into(),
    }
}

fn device_rejected(error: &SdkDriverError) -> bool {
    error.code.starts_with("S7_JOB_") || error.code.starts_with("S7_ITEM_")
}

fn parameters(area: &str, unit: u16, variable: &str, line: u16, path: &str) -> serde_json::Value {
    serde_json::json!({"area":area,"area_no":unit,"block":"M","variable":variable,
        "line":line,"count":if variable == "p0199" {25} else {1},"access_path":path})
}

/// 只有成功解码的空名称代表表尾；作业/单项拒绝与本地解码失败均保留为错误。
async fn read(
    client: &mut NckClient,
    catalog: &NckCatalog,
    parameters: serde_json::Value,
) -> Result<Value, SdkDriverError> {
    let reference = NckVariableRef::from_parameters(parameters.as_object().unwrap())
        .map_err(|e| SdkDriverError::configuration("IDENTITY_ADDRESS", e.to_string()))?;
    let def = catalog
        .lookup(reference.area, "M", &reference.variable)
        .map_err(|e| SdkDriverError::configuration("IDENTITY_CATALOG", e.to_string()))?;
    let resolved = resolve_wire(&reference, def)
        .map_err(|e| SdkDriverError::configuration("IDENTITY_ADDRESS", e.to_string()))?;
    let results = client
        .read_vars(&[NckReadItem {
            wire: resolved.wire,
            expected_data_len: resolved.expected_data_len,
            expected_transport_size: resolved.expected_transport_size,
        }])
        .await?;
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
                "变量{}被设备拒绝，原始单项码0x{:02X}",
                reference.variable, result.return_code
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
    decode_value(bytes, resolved.kind)
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

async fn axis(
    client: &mut NckClient,
    catalog: &NckCatalog,
    index: u16,
) -> Result<Option<AxisIdentity>, SdkDriverError> {
    let name = as_string(
        read(
            client,
            catalog,
            parameters("N", 1, "MDSA_AXCONF_MACHAX_NAME_TAB", index, "nc_variable"),
        )
        .await?,
    )?;
    if name.is_empty() {
        return Ok(None);
    }
    let number = as_u32(
        read(
            client,
            catalog,
            parameters("A", index, "MDCA_CTRLOUT_MODULE_NR", 1, "nc_variable"),
        )
        .await?,
    )?;
    let drive = (1..=u32::from(MAX_INSTANCE))
        .contains(&number)
        .then_some(number as u16);
    let output_type = as_u32(
        read(
            client,
            catalog,
            parameters("A", index, "MDCA_CTRLOUT_TYPE", 1, "nc_variable"),
        )
        .await?,
    )?;
    let spindle = as_u32(
        read(
            client,
            catalog,
            parameters("A", index, "SPIND_ASSIGN_TO_MACHAX", 1, "nc_variable"),
        )
        .await?,
    )?;
    let mut notes = Vec::new();
    let object_name = if let Some(drive) = drive.filter(|_| output_type == 1) {
        match read(
            client,
            catalog,
            parameters("V", drive, "p0199", 1, "nc_variable"),
        )
        .await
        {
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

fn variable_nodes(catalog: &NckCatalog, unit: u16, path: &str, label: &str) -> Vec<BrowseNode> {
    let preset = serde_json::json!({"area_no":unit,"line":1,"access_path":path});
    catalog
        .variables()
        .into_iter()
        .filter(|def| {
            def.area == NckArea::FeedDrive
                && def.block == "M"
                && (path == "nc_variable" || def.wire.drive_object_module.is_some())
        })
        .map(|def| {
            let mut node = crate::browse::leaf_node(def, preset.as_object().unwrap());
            node.label = format!("{label} · {}", node.label);
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
    catalog: &NckCatalog,
    axes: bool,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> Result<Page, SdkDriverError> {
    let mut index = instance_cursor(cursor)?;
    let mut nodes = Vec::new();
    for _ in 0..limit_size(limit).min(usize::from(MAX_INSTANCE)) {
        let result = if axes {
            axis(client, catalog, index)
                .await
                .map(|axis| axis.map(|value| axis_node(&value)))
        } else {
            read(
                client,
                catalog,
                parameters("V", index, "p0199", 1, "drive_object"),
            )
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
    catalog: &NckCatalog,
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
        Target::Axes => instance_page(&mut client, catalog, true, filter, cursor, limit).await,
        Target::Objects => instance_page(&mut client, catalog, false, filter, cursor, limit).await,
        Target::Axis(index) => match axis(&mut client, catalog, index).await {
            Ok(Some(axis)) => {
                let nodes = if let Some(drive) = axis.drive.filter(|_| axis.object_name.is_some()) {
                    variable_nodes(catalog, drive, "nc_variable", &axis.name)
                } else if !axis.notes.is_empty() {
                    vec![diagnostic(parent, axis.notes.join("；"))]
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
        Target::Object(unit) => match read(
            &mut client,
            catalog,
            parameters("V", unit, "p0199", 1, "drive_object"),
        )
        .await
        {
            Ok(value) => {
                let name = as_string(value)?;
                local_page(
                    if name.is_empty() {
                        Vec::new()
                    } else {
                        variable_nodes(catalog, unit, "drive_object", &name)
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
