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
    TaskMode, UnknownDataType, Value, ValueOrigin,
};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// IPC 协议版本。Major 不兼容直接拒绝握手；Minor 取双方较小值。
/// V1.2.1 新增 PointValue.value_origin（§5.5），Minor 1 保证新 Driver 的 typed BAD 语义可被新 Core 理解，旧端仍按 UNSPECIFIED 兼容解释
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 3;

/// Dynamic Probe RPC 可用的最低协商 Minor（§8）。协商 Minor < 2 的旧 Driver
/// 不识别 ProbeRequest（会静默忽略），Core 必须直接返回 Unsupported，
/// 不得发 RPC 干等超时。
pub const PROBE_RPC_MIN_MINOR: u32 = 2;

/// Event Plane 可用的最低协商 Minor（Event V1 §10，纯 additive 1.2 → 1.3）。
/// EventTask 存在但 negotiated_minor < 3 时，Core 必须回 EVENT_PLANE_UNSUPPORTED，
/// 不得发未知消息干等 timeout。
pub const EVENT_PLANE_MIN_MINOR: u32 = 3;

/// 单帧上限。防止恶意/异常长度前缀导致无界分配（有界原则在 IPC 层的体现）。
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

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
    Ok(pb::AcquisitionTaskProto {
        id: t.id.clone(),
        mode: t.mode.as_str().to_string(),
        interval_ms: t.interval_ms,
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
    Ok(AcquisitionTask {
        id: t.id,
        mode,
        interval_ms: t.interval_ms,
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
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: "sim.points".into(),
                config: serde_json::json!({ "points": [ { "key": "a", "kind": "counter" } ] }),
            },
        };
        let back = task_from_pb(task_to_pb(&t).unwrap()).unwrap();
        assert_eq!(back, t);
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
