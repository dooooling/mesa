//! Mesa 核心数据类型。
//!
//! 本 crate 承载方案（§9–§11）中冻结的第一版 schema：ID、时间、Quality、Value、Task 模型。
//! 这些类型是 Core 与所有 Driver 的共同契约，一经发布不得随意改动；演进必须以向后兼容的
//! 可选扩展方式进行。Core 侧与 Driver 侧均依赖本 crate，但协议细节（S7 地址 / FOCAS
//! Function / OPC UA NodeId）绝不在此出现。

pub mod capability;
pub mod descriptor;
pub mod event;
pub mod probe;
pub mod profile;
pub mod resource;
pub mod schema;

pub use capability::{ControlCatalog, DiscoveryCapabilities, DriverCapabilities};
pub use descriptor::{DriverDescriptor, DriverIdentity};
pub use event::{
    ConditionTransition, EVENT_ATTRIBUTES_MAX_FIELDS, EVENT_BATCH_MAX_BYTES,
    EVENT_RECORD_MAX_BYTES, EVENT_SEVERITY_MAX, EventBatch, EventCatalog, EventCondition,
    EventFieldDescriptor, EventRecord, EventRecordError, EventSequenceTracker,
    EventStreamDescriptor, EventTask, EventTaskError, SequenceVerdict,
};
pub use probe::{
    CapabilityItem, CapabilityState, PROBE_REPORT_MAX_BYTES, ProbeReport, ProbeWarning,
    check_report_size,
};
pub use profile::{DeviceProfile, MatchRule, Preset, expand_preset};
pub use resource::{
    AccessMode, GENERIC_BINDING_KIND, GenericBinding, OutputDescriptor, ResourceDescriptor,
    ResourceSelection, SelectedOutput, validate_selections_structure,
};
pub use schema::{
    Condition, ConditionOp, FieldDescriptor, FieldType, FieldValidation, LocalizedText,
    SchemaDescriptor, UiHints,
};

use serde::{Deserialize, Serialize};

/// 业务时间戳统一为 UTC Unix 纳秒（方案 §10）。禁止使用本地时区或其他单位。
pub type TimestampNs = i64;

/// 当前 UTC 时间的 Unix 纳秒数。系统时钟早于 1970 时返回 0（异常环境兜底）。
pub fn now_unix_ns() -> TimestampNs {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// 宿主机单调时钟（CLOCK_MONOTONIC），用于 IPC/E2E p95/p99 测量，禁止两进程 UTC 相减。
/// Linux 用 `clock_gettime(CLOCK_MONOTONIC)`，Windows 用 `QueryPerformanceCounter` 绝对计数（跨进程同频，可比）。
/// 返回绝对 QPC 换算的纳秒数（自系统启动起），同宿主所有进程共享同一 QPC counter domain，仅用于同宿主差值计算，不代表 UTC 或业务时间，可直接相减得 IPC latency（§3.8 P0-C）。
pub fn host_mono_ns() -> u64 {
    #[cfg(unix)]
    {
        let mut ts = unsafe { std::mem::zeroed::<libc::timespec>() };
        let ret = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        if ret == 0 {
            return (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
    #[cfg(windows)]
    {
        // QPC 绝对计数：counter * 1e9 / frequency，跨进程同频可比，无需 per-process START 锚点
        unsafe {
            let mut freq: i64 = 0;
            let mut cnt: i64 = 0;
            // SAFETY: windows-sys 声明为 unsafe extern "system"
            let fr = windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut freq);
            if fr == 0 || freq <= 0 {
                return 0;
            }
            let cr = windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut cnt);
            if cr == 0 {
                return 0;
            }
            ((cnt as u128 * 1_000_000_000u128) / (freq as u128)) as u64
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}

/// 点位数据类型。字符串形式用于 binding 配置、Descriptor 上报与 REST 展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataType {
    Bool,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
    String,
    Bytes,
    DateTime,
    BoolArray,
    I32Array,
    U32Array,
    I64Array,
    U64Array,
    F32Array,
    F64Array,
    StringArray,
    DateTimeArray,
}

impl DataType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DataType::Bool => "bool",
            DataType::I32 => "i32",
            DataType::U32 => "u32",
            DataType::I64 => "i64",
            DataType::U64 => "u64",
            DataType::F32 => "f32",
            DataType::F64 => "f64",
            DataType::String => "string",
            DataType::Bytes => "bytes",
            DataType::DateTime => "datetime",
            DataType::BoolArray => "bool[]",
            DataType::I32Array => "i32[]",
            DataType::U32Array => "u32[]",
            DataType::I64Array => "i64[]",
            DataType::U64Array => "u64[]",
            DataType::F32Array => "f32[]",
            DataType::F64Array => "f64[]",
            DataType::StringArray => "string[]",
            DataType::DateTimeArray => "datetime[]",
        }
    }
}

