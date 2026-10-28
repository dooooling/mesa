//! Mesa Driver IPC 协议层（方案 §14）。
//!
//! 职责：proto 生成的消息类型、Length-prefixed framing、协议版本协商、
//! 以及 core-types 与 protobuf 消息之间的双向转换。Core 与 SDK 共用本 crate，
//! 保证两侧编解码路径完全一致。

/// prost 生成的类型。字段命名与 proto/driver.proto 一一对应。
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/mesa.driver.v1.rs"));
}

use bytes::{BufMut, BytesMut};
use mesa_core_types::{
    AcquisitionTask, ConditionTransition, ConnectionState, DataBatch, DataType, DriverBinding,
    ErrorKind, EventCondition, EventRecord, EventTask, PointDescriptor, PointValue, Quality,
    TaskMode, TaskSchedule, UnknownDataType, Value, ValueOrigin,
};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// IPC 协议版本。Major 不兼容直接拒绝握手；Minor 取双方较小值。
/// V1.2.1 新增 PointValue.value_origin（§5.5），Minor 1 保证新 Driver 的 typed BAD 语义可被新 Core 理解，旧端仍按 UNSPECIFIED 兼容解释
pub const PROTOCOL_MAJOR: u32 = 1;
/// Minor 8（Browse 选择契约）：`BrowseNode` 增加 `selection_mode`(8) /
/// `selection_payload_json`(9) / `diagnostics`(10)，并新增
/// `BrowseSelectionMode` / `BrowseDiagnosticSeverity` / `BrowseSelectionDiagnostic`。
///
/// 纯 additive：既有 39 个 `oneof body` 成员（tag 10–48）与全部既有 message
/// 字段编号/类型逐字不动，`PROTOCOL_MAJOR` 仍为 1。旧端（Minor 7）读到新增
/// 字段会**忽略**（protobuf 前向兼容），因此无需发送前门控：新增字段只在
/// 响应方向出现，没有新增请求消息。旧驱动不填新字段 ⇒ `selection_mode = 0`
///（UNSPECIFIED），消费方按 1.7 的既有行为处理。
///
/// 历史（供协商排障）：Minor 7 = I-2 定义目录 / 权威身份预览；
/// Minor 6 = Foundation-3 structured WriteTarget 三元组；
/// Minor 5 = TaskSchedule 四字段；Minor 3 = Event Plane；Minor 2 = Probe RPC。
pub const PROTOCOL_MINOR: u32 = 8;

/// Dynamic Probe RPC 可用的最低协商 Minor（§8）。协商 Minor < 2 的旧 Driver
/// 不识别 ProbeRequest（会静默忽略），Core 必须直接返回 Unsupported，
/// 不得发 RPC 干等超时。
pub const PROBE_RPC_MIN_MINOR: u32 = 2;

/// Event Plane 可用的最低协商 Minor（Event V1 §10，纯 additive 1.2 → 1.3）。
/// EventTask 存在但 negotiated_minor < 3 时，Core 必须回 EVENT_PLANE_UNSUPPORTED，
/// 不得发未知消息干等 timeout。
pub const EVENT_PLANE_MIN_MINOR: u32 = 3;

/// TaskSchedule wire 支持的最低协商 Minor（Foundation-2，IPC 1.5）。
/// negotiated_minor < 5 的旧端不识别 Subscribe 四字段：Core 侧
/// `tasks_to_pb` 失败即整批拒绝（fail-closed，不静默降级为缺省调度）。
pub const TASK_SCHEDULE_MIN_MINOR: u32 = 5;

/// Structured WriteTarget wire 支持的最低协商 Minor（Foundation-3，IPC 1.6）。
/// negotiated_minor < 6 的旧端不识别三元组字段：Core 侧直接
/// `CONTROL_MODEL_UNSUPPORTED` fail-closed，不 fallback 旧 `target` 字符串。
pub const CONTROL_MODEL_MIN_MINOR: u32 = 6;

/// 定义目录 / 权威身份预览 RPC 可用的最低协商 Minor（I-2，IPC 1.7）。
///
/// negotiated_minor < 7 的旧端不识别 tag 45–48，其兜底分支会**静默忽略**
/// 未知消息（`driver-sdk` 的 `unexpected envelope ignored`）⇒ 调用方会白等
/// 一个完整 RPC 预算才失败。因此调用方（Manager）**必须先门控再发帧**：
/// 门控不通过就直接返回 `DEFINITIONS_UNSUPPORTED`（`ErrorKind::Unsupported`，
/// HTTP 501），既不排队也不超时。
pub const DEFINITIONS_RPC_MIN_MINOR: u32 = 7;

/// 定义目录 / 预览 RPC 的版本门控（**纯函数**，无任何运行期状态）。
///
/// 与 [`PROBE_RPC_MIN_MINOR`] 同族的 `>=` 语义：协商值来自 `negotiate()` 的
/// 双方较小值，因此它同时代表「对端会不会识别这两对消息」。
///
/// 注意（设计口径）：本函数**只做版本判定**。并发预算（每 driver 4 个
/// in-flight）与许可释放属 Manager 的运行期职责，**不得**放到本 crate
/// （`driver-protocol` 不持有计数器 / in-flight 状态）。
pub fn definitions_query_supported(negotiated_minor: u32) -> bool {
    negotiated_minor >= DEFINITIONS_RPC_MIN_MINOR
}

/// Browse 选择契约（`BrowseNode.selection_mode` / `selection_payload_json` /
/// `diagnostics`）引入的最小协商 Minor（IPC 1.8）。
///
/// 与 [`DEFINITIONS_RPC_MIN_MINOR`] 的**关键差别**：I-2 那两对是**请求**消息，
/// 旧端不识别就静默忽略 ⇒ 发送方必须先门控再发帧；本契约只出现在**响应**
/// 字段里，protobuf 前向兼容保证旧端会忽略未知字段，因此**没有**「必须先
/// 门控」的调用点。
///
/// 保留常量与纯函数的原因：给「谁需要判断对端会不会填这些字段」的地方一个
/// 唯一真值锚点（当前是契约测试与排障；Core 的 REST 透传不依赖它——它对
/// `selection_mode = 0` 与旧驱动一视同仁）。
pub const BROWSE_SELECTION_MIN_MINOR: u32 = 8;

/// 选择契约是否可用（**纯函数**，与 [`definitions_query_supported`] 同族语义）。
pub fn browse_selection_supported(negotiated_minor: u32) -> bool {
    negotiated_minor >= BROWSE_SELECTION_MIN_MINOR
}

/// 单帧上限。防止恶意/异常长度前缀导致无界分配（有界原则在 IPC 层的体现）。
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

/// Browse 选择契约（IPC 1.8）的**构造器**。
///
/// 为什么放在 `driver-protocol`：这三个字段是 Mesa 的 wire 契约，与任何厂商都
/// 无关。构造器集中在这里，驱动只需回答「这个节点是哪一种」以及「理由是什么」，
/// 不必各自手写 `i32` 枚举值与诊断元组——那正是字段语义漂移的入口。
///
/// 纪律：
/// - **模式与诊断必须同源产生**（见 [`Selection`]）：不允许出现「声明可选但
///   没有载荷」或「声明不可选却没给理由」的组合；
/// - 提示级诊断（INFO/WARNING）**永不**改变可选性——可选性只有 `mode` 说了算；
/// - 本轮只提供 4 种真实形态的构造器（未声明 / 不可选 / 语义 / 资源），
///   没有 `unspecified()`：旧驱动不填就是「不填」，不需要构造器帮忙。
pub mod browse_selection {
    use crate::pb::{BrowseDiagnosticSeverity, BrowseSelectionDiagnostic, BrowseSelectionMode};

    /// 选择契约三字段的构造结果（与 `BrowseNode` 的 8/9/10 号字段一一对应）。
    ///
    /// 驱动侧**逐字段显式赋值**而不是 `..Default::default()`：协议新增字段时，
    /// 编译错误会逼着调用点重新表态，而不是悄悄沿用零值。
    pub struct Selection {
        pub mode: BrowseSelectionMode,
        pub payload_json: String,
        pub diagnostics: Vec<BrowseSelectionDiagnostic>,
    }

