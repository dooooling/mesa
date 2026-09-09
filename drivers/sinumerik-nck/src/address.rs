//! NCK 地址模型（ADR 0001 §9：`NckVariableRef`）。
//!
//! 用户侧是 Siemens 原生语义（Area / Area No. / Block / Variable / Line /
//! Column），不是地址字符串；线缆侧（area 索引、unit/mode→syntax）是 S7Comm
//! 协议知识。字母↔索引映射以 Wireshark `packet-s7comm.c`
//! `nck_area_names`（0=N 1=B 2=C 3=A 4=T 5=V 6=H 7=MMC）为依据；
//! unit/mode↔syntax 以 `SYNTAXID_NCK(0x82)/_METRIC(0x83)/_INCH(0x84)` 为依据。
//!
//! NOTE: MMC（索引 7）的用户侧字母待文档/真机确认，V1 不接受；线缆结构
//! 以 u8 索引承载，未来确认后只扩展字母表，不改 Resource API。

use thiserror::Error;

// ---------------------------------------------------------------------------
// NCK Area（Siemens 字母 ↔ 线缆索引）
// ---------------------------------------------------------------------------

/// NCK 区域（Siemens NC Variable List 的 N/B/C/T/A/V/H 划分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NckArea {
    /// N：NCK 系统。
    Nck,
    /// B：Mode Group（方式组）。
    ModeGroup,
    /// C：Channel（通道）。
    Channel,
    /// A：Axis（轴。注意 Siemens 字母为 A，线缆索引为 3）。
    Axis,
    /// T：Tool（刀具）。
    Tool,
    /// V：FeedDrive（进给驱动）。
    FeedDrive,
    /// H：MainDrive（主轴驱动）。
    MainDrive,
}

impl NckArea {
    /// Siemens 用户字母。
    pub fn letter(self) -> char {
        match self {
            NckArea::Nck => 'N',
            NckArea::ModeGroup => 'B',
            NckArea::Channel => 'C',
            NckArea::Axis => 'A',
            NckArea::Tool => 'T',
            NckArea::FeedDrive => 'V',
            NckArea::MainDrive => 'H',
        }
    }

    /// 线缆索引（Wireshark `nck_area_names`：N=0 B=1 C=2 A=3 T=4 V=5 H=6）。
    pub fn index(self) -> u8 {
        match self {
            NckArea::Nck => 0,
            NckArea::ModeGroup => 1,
            NckArea::Channel => 2,
            NckArea::Axis => 3,
            NckArea::Tool => 4,
            NckArea::FeedDrive => 5,
            NckArea::MainDrive => 6,
        }
    }