/// 解析失败仅针对非法字符串；展示层应原样透传 as_str() 而非重新解析。
#[derive(Debug, thiserror::Error)]
#[error("unknown data type: {0}")]
pub struct UnknownDataType(pub String);

impl std::str::FromStr for DataType {
    type Err = UnknownDataType;
    /// 大小写不敏感，便于 YAML/JSON 配置书写（"F64"/"f64" 均可）。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const ALL: [(DataType, &str); 19] = [
            (DataType::Bool, "bool"),
            (DataType::I32, "i32"),
            (DataType::U32, "u32"),
            (DataType::I64, "i64"),
            (DataType::U64, "u64"),
            (DataType::F32, "f32"),
            (DataType::F64, "f64"),
            (DataType::String, "string"),
            (DataType::Bytes, "bytes"),
            (DataType::DateTime, "datetime"),
            (DataType::BoolArray, "bool[]"),
            (DataType::I32Array, "i32[]"),
            (DataType::U32Array, "u32[]"),
            (DataType::I64Array, "i64[]"),
            (DataType::U64Array, "u64[]"),
            (DataType::F32Array, "f32[]"),
            (DataType::F64Array, "f64[]"),
            (DataType::StringArray, "string[]"),
            (DataType::DateTimeArray, "datetime[]"),
        ];
        let lower = s.to_ascii_lowercase();
        ALL.iter()
            .find(|(_, name)| *name == lower)
            .map(|(dt, _)| *dt)
            .ok_or_else(|| UnknownDataType(s.to_string()))
    }
}

/// 数据质量（方案 §9.3）。语义对齐 OPC UA 三态；Driver 不得人工生成 UNCERTAIN，
/// 除非协议原生 StatusCode 明确给出该语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    Good,
    Uncertain,
    Bad,
}

impl Quality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Quality::Good => "GOOD",
            Quality::Uncertain => "UNCERTAIN",
            Quality::Bad => "BAD",
        }
    }
}

/// 值的来源语义（方案 §5.5 V1.2.1）：用于区分 BAD 时的 last-known 连续性与 placeholder。
/// - CURRENT：本次为协议返回的鲜活值（GOOD 时必为 CURRENT）
/// - LAST_KNOWN：BAD 时携带上一次 GOOD 的 typed 值，保证 GOOD→BAD→GOOD 连续性
/// - PLACEHOLDER：BAD 且无历史 GOOD 时的 typed neutral 占位（非业务值）
/// - UNSPECIFIED：线上传输兼容态，旧消息缺省此字段时按 `Good→Current / Bad→Placeholder` 解释；新代码禁止主动发送 UNSPECIFIED
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ValueOrigin {
    #[default]
    #[serde(rename = "UNSPECIFIED")]
    Unspecified = 0,
    #[serde(rename = "CURRENT")]
    Current = 1,
    #[serde(rename = "LAST_KNOWN")]
    LastKnown = 2,
    #[serde(rename = "PLACEHOLDER")]
    Placeholder = 3,
}

impl ValueOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            ValueOrigin::Unspecified => "UNSPECIFIED",
            ValueOrigin::Current => "CURRENT",
            ValueOrigin::LastKnown => "LAST_KNOWN",
            ValueOrigin::Placeholder => "PLACEHOLDER",
        }
    }

    /// 将线上传输的 UNSPECIFIED 按兼容规则解释为确定的业务语义（§5.5）
    pub fn normalize_unspecified(self, quality: Quality) -> Self {
        match self {
            ValueOrigin::Unspecified => {
                if quality == Quality::Good {
                    ValueOrigin::Current
                } else {
                    ValueOrigin::Placeholder
                }
            }
            other => other,
        }
    }
}

