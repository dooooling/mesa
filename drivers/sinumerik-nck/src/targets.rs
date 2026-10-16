//! Siemens 专用语义 Target（**NCK Driver 侧类型**；Core 不得引用）。
//!
//! 职责（设计 §2.4.1 / 边界文档 §3.2 C1）：
//! - 承载「这次要读哪个通道 / 方式组 / 轴 / 主轴 / 驱动对象」这一**寻址坐标集合**；
//! - **合法字段组合由变量定义声明**（`VariableDefinition.targets` /
//!   `target_required`），本模块不硬编码「哪些组合合法」——因为轴位置需要
//!   `{channel, axis}`、主轴需要 `{channel, spindle}`，都是**合法的复合 Target**；
//! - 入口身份 `DriveEntry`（module 26 / 130）**必填、无默认值**：同一 `V/n`
//!   在两条入口返回不同对象名（`identity_spec.rs:190-205` 的真机证据），
//!   因此折叠成一条带默认值的定义等于凭空制造地址归属。
//!
//! **边界原文（不得改写含义）**：`{channel, axis}` 是 **NCK 读取的寻址坐标组合**，
//! **不表示**已证明该机床轴与某个 SINAMICS 驱动对象的物理对应关系——
//! `axis` 只决定 NCK 读数地址的 `line` 段，`drive_object.number` 只决定驱动访问
//! 编号 `area_no`，二者在只读途径下**没有**被证明的映射关系。
//!
//! 错误码归属（边界文档 §3.2 C1 校验表，逐条对应矩阵 #57–#62）：
//! - `target` 对象内**未知键**、字段 **JSON 类型错**（含 `drive_object.entry`
//!   取值非法）⇒ `INVALID_BINDING_CONFIG`（与 D2/D3 同口径）；
//! - 实例号越界、`drive_object.number`/`entry` **缺失**或类型错 ⇒ `INVALID_TARGET`；
//! - 缺失必填变体 / 空 target ⇒ `MISSING_TARGET`；提供未声明的变体 ⇒ `UNSUPPORTED_TARGET`。

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::compiler::CompileError;
use crate::definition::VariableDefinition;

/// 实例号合法上界（设计 §2.4.2 D4：Target 实例号 `0..=255`）。
///
/// 注意这只是 **Target 层**的上界；`area_no` 的协议上界仍是 31
/// （`native.rs::MAX_AREA_NO`）。因此 `channel=255` 能通过 Target 校验，
/// 却会在原生解析阶段以 `INVALID_VARIABLE` 被拒绝——两层各自 fail-closed，
/// 不做「自动夹到 31」这类猜测。
pub const MAX_TARGET_INSTANCE: u32 = 255;

/// `target` 对象允许的键（`TargetRef` 的字段全集）。
const TARGET_KEYS: [&str; 5] = ["channel", "mode_group", "axis", "spindle", "drive_object"];
/// `drive_object` 对象允许的键。
const DRIVE_OBJECT_KEYS: [&str; 2] = ["number", "entry"];

/// Target 变体类型：与 [`TargetRef`] 的字段一一对应，也是身份模板里的路径名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Channel,
    ModeGroup,
    Axis,
    Spindle,
    DriveObject,
}

impl TargetKind {
    /// 身份模板中的路径名（与 JSON 键名逐字一致，禁止第二套命名）。
    pub const fn as_str(self) -> &'static str {
        match self {
            TargetKind::Channel => "channel",
            TargetKind::ModeGroup => "mode_group",
            TargetKind::Axis => "axis",
            TargetKind::Spindle => "spindle",
            TargetKind::DriveObject => "drive_object",
        }
    }

    /// 路径名 → 变体（严格：不认识的路径名返回 `None`，绝不猜测）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "channel" => Some(TargetKind::Channel),
            "mode_group" => Some(TargetKind::ModeGroup),
            "axis" => Some(TargetKind::Axis),
            "spindle" => Some(TargetKind::Spindle),
            "drive_object" => Some(TargetKind::DriveObject),
            _ => None,
        }
    }

    /// 全部变体（稳定顺序，供定义自检与文档生成使用）。
    pub const ALL: [TargetKind; 5] = [
        TargetKind::Channel,
        TargetKind::ModeGroup,
        TargetKind::Axis,
        TargetKind::Spindle,
        TargetKind::DriveObject,
    ];
}

/// 驱动对象**入口身份**：`nc_variable` → module 26，`drive_object` → module 130。
///
/// 这是 26/130 的**唯一**出处（`NativeFieldSource::DriveEntryModule` 只允许
/// 调用 [`DriveEntry::module`]），并由
/// `drive_entry_module_matches_identity_spec_constants` 与
/// `identity_spec.rs` 的探针常量对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveEntry {
    NcVariable,
    DriveObject,
}

