//! NCK Catalog（ADR 0001 §12：变量类型与 wire mapping 的唯一真相来源）。
//!
//! 链路：用户语义（Area/Block/Variable/Line/Column）→ Catalog →
//! `NckWireAddress`（syntax/area_unit/column/line/module/line_count）→ codec。
//! 系列 mapping 变化只换 Catalog，不动 Resource API / Core / Profile / Data Plane。
//!
//! 铁律（CONTRACT.md）：wire 数值必须由官方变量定义 + 确定性协议测试 +
//! 真机三方确认后方可进入 `supported` catalog；本模块只定 schema 与解析规则，
//! 空 catalog 合法（未知变量 fail-closed，不是猜类型）。

use std::collections::HashMap;

use thiserror::Error;

use crate::address::NckArea;

// ---------------------------------------------------------------------------
// 类型
// ---------------------------------------------------------------------------

/// 变量形状（决定 Line/Column 哪些必填）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NckShape {
    /// 标量：无需 line/column。
    Scalar,
    /// 行数组：需 line（如各轴分行）。
    Lines,
    /// 行列矩阵：需 line + column。
    LinesAndColumns,
}

/// 线缆定义（真机确认前不得填写真实值，见 catalog/*.json）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NckWireDefinition {
    /// NCK module 字节。
    pub module: u8,
    /// NCK column（u16BE）。
    pub column: u16,
    /// 响应 transport_size 期望（校验用）。
    pub transport_size: u8,
    /// 单元素字节数（响应切片与 DataType 解码依据）。
    pub element_size: usize,
}

/// 单个变量定义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NckVariableDefinition {
    pub area: NckArea,
    pub block: String,
    pub variable: String,
    /// Core DataType 名（如 "F64"；解析为 `mesa_core_types::DataType` 由数据面负责）。
    pub data_type: String,
    pub shape: NckShape,
    pub unit: Option<String>,
    pub wire: NckWireDefinition,
    pub supported_families: Vec<String>,
}

/// Catalog 查询键（Area + Block + Variable，大小写敏感原样）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CatalogKey {
    area: NckArea,
    block: String,
    variable: String,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CatalogError {
    #[error("未知变量 `{area:?}/{block}/{variable}`（catalog 无定义，fail-closed）")]
    UnknownVariable {
        area: NckArea,
        block: String,
        variable: String,
    },
    #[error("catalog 形状非法: {reason}")]
    Invalid { reason: String },
}

/// NCK Catalog（版本化数据资产；V1 空表合法）。
#[derive(Debug, Clone, Default)]
pub struct NckCatalog {
    entries: HashMap<CatalogKey, NckVariableDefinition>,
}

impl NckCatalog {
    pub fn empty() -> Self {
        Self::default()
    }