/// 高频数据面的统一值类型（方案 §9.2）。
///
/// 不使用通用 JSON/Object：FOCAS2 结构体优先拆成稳定 Point，OPC UA Array 保留为
/// Typed Array。数组变体与标量一一对应，避免运行期再校验元素类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    /// UTC Unix ns。i64 纳秒可表示范围约到 2262 年，超出属于配置错误。
    DateTime(TimestampNs),
    BoolArray(Vec<bool>),
    I32Array(Vec<i32>),
    U32Array(Vec<u32>),
    I64Array(Vec<i64>),
    U64Array(Vec<u64>),
    F32Array(Vec<f32>),
    F64Array(Vec<f64>),
    StringArray(Vec<String>),
    DateTimeArray(Vec<TimestampNs>),
}

impl Value {
    /// 该值对应的 DataType，用于与 PointDefinition 声明做一致性核对。
    pub fn data_type(&self) -> DataType {
        match self {
            Value::Bool(_) => DataType::Bool,
            Value::I32(_) => DataType::I32,
            Value::U32(_) => DataType::U32,
            Value::I64(_) => DataType::I64,
            Value::U64(_) => DataType::U64,
            Value::F32(_) => DataType::F32,
            Value::F64(_) => DataType::F64,
            Value::String(_) => DataType::String,
            Value::Bytes(_) => DataType::Bytes,
            Value::DateTime(_) => DataType::DateTime,
            Value::BoolArray(_) => DataType::BoolArray,
            Value::I32Array(_) => DataType::I32Array,
            Value::U32Array(_) => DataType::U32Array,
            Value::I64Array(_) => DataType::I64Array,
            Value::U64Array(_) => DataType::U64Array,
            Value::F32Array(_) => DataType::F32Array,
            Value::F64Array(_) => DataType::F64Array,
            Value::StringArray(_) => DataType::StringArray,
            Value::DateTimeArray(_) => DataType::DateTimeArray,
        }
    }

    /// 按 DataType 生成 typed neutral placeholder（BAD 且无 last-known 时使用，非业务值）
    pub fn typed_placeholder(data_type: DataType) -> Self {
        match data_type {
            DataType::Bool => Value::Bool(false),
            DataType::I32 => Value::I32(0),
            DataType::U32 => Value::U32(0),
            DataType::I64 => Value::I64(0),
            DataType::U64 => Value::U64(0),
            DataType::F32 => Value::F32(0.0),
            DataType::F64 => Value::F64(0.0),
            DataType::String => Value::String(String::new()),
            DataType::Bytes => Value::Bytes(Vec::new()),
            DataType::DateTime => Value::DateTime(0),
            DataType::BoolArray => Value::BoolArray(Vec::new()),
            DataType::I32Array => Value::I32Array(Vec::new()),
            DataType::U32Array => Value::U32Array(Vec::new()),
            DataType::I64Array => Value::I64Array(Vec::new()),
            DataType::U64Array => Value::U64Array(Vec::new()),
            DataType::F32Array => Value::F32Array(Vec::new()),
            DataType::F64Array => Value::F64Array(Vec::new()),
            DataType::StringArray => Value::StringArray(Vec::new()),
            DataType::DateTimeArray => Value::DateTimeArray(Vec::new()),
        }
    }

    /// 单点数值的浮点视图，仅供诊断/演示接口粗略展示，不参与任何业务计算。
    pub fn as_f64_approx(&self) -> Option<f64> {
        match self {
            Value::Bool(b) => Some(*b as u8 as f64),
            Value::I32(v) => Some(*v as f64),
            Value::U32(v) => Some(*v as f64),
            Value::I64(v) => Some(*v as f64),
            Value::U64(v) => Some(*v as f64),
            Value::F32(v) => Some(*v as f64),
            Value::F64(v) => Some(*v),
            _ => None,
        }
    }
}

/// 单个点位的一次采样。`quality` 缺省即为 GOOD，因此建模为显式枚举而非 Option，
/// 由构造方负责填默认值，避免下游到处解包。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointValue {
    pub point_id: u32,
    pub value: Value,
    #[serde(default = "default_quality")]
    pub quality: Quality,
    /// 协议原生或 Mesa 原因码，无则 None。
    pub quality_code: Option<i32>,
    /// 设备/协议提供的原始时间戳；协议未提供则 None（消费方回退到 Batch 时间戳）。
    pub source_timestamp_ns: Option<TimestampNs>,
    /// 值的来源语义（§5.5）：CURRENT / LAST_KNOWN / PLACEHOLDER，线上传 UNSPECIFIED 需 normalize
    #[serde(default)]
    pub value_origin: ValueOrigin,
}