impl DriveEntry {
    /// 入口身份 → module（**不允许折叠**；两条入口必须给出不同 module）。
    pub const fn module(self) -> u8 {
        match self {
            DriveEntry::NcVariable => 26,
            DriveEntry::DriveObject => 130,
        }
    }

    /// 稳定字符串（冻结向量与身份编码共用同一词表）。
    pub const fn as_str(self) -> &'static str {
        match self {
            DriveEntry::NcVariable => "nc_variable",
            DriveEntry::DriveObject => "drive_object",
        }
    }

    /// 字符串 → 入口（严格；未知取值返回 `None`，绝不默认 module 26）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "nc_variable" => Some(DriveEntry::NcVariable),
            "drive_object" => Some(DriveEntry::DriveObject),
            _ => None,
        }
    }
}

/// 驱动对象引用：`number` 是驱动访问编号（`area_no` 来源），`entry` 必填。
///
/// `entry` 在类型层是**必填字段**（无 `Option`、无 `Default`、无 serde 默认值）；
/// 这里手写 `Deserialize`（而不是 derive）是为了让「缺 `entry`」落到
/// `INVALID_TARGET`，而「`entry` 取值非法」落到 `INVALID_BINDING_CONFIG`——
/// 两者码归属不同（边界文档 §3.2 C1 表第 1/3 条），derive 无法区分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriveObjectRef {
    pub number: u32,
    pub entry: DriveEntry,
}

impl DriveObjectRef {
    /// JSON 形态（身份编码用，键名与设计 §2.4.1 的结构逐字一致）。
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("number".into(), Value::from(self.number));
        m.insert("entry".into(), Value::from(self.entry.as_str()));
        Value::Object(m)
    }

    /// 严格解析（见类型注释的码归属）。
    pub fn from_json(value: &Value) -> Result<Self, CompileError> {
        let obj = value.as_object().ok_or_else(|| {
            CompileError::invalid_binding_config(
                "`drive_object` 需为 JSON 对象（如 {\"number\":3,\"entry\":\"nc_variable\"}）",
            )
        })?;
        for key in obj.keys() {
            if !DRIVE_OBJECT_KEYS.contains(&key.as_str()) {
                return Err(CompileError::invalid_binding_config(format!(
                    "`drive_object` 内未知字段 `{key}`"
                )));
            }
        }
        let number_raw = obj.get("number").ok_or_else(|| {
            CompileError::invalid_target("`drive_object.number` 缺失（驱动访问编号必填）")
        })?;
        let number = number_raw.as_i64().ok_or_else(|| {
            CompileError::invalid_target(format!(
                "`drive_object.number` 需为整数，实际 {number_raw}"
            ))
        })?;
        if !(0..=i64::from(MAX_TARGET_INSTANCE)).contains(&number) {
            return Err(CompileError::invalid_target(format!(
                "`drive_object.number` = {number} 越界，需 0..={MAX_TARGET_INSTANCE}"
            )));
        }
        let entry_raw = obj.get("entry").ok_or_else(|| {
            CompileError::invalid_target(
                "`drive_object.entry` 缺失：入口身份必填、无默认值（不存在「默认 module 26」的回退）",
            )
        })?;
        let entry_str = entry_raw
            .as_str()
            .ok_or_else(|| CompileError::invalid_target("`drive_object.entry` 需为字符串"))?;
        let entry = DriveEntry::parse(entry_str).ok_or_else(|| {
            CompileError::invalid_binding_config(format!(
                "`drive_object.entry` 取值 `{entry_str}` 非法（仅 nc_variable / drive_object）"
            ))
        })?;
        Ok(DriveObjectRef {
            number: number as u32,
            entry,
        })
    }
}

impl<'de> Deserialize<'de> for DriveObjectRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        DriveObjectRef::from_json(&raw).map_err(D::Error::custom)
    }
}

/// 语义 target：**按变量定义校验的寻址坐标集合**（不再是「恰好一个变体」）。
///
/// `Serialize` 时未提供的变体**不出现**（浏览/预填载荷只带已确认字段，
/// 避免「显式 null」与「未提供」在载荷层被混淆）；`Deserialize` 是手写的严格解析。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct TargetRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_group: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spindle: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_object: Option<DriveObjectRef>,
}