    /// 用户字母 → Area（大小写不敏感；`M` 归属待确认，V1 拒绝并明示）。
    pub fn parse(letter: &str) -> Result<Self, AddressError> {
        match letter.trim().to_ascii_uppercase().as_str() {
            "N" => Ok(NckArea::Nck),
            "B" => Ok(NckArea::ModeGroup),
            "C" => Ok(NckArea::Channel),
            "A" => Ok(NckArea::Axis),
            "T" => Ok(NckArea::Tool),
            "V" => Ok(NckArea::FeedDrive),
            "H" => Ok(NckArea::MainDrive),
            _ => Err(AddressError::Invalid {
                input: letter.to_string(),
                reason: "area 需为 N/B/C/A/T/V/H（M/MMC 归属待真机确认，V1 不接受）".into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Unit 模式（↔ NCK Syntax ID）
// ---------------------------------------------------------------------------

/// 单位模式 → NCK Syntax（Wireshark `SYNTAXID_NCK*`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NckUnitMode {
    /// Current（控制器当前单位）→ `0x82`。
    #[default]
    Current,
    /// Metric（公制）→ `0x83`。
    Metric,
    /// Inch（英制）→ `0x84`。
    Inch,
}

impl NckUnitMode {
    /// 线缆 Syntax ID。
    pub fn syntax_id(self) -> u8 {
        match self {
            NckUnitMode::Current => 0x82,
            NckUnitMode::Metric => 0x83,
            NckUnitMode::Inch => 0x84,
        }
    }

    pub fn parse(s: &str) -> Result<Self, AddressError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "current" => Ok(NckUnitMode::Current),
            "metric" => Ok(NckUnitMode::Metric),
            "inch" => Ok(NckUnitMode::Inch),
            _ => Err(AddressError::Invalid {
                input: s.to_string(),
                reason: "unit_mode 需为 current/metric/inch".into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// 变量引用（用户语义）
// ---------------------------------------------------------------------------

#[derive(Debug, Error, Clone, PartialEq)]
pub enum AddressError {
    #[error("空地址参数")]
    Empty,
    #[error("非法地址参数 `{input}`: {reason}")]
    Invalid { input: String, reason: String },
}

/// NCK 变量引用（Siemens 原生参数模型；`data_type` 永不在此，由 Catalog 给出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NckVariableRef {
    pub area: NckArea,
    pub area_no: Option<u16>,
    /// Block 名（如 SEMA；原样保留大小写，Siemens 大小写敏感）。
    pub block: String,
    /// 变量名（如 actFeedRate；原样保留大小写）。
    pub variable: String,
    pub line: Option<u16>,
    pub column: Option<u16>,
    /// 连续个数（line_count，缺省 1）。
    pub count: u16,
    pub unit_mode: NckUnitMode,
}

impl NckVariableRef {
    /// 内部稳定身份键（browse 节点 ID / 缓存键 / 诊断 / 测试 golden 用；
    /// 不是用户主要配置接口，配置走结构化 parameters）。
    ///
    /// 规则唯一：`nck://<AREA>/<NO>/<BLOCK>/<VAR>?line=&column=&count=&unit=`，
    /// area 取大写字母，无值字段省略（count=1、unit=current 省略）。
    pub fn canonical_key(&self) -> String {
        let mut s = format!(
            "nck://{}/{}/{}/{}",
            self.area.letter(),
            self.area_no.map(|n| n.to_string()).unwrap_or_default(),
            self.block,
            self.variable
        );
        let mut q = Vec::new();
        if let Some(l) = self.line {
            q.push(format!("line={l}"));
        }
        if let Some(c) = self.column {
            q.push(format!("column={c}"));
        }
        if self.count != 1 {
            q.push(format!("count={}", self.count));
        }
        if self.unit_mode != NckUnitMode::Current {
            let u = match self.unit_mode {
                NckUnitMode::Current => "current",
                NckUnitMode::Metric => "metric",
                NckUnitMode::Inch => "inch",
            };
            q.push(format!("unit={u}"));
        }
        if !q.is_empty() {
            s.push('?');
            s.push_str(&q.join("&"));
        }
        s
    }

    /// 从 `ResourceSelection.parameters` 解析（结构化参数，不接受字符串地址）。
    pub fn from_parameters(
        p: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, AddressError> {
        let area = p
            .get("area")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AddressError::Invalid {
                input: String::new(),
                reason: "缺少 area（N/B/C/A/T/V/H）".into(),
            })
            .and_then(NckArea::parse)?;
        let block = p
            .get("block")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AddressError::Invalid {
                input: String::new(),
                reason: "缺少 block（如 SEMA）".into(),
            })?
            .to_string();
        let variable = p
            .get("variable")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AddressError::Invalid {
                input: String::new(),
                reason: "缺少 variable（如 actFeedRate）".into(),
            })?
            .to_string();
        let opt_u16 = |key: &str| -> Result<Option<u16>, AddressError> {
            match p.get(key) {
                None => Ok(None),
                Some(v) => v
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .map(Some)
                    .ok_or_else(|| AddressError::Invalid {
                        input: key.to_string(),
                        reason: format!("`{key}` 需为 u16"),
                    }),
            }
        };
        let area_no = opt_u16("area_no")?;
        let line = opt_u16("line")?;
        let column = opt_u16("column")?;
        let count = p
            .get("count")
            .and_then(|v| v.as_u64())
            .map(|n| {
                u16::try_from(n)
                    .ok()
                    .filter(|c| *c > 0)
                    .ok_or_else(|| AddressError::Invalid {
                        input: "count".into(),
                        reason: "count 需为 1..=65535".into(),
                    })
            })
            .transpose()?
            .unwrap_or(1);
        let unit_mode = p
            .get("unit_mode")
            .and_then(|v| v.as_str())
            .map(NckUnitMode::parse)
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            area,
            area_no,
            block,
            variable,
            line,
            column,
            count,
            unit_mode,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_letters_and_indices_match_wireshark() {
        // Wireshark nck_area_names：N=0 B=1 C=2 A=3 T=4 V=5 H=6。
        for (letter, idx) in [
            ('N', 0),
            ('B', 1),
            ('C', 2),
            ('A', 3),
            ('T', 4),
            ('V', 5),
            ('H', 6),
        ] {
            let a = NckArea::parse(&letter.to_string()).expect("字母合法");
            assert_eq!(a.index(), idx);
            assert_eq!(a.letter(), letter);
        }
        // 小写容忍；M 归属未确认，拒绝。
        assert_eq!(NckArea::parse("c").unwrap(), NckArea::Channel);
        assert!(NckArea::parse("M").is_err());
        assert!(NckArea::parse("X").is_err());
    }

    #[test]
    fn unit_mode_syntax_ids_match_wireshark() {
        assert_eq!(NckUnitMode::Current.syntax_id(), 0x82);
        assert_eq!(NckUnitMode::Metric.syntax_id(), 0x83);
        assert_eq!(NckUnitMode::Inch.syntax_id(), 0x84);
    }

    fn params(json: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        json.as_object().cloned().expect("对象参数")
    }

    #[test]
    fn canonical_key_rules_are_unique() {
        // Siemens 官方示例形态：C / 1 / SEMA / actFeedRate / line=3。
        let r = NckVariableRef::from_parameters(&params(serde_json::json!({
            "area": "C", "area_no": 1, "block": "SEMA",
            "variable": "actFeedRate", "line": 3,
        })))
        .expect("示例参数合法");
        assert_eq!(r.canonical_key(), "nck://C/1/SEMA/actFeedRate?line=3");
        // 默认 count=1、unit=current 省略；metric 显式。
        let m = NckVariableRef::from_parameters(&params(serde_json::json!({
            "area": "N", "block": "N", "variable": "sysVar", "unit_mode": "metric",
        })))
        .unwrap();
        assert_eq!(m.canonical_key(), "nck://N//N/sysVar?unit=metric");
    }

    #[test]
    fn parameters_reject_missing_and_bad_shapes() {
        assert!(NckVariableRef::from_parameters(&params(serde_json::json!({}))).is_err());
        assert!(
            NckVariableRef::from_parameters(&params(serde_json::json!({
                "area": "C", "block": "", "variable": "x",
            })))
            .is_err()
        );
        assert!(
            NckVariableRef::from_parameters(&params(serde_json::json!({
                "area": "C", "block": "B", "variable": "x", "count": 0,
            })))
            .is_err()
        );
        assert!(
            NckVariableRef::from_parameters(&params(serde_json::json!({
                "area": "C", "block": "B", "variable": "x", "unit_mode": "kelvin",
            })))
            .is_err()
        );
    }
}
