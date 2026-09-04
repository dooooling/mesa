//! Event Plane 契约类型（Event Plane V1 计划 §1 / §4，PR5 冻结）。
//!
//! 最核心的冻结原则：`Data = state`，`Event = occurrence`。
//! 主轴转速 1200rpm 是 Data；`Alarm 700012 Raised → Acknowledged → Cleared`
//! 是三个 immutable occurrence，永不互相覆盖（禁止 Latest-Wins）。
//!
//! 身份双层（§2）：
//! - `event_id`：单次 occurrence 身份，Endpoint 内唯一（重放/重连去重键）；
//! - `condition_id`（`EventCondition.condition_id`）：持续性 Condition/Alarm
//!   身份，多个 EventRecord 可指向同一个 condition。
//!
//! 时间三层（§3，沿用 UTC/monotonic 分离）：
//! - `occurred_at_ns`：设备/协议真实发生时间，没有就 None，禁止伪造；
//! - publish 时间：Driver 交 SDK 时的 UTC（wire `EventBatchMsg.timestamp_ns`）；
//! - `received_at_ns`：Core 提交 EventStore 时生成（Store 层字段，不在本结构）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{DriverBinding, TaskMode, TimestampNs, Value};

/// 单个 EventRecord 序列化上限 16 KiB（§4；超限 fail-closed，禁止 truncate 冒充）。
pub const EVENT_RECORD_MAX_BYTES: usize = 16 * 1024;
/// 单个 EventBatch 上限 256 KiB（§4；IPC 帧层执行）。
pub const EVENT_BATCH_MAX_BYTES: usize = 256 * 1024;
/// attributes 字段数上限 64（§4）。
pub const EVENT_ATTRIBUTES_MAX_FIELDS: usize = 64;
/// Mesa 统一严重度上限 1000（§1；0 = 协议未提供）。
pub const EVENT_SEVERITY_MAX: u16 = 1000;

/// Condition/Alarm 生命周期跃迁（§1）。报警的完整生命周期不是"修改一条记录"，
/// 而是 Raised → Acknowledged → Cleared 各自一条 immutable occurrence。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionTransition {
    Raised,
    Updated,
    Acknowledged,
    Confirmed,
    Cleared,
}

/// 条件事件状态（§1）：ConditionId 相同的多条 EventRecord 共同描述一个
/// Condition 的生命周期；普通瞬时事件 `condition` 为 None。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventCondition {
    pub condition_id: String,
    pub transition: ConditionTransition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain: Option<bool>,
}

/// 一次"事件发生"的不可变记录（§1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    /// occurrence 稳定 ID，Endpoint 内唯一。有协议原生身份必须用原生构造；
    /// 无原生身份时 Driver 可生成 opaque ID（但跨 reconnect 去重无保证，须文档声明）。
    pub event_id: String,
    /// 大类：alarm / message / program / channel / system ……
    pub category: String,
    /// 更具体的稳定机器键，如 `alarm.condition` / `program.started`。
    pub kind: String,
    /// 事件来源，如 Channel1 / Axis.X / Server / NC。
    pub source: String,
    /// Mesa 统一严重度 0..=1000（0 = 协议未提供）。
    pub severity: u16,
    /// 协议/设备原生编码。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_locale: Option<String>,
    /// 设备/协议真正提供的发生时间；没有就 None，禁止伪造 now()。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at_ns: Option<TimestampNs>,
    /// Condition 生命周期；普通瞬时事件为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<EventCondition>,
    /// 程序运行、作业、报警族等关联键。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// 低频扩展字段：typed Value，不允许嵌套任意 JSON（§1）；
    /// Secret/password/private key 永远禁止进入（§4，Driver 侧送审）。
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
}