    /// 装载随仓 catalog（`catalog/{common,840d-sl,828d}.json` 依次合并，
    /// 后者覆盖前者；目录由 `MESA_NCK_CATALOG_DIR` 覆盖，缺省随 crate 源码）。
    ///
    /// 缺失/非法即 fail-closed（资产缺失必须 loud，不能静默空跑）。
    /// NOTE: 系列冲突（同变量两系列不同 wire）当前以后文件为准；probe 确定
    /// 系列后按系列裁剪（Commit E），V1 文件皆空无冲突。
    pub fn load_shipped() -> Result<Self, CatalogError> {
        let dir = std::env::var("MESA_NCK_CATALOG_DIR")
            .unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_string() + "/catalog");
        let mut cat = Self::empty();
        for family in ["common", "840d-sl", "828d"] {
            let path = format!("{dir}/{family}.json");
            let text = std::fs::read_to_string(&path).map_err(|e| CatalogError::Invalid {
                reason: format!("catalog 缺失 {path}: {e}"),
            })?;
            let v: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| CatalogError::Invalid {
                    reason: format!("catalog 非法 {path}: {e}"),
                })?;
            cat.merge(Self::from_json(&v)?);
        }
        Ok(cat)
    }

    /// 从 JSON 文档加载（`catalog/*.json` 形态，见 `catalog/common.json` 注释）。
    pub fn from_json(v: &serde_json::Value) -> Result<Self, CatalogError> {
        let mut cat = Self::empty();
        let vars = v
            .get("variables")
            .and_then(|x| x.as_array())
            .ok_or_else(|| CatalogError::Invalid {
                reason: "缺少 variables 数组".into(),
            })?;
        for (i, e) in vars.iter().enumerate() {
            let def = parse_entry(e).map_err(|reason| CatalogError::Invalid {
                reason: format!("variables[{i}]: {reason}"),
            })?;
            let key = CatalogKey {
                area: def.area,
                block: def.block.clone(),
                variable: def.variable.clone(),
            };
            cat.entries.insert(key, def);
        }
        Ok(cat)
    }

    /// 合并另一 catalog（系列文件叠加 common；重复键以后者为准）。
    pub fn merge(&mut self, other: Self) {
        for (k, v) in other.entries {
            self.entries.insert(k, v);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 查询变量定义（未知即 `UnknownVariable`，调用方按项 BAD/配置拒绝）。
    pub fn lookup(
        &self,
        area: NckArea,
        block: &str,
        variable: &str,
    ) -> Result<&NckVariableDefinition, CatalogError> {
        self.entries
            .get(&CatalogKey {
                area,
                block: block.to_string(),
                variable: variable.to_string(),
            })
            .ok_or_else(|| CatalogError::UnknownVariable {
                area,
                block: block.to_string(),
                variable: variable.to_string(),
            })
    }
}

fn parse_entry(e: &serde_json::Value) -> Result<NckVariableDefinition, String> {
    let area = e.get("area").and_then(|v| v.as_str()).ok_or("缺少 area")?;
    let area = NckArea::parse(area).map_err(|e| e.to_string())?;
    let block = e
        .get("block")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("缺少 block")?
        .to_string();
    let variable = e
        .get("variable")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("缺少 variable")?
        .to_string();
    let data_type = e
        .get("data_type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("缺少 data_type")?
        .to_string();
    let shape = match e.get("shape").and_then(|v| v.as_str()).unwrap_or("scalar") {
        "scalar" => NckShape::Scalar,
        "lines" => NckShape::Lines,
        "lines_and_columns" => NckShape::LinesAndColumns,
        s => {
            return Err(format!(
                "shape `{s}` 非法，期望 scalar/lines/lines_and_columns"
            ));
        }
    };
    let unit = e
        .get("unit")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let wire = e.get("wire").ok_or("缺少 wire")?;
    let module = wire
        .get("module")
        .and_then(|v| v.as_u64())
        .and_then(|n| u8::try_from(n).ok())
        .ok_or("wire.module 需为 u8")?;
    let column = wire
        .get("column")
        .and_then(|v| v.as_u64())
        .and_then(|n| u16::try_from(n).ok())
        .ok_or("wire.column 需为 u16")?;
    let transport_size = wire
        .get("transport_size")
        .and_then(|v| v.as_u64())
        .and_then(|n| u8::try_from(n).ok())
        .ok_or("wire.transport_size 需为 u8")?;
    let element_size = wire
        .get("element_size")
        .and_then(|v| v.as_u64())
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("wire.element_size 需为正整数")?;
    let supported_families = e
        .get("supported_families")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Ok(NckVariableDefinition {
        area,
        block,
        variable,
        data_type,
        shape,
        unit,
        wire: NckWireDefinition {
            module,
            column,
            transport_size,
            element_size,
        },
        supported_families,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成条目（测试脚手架约定数值，非 Siemens 语义；真机数据由 catalog JSON 回填）。
    fn synthetic_entry() -> serde_json::Value {
        serde_json::json!({
            "area": "C", "block": "SEMA", "variable": "actFeedRate",
            "data_type": "F64", "shape": "lines", "unit": "mm/min",
            "wire": {"module": 18, "column": 42, "transport_size": 4, "element_size": 8},
            "supported_families": ["840d-sl"],
        })
    }

    #[test]
    fn empty_catalog_lookup_fails_closed() {
        let cat = NckCatalog::empty();
        assert!(cat.is_empty());
        let err = cat
            .lookup(NckArea::Channel, "SEMA", "actFeedRate")
            .unwrap_err();
        assert!(matches!(err, CatalogError::UnknownVariable { .. }));
    }

    #[test]
    fn synthetic_entry_roundtrip() {
        let doc = serde_json::json!({"variables": [synthetic_entry()]});
        let cat = NckCatalog::from_json(&doc).expect("合成条目合法");
        assert_eq!(cat.len(), 1);
        let d = cat.lookup(NckArea::Channel, "SEMA", "actFeedRate").unwrap();
        assert_eq!(d.data_type, "F64");
        assert_eq!(d.shape, NckShape::Lines);
        assert_eq!(d.wire.module, 18);
        assert_eq!(d.supported_families, vec!["840d-sl"]);
        // 大小写敏感：变量名原样匹配。
        assert!(cat.lookup(NckArea::Channel, "SEMA", "actfeedrate").is_err());
    }

    #[test]
    fn invalid_docs_rejected() {
        assert!(NckCatalog::from_json(&serde_json::json!({})).is_err());
        assert!(NckCatalog::from_json(&serde_json::json!({"variables": [{"area": "X"}]})).is_err());
        assert!(
            NckCatalog::from_json(
                &serde_json::json!({"variables": [{"area": "C", "block": "B", "variable": "v",
                "data_type": "F64", "shape": "cube",
                "wire": {"module": 1, "column": 1, "transport_size": 1, "element_size": 1}}]})
            )
            .is_err()
        );
    }

    #[test]
    fn shipped_catalog_files_parse_and_hold_no_memory_numbers() {
        // 随仓 catalog 必须可解析；真机确认前 variables 为空（铁律）。
        for f in ["common", "840d-sl", "828d"] {
            let path = format!("{}/catalog/{f}.json", env!("CARGO_MANIFEST_DIR"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("缺 {path}"));
            let v: serde_json::Value = serde_json::from_str(&text).expect("合法 JSON");
            let cat = NckCatalog::from_json(&v).expect("schema 合法");
            assert!(cat.is_empty(), "{f}.json 真机确认前必须为空");
        }
    }
}