    /// 构造一条诊断（`severity` 用 proto 枚举，避免各处手写数字）。
    pub fn diagnostic(
        code: &str,
        message: impl Into<String>,
        severity: BrowseDiagnosticSeverity,
    ) -> BrowseSelectionDiagnostic {
        BrowseSelectionDiagnostic {
            code: code.to_string(),
            message: message.into(),
            severity: severity as i32,
        }
    }

    /// 分支 / 诊断节点：**明确不可选**，且不需要额外理由。
    ///
    /// 为什么不用 `UNSPECIFIED`：这两类节点永远不该被选用。由驱动把这件事说
    /// 清楚，消费端就不必靠 `has_children` / 空 `binding_json` 之类的形态去猜。
    /// 空诊断 = 「无理由的不可选」，用户面不应出现红色阻断块。
    pub fn not_selectable() -> Selection {
        Selection {
            mode: BrowseSelectionMode::NotSelectable,
            payload_json: String::new(),
            diagnostics: Vec::new(),
        }
    }

    /// 被准入规则拒绝的叶：不可选，且必须给出**稳定码 + 可读文案**。
    ///
    /// `payload_json` 保留（通常与 `binding_json` 同内容）：旧客户端仍按旧形态
    /// 消费；新客户端只认 `NOT_SELECTABLE` + 诊断，载荷不构成可选许可。
    pub fn rejected(
        payload_json: impl Into<String>,
        code: &str,
        message: impl Into<String>,
    ) -> Selection {
        Selection {
            mode: BrowseSelectionMode::NotSelectable,
            payload_json: payload_json.into(),
            diagnostics: vec![diagnostic(code, message, BrowseDiagnosticSeverity::Error)],
        }
    }

    /// 可选用的语义叶：载荷只对该驱动自己的语义层有意义，Core 与通用 UI 不解释。
    pub fn semantic(
        payload_json: impl Into<String>,
        diagnostics: Vec<BrowseSelectionDiagnostic>,
    ) -> Selection {
        Selection {
            mode: BrowseSelectionMode::Semantic,
            payload_json: payload_json.into(),
            diagnostics,
        }
    }

