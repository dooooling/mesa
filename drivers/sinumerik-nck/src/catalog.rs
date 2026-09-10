//! NCK Catalog（ADR 0001 §12：变量类型与 wire mapping 的唯一真相来源）。
//!
//! 链路：用户语义（Area/Block/Variable/Line/Column）→ Catalog →
//! `NckWireAddress`（syntax/area_unit/column/line/module/line_count）→ codec。
//! 系列 mapping 变化只换 Catalog，不动 Resource API / Core / Data Plane。
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
    #[error("未知 family `{family}`（已知：{known:?}）")]
    UnknownFamily { family: String, known: Vec<String> },
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

    /// 目录由 `MESA_NCK_CATALOG_DIR` 覆盖，缺省随 crate 源码。
    pub fn catalog_dir() -> String {
        std::env::var("MESA_NCK_CATALOG_DIR")
            .unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_string() + "/catalog")
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
            // P2b：同一文件内重复键直接拒绝（last-one-wins 会静默吞定义）；
            // 只有 `common + 所选 family` 的 merge 才允许有意的系列覆盖。
            if cat.entries.contains_key(&key) {
                return Err(CatalogError::Invalid {
                    reason: format!(
                        "variables[{i}]: 重复变量 {}/{}/{}（同文件内不允许覆盖）",
                        def.area.letter(),
                        def.block,
                        def.variable
                    ),
                });
            }
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

    /// 全变量（Area 字母 → Block → Variable 稳定排序；browse 建树用）。
    pub fn variables(&self) -> Vec<&NckVariableDefinition> {
        let mut v: Vec<_> = self.entries.values().collect();
        v.sort_by(|a, b| {
            (a.area.letter(), &a.block, &a.variable).cmp(&(b.area.letter(), &b.block, &b.variable))
        });
        v
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

    /// 校验 family 归属（P1-3：`supported_families` 真参与装载）。
    ///
    /// - `family = None`（即 `common.json`）：条目必须 family 无关
    ///   （`supported_families` 为空），否则是放错文件的系列条目；
    /// - `family = Some(f)`（系列文件）：条目必须声明归属该系列。
    pub fn validate_family_scope(&self, family: Option<&str>) -> Result<(), CatalogError> {
        for d in self.entries.values() {
            match family {
                None => {
                    if !d.supported_families.is_empty() {
                        return Err(CatalogError::Invalid {
                            reason: format!(
                                "common.json 条目 {}/{}/{} 带 supported_families（系列条目必须进系列文件）",
                                d.area.letter(),
                                d.block,
                                d.variable
                            ),
                        });
                    }
                }
                Some(f) => {
                    if !d.supported_families.iter().any(|s| s == f) {
                        return Err(CatalogError::Invalid {
                            reason: format!(
                                "{f}.json 条目 {}/{}/{} 未声明归属 {f}",
                                d.area.letter(),
                                d.block,
                                d.variable
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 系列注册表（P1-3：common + exactly one family，绝不全量合并）
// ---------------------------------------------------------------------------

/// Catalog 系列注册表：`common`（全系列无关）+ 每系列独立 catalog。
///
/// 装载即 `common + exactly one family` 合并视图；同变量在两系列不同
/// mapping 时各自独立，运行时按连接 `family` 选择，互不覆盖。
/// 未来 `family = auto`：probe 检测系列后 resolve，不一致 fail-closed。
#[derive(Debug, Clone, Default)]
pub struct CatalogRegistry {
    common: NckCatalog,
    families: HashMap<String, NckCatalog>,
}

impl CatalogRegistry {
    /// 从目录装载（`common.json` 必需；其余 `*.json` 按文件名为 family）。
    ///
    /// 缺失/非法/`supported_families` 归属不符即 fail-closed。
    pub fn load_dir(dir: &str) -> Result<Self, CatalogError> {
        let common = read_catalog_file(&format!("{dir}/common.json"))?;
        common.validate_family_scope(None)?;
        let mut families = HashMap::new();
        let entries = std::fs::read_dir(dir).map_err(|e| CatalogError::Invalid {
            reason: format!("catalog 目录不可读 {dir}: {e}"),
        })?;
        let mut names: Vec<String> = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let stem = match p.file_stem().and_then(|x| x.to_str()) {
                Some(s) if s != "common" => s.to_string(),
                _ => continue,
            };
            names.push(stem);
        }
        names.sort();
        for name in names {
            let cat = read_catalog_file(&format!("{dir}/{name}.json"))?;
            cat.validate_family_scope(Some(&name))?;
            families.insert(name, cat);
        }
        Ok(Self { common, families })
    }

    /// 装载随仓 catalog（目录规则见 `NckCatalog::catalog_dir`）。
    ///
    /// 测试不变式：本仓测试一律用 `load_dir` 直传目录，**禁止读写
    /// `MESA_NCK_CATALOG_DIR`**（进程全局 env + Rust 并行测试 = CI #115
    /// 的跨测试 race；零 mutation 构造性无 race，env 分支仅生产生效）。
    pub fn load_shipped() -> Result<Self, CatalogError> {
        Self::load_dir(&NckCatalog::catalog_dir())
    }

    /// 已知系列（排序稳定）。
    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> = self.families.keys().cloned().collect();
        v.sort();
        v
    }

    /// 解析为运行视图：`common` 叠加指定系列（系列覆盖 common 同键）。
    pub fn resolve(&self, family: &str) -> Result<NckCatalog, CatalogError> {
        let fam = self
            .families
            .get(family)
            .ok_or_else(|| CatalogError::UnknownFamily {
                family: family.to_string(),
                known: self.families(),
            })?;
        let mut cat = self.common.clone();
        cat.merge(fam.clone());
        Ok(cat)
    }
}

fn read_catalog_file(path: &str) -> Result<NckCatalog, CatalogError> {
    let text = std::fs::read_to_string(path).map_err(|e| CatalogError::Invalid {
        reason: format!("catalog 缺失 {path}: {e}"),
    })?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| CatalogError::Invalid {
        reason: format!("catalog 非法 {path}: {e}"),
    })?;
    NckCatalog::from_json(&v)
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
    // P2b：supported_families 逐项严格解析——非字符串元素（如数字）
    // 直接 Invalid，不静默丢弃（fail-closed，不猜）。
    let supported_families: Vec<String> = match e.get("supported_families") {
        None => Vec::new(),
        Some(v) => {
            let a = v.as_array().ok_or("supported_families 需为字符串数组")?;
            a.iter()
                .map(|x| {
                    x.as_str()
                        .map(|s| s.to_string())
                        .ok_or("supported_families 元素需为字符串")
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
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
        // NOTE：刻意走 `load_dir` 直传目录，不碰 `MESA_NCK_CATALOG_DIR`
        // （进程全局 env 曾导致 CI #115 并行 race；测试零 env mutation，
        // 构造性无 race，见 CatalogRegistry::load_shipped 注释）。
        let dir = format!("{}/catalog", env!("CARGO_MANIFEST_DIR"));
        for f in ["common", "840d-sl", "828d"] {
            let path = format!("{dir}/{f}.json");
            let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("缺 {path}"));
            let v: serde_json::Value = serde_json::from_str(&text).expect("合法 JSON");
            let cat = NckCatalog::from_json(&v).expect("schema 合法");
            assert!(cat.is_empty(), "{f}.json 真机确认前必须为空");
        }
        // 随仓注册表：两系列已知，空系列可解析。
        let reg = CatalogRegistry::load_dir(&dir).expect("随仓注册表合法");
        assert_eq!(reg.families(), vec!["828d", "840d-sl"]);
        assert!(reg.resolve("840d-sl").unwrap().is_empty());
    }

    fn write_catalog_dir(tag: &str, files: &[(&str, &str)]) -> String {
        let dir = std::env::temp_dir().join(format!(
            "mesa-nck-catalog-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("测试目录");
        for (name, content) in files {
            std::fs::write(dir.join(name), content).expect("测试 catalog 写入");
        }
        dir.to_string_lossy().into_owned()
    }

    fn var_entry(module: u8, family: &str) -> String {
        format!(
            r#"{{"area": "C", "block": "SEMA", "variable": "actFeedRate",
             "data_type": "F64", "shape": "lines",
             "wire": {{"module": {module}, "column": 42, "transport_size": 4, "element_size": 8}},
             "supported_families": ["{family}"]}}"#
        )
    }

    #[test]
    fn catalog_840d_and_828d_same_variable_do_not_override() {
        // P1-3 核心：同变量两系列不同 mapping，各自独立解析，互不覆盖。
        let dir = write_catalog_dir(
            "families",
            &[
                ("common.json", r#"{"variables": []}"#),
                (
                    "840d-sl.json",
                    &format!(r#"{{"variables": [{}]}}"#, var_entry(18, "840d-sl")),
                ),
                (
                    "828d.json",
                    &format!(r#"{{"variables": [{}]}}"#, var_entry(21, "828d")),
                ),
            ],
        );
        let reg = CatalogRegistry::load_dir(&dir).expect("双系列注册表合法");
        assert_eq!(reg.families(), vec!["828d", "840d-sl"]);
        let d840d = reg
            .resolve("840d-sl")
            .unwrap()
            .lookup(NckArea::Channel, "SEMA", "actFeedRate")
            .unwrap()
            .clone();
        let d828d = reg
            .resolve("828d")
            .unwrap()
            .lookup(NckArea::Channel, "SEMA", "actFeedRate")
            .unwrap()
            .clone();
        assert_eq!(d840d.wire.module, 18);
        assert_eq!(d828d.wire.module, 21, "828D 不得被 840D 覆盖");
        // 未知 family fail-closed。
        let err = reg.resolve("nope").unwrap_err();
        assert!(matches!(err, CatalogError::UnknownFamily { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn family_scope_validation_rejects_misplaced_entries() {
        // common 里放系列条目、系列文件里放未声明归属的条目，装载即拒绝。
        let bad_common = write_catalog_dir(
            "bad-common",
            &[
                (
                    "common.json",
                    &format!(r#"{{"variables": [{}]}}"#, var_entry(1, "840d-sl")),
                ),
                ("840d-sl.json", r#"{"variables": []}"#),
            ],
        );
        assert!(
            CatalogRegistry::load_dir(&bad_common).is_err(),
            "common 里的系列条目必须拒绝"
        );
        let bad_family = write_catalog_dir(
            "bad-family",
            &[
                ("common.json", r#"{"variables": []}"#),
                (
                    "840d-sl.json",
                    &format!(r#"{{"variables": [{}]}}"#, var_entry(1, "828d")),
                ),
            ],
        );
        assert!(
            CatalogRegistry::load_dir(&bad_family).is_err(),
            "未声明归属的系列条目必须拒绝"
        );
        let _ = std::fs::remove_dir_all(&bad_common);
        let _ = std::fs::remove_dir_all(&bad_family);
    }

    #[test]
    fn strict_parsing_rejects_bad_families_and_dup_keys() {
        // P2b：supported_families 非字符串元素直接 Invalid（不静默丢弃）。
        let bad_fam = serde_json::json!({"variables": [{
            "area": "C", "block": "S", "variable": "v",
            "data_type": "F64", "shape": "scalar",
            "wire": {"module": 1, "column": 0, "transport_size": 4, "element_size": 8},
            "supported_families": [123],
        }]});
        assert!(NckCatalog::from_json(&bad_fam).is_err());
        // P2b：同文件内重复键直接拒绝（只有 common+family merge 允许覆盖）。
        let good = serde_json::json!({
            "area": "C", "block": "S", "variable": "v",
            "data_type": "F64", "shape": "scalar",
            "wire": {"module": 1, "column": 0, "transport_size": 4, "element_size": 8},
        });
        let dup = serde_json::json!({"variables": [good.clone(), good]});
        let err = NckCatalog::from_json(&dup).unwrap_err();
        assert!(matches!(err, CatalogError::Invalid { .. }), "实际: {err:?}");
    }
}