fn default_quality() -> Quality {
    Quality::Good
}

impl PointValue {
    pub fn good(point_id: u32, value: Value) -> Self {
        Self {
            point_id,
            value,
            quality: Quality::Good,
            quality_code: None,
            source_timestamp_ns: None,
            value_origin: ValueOrigin::Current,
        }
    }

    /// 将可能的 UNSPECIFIED 按兼容规则归一化（旧消息兼容）
    pub fn normalized(mut self) -> Self {
        self.value_origin = self.value_origin.normalize_unspecified(self.quality);
        self
    }
}

/// 高频数据批次（方案 §10）。
///
/// - `stream_epoch`：每次 Start/Reopen 由 Core 生成的新随机值，用于识别重启；
/// - `sequence`：同一 `(connection_handle, stream_epoch)` 内从 1 严格递增；
/// - 背压合并后的批次允许 sequence 出现缺口，缺口本身即丢弃计数的语义体现。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataBatch {
    pub connection_handle: u32,
    pub stream_epoch: u64,
    pub sequence: u64,
    pub timestamp_ns: TimestampNs,
    pub values: Vec<PointValue>,
    /// 单调时钟埋点（Driver 侧 publish 时的 Instant 采样，单位 ns），用于 IPC/E2E p95/p99。
    /// 0/None 表示未埋点（旧 Driver 兼容）。
    #[serde(default)]
    pub mono_ns: Option<u64>,
}

/// 任务执行模式（方案 §5.5）：Poll 周期轮询；Subscribe 由服务端推送（OPC UA）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskMode {
    Poll,
    Subscribe,
}

impl TaskMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskMode::Poll => "poll",
            TaskMode::Subscribe => "subscribe",
        }
    }
}

/// Driver 私有绑定描述。Core 只做 Schema 校验和持久化，绝不解释 `config`
/// （方案核心原则 #6：Core 不懂协议）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverBinding {
    pub kind: String,
    pub config: serde_json::Value,
}

/// 一个采集任务。`binding.kind` 的合法取值由各 Driver 自行定义并校验。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionTask {
    pub id: String,
    pub mode: TaskMode,
    pub interval_ms: Option<u64>,
    pub binding: DriverBinding,
}

impl AcquisitionTask {
    /// Core 与 Driver 双侧共用的结构级校验；协议语义由 Driver 在 configure 中补充校验。
    pub fn validate(&self) -> Result<(), TaskValidationError> {
        if self.id.trim().is_empty() {
            return Err(TaskValidationError::EmptyTaskId);
        }
        match self.mode {
            TaskMode::Poll => {
                if self.interval_ms.unwrap_or(0) == 0 {
                    return Err(TaskValidationError::PollRequiresInterval {
                        task: self.id.clone(),
                    });
                }
            }
            // Subscribe 模式的节拍由订阅参数决定，interval 无意义
            TaskMode::Subscribe => {}
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TaskValidationError {
    #[error("task id 不能为空")]
    EmptyTaskId,
    #[error("Poll 任务 `{task}` 必须提供正整数 interval_ms")]
    PollRequiresInterval { task: String },
}

/// point_key -> point_id 的稳定映射（方案 §6.2 ApplyPointMap）。
/// 高频通道自此只携带 point_id，不再出现 point_key。
pub type PointMap = std::collections::HashMap<String, u32>;

/// Driver 在 ConfigureTasks 后上报的点描述，**不含 point_id**——ID 由 Core 分配（方案 §6.2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointDescriptor {
    pub point_key: String,
    pub data_type: DataType,
    pub unit: Option<String>,
}

/// Core 为 Descriptor 分配稳定 point_id 后形成的正式定义（持久化于 PointRegistry）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointDefinition {
    pub point_id: u32,
    pub point_key: String,
    pub data_type: DataType,
    pub unit: Option<String>,
}

/// 同一 Endpoint 全量配置内 point_key 必须唯一（方案 §6.2 双重保护）。
#[derive(Debug, thiserror::Error, PartialEq)]
#[error("duplicate point_key: `{0}`")]
pub struct DuplicatePointKey(pub String);

/// 对 Descriptor 集合做唯一性校验。Driver 在 configure 返回前自查一次，
/// Core 写入 PointRegistry 前再审一次，形成双重保护。
pub fn ensure_unique_point_keys(descriptors: &[PointDescriptor]) -> Result<(), DuplicatePointKey> {
    let mut seen = std::collections::HashSet::with_capacity(descriptors.len());
    for d in descriptors {
        if !seen.insert(d.point_key.as_str()) {
            return Err(DuplicatePointKey(d.point_key.clone()));
        }
    }
    Ok(())
}

/// Connection 运行态（方案 §13）。单 Connection 异常不得影响同 Driver 下其他设备。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ConnectionState {
    Stopped,
    Connecting,
    Running,
    Reconnecting,
    Failed,
}