impl TargetRef {
    /// 严格解析：未知键 / 字段类型错 ⇒ `INVALID_BINDING_CONFIG`；
    /// 实例号越界 / `drive_object` 内部缺失或类型错 ⇒ `INVALID_TARGET`。
    ///
    /// 手写解析（而非 derive）是**必需**的：`{"channel": -1}` 与
    /// `{"channel": 4294967296}` 是「整数但无效的实例号」，必须落到
    /// `INVALID_TARGET`；若用 `Option<u32>` 的 derive，serde 会先把它们
    /// 判成类型错，从而错报 `INVALID_BINDING_CONFIG`（矩阵 #61 明确要求
    /// 这两者都是 `INVALID_TARGET`）。
    pub fn from_json(value: &Value) -> Result<Self, CompileError> {
        let obj = value
            .as_object()
            .ok_or_else(|| CompileError::invalid_binding_config("`target` 需为 JSON 对象"))?;
        for key in obj.keys() {
            if !TARGET_KEYS.contains(&key.as_str()) {
                return Err(CompileError::invalid_binding_config(format!(
                    "`target` 内未知字段 `{key}`"
                )));
            }
        }
        let drive_object = match obj.get("drive_object") {
            None | Some(Value::Null) => None,
            Some(v) => Some(DriveObjectRef::from_json(v)?),
        };
        Ok(TargetRef {
            channel: instance_of(obj, "channel")?,
            mode_group: instance_of(obj, "mode_group")?,
            axis: instance_of(obj, "axis")?,
            spindle: instance_of(obj, "spindle")?,
            drive_object,
        })
    }

    /// 已提供的变体集合 `S`（**只**依据实际给了值的字段）。
    pub fn provided_kinds(&self) -> Vec<TargetKind> {
        let mut out = Vec::new();
        for (kind, present) in [
            (TargetKind::Channel, self.channel.is_some()),
            (TargetKind::ModeGroup, self.mode_group.is_some()),
            (TargetKind::Axis, self.axis.is_some()),
            (TargetKind::Spindle, self.spindle.is_some()),
            (TargetKind::DriveObject, self.drive_object.is_some()),
        ] {
            if present {
                out.push(kind);
            }
        }
        out
    }

    /// 已提供变体的实例号。
    ///
    /// **不得**在未提供时给出默认值：`None` 的唯一含义是「用户没给」，
    /// 调用方必须把它当成错误而不是 `unwrap_or(1)`。
    pub fn instance(&self, kind: TargetKind) -> Option<u32> {
        match kind {
            TargetKind::Channel => self.channel,
            TargetKind::ModeGroup => self.mode_group,
            TargetKind::Axis => self.axis,
            TargetKind::Spindle => self.spindle,
            TargetKind::DriveObject => self.drive_object.as_ref().map(|d| d.number),
        }
    }

    /// 身份模板占位符 `{path}` 的取值（`None` = 该路径未提供，绝不编造）。
    ///
    /// `drive_object` 返回**递归对象**（`{"number":n,"entry":"…"}`），
    /// 因此「哪条入口」被编码进身份——这正是 26/130 不可折叠的身份层保证。
    pub fn value_for_path(&self, path: &str) -> Option<Value> {
        match path {
            "channel" => self.channel.map(Value::from),
            "mode_group" => self.mode_group.map(Value::from),
            "axis" => self.axis.map(Value::from),
            "spindle" => self.spindle.map(Value::from),
            "drive_object" => self.drive_object.as_ref().map(DriveObjectRef::to_json),
            _ => None,
        }
    }

