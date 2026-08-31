//! Mesa Driver IPC 协议层（方案 §14）。
//!
//! 职责：proto 生成的消息类型、Length-prefixed framing、协议版本协商、
//! 以及 core-types 与 protobuf 消息之间的双向转换。Core 与 SDK 共用本 crate，
//! 保证两侧编解码路径完全一致。

/// prost 生成的类型。字段命名与 proto/driver.proto 一一对应。
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/Mesa.driver.v1.rs"));
}

use bytes::{BufMut, BytesMut};
use mesa_core_types::{
    AcquisitionTask, ConnectionState, DataBatch, DataType, DriverBinding, ErrorKind,
    PointDescriptor, PointValue, Quality, TaskMode, UnknownDataType, Value,
};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// IPC 协议版本。Major 不兼容直接拒绝握手；Minor 取双方较小值。
pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 0;

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
pub fn negotiate(
    driver: (u32, u32),
    core: (u32, u32),
) -> Result<(u32, u32), IncompatibleProtocol> {
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

pub fn point_value_to_pb(pv: &PointValue) -> pb::PointValueMsg {
    pb::PointValueMsg {
        point_id: pv.point_id,
        value: Some(value_to_pb(&pv.value)),
        quality: quality_to_pb(pv.quality),
        quality_code: pv.quality_code,
        source_timestamp_ns: pv.source_timestamp_ns,
    }
}

pub fn point_value_from_pb(pv: pb::PointValueMsg) -> Result<PointValue, ConvertError> {
    Ok(PointValue {
        point_id: pv.point_id,
        value: value_from_pb(pv.value.ok_or(ConvertError::EmptyValue)?)?,
        quality: quality_from_pb(&pv.quality)?,
        quality_code: pv.quality_code,
        source_timestamp_ns: pv.source_timestamp_ns,
    })
}

pub fn batch_to_pb(b: &DataBatch) -> pb::DataBatchMsg {
    pb::DataBatchMsg {
        connection_handle: b.connection_handle,
        stream_epoch: b.stream_epoch,
        sequence: b.sequence,
        timestamp_ns: b.timestamp_ns,
        values: b.values.iter().map(point_value_to_pb).collect(),
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
    })
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

pub fn tasks_to_pb(tasks: &[AcquisitionTask]) -> Result<Vec<pb::AcquisitionTaskProto>, ConvertError> {
    tasks.iter().map(task_to_pb).collect()
}

pub fn tasks_from_pb(tasks: Vec<pb::AcquisitionTaskProto>) -> Result<Vec<AcquisitionTask>, ConvertError> {
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
            Some(error_detail(ErrorKind::Internal, "", "peer returned failure without detail"))
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

    /// 超限长度前缀必须在读取 payload 前被拒绝，杜绝无界内存分配。
    #[tokio::test]
    async fn oversized_frame_is_rejected_without_allocation() {
        let mut buf = BytesMut::new();
        buf.put_u32_le(MAX_FRAME_LEN + 1);
        let mut r = &buf.freeze()[..];
        match read_envelope(&mut r).await {
            Err(ProtocolError::FrameTooLarge { size, .. }) => assert_eq!(size, (MAX_FRAME_LEN + 1) as u64),
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
}