impl ConnectionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionState::Stopped => "STOPPED",
            ConnectionState::Connecting => "CONNECTING",
            ConnectionState::Running => "RUNNING",
            ConnectionState::Reconnecting => "RECONNECTING",
            ConnectionState::Failed => "FAILED",
        }
    }
}

/// 统一错误类别（方案 §13）。Core 只识别类别；原生错误码与详细信息由 Driver 附带。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    Configuration,
    Connection,
    Timeout,
    Address,
    Protocol,
    Decode,
    Device,
    Unsupported,
    Internal,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::Configuration => "ConfigurationError",
            ErrorKind::Connection => "ConnectionError",
            ErrorKind::Timeout => "Timeout",
            ErrorKind::Address => "AddressError",
            ErrorKind::Protocol => "ProtocolError",
            ErrorKind::Decode => "DecodeError",
            ErrorKind::Device => "DeviceError",
            ErrorKind::Unsupported => "Unsupported",
            ErrorKind::Internal => "InternalError",
        }
    }
}

/// Driver 元数据，来自 Manifest 并经 Hello 握手复核。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverMetadata {
    pub driver_id: String,
    pub name: String,
    pub version: String,
    pub protocol_major: u32,
    pub protocol_minor: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_type_parse_roundtrip_is_case_insensitive() {
        assert_eq!("f64".parse::<DataType>().unwrap(), DataType::F64);
        assert_eq!("F64".parse::<DataType>().unwrap(), DataType::F64);
        assert_eq!(
            "STRING[]".parse::<DataType>().unwrap(),
            DataType::StringArray
        );
        assert_eq!(
            DataType::DateTime.as_str().parse::<DataType>().unwrap(),
            DataType::DateTime
        );
        assert!("int".parse::<DataType>().is_err());
    }

    #[test]
    fn poll_task_requires_interval_but_subscribe_does_not() {
        let mk = |mode, interval| AcquisitionTask {
            id: "t1".into(),
            mode,
            interval_ms: interval,
            binding: DriverBinding {
                kind: "sim".into(),
                config: serde_json::json!({}),
            },
        };
        assert_eq!(
            mk(TaskMode::Poll, None).validate(),
            Err(TaskValidationError::PollRequiresInterval { task: "t1".into() })
        );
        assert_eq!(
            mk(TaskMode::Poll, Some(0)).validate(),
            Err(TaskValidationError::PollRequiresInterval { task: "t1".into() })
        );
        mk(TaskMode::Poll, Some(100)).validate().unwrap();
        // Subscribe 节拍由订阅参数决定，interval 缺省合法
        mk(TaskMode::Subscribe, None).validate().unwrap();
    }

    #[test]
    fn empty_task_id_rejected() {
        let t = AcquisitionTask {
            id: "  ".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: "k".into(),
                config: serde_json::json!({}),
            },
        };
        assert_eq!(t.validate(), Err(TaskValidationError::EmptyTaskId));
    }

    #[test]
    fn duplicate_point_key_detection_hits_second_occurrence() {
        let d = |k: &str| PointDescriptor {
            point_key: k.into(),
            data_type: DataType::F64,
            unit: None,
        };
        ensure_unique_point_keys(&[d("a"), d("b")]).unwrap();
        assert_eq!(
            ensure_unique_point_keys(&[d("a"), d("a")]),
            Err(DuplicatePointKey("a".into()))
        );
    }

    #[test]
    fn value_data_type_matches_variant() {
        assert_eq!(Value::F64(1.0).data_type(), DataType::F64);
        assert_eq!(
            Value::DateTimeArray(vec![1]).data_type(),
            DataType::DateTimeArray
        );
    }
}