    /// 按 `def.targets` / `def.target_required` 校验合法字段组合（C1 校验表）。
    ///
    /// 组合**不在代码里硬编码**：换一个定义就换一套合法组合。
    pub fn validate(&self, def: &VariableDefinition) -> Result<(), CompileError> {
        let provided = self.provided_kinds();
        // (d) 空 target ⇒ MISSING_TARGET
        if provided.is_empty() {
            return Err(CompileError::missing_target(
                &def.variable_id,
                &def.target_required,
                "target 为空对象",
            ));
        }
        // (b) 缺失必填字段 ⇒ MISSING_TARGET（含 {channel,axis} 定义只给 {channel}）
        for required in &def.target_required {
            if !provided.contains(required) {
                return Err(CompileError::missing_target(
                    &def.variable_id,
                    &def.target_required,
                    &format!("缺少必填变体 `{}`", required.as_str()),
                ));
            }
        }
        // (c) 多余变体 ⇒ UNSUPPORTED_TARGET（提供了该变量未声明的变体）
        for kind in &provided {
            if !def.targets.contains(kind) {
                return Err(CompileError::unsupported_target(
                    &def.variable_id,
                    kind.as_str(),
                    &def.targets,
                ));
            }
        }
        // (e) 实例号范围兜底：`from_json` 已判，但**手工构造**的 `TargetRef`
        // 也必须走同一判据（不做任何默认值或夹取）。
        for kind in &provided {
            if let Some(v) = self.instance(*kind)
                && v > MAX_TARGET_INSTANCE
            {
                return Err(CompileError::invalid_target(format!(
                    "`{}` = {v} 越界，需 0..={MAX_TARGET_INSTANCE}",
                    kind.as_str()
                )));
            }
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TargetRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        TargetRef::from_json(&raw).map_err(D::Error::custom)
    }
}

/// 单个实例号字段的严格解析（整数 + 范围；不做任何自动猜测）。
fn instance_of(obj: &Map<String, Value>, key: &'static str) -> Result<Option<u32>, CompileError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_i64().ok_or_else(|| {
                // JSON 类型错（字符串 / 小数 / 布尔 / 对象）⇒ D3 口径
                CompileError::invalid_binding_config(format!(
                    "`{key}` 需为整数（实例号），实际 {v}"
                ))
            })?;
            if !(0..=i64::from(MAX_TARGET_INSTANCE)).contains(&n) {
                // 整数但无效的实例号 ⇒ D7 口径
                return Err(CompileError::invalid_target(format!(
                    "`{key}` = {n} 越界，需 0..={MAX_TARGET_INSTANCE}"
                )));
            }
            Ok(Some(n as u32))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drive_entry_module_is_the_single_source_of_26_130() {
        assert_eq!(DriveEntry::NcVariable.module(), 26);
        assert_eq!(DriveEntry::DriveObject.module(), 130);
        assert_ne!(
            DriveEntry::NcVariable.module(),
            DriveEntry::DriveObject.module()
        );
        assert_eq!(
            DriveEntry::parse("nc_variable"),
            Some(DriveEntry::NcVariable)
        );
        assert_eq!(
            DriveEntry::parse("drive_object"),
            Some(DriveEntry::DriveObject)
        );
        // 未知取值绝不默认 26。
        assert_eq!(DriveEntry::parse("m130"), None);
    }

    #[test]
    fn target_json_round_trip_keeps_entry_and_is_strict() {
        let t =
            TargetRef::from_json(&json!({"drive_object": {"number": 3, "entry": "drive_object"}}))
                .unwrap();
        assert_eq!(t.instance(TargetKind::DriveObject), Some(3));
        let j = t.value_for_path("drive_object").unwrap();
        assert_eq!(j["entry"], "drive_object");
        assert_eq!(j["number"], 3);

        // 缺 entry ⇒ INVALID_TARGET（不是 INVALID_BINDING_CONFIG）
        let e = TargetRef::from_json(&json!({"drive_object": {"number": 1}})).unwrap_err();
        assert_eq!(e.code(), "INVALID_TARGET");
        // entry 取值非法 ⇒ INVALID_BINDING_CONFIG
        let e = TargetRef::from_json(&json!({"drive_object": {"number": 1, "entry": "m130"}}))
            .unwrap_err();
        assert_eq!(e.code(), "INVALID_BINDING_CONFIG");
        // 未知键 / 类型错 ⇒ INVALID_BINDING_CONFIG
        assert_eq!(
            TargetRef::from_json(&json!({"channel": 1, "tool": 2}))
                .unwrap_err()
                .code(),
            "INVALID_BINDING_CONFIG"
        );
        assert_eq!(
            TargetRef::from_json(&json!({"channel": "1"}))
                .unwrap_err()
                .code(),
            "INVALID_BINDING_CONFIG"
        );
        // 整数但越界 ⇒ INVALID_TARGET
        for bad in [json!(-1), json!(256), json!(4294967296i64)] {
            assert_eq!(
                TargetRef::from_json(&json!({"channel": bad}))
                    .unwrap_err()
                    .code(),
                "INVALID_TARGET",
                "channel={bad}"
            );
        }
        // 边界 0 / 255 合法；null 视为未提供。
        assert_eq!(
            TargetRef::from_json(&json!({"channel": 0}))
                .unwrap()
                .instance(TargetKind::Channel),
            Some(0)
        );
        assert_eq!(
            TargetRef::from_json(&json!({"channel": 255}))
                .unwrap()
                .instance(TargetKind::Channel),
            Some(255)
        );
        assert!(
            TargetRef::from_json(&json!({"channel": null}))
                .unwrap()
                .provided_kinds()
                .is_empty()
        );
    }
}
