//! 按公开变量定义只读核对轴和驱动对象；同名对象是两个入口编号的关联依据。
//! 网络访问只发生在用户展开对象目录时，扫描上限来自NCK五位实例字段。
use crate::{
    NckArea, NckCatalog, NckClient, NckConnConfig, NckReadItem, NckVariableRef, decode_value,
    resolve_wire,
};
use mesa_core_types::Value;
use mesa_driver_protocol::pb::BrowseNode;
use mesa_driver_sdk::SdkDriverError;

pub const ROOT: &str = "nck://identity";

#[derive(Debug, Clone)]
pub struct AxisIdentity {
    pub index: u16,
    pub name: String,
    pub drive: Option<u16>,
    pub output_type: Option<u32>,
    pub object_name: Option<String>,
    pub object_unit: Option<u16>,
    pub spindle: Option<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct IdentityMap {
    pub axes: Vec<AxisIdentity>,
    pub objects: Vec<(u16, String)>,
    /// 枚举遇到设备拒绝时显式报告边界，已读对象仍可核对，不声称目录完整。
    pub notes: Vec<String>,
}

pub fn supported(catalog: &NckCatalog) -> bool {
    catalog
        .lookup(NckArea::Nck, "M", "MDSA_AXCONF_MACHAX_NAME_TAB")
        .is_ok()
        && catalog.lookup(NckArea::FeedDrive, "M", "p0199").is_ok()
}

pub fn root_node() -> BrowseNode {
    BrowseNode {
        id: ROOT.into(),
        label: "轴与驱动对象（实时只读核对）".into(),
        kind: "folder".into(),
        data_type: String::new(),
        access: "read".into(),
        has_children: true,
        binding_json: "{}".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_uses_unique_names_and_never_associates_disabled_axis_by_number() {
        let mut map = IdentityMap {
            axes: vec![
                AxisIdentity {
                    index: 4,
                    name: "MSP1".into(),
                    drive: Some(4),
                    output_type: Some(1),
                    object_name: Some("SERVO_3.3:3".into()),
                    object_unit: None,
                    spindle: None,
                },
                AxisIdentity {
                    index: 6,
                    name: "MB1".into(),
                    drive: Some(6),
                    output_type: Some(0),
                    object_name: None,
                    object_unit: None,
                    spindle: None,
                },
            ],
            objects: vec![(1, "SERVO_3.3:3".into()), (6, "SERVO_3.3:8".into())],
            notes: Vec::new(),
        };
        correlate(&mut map);
        assert_eq!(map.axes[0].object_unit, Some(1));
        assert_eq!(map.axes[1].object_unit, None);
        map.objects.push((9, "SERVO_3.3:3".into()));
        correlate(&mut map);
        assert_eq!(map.axes[0].object_unit, None, "重名对象不能猜测编号");
    }

    #[tokio::test]
    async fn recorded_directory_keeps_known_objects_when_next_object_is_rejected() {
        fn bytes(value: &str) -> Vec<u8> {
            value
                .split_whitespace()
                .map(|part| u8::from_str_radix(part, 16).unwrap())
                .collect()
        }
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
        // 独立边界记录：轴名行9为空；对象9存在，对象10拒绝整个请求。
        items.insert(
            bytes("12 08 82 01 27 10 00 09 1a 01"),
            (0xff, 9, vec![0; 16]),
        );
        let name: Vec<u8> = "SERVO_3.15:5"
            .encode_utf16()
            .chain(std::iter::repeat_n(0, 13))
            .flat_map(u16::to_be_bytes)
            .collect();
        items.insert(bytes("12 08 82 a9 00 c7 00 01 82 19"), (0xff, 6, name));
        jobs.insert(bytes("12 08 82 aa 00 c7 00 01 82 19"), 0x8104);
        let fixture = crate::fixture::NckFixture::spawn(crate::fixture::NckFixtureState {
            recorded_items: Some(items),
            recorded_jobs: jobs,
            ..Default::default()
        })
        .await;
        let cfg = NckConnConfig::from_json(
            &serde_json::json!({"host":"127.0.0.1","port":fixture.addr.port(),
            "family":"828d","local_tsap":256,"remote_tsap":258,"max_read_items":1}),
        )
        .unwrap();
        let catalog =
            crate::CatalogRegistry::load_dir(&format!("{}/catalog", env!("CARGO_MANIFEST_DIR")))
                .unwrap()
                .resolve("828d")
                .unwrap();
        let map = load(&cfg, &catalog).await.unwrap();
        assert_eq!(map.axes.len(), 8);
        assert_eq!(map.objects.len(), 9);
        assert_eq!(map.axes[3].object_unit, Some(1));
        assert_eq!(map.axes[5].object_unit, None);
        assert!(map.notes.iter().any(|note| note.contains("0x8104")));
        let (root, _) = nodes(&map, &catalog, ROOT, "", "", 100);
        assert!(
            root.iter()
                .any(|node| node.label.contains("SERVO_3.3:8") && node.id.ends_with("/object/6"))
        );
        let (leaves, _) = nodes(&map, &catalog, "nck://identity/object/6", "r0080", "", 100);
        let binding: serde_json::Value = serde_json::from_str(&leaves[0].binding_json).unwrap();
        assert_eq!(binding["parameters"]["area_no"], 6);
        assert_eq!(binding["parameters"]["access_path"], "drive_object");
    }
}

async fn read(
    client: &mut NckClient,
    catalog: &NckCatalog,
    area: &str,
    unit: u16,
    variable: &str,
    line: u16,
    count: u16,
    path: &str,
) -> Result<Option<Value>, SdkDriverError> {
    let parameters = serde_json::json!({"area":area,"area_no":unit,"block":"M","variable":variable,
        "line":line,"count":count,"access_path":path});
    let reference = NckVariableRef::from_parameters(parameters.as_object().unwrap())
        .map_err(|e| SdkDriverError::configuration("IDENTITY_ADDRESS", e.to_string()))?;
    let def = catalog
        .lookup(reference.area, "M", variable)
        .map_err(|e| SdkDriverError::configuration("IDENTITY_CATALOG", e.to_string()))?;
    let resolved = resolve_wire(&reference, def)
        .map_err(|e| SdkDriverError::configuration("IDENTITY_ADDRESS", e.to_string()))?;
    let results = match client
        .read_vars(&[NckReadItem {
            wire: resolved.wire,
            expected_data_len: resolved.expected_data_len,
            expected_transport_size: resolved.expected_transport_size,
        }])
        .await
    {
        Ok(results) => results,
        // 对象目录允许呈现缺失对象；采集run仍保留作业拒绝即失败的既有规则。
        Err(error) if error.code == "S7_JOB_0x8201" => return Ok(None),
        Err(error) => return Err(error),
    };
    match results.first().and_then(|result| result.data.as_deref()) {
        Some(bytes) => decode_value(bytes, resolved.kind)
            .map(Some)
            .map_err(|e| SdkDriverError::configuration("IDENTITY_DECODE", e.to_string())),
        None => Ok(None),
    }
}

/// 对象名重复时不猜编号；轴未启用驱动输出时不将同编号对象绑定到该轴。
pub fn correlate(map: &mut IdentityMap) {
    for axis in &mut map.axes {
        axis.object_unit = None;
        if let Some(name) = &axis.object_name {
            let matches: Vec<u16> = map
                .objects
                .iter()
                .filter(|(_, object)| object == name)
                .map(|(unit, _)| *unit)
                .collect();
            if matches.len() == 1 && axis.output_type == Some(1) {
                axis.object_unit = Some(matches[0]);
            }
        }
    }
}

pub async fn load(
    cfg: &NckConnConfig,
    catalog: &NckCatalog,
) -> Result<IdentityMap, SdkDriverError> {
    let mut client = NckClient::connect(cfg).await?;
    let mut map = IdentityMap::default();
    // 名称连续表遇到未定义行即停止；最多31项，不能无界枚举控制器对象。
    for index in 1..=31 {
        let Some(Value::String(name)) = read(
            &mut client,
            catalog,
            "N",
            1,
            "MDSA_AXCONF_MACHAX_NAME_TAB",
            index,
            1,
            "nc_variable",
        )
        .await?
        else {
            break;
        };
        if name.is_empty() {
            break;
        }
        let drive = match read(
            &mut client,
            catalog,
            "A",
            index,
            "MDCA_CTRLOUT_MODULE_NR",
            1,
            1,
            "nc_variable",
        )
        .await?
        {
            Some(Value::U32(drive)) if (1..=31).contains(&drive) => Some(drive as u16),
            _ => None,
        };
        let output_type = match read(
            &mut client,
            catalog,
            "A",
            index,
            "MDCA_CTRLOUT_TYPE",
            1,
            1,
            "nc_variable",
        )
        .await?
        {
            Some(Value::U32(value)) => Some(value),
            _ => None,
        };
        let object_name = if let Some(drive) = drive.filter(|_| output_type == Some(1)) {
            match read(
                &mut client,
                catalog,
                "V",
                drive,
                "p0199",
                1,
                25,
                "nc_variable",
            )
            .await?
            {
                Some(Value::String(name)) if !name.is_empty() => Some(name),
                _ => None,
            }
        } else {
            None
        };
        let spindle = match read(
            &mut client,
            catalog,
            "A",
            index,
            "SPIND_ASSIGN_TO_MACHAX",
            1,
            1,
            "nc_variable",
        )
        .await?
        {
            Some(Value::U32(number)) if number > 0 && output_type == Some(1) => Some(number as u16),
            _ => None,
        };
        map.axes.push(AxisIdentity {
            index,
            name,
            drive,
            output_type,
            object_name,
            object_unit: None,
            spindle,
        });
    }
    for unit in 1..=31 {
        match read(
            &mut client,
            catalog,
            "V",
            unit,
            "p0199",
            1,
            25,
            "drive_object",
        )
        .await
        {
            Ok(Some(Value::String(name))) if !name.is_empty() => map.objects.push((unit, name)),
            Err(error) if error.code.starts_with("S7_JOB_") => {
                map.notes.push(format!(
                    "对象编号{unit}返回{}；后续对象未枚举，目录完整性待确认",
                    error.code
                ));
                break;
            }
            Err(error) => return Err(error),
            _ => {
                map.notes
                    .push(format!("对象编号{unit}无可读名称；后续对象未枚举"));
                break;
            }
        }
    }
    correlate(&mut map);
    Ok(map)
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

pub fn nodes(
    map: &IdentityMap,
    catalog: &NckCatalog,
    parent: &str,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> (Vec<BrowseNode>, Option<String>) {
    let mut nodes = Vec::new();
    if parent == ROOT {
        for (index, note) in map.notes.iter().enumerate() {
            nodes.push(BrowseNode {
                id: format!("{ROOT}/note/{index}"),
                label: note.clone(),
                kind: "diagnostic".into(),
                data_type: String::new(),
                access: "read".into(),
                has_children: false,
                binding_json: "{}".into(),
            });
        }
        for axis in &map.axes {
            nodes.push(BrowseNode {
                id: format!("{ROOT}/axis/{}", axis.index),
                label: format!(
                    "{} · 轴{} → 驱动{} · {} → {} · 对象入口编号{}",
                    if let Some(number) = axis.spindle {
                        format!("{}（主轴{number}）", axis.name)
                    } else {
                        axis.name.clone()
                    },
                    axis.index,
                    axis.drive
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "未确认".into()),
                    if axis.output_type == Some(1) {
                        "已分配驱动输出"
                    } else {
                        "未确认有效驱动输出"
                    },
                    axis.object_name.as_deref().unwrap_or("未确认对象"),
                    axis.object_unit
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "未确认".into())
                ),
                kind: "folder".into(),
                data_type: String::new(),
                access: "read".into(),
                has_children: axis.object_name.is_some(),
                binding_json: "{}".into(),
            });
        }
        for (unit, name) in &map.objects {
            if !map.axes.iter().any(|axis| axis.object_unit == Some(*unit)) {
                nodes.push(BrowseNode {
                    id: format!("{ROOT}/object/{unit}"),
                    label: format!("{name} · 对象入口编号{unit} · 轴关联未确认"),
                    kind: "folder".into(),
                    data_type: String::new(),
                    access: "read".into(),
                    has_children: true,
                    binding_json: "{}".into(),
                });
            }
        }
    } else if let Some(index) = parent
        .strip_prefix(&format!("{ROOT}/axis/"))
        .and_then(|s| s.parse::<u16>().ok())
    {
        if let Some(axis) = map.axes.iter().find(|axis| axis.index == index) {
            if axis.object_name.is_some() {
                if let Some(drive) = axis.drive {
                    nodes.extend(variable_nodes(catalog, drive, "nc_variable", &axis.name));
                }
                if let Some(unit) = axis.object_unit {
                    nodes.extend(variable_nodes(catalog, unit, "drive_object", &axis.name));
                }
            }
        }
    } else if let Some(unit) = parent
        .strip_prefix(&format!("{ROOT}/object/"))
        .and_then(|s| s.parse::<u16>().ok())
    {
        if let Some((_, name)) = map.objects.iter().find(|(number, _)| *number == unit) {
            nodes.extend(variable_nodes(catalog, unit, "drive_object", name));
        }
    }
    if !filter.is_empty() {
        nodes.retain(|node| node.label.contains(filter) || node.id.contains(filter));
    }
    let start = cursor.parse::<usize>().unwrap_or(0).min(nodes.len());
    let end = (start
        + if limit == 0 {
            50
        } else {
            limit.min(500) as usize
        })
    .min(nodes.len());
    (
        nodes[start..end].to_vec(),
        (end < nodes.len()).then(|| end.to_string()),
    )
}