    /// 可选用的通用资源叶：载荷是 Mesa 通用资源选择（`resource_id` + `parameters`）。
    pub fn resource(payload_json: impl Into<String>) -> Selection {
        Selection {
            mode: BrowseSelectionMode::Resource,
            payload_json: payload_json.into(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame too large: {size} bytes (max {max})")]
    FrameTooLarge { size: u64, max: u32 },
    #[error("protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("protobuf encode error: {0}")]
    Encode(#[from] prost::EncodeError),
}

/// 写入一个完整帧：4 字节小端长度前缀 + protobuf 编码体。
pub async fn write_envelope<W: AsyncWrite + Unpin + Send>(
    w: &mut W,
    env: &pb::Envelope,
) -> Result<(), ProtocolError> {
    let mut buf = BytesMut::with_capacity(env.encoded_len() + 4);
    // put_u32_le 写入长度占位，再拼接编码体；encoded_len 由 prost 精确给出
    buf.put_u32_le(env.encoded_len() as u32);
    env.encode(&mut buf)?;
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

/// 读取一个完整帧并解码。长度超过 [`MAX_FRAME_LEN`] 时立即报错，不预分配内存。
pub async fn read_envelope<R: AsyncRead + Unpin + Send>(
    r: &mut R,
) -> Result<pb::Envelope, ProtocolError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge {
            size: len as u64,
            max: MAX_FRAME_LEN,
        });
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok(pb::Envelope::decode(body.as_slice())?)
}

/// 版本协商（§14.3）：Major 必须相等，否则拒绝；Minor 取双方较小值，
/// 保证两端都只使用彼此都支持的特性集。
pub fn negotiate(driver: (u32, u32), core: (u32, u32)) -> Result<(u32, u32), IncompatibleProtocol> {
    if driver.0 != core.0 {
        return Err(IncompatibleProtocol {
            driver_major: driver.0,
            core_major: core.0,
        });
    }
    Ok((core.0, driver.1.min(core.1)))
}

#[derive(Debug, thiserror::Error)]
#[error("incompatible protocol major: driver={driver_major}, core={core_major}")]
pub struct IncompatibleProtocol {
    pub driver_major: u32,
    pub core_major: u32,
}

// ---------------------------------------------------------------------------
// core-types <-> protobuf 转换
//
// 转换失败集中在 Decode 类错误：缺省 oneof、非法枚举字符串等属于对端违约，
// 上层应按 DecodeError 处理而不是静默吞掉。
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("value message has empty oneof")]
    EmptyValue,
    #[error(transparent)]
    DataType(#[from] UnknownDataType),
    #[error("invalid mode: {0}")]
    InvalidMode(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("invalid quality: {0}")]
    InvalidQuality(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid event task: {0}")]
    InvalidEventTask(String),
    #[error("invalid event record: {0}")]
    InvalidEventRecord(String),
    #[error("event batch too large: {0}")]
    EventBatchTooLarge(String),
}

pub fn value_to_pb(v: &Value) -> pb::ValueMsg {
    let kind = match v {
        Value::Bool(x) => pb::value_msg::Kind::BoolV(*x),
        Value::I32(x) => pb::value_msg::Kind::I32V(*x),
        Value::U32(x) => pb::value_msg::Kind::U32V(*x),
        Value::I64(x) => pb::value_msg::Kind::I64V(*x),
        Value::U64(x) => pb::value_msg::Kind::U64V(*x),
        Value::F32(x) => pb::value_msg::Kind::F32V(*x),
        Value::F64(x) => pb::value_msg::Kind::F64V(*x),
        Value::String(x) => pb::value_msg::Kind::StringV(x.clone()),
        Value::Bytes(x) => pb::value_msg::Kind::BytesV(x.clone()),
        Value::DateTime(ns) => pb::value_msg::Kind::DatetimeNsV(*ns),
        Value::BoolArray(xs) => pb::value_msg::Kind::BoolArrayV(pb::BoolArrayV { v: xs.clone() }),
        Value::I32Array(xs) => pb::value_msg::Kind::I32ArrayV(pb::I32ArrayV { v: xs.clone() }),
        Value::U32Array(xs) => pb::value_msg::Kind::U32ArrayV(pb::U32ArrayV { v: xs.clone() }),
        Value::I64Array(xs) => pb::value_msg::Kind::I64ArrayV(pb::I64ArrayV { v: xs.clone() }),
        Value::U64Array(xs) => pb::value_msg::Kind::U64ArrayV(pb::U64ArrayV { v: xs.clone() }),
        Value::F32Array(xs) => pb::value_msg::Kind::F32ArrayV(pb::F32ArrayV { v: xs.clone() }),
        Value::F64Array(xs) => pb::value_msg::Kind::F64ArrayV(pb::F64ArrayV { v: xs.clone() }),
        Value::StringArray(xs) => {
            pb::value_msg::Kind::StringArrayV(pb::StringArrayV { v: xs.clone() })
        }
        Value::DateTimeArray(xs) => {
            pb::value_msg::Kind::DatetimeArrayV(pb::DateTimeArrayV { v: xs.clone() })
        }
    };
    pb::ValueMsg { kind: Some(kind) }
}

pub fn value_from_pb(v: pb::ValueMsg) -> Result<Value, ConvertError> {
    use pb::value_msg::Kind;
    match v.kind {
        None => Err(ConvertError::EmptyValue),
        Some(Kind::BoolV(x)) => Ok(Value::Bool(x)),
        Some(Kind::I32V(x)) => Ok(Value::I32(x)),
        Some(Kind::U32V(x)) => Ok(Value::U32(x)),
        Some(Kind::I64V(x)) => Ok(Value::I64(x)),
        Some(Kind::U64V(x)) => Ok(Value::U64(x)),
        Some(Kind::F32V(x)) => Ok(Value::F32(x)),
        Some(Kind::F64V(x)) => Ok(Value::F64(x)),
        Some(Kind::StringV(x)) => Ok(Value::String(x)),
        Some(Kind::BytesV(x)) => Ok(Value::Bytes(x)),
        Some(Kind::DatetimeNsV(x)) => Ok(Value::DateTime(x)),
        Some(Kind::BoolArrayV(a)) => Ok(Value::BoolArray(a.v)),
        Some(Kind::I32ArrayV(a)) => Ok(Value::I32Array(a.v)),
        Some(Kind::U32ArrayV(a)) => Ok(Value::U32Array(a.v)),
        Some(Kind::I64ArrayV(a)) => Ok(Value::I64Array(a.v)),
        Some(Kind::U64ArrayV(a)) => Ok(Value::U64Array(a.v)),
        Some(Kind::F32ArrayV(a)) => Ok(Value::F32Array(a.v)),
        Some(Kind::F64ArrayV(a)) => Ok(Value::F64Array(a.v)),
        Some(Kind::StringArrayV(a)) => Ok(Value::StringArray(a.v)),
        Some(Kind::DatetimeArrayV(a)) => Ok(Value::DateTimeArray(a.v)),
    }
}

/// 线路上空字符串即 GOOD 的编码约定（proto3 无枚举默认值的简洁表达）。
pub fn quality_to_pb(q: Quality) -> String {
    q.as_str().to_string()
}

pub fn quality_from_pb(s: &str) -> Result<Quality, ConvertError> {
    match s {
        "" | "GOOD" => Ok(Quality::Good),
        "UNCERTAIN" => Ok(Quality::Uncertain),
        "BAD" => Ok(Quality::Bad),
        other => Err(ConvertError::InvalidQuality(other.to_string())),
    }
}

pub fn value_origin_to_pb(vo: ValueOrigin) -> i32 {
    match vo {
        ValueOrigin::Unspecified => pb::ValueOrigin::Unspecified as i32,
        ValueOrigin::Current => pb::ValueOrigin::Current as i32,
        ValueOrigin::LastKnown => pb::ValueOrigin::LastKnown as i32,
        ValueOrigin::Placeholder => pb::ValueOrigin::Placeholder as i32,
    }
}

pub fn value_origin_from_pb(v: i32) -> ValueOrigin {
    match pb::ValueOrigin::try_from(v).unwrap_or(pb::ValueOrigin::Unspecified) {
        pb::ValueOrigin::Unspecified => ValueOrigin::Unspecified,
        pb::ValueOrigin::Current => ValueOrigin::Current,
        pb::ValueOrigin::LastKnown => ValueOrigin::LastKnown,
        pb::ValueOrigin::Placeholder => ValueOrigin::Placeholder,
    }
}

pub fn point_value_to_pb(pv: &PointValue) -> pb::PointValueMsg {
    // 新 Driver 禁止发送 UNSPECIFIED；若误传则按兼容规则归一化后再编码，避免对端歧义
    let normalized_origin = pv.value_origin.normalize_unspecified(pv.quality);
    pb::PointValueMsg {
        point_id: pv.point_id,
        value: Some(value_to_pb(&pv.value)),
        quality: quality_to_pb(pv.quality),
        quality_code: pv.quality_code,
        source_timestamp_ns: pv.source_timestamp_ns,
        value_origin: value_origin_to_pb(normalized_origin),
    }
}

pub fn point_value_from_pb(pv: pb::PointValueMsg) -> Result<PointValue, ConvertError> {
    let quality = quality_from_pb(&pv.quality)?;
    let raw_origin = value_origin_from_pb(pv.value_origin);
    // 线上传输 UNSPECIFIED 按 §5.5 兼容解释：GOOD→Current / 非GOOD→Placeholder
    let value_origin = raw_origin.normalize_unspecified(quality);
    Ok(PointValue {
        point_id: pv.point_id,
        value: value_from_pb(pv.value.ok_or(ConvertError::EmptyValue)?)?,
        quality,
        quality_code: pv.quality_code,
        source_timestamp_ns: pv.source_timestamp_ns,
        value_origin,
    })
}

pub fn batch_to_pb(b: &DataBatch) -> pb::DataBatchMsg {
    pb::DataBatchMsg {
        connection_handle: b.connection_handle,
        stream_epoch: b.stream_epoch,
        sequence: b.sequence,
        timestamp_ns: b.timestamp_ns,
        values: b.values.iter().map(point_value_to_pb).collect(),
        mono_ns: b.mono_ns,
    }
}

pub fn batch_from_pb(b: pb::DataBatchMsg) -> Result<DataBatch, ConvertError> {
    let values = b
        .values
        .into_iter()
        .map(point_value_from_pb)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DataBatch {
        connection_handle: b.connection_handle,
        stream_epoch: b.stream_epoch,
        sequence: b.sequence,
        timestamp_ns: b.timestamp_ns,
        values,
        mono_ns: b.mono_ns,
    })
}

/// Condition 跃迁的 wire 字符串（serde snake_case 形态复用，与 JSON 一致）。
fn transition_to_str(t: ConditionTransition) -> &'static str {
    match t {
        ConditionTransition::Raised => "raised",
        ConditionTransition::Updated => "updated",
        ConditionTransition::Acknowledged => "acknowledged",
        ConditionTransition::Confirmed => "confirmed",
        ConditionTransition::Cleared => "cleared",
    }
}

fn transition_from_str(s: &str) -> Result<ConditionTransition, ConvertError> {
    match s {
        "raised" => Ok(ConditionTransition::Raised),
        "updated" => Ok(ConditionTransition::Updated),
        "acknowledged" => Ok(ConditionTransition::Acknowledged),
        "confirmed" => Ok(ConditionTransition::Confirmed),
        "cleared" => Ok(ConditionTransition::Cleared),
        other => Err(ConvertError::InvalidEventRecord(format!(
            "transition 非法: {other}"
        ))),
    }
}

pub fn event_task_to_pb(t: &EventTask) -> Result<pb::EventTaskProto, ConvertError> {
    Ok(pb::EventTaskProto {
        id: t.id.clone(),
        mode: t.mode.as_str().to_string(),
        interval_ms: t.interval_ms,
        binding_kind: t.binding.kind.clone(),
        binding_config_json: serde_json::to_string(&t.binding.config)?,
    })
}

pub fn event_task_from_pb(t: pb::EventTaskProto) -> Result<EventTask, ConvertError> {
    let mode = match t.mode.as_str() {
        "poll" => TaskMode::Poll,
        "subscribe" => TaskMode::Subscribe,
        other => return Err(ConvertError::InvalidMode(other.to_string())),
    };
    let task = EventTask {
        id: t.id,
        mode,
        interval_ms: t.interval_ms,
        binding: DriverBinding {
            kind: t.binding_kind,
            config: serde_json::from_str(&t.binding_config_json)?,
        },
    };
    task.validate()
        .map_err(|e| ConvertError::InvalidEventTask(e.to_string()))?;
    Ok(task)
}

pub fn event_record_to_pb(e: &EventRecord) -> Result<pb::EventRecordMsg, ConvertError> {
    e.validate()
        .map_err(|err| ConvertError::InvalidEventRecord(err.to_string()))?;
    Ok(pb::EventRecordMsg {
        event_id: e.event_id.clone(),
        category: e.category.clone(),
        kind: e.kind.clone(),
        source: e.source.clone(),
        severity: u32::from(e.severity),
        code: e.code.clone(),
        message: e.message.clone(),
        message_locale: e.message_locale.clone(),
        occurred_at_ns: e.occurred_at_ns,
        condition: e.condition.as_ref().map(|c| pb::EventConditionMsg {
            condition_id: c.condition_id.clone(),
            transition: transition_to_str(c.transition).to_string(),
            active: c.active,
            acknowledged: c.acknowledged,
            confirmed: c.confirmed,
            retain: c.retain,
        }),
        correlation_id: e.correlation_id.clone(),
        attributes: e
            .attributes
            .iter()
            .map(|(k, v)| pb::EventAttributeMsg {
                key: k.clone(),
                value: Some(value_to_pb(v)),
            })
            .collect(),
    })
}

pub fn event_record_from_pb(e: pb::EventRecordMsg) -> Result<EventRecord, ConvertError> {
    let mut attributes = std::collections::BTreeMap::new();
    for a in e.attributes {
        // P5-review：重复 key 静默覆盖等于丢失历史事实，必须整条拒收
        if attributes.contains_key(&a.key) {
            return Err(ConvertError::InvalidEventRecord(format!(
                "duplicate attribute key: {}",
                a.key
            )));
        }
        let v = value_from_pb(a.value.ok_or(ConvertError::EmptyValue)?)?;
        attributes.insert(a.key, v);
    }
    let condition = e
        .condition
        .map(|c| {
            Ok::<_, ConvertError>(EventCondition {
                condition_id: c.condition_id,
                transition: transition_from_str(&c.transition)?,
                active: c.active,
                acknowledged: c.acknowledged,
                confirmed: c.confirmed,
                retain: c.retain,
            })
        })
        .transpose()?;
    // P5-review：severity 截断等于把非法输入洗成合法数据；
    // 溢出必须拒收，再由 validate() 检查 0..=1000
    let severity = u16::try_from(e.severity).map_err(|_| {
        ConvertError::InvalidEventRecord(format!("severity overflow: {}", e.severity))
    })?;
    let rec = EventRecord {
        event_id: e.event_id,
        category: e.category,
        kind: e.kind,
        source: e.source,
        severity,
        code: e.code,
        message: e.message,
        message_locale: e.message_locale,
        occurred_at_ns: e.occurred_at_ns,
        condition,
        correlation_id: e.correlation_id,
        attributes,
    };
    rec.validate()
        .map_err(|err| ConvertError::InvalidEventRecord(err.to_string()))?;
    Ok(rec)
}

/// 整批事件转换（§11 顺序契约由发送侧保证；此处只做逐条校验 + 256 KiB 上限，
/// 任一非法即整批拒收，不收半批）。形态镜像 DataBatch：header 元数据完整保留，
/// PR6 ingress 依赖 connection_handle（找 Endpoint）/stream_epoch（stale 门）
/// /sequence（gap 门）/timestamp_ns/mono_ns，一律不丢。
pub fn event_batch_to_pb(
    batch: &mesa_core_types::EventBatch,
) -> Result<pb::EventBatchMsg, ConvertError> {
    let wire = pb::EventBatchMsg {
        connection_handle: batch.connection_handle,
        stream_epoch: batch.stream_epoch,
        sequence: batch.sequence,
        timestamp_ns: batch.timestamp_ns,
        mono_ns: batch.mono_ns,
        events: batch
            .events
            .iter()
            .map(event_record_to_pb)
            .collect::<Result<Vec<_>, _>>()?,
    };
    check_event_batch_size(&wire)?;
    Ok(wire)
}

pub fn event_batch_from_pb(
    b: pb::EventBatchMsg,
) -> Result<mesa_core_types::EventBatch, ConvertError> {
    check_event_batch_size(&b)?;
    let events = b
        .events
        .into_iter()
        .map(event_record_from_pb)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(mesa_core_types::EventBatch {
        connection_handle: b.connection_handle,
        stream_epoch: b.stream_epoch,
        sequence: b.sequence,
        timestamp_ns: b.timestamp_ns,
        events,
        mono_ns: b.mono_ns,
    })
}

fn check_event_batch_size(b: &pb::EventBatchMsg) -> Result<(), ConvertError> {
    use prost::Message;
    let n = b.encoded_len();
    if n > mesa_core_types::EVENT_BATCH_MAX_BYTES {
        return Err(ConvertError::EventBatchTooLarge(format!(
            "event batch {n} > {} bytes",
            mesa_core_types::EVENT_BATCH_MAX_BYTES
        )));
    }
    Ok(())
}

pub fn task_to_pb(t: &AcquisitionTask) -> Result<pb::AcquisitionTaskProto, ConvertError> {
    // Foundation-2 wire 形态（IPC 1.5）：Poll 传 interval_ms；
    // Subscribe 传四参数完整 roundtrip（缺一不可，不做缺省回填——
    // 缺字段的 wire 即违约，由 task_from_pb 拒绝）。
    let (interval_ms, publishing_interval_ms, sampling_interval_ms, queue_size, discard_oldest) =
        match t.schedule {
            TaskSchedule::Poll { interval_ms } => (Some(interval_ms), None, None, None, None),
            TaskSchedule::Subscribe {
                publishing_interval_ms,
                sampling_interval_ms,
                queue_size,
                discard_oldest,
            } => (
                None,
                Some(publishing_interval_ms),
                Some(sampling_interval_ms),
                Some(queue_size),
                Some(discard_oldest),
            ),
        };
    Ok(pb::AcquisitionTaskProto {
        id: t.id.clone(),
        mode: t.mode().as_str().to_string(),
        interval_ms,
        publishing_interval_ms,
        sampling_interval_ms,
        queue_size,
        discard_oldest,
        binding_kind: t.binding.kind.clone(),
        binding_config_json: serde_json::to_string(&t.binding.config)?,
    })
}

pub fn task_from_pb(t: pb::AcquisitionTaskProto) -> Result<AcquisitionTask, ConvertError> {
    let mode = match t.mode.as_str() {
        "poll" => TaskMode::Poll,
        "subscribe" => TaskMode::Subscribe,
        other => return Err(ConvertError::InvalidMode(other.to_string())),
    };
    let schedule = match mode {
        TaskMode::Poll => {
            let interval_ms = t.interval_ms.unwrap_or(0);
            if interval_ms == 0 {
                return Err(ConvertError::InvalidMode(
                    "poll task 缺少正整数 interval_ms".into(),
                ));
            }
            TaskSchedule::Poll { interval_ms }
        }
        TaskMode::Subscribe => {
            // 四参数缺一即违约（零值同样拒绝：validate 与 Driver 双重确认，
            // 此处先拦 wire 形态，避免 0 悄悄进调度）。
            let (
                Some(publishing_interval_ms),
                Some(sampling_interval_ms),
                Some(queue_size),
                Some(discard_oldest),
            ) = (
                t.publishing_interval_ms,
                t.sampling_interval_ms,
                t.queue_size,
                t.discard_oldest,
            )
            else {
                return Err(ConvertError::InvalidMode(
                    "subscribe task 缺少完整调度参数 (publishing/sampling/queue/discard_oldest)"
                        .into(),
                ));
            };
            if publishing_interval_ms == 0 || sampling_interval_ms == 0 || queue_size == 0 {
                return Err(ConvertError::InvalidMode(
                    "subscribe task 调度参数需 >0".into(),
                ));
            }
            TaskSchedule::Subscribe {
                publishing_interval_ms,
                sampling_interval_ms,
                queue_size,
                discard_oldest,
            }
        }
    };
    Ok(AcquisitionTask {
        id: t.id,
        schedule,
        binding: DriverBinding {
            kind: t.binding_kind,
            config: serde_json::from_str(&t.binding_config_json)?,
        },
    })
}

pub fn tasks_to_pb(
    tasks: &[AcquisitionTask],
) -> Result<Vec<pb::AcquisitionTaskProto>, ConvertError> {
    tasks.iter().map(task_to_pb).collect()
}

pub fn tasks_from_pb(
    tasks: Vec<pb::AcquisitionTaskProto>,
) -> Result<Vec<AcquisitionTask>, ConvertError> {
    tasks.into_iter().map(task_from_pb).collect()
}

pub fn data_type_to_pb(dt: DataType) -> String {
    dt.as_str().to_string()
}

pub fn data_type_from_pb(s: &str) -> Result<DataType, ConvertError> {
    s.parse::<DataType>().map_err(Into::into)
}

pub fn descriptor_from_pb(d: pb::PointDescriptorProto) -> Result<PointDescriptor, ConvertError> {
    Ok(PointDescriptor {
        point_key: d.point_key,
        data_type: data_type_from_pb(&d.data_type)?,
        unit: d.unit,
        source_label: d.source_label,
    })
}

pub fn connection_state_from_pb(s: &str) -> Result<ConnectionState, ConvertError> {
    match s {
        "STOPPED" => Ok(ConnectionState::Stopped),
        "CONNECTING" => Ok(ConnectionState::Connecting),
        "RUNNING" => Ok(ConnectionState::Running),
        "RECONNECTING" => Ok(ConnectionState::Reconnecting),
        "FAILED" => Ok(ConnectionState::Failed),
        other => Err(ConvertError::InvalidState(other.to_string())),
    }
}

pub fn ok_result() -> pb::GenericResult {
    pb::GenericResult {
        ok: true,
        error: None,
    }
}

pub fn err_result(kind: ErrorKind, code: &str, message: impl Into<String>) -> pb::GenericResult {
    pb::GenericResult {
        ok: false,
        error: Some(error_detail(kind, code, message)),
    }
}

pub fn error_detail(kind: ErrorKind, code: &str, message: impl Into<String>) -> pb::ErrorDetail {
    pb::ErrorDetail {
        kind: kind.as_str().to_string(),
        code: code.to_string(),
        message: message.into(),
    }
}

/// 从 GenericResult 还原错误；ok=true 返回 None。
pub fn result_into_error(r: pb::GenericResult) -> Option<pb::ErrorDetail> {
    if r.ok {
        None
    } else {
        r.error.or_else(|| {
            // 对端违约：ok=false 却未附错误详情，按 Internal 兜底
            Some(error_detail(
                ErrorKind::Internal,
                "",
                "peer returned failure without detail",
            ))
        })
    }
}

impl From<&pb::ErrorDetail> for ErrorDetailBox {
    fn from(d: &pb::ErrorDetail) -> Self {
        Self {
            kind: d.kind.clone(),
            code: d.code.clone(),
            message: d.message.clone(),
        }
    }
}

/// 错误详情的纯 Rust 视图，避免 Core/SDK 直接依赖 prost 类型做业务判断。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ErrorDetailBox {
    pub kind: String,
    pub code: String,
    pub message: String,
}