impl EventRecord {
    /// 结构 + 大小校验（§4；违反 → `EVENT_RECORD_INVALID` /
    /// `EVENT_RECORD_TOO_LARGE`，不能 truncate 后继续）。
    pub fn validate(&self) -> Result<(), EventRecordError> {
        if self.event_id.trim().is_empty() {
            return Err(EventRecordError::Invalid("event_id 不能为空".into()));
        }
        if self.event_id.len() > 256 {
            return Err(EventRecordError::TooLarge("event_id 超过 256 bytes".into()));
        }
        for (name, v) in [
            ("category", self.category.as_str()),
            ("kind", self.kind.as_str()),
            ("code", self.code.as_deref().unwrap_or("")),
        ] {
            if v.len() > 128 {
                return Err(EventRecordError::TooLarge(format!("{name} 超过 128 bytes")));
            }
        }
        if self.category.trim().is_empty() || self.kind.trim().is_empty() {
            return Err(EventRecordError::Invalid("category/kind 不能为空".into()));
        }
        if self.source.len() > 512 {
            return Err(EventRecordError::TooLarge("source 超过 512 bytes".into()));
        }
        if self.message.as_deref().is_some_and(|m| m.len() > 4096) {
            return Err(EventRecordError::TooLarge("message 超过 4096 bytes".into()));
        }
        if self.severity > EVENT_SEVERITY_MAX {
            return Err(EventRecordError::Invalid(format!(
                "severity {} 超出 0..=1000",
                self.severity
            )));
        }
        if self.attributes.len() > EVENT_ATTRIBUTES_MAX_FIELDS {
            return Err(EventRecordError::TooLarge(format!(
                "attributes 超过 {} fields",
                EVENT_ATTRIBUTES_MAX_FIELDS
            )));
        }
        for k in self.attributes.keys() {
            if k.len() > 128 {
                return Err(EventRecordError::TooLarge(
                    "attribute key 超过 128 bytes".into(),
                ));
            }
        }
        if let Some(c) = &self.condition
            && c.condition_id.trim().is_empty()
        {
            return Err(EventRecordError::Invalid("condition_id 不能为空".into()));
        }
        let n = serde_json::to_string(self)
            .map(|s| s.len())
            .unwrap_or(usize::MAX);
        if n > EVENT_RECORD_MAX_BYTES {
            return Err(EventRecordError::TooLarge(format!(
                "record 序列化 {n} > {EVENT_RECORD_MAX_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// 记录级校验失败（wire 原因码：`EVENT_RECORD_INVALID` / `EVENT_RECORD_TOO_LARGE`）。
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EventRecordError {
    #[error("EVENT_RECORD_INVALID: {0}")]
    Invalid(String),
    #[error("EVENT_RECORD_TOO_LARGE: {0}")]
    TooLarge(String),
}

/// 事件任务（§6）：与 AcquisitionTask 严格对等但独立——AcquisitionTask 产生
/// PointDescriptor/DataBatch，EventTask 产生 EventRecord/EventBatch。
/// Core 永不解析 binding 语义（opcua event filter / sinumerik alarm area 照旧由 Driver 解释）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventTask {
    pub id: String,
    /// 原生订阅或 Driver 内部 poll-detect。
    pub mode: TaskMode,
    /// Subscribe 可为空；Poll-backed event detector 使用。
    pub interval_ms: Option<u64>,
    pub binding: DriverBinding,
}

impl EventTask {
    /// 结构级校验（镜像 AcquisitionTask::validate；协议语义由 Driver 在
    /// configure_events 中补充校验；坏 EventTask 必须让整个 revision 失败，§35）。
    pub fn validate(&self) -> Result<(), EventTaskError> {
        if self.id.trim().is_empty() {
            return Err(EventTaskError::EmptyTaskId);
        }
        if self.mode == TaskMode::Poll && self.interval_ms.unwrap_or(0) == 0 {
            return Err(EventTaskError::PollRequiresInterval {
                task: self.id.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EventTaskError {
    #[error("event task id 不能为空")]
    EmptyTaskId,
    #[error("Poll 事件任务 `{task}` 必须提供正整数 interval_ms")]
    PollRequiresInterval { task: String },
}

/// 事件流描述（§5 EventCatalog）：Driver 声明自己有哪些事件源，供通用 UI
/// 动态渲染 EventTaskEditor（禁止 `if driver == ...` 协议分支，§27）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventStreamDescriptor {
    pub id: String,
    pub label: crate::schema::LocalizedText,
    /// 支持的采集方式：subscribe / poll。
    #[serde(default)]
    pub modes: Vec<TaskMode>,
    /// Event source/filter 参数 Schema（Core 只做 Schema 校验）。
    #[serde(default)]
    pub parameters: crate::schema::SchemaDescriptor,
    /// 可能出现的扩展字段（通用 UI 展示列）。
    #[serde(default)]
    pub fields: Vec<EventFieldDescriptor>,
}

/// 事件扩展字段描述（§5 fields）：只描述"会出现什么列"，不约束运行时载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventFieldDescriptor {
    pub key: String,
    pub label: crate::schema::LocalizedText,
    /// 取值同 DataType::as_str()（展示用，不做运行时类型强制）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_type: Option<String>,
}

/// Driver 的事件目录（§5）：挂到 DriverDescriptor.events，serde(default) 保证
/// 老 Driver（无该字段）按 `events = empty` 正常工作，Descriptor Major 不升级。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EventCatalog {
    #[serde(default)]
    pub streams: Vec<EventStreamDescriptor>,
}

impl EventCatalog {
    pub fn validate(&self) -> Result<(), String> {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for s in &self.streams {
            if s.id.trim().is_empty() {
                return Err("event stream id 不能为空".into());
            }
            if !seen.insert(&s.id) {
                return Err(format!("event stream id 重复: {}", s.id));
            }
            s.parameters.validate()?;
            let mut fseen = HashSet::new();
            for f in &s.fields {
                if f.key.trim().is_empty() {
                    return Err(format!("event stream {} field key 不能为空", s.id));
                }
                if !fseen.insert(&f.key) {
                    return Err(format!("event stream {} field key 重复: {}", s.id, f.key));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> EventRecord {
        EventRecord {
            event_id: "A700012:raised:7".into(),
            category: "alarm".into(),
            kind: "alarm.condition".into(),
            source: "Channel1".into(),
            severity: 700,
            code: Some("700012".into()),
            message: Some("overtemp".into()),
            message_locale: Some("en".into()),
            occurred_at_ns: Some(1_700_000_000_000_000_000),
            condition: Some(EventCondition {
                condition_id: "A700012".into(),
                transition: ConditionTransition::Raised,
                active: Some(true),
                acknowledged: None,
                confirmed: None,
                retain: Some(true),
            }),
            correlation_id: None,
            attributes: BTreeMap::from([("axis".into(), Value::I32(1))]),
        }
    }

    #[test]
    fn record_json_roundtrip() {
        let r = record();
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<EventRecord>(&s).unwrap(), r);
    }

    #[test]
    fn alarm_lifecycle_is_three_immutable_records() {
        // Raised → Acknowledged → Cleared 是三条独立 occurrence，不是修改同一条
        let mut ack = record();
        ack.event_id = "A700012:ack:9".into();
        ack.condition.as_mut().unwrap().transition = ConditionTransition::Acknowledged;
        ack.condition.as_mut().unwrap().acknowledged = Some(true);
        let mut clr = record();
        clr.event_id = "A700012:cleared:12".into();
        clr.condition.as_mut().unwrap().transition = ConditionTransition::Cleared;
        clr.condition.as_mut().unwrap().active = Some(false);
        for r in [&record(), &ack, &clr] {
            r.validate().unwrap();
        }
        assert_ne!(record().event_id, ack.event_id);
        assert_ne!(ack.event_id, clr.event_id);
    }

    #[test]
    fn size_limits_are_fail_closed() {
        let mut r = record();
        r.message = Some("x".repeat(4097));
        assert_eq!(
            r.validate(),
            Err(EventRecordError::TooLarge("message 超过 4096 bytes".into()))
        );
        let mut r = record();
        r.severity = 1001;
        assert!(matches!(r.validate(), Err(EventRecordError::Invalid(_))));
        let mut r = record();
        for i in 0..70 {
            r.attributes.insert(format!("k{i}"), Value::I32(i));
        }
        assert!(matches!(r.validate(), Err(EventRecordError::TooLarge(_))));
        // 16 KiB 整记录上限：大 attributes 触发
        let mut r = record();
        r.attributes
            .insert("big".into(), Value::String("y".repeat(20_000)));
        assert!(matches!(r.validate(), Err(EventRecordError::TooLarge(_))));
    }

    #[test]
    fn event_task_validate_mirrors_acquisition_task() {
        let bad = EventTask {
            id: "  ".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: "k".into(),
                config: serde_json::Value::Null,
            },
        };
        assert_eq!(bad.validate(), Err(EventTaskError::EmptyTaskId));
        let bad = EventTask {
            id: "e1".into(),
            mode: TaskMode::Poll,
            interval_ms: None,
            binding: DriverBinding {
                kind: "k".into(),
                config: serde_json::Value::Null,
            },
        };
        assert!(matches!(
            bad.validate(),
            Err(EventTaskError::PollRequiresInterval { .. })
        ));
        let ok = EventTask {
            id: "e1".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: "k".into(),
                config: serde_json::Value::Null,
            },
        };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn catalog_validates_stream_and_field_uniqueness() {
        let mut c = EventCatalog::default();
        assert!(c.validate().is_ok());
        c.streams.push(EventStreamDescriptor {
            id: "alarms".into(),
            label: crate::schema::LocalizedText {
                default: "Alarms".into(),
                zh_cn: None,
            },
            modes: vec![TaskMode::Subscribe],
            parameters: crate::schema::SchemaDescriptor::default(),
            fields: vec![EventFieldDescriptor {
                key: "code".into(),
                label: crate::schema::LocalizedText {
                    default: "Code".into(),
                    zh_cn: None,
                },
                data_type: Some("string".into()),
            }],
        });
        assert!(c.validate().is_ok());
        c.streams.push(c.streams[0].clone());
        assert!(c.validate().is_err());
    }
}