impl ErrorDetailBox {
    /// 是否为配置类错误——决定 Endpoint 是否进入"不自动重试"的 FAILED 分支（§11.1）。
    pub fn is_configuration_error(&self) -> bool {
        self.kind == ErrorKind::Configuration.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// 版本常量冻结：wire 增补必须是 **additive**，因此只允许 Minor +1，
    /// Major 必须仍为 1。
    /// 这条断言的作用不是"复述常量"，而是把「有人顺手把 Major 也改了」这种
    /// 破坏性变更挡在 CI：Major 变化会让所有旧驱动握手被拒。
    #[test]
    fn protocol_major_frozen_at_1_and_minor_is_8() {
        assert_eq!(
            PROTOCOL_MAJOR, 1,
            "PROTOCOL_MAJOR 必须保持 1（additive 增补）"
        );
        assert_eq!(PROTOCOL_MINOR, 8, "Browse 选择契约的 wire 增补对应 Minor 8");
        assert_eq!(
            DEFINITIONS_RPC_MIN_MINOR, 7,
            "I-2 RPC 的可用门槛必须停在引入它的 Minor（不得随本次 bump 漂移）"
        );
        assert_eq!(
            BROWSE_SELECTION_MIN_MINOR, PROTOCOL_MINOR,
            "选择契约的引入 Minor 必须等于当前 Minor，否则门控与常量会漂移"
        );
    }

    /// V2 门控语义（纯函数）：协商 Minor < 8 的旧端不会填选择契约字段，
    /// 必须判为「不支持」；既有 5 个 Minor 门（2/3/5/6/7）语义不变。
    #[test]
    fn browse_selection_gate_requires_minor_8() {
        for minor in 0..BROWSE_SELECTION_MIN_MINOR {
            assert!(
                !browse_selection_supported(minor),
                "negotiated_minor={minor} 必须被门控判为不支持选择契约"
            );
        }
        for minor in BROWSE_SELECTION_MIN_MINOR..=BROWSE_SELECTION_MIN_MINOR + 3 {
            assert!(
                browse_selection_supported(minor),
                "negotiated_minor={minor} 必须放行"
            );
        }
        // 既有门控常量未被本次增补改动（各自引入的 Minor 不变）
        assert_eq!(PROBE_RPC_MIN_MINOR, 2);
        assert_eq!(EVENT_PLANE_MIN_MINOR, 3);
        assert_eq!(TASK_SCHEDULE_MIN_MINOR, 5);
        assert_eq!(CONTROL_MODEL_MIN_MINOR, 6);
        assert_eq!(DEFINITIONS_RPC_MIN_MINOR, 7);
    }

    /// V2 门控语义（纯函数）：协商 Minor < 7 的旧端不识别 tag 45–48，
    /// 必须判为「不支持」；同时既有 4 个 Minor 门（2/3/5/6）语义不变。
    #[test]
    fn definitions_rpc_gate_requires_minor_7() {
        for minor in 0..DEFINITIONS_RPC_MIN_MINOR {
            assert!(
                !definitions_query_supported(minor),
                "negotiated_minor={minor} 必须被门控拦住（旧端会静默忽略未知消息）"
            );
        }
        for minor in DEFINITIONS_RPC_MIN_MINOR..=DEFINITIONS_RPC_MIN_MINOR + 3 {
            assert!(
                definitions_query_supported(minor),
                "negotiated_minor={minor} 必须放行"
            );
        }
        // 既有门控常量未被本次增补改动（各自引入的 Minor 不变）
        assert_eq!(PROBE_RPC_MIN_MINOR, 2);
        assert_eq!(EVENT_PLANE_MIN_MINOR, 3);
        assert_eq!(TASK_SCHEDULE_MIN_MINOR, 5);
        assert_eq!(CONTROL_MODEL_MIN_MINOR, 6);
    }

    /// 协商取较小值（`negotiate` 逐字未变）：新 Core ↔ 旧驱动 / 旧 Core ↔
    /// 新驱动两种组合都必须降到 7，且 I-2 门控据此关闭 ⇒ 降级是静默且安全的。
    ///
    /// 同时覆盖 Browse 选择契约：协商降到 7 时 `browse_selection_supported`
    /// 必须为 false（旧驱动不会填 tag 8/9/10），而响应字段本身是 additive，
    /// 因此**不需要**像 I-2 那样在发送前门控。
    #[test]
    fn negotiate_min_keeps_definitions_gate_closed_across_versions() {
        let core = (PROTOCOL_MAJOR, PROTOCOL_MINOR);
        // 新 Core ↔ 新驱动：8，两道门控都放行
        let (maj, minor) = negotiate(core, (PROTOCOL_MAJOR, PROTOCOL_MINOR)).unwrap();
        assert_eq!(maj, PROTOCOL_MAJOR);
        assert_eq!(minor, 8);
        assert!(definitions_query_supported(minor));
        assert!(browse_selection_supported(minor));
        // 新 Core ↔ 旧驱动二进制（Minor 6）：协商 6，两道门控都关闭
        let (_, minor) = negotiate((PROTOCOL_MAJOR, 6), core).unwrap();
        assert_eq!(minor, 6);
        assert!(!definitions_query_supported(minor));
        assert!(!browse_selection_supported(minor));
        // 旧 Core（Minor 7）↔ 新驱动二进制：协商 7（旧 Core 不认识 tag 45–48
        // 之外的任何新东西；选择契约是响应字段，旧 Core 直接忽略）
        let (_, minor) = negotiate(core, (PROTOCOL_MAJOR, 7)).unwrap();
        assert_eq!(minor, 7);
        assert!(definitions_query_supported(minor));
        assert!(
            !browse_selection_supported(minor),
            "旧 Core 协商到 7 时不具备选择契约（字段会被忽略）"
        );
    }

    /// 新增 4 个消息的编解码往返：`GetDefinitions.request_json` 必须原样往返
    /// （含中文/缩进/特殊字符），空串时编码体仍为 0 字节——空 oneof 成员照样
    /// 能凭 tag 区分，这是 additive 增补的基本前提。
    #[test]
    fn definitions_messages_roundtrip_on_wire() {
        // 空请求：编码后 0 字节，但信封层仍能凭 tag 区分成员
        let env = pb::Envelope {
            msg_id: 9,
            body: Some(pb::envelope::Body::GetDefinitions(pb::GetDefinitions {
                request_json: String::new(),
            })),
        };
        let bytes = env.encode_to_vec();
        let back = pb::Envelope::decode(bytes.as_slice()).unwrap();
        match back.body {
            Some(pb::envelope::Body::GetDefinitions(r)) => assert!(r.request_json.is_empty()),
            other => panic!("expected GetDefinitions, got {other:?}"),
        }

        // 带参数的请求：查询 JSON 必须**逐字**往返（驱动层才知道键的语义）
        let query =
            "{\n  \"family\": \"828d\",\n  \"variable_id\": \"通道参数\",\n  \"limit\": 50\n}";
        let env = pb::Envelope {
            msg_id: 13,
            body: Some(pb::envelope::Body::GetDefinitions(pb::GetDefinitions {
                request_json: query.into(),
            })),
        };
        let back = pb::Envelope::decode(env.encode_to_vec().as_slice()).unwrap();
        match back.body {
            Some(pb::envelope::Body::GetDefinitions(r)) => {
                assert_eq!(r.request_json, query, "查询 JSON 必须逐字往返");
            }
            other => panic!("expected GetDefinitions, got {other:?}"),
        }

        let env = pb::Envelope {
            msg_id: 10,
            body: Some(pb::envelope::Body::DefinitionsReport(
                pb::DefinitionsReport {
                    result: Some(ok_result()),
                    definitions_json: r#"{"definitions":[]}"#.into(),
                },
            )),
        };
        let back = pb::Envelope::decode(env.encode_to_vec().as_slice()).unwrap();
        match back.body {
            Some(pb::envelope::Body::DefinitionsReport(r)) => {
                assert!(r.result.unwrap().ok);
                assert_eq!(r.definitions_json, r#"{"definitions":[]}"#);
            }
            other => panic!("expected DefinitionsReport, got {other:?}"),
        }

        let env = pb::Envelope {
            msg_id: 11,
            body: Some(pb::envelope::Body::PreviewPointKey(pb::PreviewPointKey {
                request_json: r#"{"family":"828d"}"#.into(),
            })),
        };
        let back = pb::Envelope::decode(env.encode_to_vec().as_slice()).unwrap();
        match back.body {
            Some(pb::envelope::Body::PreviewPointKey(r)) => {
                assert_eq!(r.request_json, r#"{"family":"828d"}"#);
            }
            other => panic!("expected PreviewPointKey, got {other:?}"),
        }

        let env = pb::Envelope {
            msg_id: 12,
            body: Some(pb::envelope::Body::PreviewIdentityReport(
                pb::PreviewIdentityReport {
                    result: Some(err_result(
                        ErrorKind::Unsupported,
                        "DEFINITIONS_UNSUPPORTED",
                        "not implemented",
                    )),
                    result_json: String::new(),
                },
            )),
        };
        let back = pb::Envelope::decode(env.encode_to_vec().as_slice()).unwrap();
        match back.body {
            Some(pb::envelope::Body::PreviewIdentityReport(r)) => {
                let err = r.result.unwrap().error.unwrap();
                assert_eq!(err.kind, ErrorKind::Unsupported.as_str());
                assert_eq!(err.code, "DEFINITIONS_UNSUPPORTED");
                assert!(r.result_json.is_empty());
            }
            other => panic!("expected PreviewIdentityReport, got {other:?}"),
        }
    }

    /// 选择契约构造器：模式与诊断必须同源自洽。
    ///
    /// 断言口径是「不允许出现自相矛盾的组合」：分支节点没有理由、被拒绝必有
    /// ERROR 级理由、可选叶的诊断不会把可选性拉成 false。
    #[test]
    fn browse_selection_builders_stay_consistent() {
        use crate::browse_selection as sel;

        let f = sel::not_selectable();
        assert_eq!(f.mode, pb::BrowseSelectionMode::NotSelectable);
        assert!(f.payload_json.is_empty());
        assert!(
            f.diagnostics.is_empty(),
            "分支/诊断节点不应产生红色阻断理由"
        );

        let r = sel::rejected(r#"{"kind":"x"}"#, "SOME_CODE", "为什么不可选");
        assert_eq!(r.mode, pb::BrowseSelectionMode::NotSelectable);
        assert_eq!(r.payload_json, r#"{"kind":"x"}"#);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].code, "SOME_CODE");
        assert_eq!(r.diagnostics[0].message, "为什么不可选");
        assert_eq!(
            r.diagnostics[0].severity,
            pb::BrowseDiagnosticSeverity::Error as i32,
            "拒绝理由必须是 ERROR 级，否则前端不会渲染成阻断"
        );

        let s = sel::semantic(
            "{}",
            vec![sel::diagnostic(
                "IDENTITY_UNRESOLVED",
                "身份未确定",
                pb::BrowseDiagnosticSeverity::Warning,
            )],
        );
        assert_eq!(s.mode, pb::BrowseSelectionMode::Semantic);
        assert_eq!(
            s.diagnostics[0].severity,
            pb::BrowseDiagnosticSeverity::Warning as i32
        );

        let g = sel::resource(r#"{"resource_id":"node","parameters":{}}"#);
        assert_eq!(g.mode, pb::BrowseSelectionMode::Resource);
        assert!(g.diagnostics.is_empty());
        // 这条断言防「诊断悄悄改写可选性」：模式仍是调用方选择的那一个。
        assert_ne!(g.mode, pb::BrowseSelectionMode::NotSelectable);
    }

    /// Browse 选择契约的 **wire 兼容**（IPC 1.8 additive 的核心保证）。
    ///
    /// ① 旧驱动（Minor 7）不填 tag 8/9/10 ⇒ 解码后 `selection_mode = 0`、
    ///    载荷为空串、诊断为空：普通资源浏览必须继续按 1.7 的行为工作，
    ///    因此消费方**不得**把 0 当成「不可选」。
    /// ② 新驱动填了字段 ⇒ 逐字往返（含中文文案与多条诊断）。
    /// ③ 未来驱动给出未知枚举值（如 99）⇒ 解码**不得报错**，原值保留；
    ///    消费方据此 fail-closed（`try_from` 失败即阻断），而不是 panic。
    #[test]
    fn browse_selection_fields_are_additive_and_forward_compatible() {
        // ① 旧驱动的字节形态：只有 tag 1–7（默认值不编码，与 1.7 完全一致）
        let legacy = pb::BrowseNode {
            id: "nsu=urn:mesa:fake:1;i=1".into(),
            label: "Counter".into(),
            kind: "node".into(),
            data_type: "STRING".into(),
            access: "read".into(),
            has_children: true,
            binding_json: r#"{"node_id":"nsu=urn:mesa:fake:1;i=1"}"#.into(),
            ..Default::default()
        };
        let bytes = legacy.encode_to_vec();
        let back = pb::BrowseNode::decode(bytes.as_slice()).unwrap();
        assert_eq!(back.selection_mode, 0, "旧驱动不填 ⇒ UNSPECIFIED(0)");
        assert_eq!(back.selection_payload_json, "", "旧驱动不填 ⇒ 空载荷");
        assert!(back.diagnostics.is_empty(), "旧驱动不填 ⇒ 无诊断");
        assert_eq!(
            back.binding_json, legacy.binding_json,
            "既有 7 字段语义逐字不变"
        );

        // ② 新驱动的字节形态：三条新字段逐字往返
        let modern = pb::BrowseNode {
            id: "nck://presets/x".into(),
            kind: "nck_variable".into(),
            selection_mode: pb::BrowseSelectionMode::Semantic as i32,
            selection_payload_json: r#"{"kind":"nck_variable","parameters":{"variable_id":"a.b"}}"#
                .into(),
            diagnostics: vec![pb::BrowseSelectionDiagnostic {
                code: "NCK_PREVIEW_POINT_KEY".into(),
                message: "Browse 预览：a.b@channel=i:1（仅展示）".into(),
                severity: pb::BrowseDiagnosticSeverity::Info as i32,
            }],
            ..Default::default()
        };
        let back = pb::BrowseNode::decode(modern.encode_to_vec().as_slice()).unwrap();
        assert_eq!(back.selection_mode, 2);
        assert_eq!(back.selection_payload_json, modern.selection_payload_json);
        assert_eq!(back.diagnostics.len(), 1);
        assert_eq!(back.diagnostics[0].code, "NCK_PREVIEW_POINT_KEY");
        assert_eq!(
            back.diagnostics[0].message,
            "Browse 预览：a.b@channel=i:1（仅展示）"
        );

        // ③ 未知枚举值：解码保留原值，`try_from` 失败 ⇒ 消费侧 fail-closed
        let unknown = pb::BrowseNode {
            id: "future://x".into(),
            selection_mode: 99,
            ..Default::default()
        };
        let back = pb::BrowseNode::decode(unknown.encode_to_vec().as_slice()).unwrap();
        assert_eq!(
            back.selection_mode, 99,
            "未知模式必须原值保留，不得被吞成 0"
        );
        assert!(
            pb::BrowseSelectionMode::try_from(back.selection_mode).is_err(),
            "未知模式必须让 try_from 失败，消费方据此明确阻断"
        );
    }

    /// P1 invariant：in-tree driver.toml 的 protocol_minor 必须与
    /// PROTOCOL_MINOR 同步（管理面展示读 manifest，运行协商读常量；
    /// bump 常量忘改 toml 即漂移）。Minor 只做展示一致性，不断言 gate。
    #[test]
    fn in_tree_manifest_minor_matches_protocol() {
        // workspace 根 → drivers/<id>/driver.toml（4 个正式 in-tree 驱动）+
        // tests/support/test-driver/driver.toml（test-driver 虽退役产品身份，
        // 仍是 IPC participant，manifest minor 不允许漂移）。
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        for rel in [
            "drivers/s7",
            "drivers/opcua",
            "drivers/focas2",
            "drivers/sinumerik-nck",
            "tests/support/test-driver",
        ] {
            let p = root.join(rel).join("driver.toml");
            let text =
                std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing {}", p.display()));
            let minor = text
                .lines()
                .find_map(|l| {
                    let t = l.trim();
                    t.strip_prefix("protocol_minor")
                        .and_then(|v| v.trim().strip_prefix('='))
                        .and_then(|v| v.trim().parse::<u32>().ok())
                })
                .unwrap_or_else(|| panic!("{rel} driver.toml 缺 protocol_minor"));
            assert_eq!(
                minor, PROTOCOL_MINOR,
                "{rel} driver.toml minor {minor} != PROTOCOL_MINOR {PROTOCOL_MINOR}"
            );
        }
    }

    /// 全部 Value 变体的编解码必须无损往返，这是高频通道正确性的根基。
    #[test]
    fn value_roundtrip_all_variants() {
        let samples = vec![
            Value::Bool(true),
            Value::I32(-5),
            Value::U32(7),
            Value::I64(i64::MIN),
            Value::U64(u64::MAX),
            Value::F32(1.5),
            Value::F64(-2.25),
            Value::String("中文值".into()),
            Value::Bytes(vec![0, 1, 255]),
            Value::DateTime(1_700_000_000_123_456_789),
            Value::BoolArray(vec![true, false]),
            Value::I32Array(vec![-1, 2]),
            Value::U32Array(vec![3]),
            Value::I64Array(vec![]),
            Value::U64Array(vec![9, 10]),
            Value::F32Array(vec![0.5]),
            Value::F64Array(vec![1.0 / 3.0, 2.5]),
            Value::StringArray(vec!["a".into(), "".into()]),
            Value::DateTimeArray(vec![1, -1]),
        ];
        for v in samples {
            assert_eq!(value_from_pb(value_to_pb(&v)).unwrap(), v);
        }
    }

    /// EventRecord wire 往返：condition/attributes/occurred_at 全保留；
    /// 非法 transition 与超限 severity 在 from 侧拒收。
    #[test]
    fn event_record_proto_roundtrip() {
        use mesa_core_types::{EventCondition, EventRecord};
        let rec = EventRecord {
            event_id: "A700012:raised:7".into(),
            category: "alarm".into(),
            kind: "alarm.condition".into(),
            source: "Channel1".into(),
            severity: 700,
            code: Some("700012".into()),
            message: Some("overtemp".into()),
            message_locale: None,
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
            attributes: BTreeMap::from([
                ("axis".into(), Value::I32(1)),
                ("temp".into(), Value::F64(89.5)),
            ]),
        };
        let back = event_record_from_pb(event_record_to_pb(&rec).unwrap()).unwrap();
        assert_eq!(back, rec);
        // 非法 transition 拒收
        let mut bad = event_record_to_pb(&rec).unwrap();
        bad.condition.as_mut().unwrap().transition = "exploded".into();
        assert!(event_record_from_pb(bad).is_err());
        // 瞬时事件（无 condition/occurred_at）同样往返
        let mut flat = rec.clone();
        flat.condition = None;
        flat.occurred_at_ns = None;
        assert_eq!(
            event_record_from_pb(event_record_to_pb(&flat).unwrap()).unwrap(),
            flat
        );
    }

    /// P5-review：severity 溢出整体拒收（禁止截断洗白），重复 attribute key
    /// 整条拒收（禁止静默覆盖历史事实）。
    #[test]
    fn event_record_overflow_and_dupkey_rejected() {
        use mesa_core_types::EventRecord;
        let rec = EventRecord {
            event_id: "e".into(),
            category: "c".into(),
            kind: "k".into(),
            source: "s".into(),
            severity: 1,
            code: None,
            message: None,
            message_locale: None,
            occurred_at_ns: None,
            condition: None,
            correlation_id: None,
            attributes: BTreeMap::new(),
        };
        let mut wire = event_record_to_pb(&rec).unwrap();
        wire.severity = 999_999;
        assert!(matches!(
            event_record_from_pb(wire),
            Err(ConvertError::InvalidEventRecord(_))
        ));
        // wire 合法但超 1000 的 severity 同样拒收（validate 0..=1000）
        let mut wire = event_record_to_pb(&rec).unwrap();
        wire.severity = 5000;
        assert!(event_record_from_pb(wire).is_err());
        // 重复 attribute key 拒收
        let mut wire = event_record_to_pb(&rec).unwrap();
        let attr = pb::EventAttributeMsg {
            key: "axis".into(),
            value: Some(value_to_pb(&Value::I32(1))),
        };
        wire.attributes = vec![attr.clone(), attr];
        assert!(matches!(
            event_record_from_pb(wire),
            Err(ConvertError::InvalidEventRecord(_))
        ));
    }

    /// EventTask wire 往返 + 坏任务拒收（坏 EventTask 必须让 revision 失败，§35）。
    #[test]
    fn event_task_proto_roundtrip() {
        use mesa_core_types::{DriverBinding, EventTask};
        let t = EventTask {
            id: "alarms".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: "ac".into(),
                config: serde_json::json!({"area": "NCK"}),
            },
        };
        assert_eq!(
            event_task_from_pb(event_task_to_pb(&t).unwrap()).unwrap(),
            t
        );
        // Poll 无 interval 拒收
        let bad = pb::EventTaskProto {
            id: "e1".into(),
            mode: "poll".into(),
            interval_ms: None,
            binding_kind: "k".into(),
            binding_config_json: "{}".into(),
        };
        assert!(matches!(
            event_task_from_pb(bad),
            Err(ConvertError::InvalidEventTask(_))
        ));
    }

    /// EventBatch 上限 256 KiB 双向执行（发送侧与接收侧都不收半批）。
    #[test]
    fn event_batch_size_cap_enforced_both_directions() {
        use mesa_core_types::EventRecord;
        let big = EventRecord {
            event_id: "e".into(),
            category: "c".into(),
            kind: "k".into(),
            source: "s".into(),
            severity: 1,
            code: None,
            message: Some("z".repeat(4000)),
            message_locale: None,
            occurred_at_ns: None,
            condition: None,
            correlation_id: None,
            attributes: BTreeMap::new(),
        };
        // 70 条 × ~4KiB > 256KiB
        let events = vec![big; 70];
        let batch = mesa_core_types::EventBatch {
            connection_handle: 7,
            stream_epoch: 99,
            sequence: 5,
            timestamp_ns: 1_700_000_000_000_000_000,
            events,
            mono_ns: Some(123),
        };
        assert!(matches!(
            event_batch_to_pb(&batch),
            Err(ConvertError::EventBatchTooLarge(_))
        ));
        // 接收侧同样拒收（逐条合法但整批超限）
        let mut wire = pb::EventBatchMsg {
            connection_handle: 7,
            stream_epoch: 99,
            sequence: 5,
            timestamp_ns: 1_700_000_000_000_000_000,
            mono_ns: Some(123),
            events: batch
                .events
                .iter()
                .map(|e| event_record_to_pb(e).unwrap())
                .collect(),
        };
        assert!(event_batch_from_pb(wire.clone()).is_err());
        wire.events.truncate(2);
        // header 元数据完整保留（PR6 ingress 依赖，一律不丢）
        let back = event_batch_from_pb(wire).unwrap();
        assert_eq!(back.connection_handle, 7);
        assert_eq!(back.stream_epoch, 99);
        assert_eq!(back.sequence, 5);
        assert_eq!(back.timestamp_ns, 1_700_000_000_000_000_000);
        assert_eq!(back.mono_ns, Some(123));
        assert_eq!(back.events.len(), 2);
    }

    /// 空字符串 quality 必须按 GOOD 解释——这是"缺省即 GOOD"语义的落点。
    #[test]
    fn quality_default_is_good() {
        assert_eq!(quality_from_pb("").unwrap(), Quality::Good);
        assert!(quality_from_pb("EXCELLENT").is_err());
    }

    #[tokio::test]
    async fn envelope_frame_roundtrip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(8 * 1024);
        let env = pb::Envelope {
            msg_id: 42,
            body: Some(pb::envelope::Body::Hello(pb::Hello {
                driver_id: "s7".into(),
                session_token: "secret".into(),
                protocol_major: PROTOCOL_MAJOR,
                protocol_minor: PROTOCOL_MINOR,
                ..Default::default()
            })),
        };
        write_envelope(&mut a, &env).await.unwrap();
        let got = read_envelope(&mut b).await.unwrap();
        assert_eq!(got.msg_id, 42);
        match got.body {
            Some(pb::envelope::Body::Hello(h)) => {
                assert_eq!(h.session_token, "secret");
                assert_eq!(h.protocol_major, 1);
            }
            other => panic!("unexpected body: {other:?}"),
        }
    }

    /// Probe RPC framing 回环：请求带 connection_handle，响应 JSON 原样透传。
    #[tokio::test]
    async fn probe_envelopes_roundtrip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(8 * 1024);
        let req = pb::Envelope {
            msg_id: 7,
            body: Some(pb::envelope::Body::ProbeRequest(pb::ProbeRequest {
                connection_handle: 999,
            })),
        };
        write_envelope(&mut a, &req).await.unwrap();
        match read_envelope(&mut b).await.unwrap().body {
            Some(pb::envelope::Body::ProbeRequest(r)) => {
                assert_eq!(r.connection_handle, 999);
            }
            other => panic!("unexpected body: {other:?}"),
        }
        let resp = pb::Envelope {
            msg_id: 7,
            body: Some(pb::envelope::Body::ProbeResponse(pb::ProbeResponse {
                report_json: r#"{"reachable":true}"#.into(),
            })),
        };
        write_envelope(&mut b, &resp).await.unwrap();
        match read_envelope(&mut a).await.unwrap().body {
            Some(pb::envelope::Body::ProbeResponse(r)) => {
                assert_eq!(r.report_json, r#"{"reachable":true}"#);
            }
            other => panic!("unexpected body: {other:?}"),
        }
    }

    /// 超限长度前缀必须在读取 payload 前被拒绝，杜绝无界内存分配。
    #[tokio::test]
    async fn oversized_frame_is_rejected_without_allocation() {
        let mut buf = BytesMut::new();
        buf.put_u32_le(MAX_FRAME_LEN + 1);
        let mut r = &buf.freeze()[..];
        match read_envelope(&mut r).await {
            Err(ProtocolError::FrameTooLarge { size, .. }) => {
                assert_eq!(size, (MAX_FRAME_LEN + 1) as u64)
            }
            other => panic!("expected FrameTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn negotiate_major_mismatch_rejected_and_minor_minimized() {
        assert_eq!(negotiate((1, 5), (1, 2)).unwrap(), (1, 2));
        assert_eq!(negotiate((1, 0), (1, 9)).unwrap(), (1, 0));
        assert!(negotiate((2, 0), (1, 0)).is_err());
    }

    #[test]
    fn task_conversion_preserves_binding_json() {
        let t = AcquisitionTask {
            id: "fast".into(),
            schedule: TaskSchedule::Poll { interval_ms: 100 },
            binding: DriverBinding {
                kind: "sim.points".into(),
                config: serde_json::json!({ "points": [ { "key": "a", "kind": "counter" } ] }),
            },
        };
        let back = task_from_pb(task_to_pb(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }

    /// Foundation-2 P0 门：Subscribe 四参数完整 roundtrip（非默认参数
    /// 不得在 IPC 中丢失为缺省值）。
    #[test]
    fn subscribe_schedule_roundtrips_exact_parameters() {
        let t = AcquisitionTask {
            id: "sub".into(),
            schedule: TaskSchedule::Subscribe {
                publishing_interval_ms: 1234,
                sampling_interval_ms: 321,
                queue_size: 77,
                discard_oldest: false,
            },
            binding: DriverBinding {
                kind: "mesa.resources.v1".into(),
                config: serde_json::json!({"selections": []}),
            },
        };
        let wire = task_to_pb(&t).unwrap();
        assert_eq!(wire.publishing_interval_ms, Some(1234));
        assert_eq!(wire.sampling_interval_ms, Some(321));
        assert_eq!(wire.queue_size, Some(77));
        assert_eq!(wire.discard_oldest, Some(false));
        let back = task_from_pb(wire).unwrap();
        assert_eq!(back, t);
    }

    /// 缺字段的 Subscribe wire 即违约（不得静默归一缺省调度）。
    #[test]
    fn subscribe_schedule_missing_fields_rejected() {
        let mut wire = task_to_pb(&AcquisitionTask {
            id: "sub".into(),
            schedule: TaskSchedule::default_subscribe(),
            binding: DriverBinding {
                kind: "k".into(),
                config: serde_json::json!({}),
            },
        })
        .unwrap();
        wire.queue_size = None;
        assert!(task_from_pb(wire).is_err());
    }

    #[test]
    fn legacy_proto_unspecified_maps_to_current_or_placeholder() {
        // 旧消息缺省 value_origin=0 + GOOD → Current
        let pv_good = PointValue {
            point_id: 1,
            value: Value::I32(5),
            quality: Quality::Good,
            quality_code: None,
            source_timestamp_ns: None,
            value_origin: ValueOrigin::Unspecified,
        };
        let pb_good = point_value_to_pb(&pv_good);
        // normalize 后应为 Current (1)
        assert_eq!(pb_good.value_origin, pb::ValueOrigin::Current as i32);
        // 手造 legacy pb: origin=0 + quality BAD → Placeholder
        let legacy_pb = pb::PointValueMsg {
            point_id: 2,
            value: Some(value_to_pb(&Value::I32(0))),
            quality: "BAD".into(),
            quality_code: Some(1),
            source_timestamp_ns: None,
            value_origin: 0, // Unspecified
        };
        let decoded = point_value_from_pb(legacy_pb).unwrap();
        assert_eq!(decoded.value_origin, ValueOrigin::Placeholder);
        assert_eq!(decoded.quality, Quality::Bad);
        // legacy GOOD → Current
        let legacy_good_pb = pb::PointValueMsg {
            point_id: 3,
            value: Some(value_to_pb(&Value::I32(7))),
            quality: "".into(),
            quality_code: None,
            source_timestamp_ns: None,
            value_origin: 0,
        };
        let decoded_good = point_value_from_pb(legacy_good_pb).unwrap();
        assert_eq!(decoded_good.value_origin, ValueOrigin::Current);
        assert_eq!(decoded_good.quality, Quality::Good);
    }
}
