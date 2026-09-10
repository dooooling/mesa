#![allow(clippy::collapsible_if)]
//! OPC UA Driver — 方案 §7.3 节点/订阅/浏览型（V1 只读，44/44 2026-08-29）。
//!
//! - Poll 绑定：`opcua.node-group`
//!   ```json
//!   { "nodes": [
//!       { "key": "counter", "node_id": "nsu=http://example.com/MyModel/;s=Counter", "data_type": "U32" },
//!       { "key": "sine",    "node_id": "nsu=http://example.com/MyModel/;i=2",        "data_type": "DOUBLE" }
//!   ]}
//!   ```
//!   node_id 一律 canonical `nsu=` 形态（`ns=<index>` 拒绝，见 `address`）；
//!   运行期每次建会话后经 NamespaceArray 换算为当前 index（漂移自愈）。
//! - Subscribe 绑定：`opcua.subscription`（publishing 500 sampling 30 queue10，§7.3）
//!   ```json
//!   { "publishing_interval_ms": 500, "sampling_interval_ms": 250, "queue_size": 10,
//!     "discard_oldest": true, "nodes": [ {"key":"k","node_id":"nsu=http://example.com/MyModel/;i=2"} ] }
//!   ```
//! - Browse 绑定：`opcua.browse`（周期浏览，§7.3 V1 支持，父子一律 canonical）
//!   ```json
//!   { "nodes": [ {"key":"objects","node_id":"nsu=http://opcfoundation.org/UA/;i=85","data_type":"STRING"} ] }
//!   ```
//! - NodeId 解析见 `address::parse_address`；Core 不触及此文件（硬约束）。
//! - SecurityPolicy/MessageSecurityMode 透传至 Native ClientBuilder pki_dir/own.der/key trust false verify true
//! - SourceTimestamp 1601 ticks→Unix ns 精确保留，Quality GOOD/UNCERTAIN/BAD 按 StatusCode 映射，Array→Typed Array

mod address;
mod event;
mod event_runtime;
mod opcua_api;
mod probe;
mod transport_adapter;

pub use address::{AddressError, Identifier, OpcUaAddress, parse_address};
pub use event::{
    DecodedEvent, EventDecodeContext, EventScope, OPCUA_EVENT_STREAM_ID, OpcUaEventPlanSnapshot,
    OpcUaEventTaskPlan, STANDARD_EVENT_FIELD_COUNT, STANDARD_EVENT_FIELDS, StandardEventClause,
    StandardEventField, canonical_node_id, decode_event_fields, standard_event_clauses,
};
pub use mesa_opcua_transport::DEFAULT_OPCUA_PORT;
pub use opcua_api::{FakeOpcUaApi, OpcUaApi};
pub use transport_adapter::TransportApiAdapter;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{
    AcquisitionTask, DataBatch, DataType, DriverMetadata, DuplicatePointKey, GENERIC_BINDING_KIND,
    GenericBinding, PointDescriptor, PointMap, PointValue, Quality, TaskMode, Value, ValueOrigin,
    ensure_unique_point_keys, now_unix_ns,
};
use mesa_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
use tokio_util::sync::CancellationToken;

pub const BINDING_POLL: &str = "opcua.node-group";
pub const BINDING_SUB: &str = "opcua.subscription";
pub const BINDING_BROWSE: &str = "opcua.browse";

use opcua_api::OpcUaApi as OpcUaApiTrait;

// ---------------------------------------------------------------------------
// 驱动入口
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct OpcUaDriver;

#[async_trait::async_trait]
impl Driver for OpcUaDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: "opcua".into(),
            name: "OPC UA".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol_major: 1,
            protocol_minor: 0,
        }
    }

    fn descriptor(&self) -> mesa_core_types::DriverDescriptor {
        use mesa_core_types::{
            AccessMode, DataType, DriverCapabilities, DriverDescriptor, DriverIdentity,
            FieldDescriptor, FieldType, LocalizedText, OutputDescriptor, OutputTypeSpec,
            ResourceDescriptor, ResourceSelectionMethod, SchemaDescriptor,
        };
        let m = self.metadata();
        DriverDescriptor {
            contract_major: 1,
            contract_minor: 0,
            identity: DriverIdentity {
                driver_id: m.driver_id,
                name: m.name,
                version: m.version,
            },
            connection: SchemaDescriptor {
                fields: vec![
                    FieldDescriptor::new("endpoint_url", "Endpoint URL", FieldType::Url)
                        .required(true)
                        .default_value(serde_json::json!("opc.tcp://127.0.0.1:4840")),
                    {
                        let mut f = FieldDescriptor::new(
                            "security_policy",
                            "Security Policy",
                            FieldType::Enum,
                        )
                        .required(false)
                        .default_value(serde_json::json!("None"));
                        f.validation.enum_options = Some(vec![
                            "None".into(),
                            "Basic128Rsa15".into(),
                            "Basic256".into(),
                            "Basic256Sha256".into(),
                            "Aes128_Sha256_RsaOaep".into(),
                            "Aes256_Sha256_RsaPss".into(),
                        ]);
                        f
                    },
                    {
                        let mut f =
                            FieldDescriptor::new("security_mode", "Security Mode", FieldType::Enum)
                                .required(false)
                                .default_value(serde_json::json!("None"));
                        f.validation.enum_options =
                            Some(vec!["None".into(), "Sign".into(), "SignAndEncrypt".into()]);
                        f
                    },
                    FieldDescriptor::new("username", "Username", FieldType::String).required(false),
                    FieldDescriptor::new("password", "Password", FieldType::Secret).required(false),
                    FieldDescriptor::new("timeout_ms", "Timeout ms", FieldType::Duration)
                        .required(false)
                        .default_value(serde_json::json!(5000)),
                    FieldDescriptor::new("use_native", "Use Native Client", FieldType::Boolean)
                        .required(false)
                        .default_value(serde_json::json!(true)),
                ],
            },
            resources: vec![ResourceDescriptor {
                id: "node".into(),
                label: LocalizedText::new("Node"),
                parameters: SchemaDescriptor {
                    fields: vec![
                        FieldDescriptor::new("node_id", "NodeId", FieldType::String).required(true),
                        {
                            let mut f =
                                FieldDescriptor::new("attribute", "Attribute", FieldType::Enum)
                                    .required(false)
                                    .default_value(serde_json::json!("Value"));
                            f.validation.enum_options = Some(vec![
                                "Value".into(),
                                "BrowseName".into(),
                                "DisplayName".into(),
                                "DataType".into(),
                            ]);
                            f
                        },
                        {
                            let mut f =
                                FieldDescriptor::new("data_type", "Data Type", FieldType::Enum)
                                    .required(false)
                                    .default_value(serde_json::json!("STRING"));
                            f.validation.enum_options = Some(vec![
                                "STRING".into(),
                                "INT32".into(),
                                "INT64".into(),
                                "FLOAT".into(),
                                "DOUBLE".into(),
                                "BOOL".into(),
                            ]);
                            f
                        },
                    ],
                },
                outputs: vec![OutputDescriptor {
                    id: "value".into(),
                    label: LocalizedText::new("Value"),
                    // 类型随 data_type 参数（parse_data_type 口径转写）。
                    type_spec: OutputTypeSpec::FromParameter {
                        parameter: "data_type".into(),
                        mapping: [
                            ("STRING", DataType::String),
                            ("INT32", DataType::I32),
                            ("INT64", DataType::I64),
                            ("FLOAT", DataType::F32),
                            ("DOUBLE", DataType::F64),
                            ("BOOL", DataType::Bool),
                        ]
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                    },
                    unit: None,
                    access: AccessMode::Read,
                }],
                modes: vec![
                    mesa_core_types::TaskMode::Poll,
                    mesa_core_types::TaskMode::Subscribe,
                ],
            }],
            controls: mesa_core_types::ControlCatalog::default(),
            resource_selection_methods: vec![
                ResourceSelectionMethod::Manual,
                ResourceSelectionMethod::Browse,
            ],
            capabilities: DriverCapabilities {
                poll: true,
                subscribe: true,
                events: true,
                ..Default::default()
            },
            // PR9：声明唯一 subscribe-only 事件流 `opcua.events`（Stage ③）。
            events: event::opcua_event_catalog(),
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        let v: serde_json::Value = serde_json::from_str(config_json).map_err(|e| {
            SdkDriverError::configuration("BAD_CONFIG", format!("connection JSON 非法: {e}"))
        })?;
        let cfg = OpcUaConnConfig::from_json(&v)?;
        let use_native = v
            .get("use_native")
            .and_then(|x| x.as_bool())
            .unwrap_or(true);
        if !use_native && std::env::var("MESA_ALLOW_FAKE_NATIVE").ok().as_deref() != Some("1") {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                "use_native=false 仅在测试环境 MESA_ALLOW_FAKE_NATIVE=1 时允许",
            ));
        }
        // transport 与 adapter 共享同一会话实例：创建一次，两处持有同一 Arc，
        // probe 复用它，绝不另建第二会话（P0-2）。
        // Native 会话经公共 transport 建立（PKI 由驱动侧注入 options）。
        // Fake 分支：运行路径保持旧 FakeOpcUaApi 不动（canned 行为被现有单测
        // 与 discovery 合同锁定），probe 用 transport fake；两者皆为无状态桩，
        // 不存在真实会话可泄漏。
        let (api, transport): (
            Arc<dyn OpcUaApiTrait>,
            Arc<dyn mesa_opcua_transport::OpcUaTransport>,
        ) = if use_native {
            let native = Arc::new(mesa_opcua_transport::NativeOpcUaTransport::new(
                cfg.connect_options(),
            ));
            let api: Arc<dyn OpcUaApiTrait> =
                Arc::new(TransportApiAdapter::with_transport(native.clone()));
            (api, native)
        } else {
            // Fake 命名空间契约（测试专用，确定性）：[OPC 基础, mesa fake]。
            // 运行期 canonical 换算与 browse 逆向以此快照为基准。
            let api: Arc<dyn OpcUaApiTrait> = Arc::new(FakeOpcUaApi::new());
            let fake: Arc<dyn mesa_opcua_transport::OpcUaTransport> = Arc::new(
                mesa_opcua_transport::FakeOpcUaTransport::new().with_namespace_array(vec![
                    mesa_opcua_transport::OPC_BASE_NAMESPACE_URI.to_string(),
                    "urn:mesa:fake:1".to_string(),
                ]),
            );
            (api, fake)
        };
        Ok(Box::new(OpcUaConnection {
            cfg,
            api,
            transport,
            plan: None,
            event_plan: None,
        }))
    }
}
// ---------------------------------------------------------------------------
// 连接配置
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct OpcUaConnConfig {
    endpoint_url: String,
    timeout_ms: u64,
    /// SecurityPolicy，如 "None" / "Basic256Sha256"（§19.3）
    security_policy: String,
    /// MessageSecurityMode，如 "None" / "Sign" / "SignAndEncrypt"
    security_mode: String,
    username: Option<String>,
    password: Option<String>,
}

impl Default for OpcUaConnConfig {
    fn default() -> Self {
        Self {
            endpoint_url: "opc.tcp://127.0.0.1:4840".into(),
            timeout_ms: 5000,
            security_policy: "None".into(),
            security_mode: "None".into(),
            username: None,
            password: None,
        }
    }
}

impl OpcUaConnConfig {
    /// PKI 目录解析（驱动侧职责）：仅允许环境变量 MESA_OPCUA_PKI_DIR 或默认值，
    /// 禁止 Endpoint JSON 指定路径；解析后经 options 注入 transport（transport 不读环境变量）。
    fn resolve_pki_dir() -> std::path::PathBuf {
        std::env::var("MESA_OPCUA_PKI_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("data/certificates/opcua"))
    }

    /// 组装 transport 连接选项（含 PKI 注入）。
    fn connect_options(&self) -> mesa_opcua_transport::OpcUaConnectOptions {
        mesa_opcua_transport::OpcUaConnectOptions {
            endpoint_url: self.endpoint_url.clone(),
            timeout_ms: self.timeout_ms,
            security_policy: self.security_policy.clone(),
            security_mode: self.security_mode.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            pki_dir: Self::resolve_pki_dir(),
            ..Default::default()
        }
    }

    fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        // 兼容多种写法：endpoint_url / endpoint / url / host+port
        let endpoint_url = if let Some(s) = v
            .get("endpoint_url")
            .or_else(|| v.get("endpoint"))
            .or_else(|| v.get("url"))
            .and_then(|x| x.as_str())
        {
            s.to_string()
        } else if let Some(host) = v.get("host").and_then(|x| x.as_str()) {
            let port = v
                .get("port")
                .and_then(|x| x.as_u64())
                .unwrap_or(DEFAULT_OPCUA_PORT as u64) as u16;
            format!("opc.tcp://{host}:{port}")
        } else {
            "opc.tcp://127.0.0.1:4840".to_string()
        };
        let timeout_ms = v
            .get("timeout_ms")
            .or_else(|| v.get("timeout"))
            .and_then(|x| x.as_u64())
            .unwrap_or(5000);
        let security_policy = v
            .get("security_policy")
            .and_then(|x| x.as_str())
            .unwrap_or("None")
            .to_string();
        let security_mode = v
            .get("security_mode")
            .and_then(|x| x.as_str())
            .unwrap_or("None")
            .to_string();
        if endpoint_url.trim().is_empty() {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                "endpoint_url 不能为空",
            ));
        }
        if !endpoint_url.starts_with("opc.tcp://") {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!("endpoint_url `{endpoint_url}` 非法，需 opc.tcp://host:port"),
            ));
        }
        if timeout_ms == 0 {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                "timeout_ms 需 >0",
            ));
        }
        // 校验 SecurityPolicy
        let valid_policies = [
            "None",
            "Basic128Rsa15",
            "Basic256",
            "Basic256Sha256",
            "Aes128_Sha256_RsaOaep",
            "Aes256_Sha256_RsaPss",
        ];
        if !valid_policies.contains(&security_policy.as_str()) {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!(
                    "security_policy `{security_policy}` 非法，期望 {:?}",
                    valid_policies
                ),
            ));
        }
        let valid_modes = ["None", "Sign", "SignAndEncrypt"];
        if !valid_modes.contains(&security_mode.as_str()) {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!(
                    "security_mode `{security_mode}` 非法，期望 {:?}",
                    valid_modes
                ),
            ));
        }
        let username = v
            .get("username")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let password = v
            .get("password")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        // 部分凭据必须拒绝而非静默 Anonymous
        match (&username, &password) {
            (Some(_), None) | (None, Some(_)) => {
                return Err(SdkDriverError::configuration(
                    "BAD_CONFIG",
                    "username 与 password 需同时提供或同时为空；仅提供其一视为配置错误",
                ));
            }
            _ => {}
        }
        // 禁止生产默认忽略校验：若 policy/mode 为 None 则 trust_server_certs 需显式，但此处仅校验，实际连接由证书目录决定
        Ok(Self {
            endpoint_url,
            timeout_ms,
            security_policy,
            security_mode,
            username,
            password,
        })
    }
}

// ---------------------------------------------------------------------------
// 采集计划
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PointSpec {
    key: String,
    addr: OpcUaAddress,
    data_type: DataType,
}

#[derive(Debug, Clone)]
enum TaskKind {
    Poll {
        interval_ms: u64,
    },
    Subscribe {
        publishing_interval_ms: u64,
        sampling_interval_ms: u64,
        queue_size: u32,
        discard_oldest: bool,
    },
    Browse {
        interval_ms: u64,
    },
}

#[derive(Debug)]
struct TaskPlan {
    id: String,
    kind: TaskKind,
    point_indices: Vec<usize>,
}

#[allow(dead_code)]
#[derive(Debug)]
struct PlanSnapshot {
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    map: Option<PointMap>,
}

struct OpcUaConnection {
    cfg: OpcUaConnConfig,
    api: Arc<dyn OpcUaApiTrait>,
    /// probe 复用的传输实例：与 api 背后的会话是同一个（open 时一次创建，
    /// 两处共享同一 Arc），绝不为探测另建第二会话。
    transport: Arc<dyn mesa_opcua_transport::OpcUaTransport>,
    plan: Option<PlanSnapshot>,
    /// 事件计划快照（Stage ③）：`configure_events` 原子替换，供 run() 启动
    /// Event workers；空表/None = 无事件订阅。
    event_plan: Option<OpcUaEventPlanSnapshot>,
}

impl std::fmt::Debug for OpcUaConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcUaConnection")
            .field("cfg", &self.cfg)
            .field("has_plan", &self.plan.is_some())
            .finish()
    }
}

fn parse_data_type(s: &str) -> Result<DataType, SdkDriverError> {
    match s.trim().to_ascii_uppercase().as_str() {
        "BOOL" | "BOOLEAN" => Ok(DataType::Bool),
        "I32" | "INT32" | "INT" => Ok(DataType::I32),
        "U32" | "UINT32" | "DWORD" => Ok(DataType::U32),
        "I64" | "INT64" => Ok(DataType::I64),
        "U64" | "UINT64" => Ok(DataType::U64),
        "F32" | "FLOAT" | "REAL" => Ok(DataType::F32),
        "F64" | "DOUBLE" | "LREAL" => Ok(DataType::F64),
        "STRING" | "STR" => Ok(DataType::String),
        // OPC UA 扩展：Bytes/DateTime 按 String 兼容，前置校验放宽
        "BYTES" => Ok(DataType::Bytes),
        "DATETIME" => Ok(DataType::DateTime),
        _ => Err(SdkDriverError::configuration(
            "INVALID_DATA_TYPE",
            format!(
                "data_type `{s}` 非法，期望 BOOL/I32/U32/I64/U64/F32/F64/STRING/BYTES/DATETIME"
            ),
        )),
    }
}

fn value_fits_data_type(v: &Value, dt: DataType) -> bool {
    match (v, dt) {
        (Value::Bool(_), DataType::Bool) => true,
        (Value::I32(_), DataType::I32) => true,
        (Value::U32(_), DataType::U32) => true,
        (Value::I64(_), DataType::I64) => true,
        (Value::U64(_), DataType::U64) => true,
        (Value::F32(_), DataType::F32) => true,
        (Value::F64(_), DataType::F64) => true,
        (Value::String(_), DataType::String) => true,
        (Value::Bytes(_), DataType::Bytes) => true,
        (Value::DateTime(_), DataType::DateTime) => true,
        (Value::BoolArray(_), DataType::Bool) => true,
        (Value::I32Array(_), DataType::I32) => true,
        (Value::U32Array(_), DataType::U32) => true,
        (Value::I64Array(_), DataType::I64) => true,
        (Value::U64Array(_), DataType::U64) => true,
        (Value::F32Array(_), DataType::F32) => true,
        (Value::F64Array(_), DataType::F64) => true,
        (Value::StringArray(_), DataType::String) => true,
        (Value::DateTimeArray(_), DataType::DateTime) => true,
        // 宽容：U32/I32 互通，F32/F64 互通，I32→I64、U32→U64
        (Value::U32(_), DataType::I32) => true,
        (Value::I32(_), DataType::U32) => true,
        (Value::F32(_), DataType::F64) => true,
        (Value::F64(_), DataType::F32) => true,
        (Value::I32(_), DataType::I64) => true,
        (Value::U32(_), DataType::U64) => true,
        _ => false,
    }
}

fn coerce_value(v: Value, dt: DataType) -> Value {
    match (v, dt) {
        (Value::U32(x), DataType::I32) => Value::I32(x as i32),
        (Value::I32(x), DataType::U32) => Value::U32(x as u32),
        (Value::U32(x), DataType::F64) => Value::F64(x as f64),
        (Value::I32(x), DataType::F64) => Value::F64(x as f64),
        (Value::U32(x), DataType::F32) => Value::F32(x as f32),
        (Value::I32(x), DataType::F32) => Value::F32(x as f32),
        (Value::F32(x), DataType::F64) => Value::F64(x as f64),
        (Value::F64(x), DataType::F32) => Value::F32(x as f32),
        (Value::I32(x), DataType::I64) => Value::I64(x as i64),
        (Value::U32(x), DataType::U64) => Value::U64(x as u64),
        (other, _) => other,
    }
}

/// OPC UA DateTime ticks (1601-01-01, 100ns) → Unix ns（§7.3 精确保留）
pub(crate) fn ticks_to_unix_ns(ticks: i64) -> i64 {
    const TICKS_PER_SEC: i64 = 10_000_000;
    const UNIX_TICKS_OFFSET: i64 = 11644473600 * TICKS_PER_SEC;
    (ticks - UNIX_TICKS_OFFSET) * 100
}

fn source_timestamp_ns_from_dv(dv: &opcua_types::DataValue) -> Option<i64> {
    dv.source_timestamp.map(|dt| ticks_to_unix_ns(dt.ticks()))
}

/// 最后一次 GOOD 的缓存（§5.5）：值 + 其 SourceTimestamp，避免 LastKnown 携带 BAD 时的伪时间
#[derive(Debug, Clone)]
struct LastKnownSample {
    value: Value,
    source_timestamp_ns: Option<i64>,
}

/// 统一解码：Poll 与 Subscribe 共享（P0-A.2），避免双实现分叉
/// 语义冻结（V1.2.1）：
/// - GOOD + 有效 typed 值 → CURRENT，更新 last_known
/// - UNCERTAIN + 有效 typed 值 → CURRENT（质量 Uncertain），不更新 last_known
/// - BAD / 无值 / 类型不匹配 → LAST_KNOWN（有缓存）或 PLACEHOLDER（无缓存），Placeholder 时 source_timestamp=None
fn decode_data_value(
    spec: &PointSpec,
    point_id: u32,
    dv: opcua_types::DataValue,
    last_known: &mut HashMap<u32, LastKnownSample>,
) -> PointValue {
    use opcua_types::StatusCode;
    let status = dv.status.unwrap_or(StatusCode::Good);
    let source_ts_current = source_timestamp_ns_from_dv(&dv);

    // 尝试提取并转换 Variant → Value
    let maybe_raw = dv
        .value
        .as_ref()
        .and_then(crate::opcua_api::variant_to_value);

    // 类型兼容性校验（若有值）
    let maybe_coerced = maybe_raw.map(|v| coerce_value(v, spec.data_type));

    // GOOD 路径：必须有值且类型匹配 → CURRENT 并更新缓存
    if status.is_good() {
        if let Some(coerced) = maybe_coerced {
            if value_fits_data_type(&coerced, spec.data_type) {
                let sample = LastKnownSample {
                    value: coerced.clone(),
                    source_timestamp_ns: source_ts_current,
                };
                last_known.insert(point_id, sample);
                return PointValue {
                    point_id,
                    value: coerced,
                    quality: Quality::Good,
                    quality_code: None,
                    source_timestamp_ns: source_ts_current,
                    value_origin: ValueOrigin::Current,
                };
            } else {
                // GOOD 状态但类型不匹配 → BadTypeMismatch，按 BAD 隔离（P0-A.6）
                let cached = last_known.get(&point_id);
                let (val, origin, src) = match cached {
                    Some(s) => (
                        s.value.clone(),
                        ValueOrigin::LastKnown,
                        s.source_timestamp_ns,
                    ),
                    None => (
                        Value::typed_placeholder(spec.data_type),
                        ValueOrigin::Placeholder,
                        None,
                    ),
                };
                // Placeholder 强制 source=None
                let src = if origin == ValueOrigin::Placeholder {
                    None
                } else {
                    src
                };
                return PointValue {
                    point_id,
                    value: val,
                    quality: Quality::Bad,
                    quality_code: Some(StatusCode::BadTypeMismatch.bits() as i32),
                    source_timestamp_ns: src,
                    value_origin: origin,
                };
            }
        }
        // GOOD 但无值 → BadUnexpectedError，按 BAD 隔离
        let cached = last_known.get(&point_id);
        let (val, origin, src) = match cached {
            Some(s) => (
                s.value.clone(),
                ValueOrigin::LastKnown,
                s.source_timestamp_ns,
            ),
            None => (
                Value::typed_placeholder(spec.data_type),
                ValueOrigin::Placeholder,
                None,
            ),
        };
        let src = if origin == ValueOrigin::Placeholder {
            None
        } else {
            src
        };
        return PointValue {
            point_id,
            value: val,
            quality: Quality::Bad,
            quality_code: Some(StatusCode::BadUnexpectedError.bits() as i32),
            source_timestamp_ns: src,
            value_origin: origin,
        };
    }

    // UNCERTAIN：若有有效 typed 值则直接透传为 CURRENT（不更新 last_known），否则按 BAD 隔离
    if status.is_uncertain() {
        if let Some(coerced) = maybe_coerced {
            if value_fits_data_type(&coerced, spec.data_type) {
                return PointValue {
                    point_id,
                    value: coerced,
                    quality: Quality::Uncertain,
                    quality_code: Some(status.bits() as i32),
                    source_timestamp_ns: source_ts_current,
                    value_origin: ValueOrigin::Current,
                };
            } else {
                let cached = last_known.get(&point_id);
                let (val, origin, src) = match cached {
                    Some(s) => (
                        s.value.clone(),
                        ValueOrigin::LastKnown,
                        s.source_timestamp_ns,
                    ),
                    None => (
                        Value::typed_placeholder(spec.data_type),
                        ValueOrigin::Placeholder,
                        None,
                    ),
                };
                let src = if origin == ValueOrigin::Placeholder {
                    None
                } else {
                    src
                };
                return PointValue {
                    point_id,
                    value: val,
                    quality: Quality::Bad,
                    quality_code: Some(StatusCode::BadTypeMismatch.bits() as i32),
                    source_timestamp_ns: src,
                    value_origin: origin,
                };
            }
        }
        // UNCERTAIN 但无值 → 按 LastKnown/Placeholder 隔离，质量保持 Uncertain
        let cached = last_known.get(&point_id);
        let (val, origin, src) = match cached {
            Some(s) => (
                s.value.clone(),
                ValueOrigin::LastKnown,
                s.source_timestamp_ns,
            ),
            None => (
                Value::typed_placeholder(spec.data_type),
                ValueOrigin::Placeholder,
                None,
            ),
        };
        let src = if origin == ValueOrigin::Placeholder {
            None
        } else {
            src
        };
        return PointValue {
            point_id,
            value: val,
            quality: Quality::Uncertain,
            quality_code: Some(status.bits() as i32),
            source_timestamp_ns: src,
            value_origin: origin,
        };
    }

    // BAD：LastKnown（有缓存）或 Placeholder（无缓存），Placeholder 时 source=None
    let cached = last_known.get(&point_id);
    let (val, origin, src) = match cached {
        Some(s) => (
            s.value.clone(),
            ValueOrigin::LastKnown,
            s.source_timestamp_ns,
        ),
        None => (
            Value::typed_placeholder(spec.data_type),
            ValueOrigin::Placeholder,
            None,
        ),
    };
    let src = if origin == ValueOrigin::Placeholder {
        None
    } else {
        src
    };
    let q = crate::opcua_api::status_to_quality(status);
    PointValue {
        point_id,
        value: val,
        quality: q,
        quality_code: Some(status.bits() as i32),
        source_timestamp_ns: src,
        value_origin: origin,
    }
}

#[async_trait::async_trait]
impl DriverConnection for OpcUaConnection {
    /// OPC UA 动态探测：复用本连接的 transport（open 时与 adapter 共享同一 Arc，
    /// 与采集同一会话），流程见 [`crate::probe::probe_with_transport`]。
    async fn probe(&mut self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        crate::probe::probe_with_transport(&*self.transport).await
    }

    async fn configure(
        &mut self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        let mut new_points: Vec<PointSpec> = Vec::new();
        let mut new_tasks: Vec<TaskPlan> = Vec::new();

        for task in &tasks {
            task.validate()
                .map_err(|e| SdkDriverError::configuration("INVALID_TASK", e.to_string()))?;
            if task.binding.kind == GENERIC_BINDING_KIND {
                let binding: GenericBinding = serde_json::from_value(task.binding.config.clone())
                    .map_err(|e| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: invalid generic binding: {e}", task.id),
                    )
                })?;
                mesa_core_types::validate_selections_structure(&binding.selections)
                    .map_err(|e| SdkDriverError::configuration("INVALID_BINDING_CONFIG", e))?;
                if task.mode != TaskMode::Poll {
                    return Err(SdkDriverError::new(
                        mesa_core_types::ErrorKind::Unsupported,
                        "MODE_NOT_SUPPORTED",
                        format!("task `{}`: generic opcua only supports poll", task.id),
                    ));
                }
                let mut indices = Vec::new();
                for sel in &binding.selections {
                    if sel.resource_id != "node" {
                        return Err(SdkDriverError::configuration(
                            "UNSUPPORTED_RESOURCE",
                            format!("task `{}`: opcua generic only supports node", task.id),
                        ));
                    }
                    for out in &sel.outputs {
                        let node_id = sel
                            .parameters
                            .get("node_id")
                            .or_else(|| sel.parameters.get("address"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&out.output)
                            .to_string();
                        let dt_str = sel
                            .parameters
                            .get("data_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("STRING");
                        let addr = parse_address(&node_id).map_err(|e| match e {
                            AddressError::Empty => SdkDriverError::configuration(
                                "INVALID_ADDRESS",
                                format!("point `{}` node_id 为空", out.point_key),
                            ),
                            AddressError::Invalid { reason, .. } => SdkDriverError::new(
                                mesa_core_types::ErrorKind::Address,
                                "INVALID_ADDRESS",
                                format!(
                                    "point `{}` node_id `{node_id}` 非法: {reason}",
                                    out.point_key
                                ),
                            ),
                        })?;
                        let data_type = parse_data_type(dt_str)?;
                        indices.push(new_points.len());
                        new_points.push(PointSpec {
                            key: out.point_key.clone(),
                            addr,
                            data_type,
                        });
                    }
                }
                let interval = task.interval_ms.expect("validated");
                new_tasks.push(TaskPlan {
                    id: task.id.clone(),
                    kind: TaskKind::Poll {
                        interval_ms: interval,
                    },
                    point_indices: indices,
                });
                continue;
            }
            // Poll / Subscribe / Browse 三分支
            let is_poll = task.binding.kind == BINDING_POLL;
            let is_sub = task.binding.kind == BINDING_SUB;
            let is_browse = task.binding.kind == BINDING_BROWSE;
            if !is_poll && !is_sub && !is_browse {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: 期望 {BINDING_POLL}/{BINDING_SUB}/{BINDING_BROWSE} 或 {GENERIC_BINDING_KIND}，实际 {}",
                        task.id, task.binding.kind
                    ),
                ));
            }
            if is_poll && task.mode != TaskMode::Poll {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: opcua.node-group 仅支持 poll", task.id),
                ));
            }
            if is_sub && task.mode != TaskMode::Subscribe {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: opcua.subscription 仅支持 subscribe", task.id),
                ));
            }
            if is_browse && task.mode != TaskMode::Poll {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: opcua.browse 仅支持 poll", task.id),
                ));
            }
            // nodes 统一解析
            let nodes = task
                .binding
                .config
                .get("nodes")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: 缺少 nodes 数组", task.id),
                    )
                })?;
            if nodes.is_empty() {
                return Err(SdkDriverError::configuration(
                    "INVALID_BINDING_CONFIG",
                    format!("task `{}`: nodes 不能为空", task.id),
                ));
            }
            let mut indices = Vec::with_capacity(nodes.len());
            for node in nodes {
                let key = node.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
                    SdkDriverError::configuration(
                        "INVALID_POINT",
                        format!("task `{}`: node 缺少 key", task.id),
                    )
                })?;
                if key.trim().is_empty() {
                    return Err(SdkDriverError::configuration(
                        "INVALID_POINT",
                        "key 不能为空",
                    ));
                }
                let node_id = node
                    .get("node_id")
                    .or_else(|| node.get("nodeId"))
                    .or_else(|| node.get("address"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        SdkDriverError::configuration(
                            "INVALID_POINT",
                            format!("point `{key}` 缺少 node_id"),
                        )
                    })?;
                let dt_str = node
                    .get("data_type")
                    .or_else(|| node.get("dataType"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        SdkDriverError::configuration(
                            "INVALID_POINT",
                            format!("point `{key}` 缺少 data_type"),
                        )
                    })?;
                let addr = parse_address(node_id).map_err(|e| match e {
                    AddressError::Empty => SdkDriverError::configuration(
                        "INVALID_ADDRESS",
                        format!("point `{key}` node_id 为空"),
                    ),
                    AddressError::Invalid { reason, .. } => SdkDriverError::new(
                        mesa_core_types::ErrorKind::Address,
                        "INVALID_ADDRESS",
                        format!("point `{key}` node_id `{node_id}` 非法: {reason}"),
                    ),
                })?;
                let data_type = parse_data_type(dt_str)?;
                indices.push(new_points.len());
                new_points.push(PointSpec {
                    key: key.to_string(),
                    addr,
                    data_type,
                });
            }
            if is_poll {
                let interval = task.interval_ms.expect("validated");
                if interval == 0 {
                    return Err(SdkDriverError::configuration(
                        "INVALID_TASK",
                        "interval_ms 需 >0",
                    ));
                }
                new_tasks.push(TaskPlan {
                    id: task.id.clone(),
                    kind: TaskKind::Poll {
                        interval_ms: interval,
                    },
                    point_indices: indices,
                });
            } else if is_browse {
                let interval = task.interval_ms.expect("validated");
                if interval == 0 {
                    return Err(SdkDriverError::configuration(
                        "INVALID_TASK",
                        "interval_ms 需 >0",
                    ));
                }
                new_tasks.push(TaskPlan {
                    id: task.id.clone(),
                    kind: TaskKind::Browse {
                        interval_ms: interval,
                    },
                    point_indices: indices,
                });
            } else {
                // Subscribe 参数：publishing_interval_ms / sampling_interval_ms / queue_size / discard_oldest
                let publishing_interval_ms = task
                    .binding
                    .config
                    .get("publishing_interval_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(500);
                let sampling_interval_ms = task
                    .binding
                    .config
                    .get("sampling_interval_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(250);
                let queue_size = task
                    .binding
                    .config
                    .get("queue_size")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10) as u32;
                let discard_oldest = task
                    .binding
                    .config
                    .get("discard_oldest")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if publishing_interval_ms == 0 || sampling_interval_ms == 0 || queue_size == 0 {
                    return Err(SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: publishing/sampling/queue 需 >0", task.id),
                    ));
                }
                new_tasks.push(TaskPlan {
                    id: task.id.clone(),
                    kind: TaskKind::Subscribe {
                        publishing_interval_ms,
                        sampling_interval_ms,
                        queue_size,
                        discard_oldest,
                    },
                    point_indices: indices,
                });
            }
        }

        let descriptors: Vec<PointDescriptor> = new_points
            .iter()
            .map(|p| PointDescriptor {
                point_key: p.key.clone(),
                data_type: p.data_type,
                unit: None,
            })
            .collect();
        ensure_unique_point_keys(&descriptors).map_err(|DuplicatePointKey(k)| {
            SdkDriverError::configuration("DUPLICATE_POINT_KEY", format!("`{k}` 重复"))
        })?;

        tracing::info!(
            revision,
            points = new_points.len(),
            tasks = new_tasks.len(),
            "OPC UA 采集计划构建完成"
        );
        self.plan = Some(PlanSnapshot {
            revision,
            points: new_points,
            tasks: new_tasks,
            map: None,
        });
        Ok(descriptors)
    }

    async fn apply_point_map(&mut self, map: PointMap) -> Result<(), SdkDriverError> {
        let snap = self.plan.as_mut().ok_or_else(|| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Internal,
                "NOT_CONFIGURED",
                "apply 在 configure 之前",
            )
        })?;
        for p in &snap.points {
            if !map.contains_key(&p.key) {
                return Err(SdkDriverError::configuration(
                    "MISSING_POINT_ID",
                    format!("point `{}` 缺少映射", p.key),
                ));
            }
        }
        snap.map = Some(map);
        Ok(())
    }

    /// 事件任务配置（PR9 Stage ③）：只接受 `mesa.events.v1` 标准 binding；
    /// 全部解析成功才原子替换旧计划（Stage ⑤ run() 按快照起 Event workers）。
    async fn configure_events(
        &mut self,
        revision: u64,
        tasks: Vec<mesa_core_types::EventTask>,
    ) -> Result<(), SdkDriverError> {
        let plans = event::parse_event_tasks(&tasks)?;
        tracing::info!(revision, tasks = plans.len(), "opcua event plan built");
        self.event_plan = Some(OpcUaEventPlanSnapshot {
            revision,
            tasks: plans,
        });
        Ok(())
    }

    async fn browse(
        &mut self,
        parent: &str,
        filter: &str,
        cursor: &str,
        limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), SdkDriverError> {
        // 确保已连接（browse 可能在 run 之外被调用）
        if let Err(e) = self
            .api
            .connect(&self.cfg.endpoint_url, self.cfg.timeout_ms)
            .await
        {
            return Err(SdkDriverError::new(
                mesa_core_types::ErrorKind::Connection,
                "CONNECT_FAILED",
                e,
            ));
        }
        let parent_str = if parent.is_empty() {
            format!("nsu={};i=85", mesa_opcua_transport::OPC_BASE_NAMESPACE_URI)
        } else {
            parent.to_string()
        };
        let mut parent_addr = parse_address(&parent_str).map_err(|e| match e {
            AddressError::Empty => SdkDriverError::configuration("INVALID_ADDRESS", "parent 为空"),
            AddressError::Invalid { reason, .. } => SdkDriverError::new(
                mesa_core_types::ErrorKind::Address,
                "INVALID_ADDRESS",
                format!("parent `{parent_str}` 非法: {reason}"),
            ),
        })?;
        let namespaces = self.transport.read_namespace_array().await.map_err(|e| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Connection,
                "BROWSE_FAILED",
                format!("NamespaceArray 读取失败: {e}"),
            )
        })?;
        parent_addr.resolve(&namespaces).map_err(|e| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Address,
                "UNKNOWN_NAMESPACE",
                format!("parent `{parent_str}`: {e}"),
            )
        })?;
        let children = self.api.browse(&parent_addr).await.map_err(|e| {
            SdkDriverError::new(mesa_core_types::ErrorKind::Internal, "BROWSE_FAILED", e)
        })?;
        // 子节点 index → canonical（越界项跳过并告警，不杀整页）。
        let mut canonical_children: Vec<(String, String)> = Vec::with_capacity(children.len());
        for c in &children {
            match mesa_opcua_transport::OpcUaNodeId::from_index_ref(&c.node, &namespaces) {
                Some(id) => canonical_children.push((c.name.clone(), id.canonical_key())),
                None => tracing::warn!(
                    name = %c.name,
                    ns = c.node.namespace,
                    "browse 子节点命名空间越界，已跳过",
                ),
            }
        }
        // 过滤与分页（过滤同时匹配展示名与 canonical 身份）。
        let filtered: Vec<(String, String)> = canonical_children
            .into_iter()
            .filter(|(name, id)| filter.is_empty() || name.contains(filter) || id.contains(filter))
            .collect();
        let start = cursor.parse::<usize>().unwrap_or(0);
        let lim = if limit == 0 { 50 } else { limit as usize };
        let end = (start + lim).min(filtered.len());
        let slice = &filtered[start..end];
        let next_cursor = if end < filtered.len() {
            Some(end.to_string())
        } else {
            None
        };
        let nodes = slice
            .iter()
            .map(|(name, node_id)| mesa_driver_protocol::pb::BrowseNode {
                id: node_id.clone(),
                label: name.clone(),
                kind: "node".into(),
                data_type: "String".into(),
                access: "read".into(),
                has_children: true,
                binding_json: serde_json::json!({"node_id": node_id, "data_type": "String"})
                    .to_string(),
            })
            .collect();
        Ok((nodes, next_cursor))
    }

    async fn run(
        &mut self,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError> {
        // §18：一等 Event-only——Data 计划缺席/空表不再是错误；PointMap 只在
        // 有 Data 任务时要求。Event-only 端点无需任何 Data point 即可 Start。
        let has_data_tasks = self
            .plan
            .as_ref()
            .map(|s| !s.tasks.is_empty())
            .unwrap_or(false);

        // 建会话：Fake 即时成功；Native 失败则由 Manager 退避
        if let Err(e) = self
            .api
            .connect(&self.cfg.endpoint_url, self.cfg.timeout_ms)
            .await
        {
            if e.contains("NOT_IMPLEMENTED") || e.contains("未实现") {
                return Err(SdkDriverError::configuration("NOT_IMPLEMENTED", e));
            } else {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Connection,
                    "CONNECT_FAILED",
                    e,
                ));
            }
        }

        // canonical → index：建会话后读新鲜 NamespaceArray，全点一次换算
        // （漂移自愈；未知 URI 即 fail-closed，绝不带着过期 index 运行）。
        // Fake 路径不消费 index（identifier 直通），Native 路径此处全部落定。
        // 快照同时供 Event workers 换算 notifier（run 期一次读取）。
        // NOTE: 必须在 data_plan 不可变借用之前完成（&mut 先结束，NLL 允许后借）。
        let namespaces = self.transport.read_namespace_array().await.map_err(|e| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Connection,
                "NAMESPACE_FAILED",
                format!("NamespaceArray 读取失败: {e}"),
            )
        })?;
        if let Some(snap) = self.plan.as_mut() {
            for p in &mut snap.points {
                p.addr.resolve(&namespaces).map_err(|e| {
                    SdkDriverError::new(
                        mesa_core_types::ErrorKind::Address,
                        "UNKNOWN_NAMESPACE",
                        format!("point `{}`: {e}", p.key),
                    )
                })?;
            }
        }
        let namespaces = Arc::new(namespaces);

        let data_plan = if has_data_tasks {
            Some(self.plan.as_ref().ok_or_else(|| {
                SdkDriverError::new(
                    mesa_core_types::ErrorKind::Internal,
                    "NO_PLAN",
                    "run 前未 configure+apply",
                )
            })?)
        } else {
            None
        };
        let data_map = match data_plan {
            None => None,
            Some(snap) => Some(snap.map.as_ref().ok_or_else(|| {
                SdkDriverError::new(
                    mesa_core_types::ErrorKind::Internal,
                    "NO_POINT_MAP",
                    "run 前未 apply_point_map",
                )
            })?),
        };
        let event_tasks: Vec<event::OpcUaEventTaskPlan> = self
            .event_plan
            .as_ref()
            .map(|e| e.tasks.clone())
            .unwrap_or_default();

        use std::sync::atomic::{AtomicU64, Ordering};
        let seq = Arc::new(AtomicU64::new(1));
        let shared_api = Arc::clone(&self.api);
        // §20 轻量 supervisor：任一 child Err 即记 first error + cancel 全体，
        // reap 全部后统一收尾（旧顺序 await 会被 hung worker 挡住错误传播）。
        let mut set: tokio::task::JoinSet<Result<(), SdkDriverError>> = tokio::task::JoinSet::new();
        if let (Some(snap), Some(map)) = (data_plan, data_map) {
            for task in &snap.tasks {
                let indices = task.point_indices.clone();
                let points: Vec<(PointSpec, u32)> = indices
                    .iter()
                    .map(|&i| {
                        let p = snap.points[i].clone();
                        let pid = map[&p.key];
                        (p, pid)
                    })
                    .collect();
                let sink = sink.clone();
                let shutdown = shutdown.clone();
                let seq = Arc::clone(&seq);
                let api = Arc::clone(&shared_api);
                let task_id = task.id.clone();
                let kind = task.kind.clone();
                match kind {
                    TaskKind::Poll { interval_ms } => {
                        let interval = Duration::from_millis(interval_ms);
                        set.spawn(async move {
                        let mut ticker = tokio::time::interval(interval);
                        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        // §5.5 last-known 连续性：按 point_id 缓存最近 GOOD 的 typed 值
                        let mut last_known: HashMap<u32, LastKnownSample> = HashMap::new();
                        loop {
                            tokio::select! {
                                _ = ticker.tick() => {},
                                _ = shutdown.cancelled() => break,
                            }
                            let addrs: Vec<OpcUaAddress> = points.iter().map(|(s, _)| s.addr.clone()).collect();
                            let data_values = match api.read_batch(&addrs).await {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::error!(task=%task_id, error=%e, "OPC UA 读失败");
                                    return Err(SdkDriverError::new(mesa_core_types::ErrorKind::Connection, "READ_FAILED", e));
                                }
                            };
                            if data_values.len() != points.len() {
                                tracing::warn!(task=%task_id, got=data_values.len(), expected=points.len(), "OPC UA 返回数量不一致");
                                continue;
                            }
                            let mut batch_vals = Vec::with_capacity(points.len());
                            for ((spec, pid), dv) in points.iter().zip(data_values) {
                                let pv = decode_data_value(spec, *pid, dv, &mut last_known);
                                batch_vals.push(pv);
                            }
                            if batch_vals.is_empty() { continue; }
                            sink.publish(DataBatch {
                                connection_handle: 0,
                                stream_epoch: 0,
                                sequence: seq.fetch_add(1, Ordering::Relaxed),
                                timestamp_ns: now_unix_ns(),
                                values: batch_vals,
                                        mono_ns: None,
        }).await;
                        }
                        Ok::<(), SdkDriverError>(())
                    });
                    }
                    TaskKind::Browse { interval_ms } => {
                        let interval = Duration::from_millis(interval_ms);
                        let namespaces = Arc::clone(&namespaces);
                        set.spawn(async move {
                        let mut ticker = tokio::time::interval(interval);
                        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {
                                _ = ticker.tick() => {},
                                _ = shutdown.cancelled() => break,
                            }
                            let mut batch_vals = Vec::with_capacity(points.len());
                            for (spec, pid) in &points {
                                match api.browse(&spec.addr).await {
                                    Ok(refs) => {
                                        // 子节点 canonical 身份（越界项跳过并告警，不杀整轮）。
                                        let mut parts = Vec::with_capacity(refs.len());
                                        for c in &refs {
                                            match mesa_opcua_transport::OpcUaNodeId::from_index_ref(
                                                &c.node,
                                                &namespaces,
                                            ) {
                                                Some(id) => parts.push(format!(
                                                    "{} {}",
                                                    c.name,
                                                    id.canonical_key()
                                                )),
                                                None => tracing::warn!(
                                                    key = %spec.key,
                                                    name = %c.name,
                                                    ns = c.node.namespace,
                                                    "Browse 子节点命名空间越界，已跳过",
                                                ),
                                            }
                                        }
                                        let val = Value::String(parts.join(";"));
                                        batch_vals.push(PointValue {
                                            point_id: *pid,
                                            value: val,
                                            quality: Quality::Good,
                                            quality_code: None,
                                            source_timestamp_ns: None,
                                            value_origin: ValueOrigin::Current,
                                        });
                                    }
                                    Err(e) => {
                                        tracing::warn!(key=%spec.key, error=%e, "Browse 单点 Bad");
                                        // Browse 失败亦为 typed BAD + Placeholder（无 last-known 缓存，Browse 非连续信号）
                                        batch_vals.push(PointValue {
                                            point_id: *pid,
                                            value: Value::typed_placeholder(spec.data_type),
                                            quality: Quality::Bad,
                                            quality_code: Some(
                                                opcua_types::StatusCode::BadUnexpectedError.bits()
                                                    as i32,
                                            ),
                                            source_timestamp_ns: None,
                                            value_origin: ValueOrigin::Placeholder,
                                        });
                                    }
                                }
                            }
                            if batch_vals.is_empty() {
                                continue;
                            }
                            sink.publish(DataBatch {
                                connection_handle: 0,
                                stream_epoch: 0,
                                sequence: seq.fetch_add(1, Ordering::Relaxed),
                                timestamp_ns: now_unix_ns(),
                                values: batch_vals,
                                mono_ns: None,
                            })
                            .await;
                        }
                        Ok::<(), SdkDriverError>(())
                    });
                    }
                    TaskKind::Subscribe {
                        publishing_interval_ms,
                        sampling_interval_ms,
                        queue_size,
                        discard_oldest,
                    } => {
                        let addrs: Vec<OpcUaAddress> =
                            points.iter().map(|(s, _)| s.addr.clone()).collect();
                        // handle -> (spec, pid) 映射，client_handle = idx+1
                        let mut handle_map: HashMap<u32, (PointSpec, u32)> = HashMap::new();
                        for (idx, (spec, pid)) in points.iter().enumerate() {
                            handle_map.insert((idx as u32) + 1, (spec.clone(), *pid));
                        }
                        let handle_map = Arc::new(handle_map);
                        set.spawn(async move {
                        let (sub_id, mut rx) = match api.subscribe(&addrs, publishing_interval_ms, sampling_interval_ms, queue_size, discard_oldest).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!(task=%task_id, error=%e, "OPC UA 订阅失败");
                                return Err(SdkDriverError::new(mesa_core_types::ErrorKind::Connection, "SUBSCRIBE_FAILED", e));
                            }
                        };
                        tracing::info!(task=%task_id, sub_id=%sub_id, "OPC UA 订阅已建立");
                        let mut last_known: HashMap<u32, LastKnownSample> = HashMap::new();
                        // 订阅事件循环：DataChangeCallback → mpsc → 批量聚合 → DataSink，KeepAlive 自然无事件不产批
                        loop {
                            let first = tokio::select! {
                                ev = rx.recv() => ev,
                                _ = shutdown.cancelled() => break,
                            };
                            let Some(first_ev) = first else {
                                tracing::warn!(task=%task_id, "订阅通道关闭");
                                break;
                            };
                            // 收集当前已就绪的全部事件，合并为一个 DataBatch（Latest-Wins 由 Sink 承接）
                            let mut events = vec![first_ev];
                            while let Ok(ev) = rx.try_recv() {
                                events.push(ev);
                                if events.len() >= 64 { break; }
                            }
                            let mut batch_vals = Vec::with_capacity(events.len());
                            for ev in events {
                                let Some((spec, pid)) = handle_map.get(&ev.client_handle) else {
                                    tracing::warn!(task=%task_id, handle=%ev.client_handle, "未知 client_handle");
                                    continue;
                                };
                                let pv = decode_data_value(spec, *pid, ev.data_value, &mut last_known);
                                batch_vals.push(pv);
                            }
                            if batch_vals.is_empty() {
                                // KeepAlive 或全过滤：不递增 sequence，不产批 (§7.3)
                                continue;
                            }
                            sink.publish(DataBatch {
                                connection_handle: 0,
                                stream_epoch: 0,
                                sequence: seq.fetch_add(1, Ordering::Relaxed),
                                timestamp_ns: now_unix_ns(),
                                values: batch_vals,
                                        mono_ns: None,
        }).await;
                        }
                        // 清理订阅：失败不影响 shutdown 返回，但必须 warn
                        //（P1-B7 保留的 cleanup error 到此是最后可见机会）。
                        if let Err(e) = api.unsubscribe(sub_id).await {
                            tracing::warn!(sub_id, error = %e, "shutdown 清理订阅失败（仅诊断）");
                        }
                        Ok::<(), SdkDriverError>(())
                    });
                    }
                }
            } // end for task in data_tasks
        } // end if-let data tasks
        // Event workers：与 Data 共享同一 Session（transport Arc 唯一），
        // 但队列语义隔离（FIFO fail-closed vs Latest-Wins，§19）。
        // NamespaceArray 快照与 Data 路径共享（run 期一次读取）。
        if !event_tasks.is_empty() {
            let event_sink = sink.events();
            for task in event_tasks {
                set.spawn(event_runtime::run_event_task(
                    Arc::clone(&self.transport),
                    task,
                    Arc::clone(&namespaces),
                    event_sink.clone(),
                    shutdown.clone(),
                ));
            }
        }

        let mut final_err: Option<SdkDriverError> = None;
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if final_err.is_none() {
                        final_err = Some(e);
                    }
                    // 任一 child Err 即 cancel 全体（§20），继续 reap 剩余 workers。
                    shutdown.cancel();
                }
                Err(join_err) => {
                    tracing::error!(%join_err, "OPC UA 任务 panic");
                    if final_err.is_none() {
                        final_err = Some(SdkDriverError::new(
                            mesa_core_types::ErrorKind::Internal,
                            "TASK_PANIC",
                            join_err.to_string(),
                        ));
                    }
                    shutdown.cancel();
                }
            }
        }
        // §21 连接 teardown 末端：best-effort disconnect（会话槽位清空，
        // 下次 Start 经 ensure_session 重建）。失败仅诊断：不掩盖原始错误，
        // 不让干净 Stop 失败。必须加 bound：对已死会话发 CloseSession 可能
        // 永不返回（P0-3），teardown 挂起会把已经判定的 Err/Ok 拖成永久 RUNNING。
        let dc_timeout = std::time::Duration::from_millis(self.cfg.timeout_ms);
        match tokio::time::timeout(dc_timeout, self.transport.disconnect()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "OPC UA run 结束 disconnect 失败（仅诊断）");
            }
            Err(_) => {
                tracing::warn!(
                    "OPC UA run 结束 disconnect 超时（仅诊断，会话槽位已在 transport 内清空）"
                );
            }
        }
        if let Some(e) = final_err {
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode};

    fn task_with_nodes(nodes: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: BINDING_POLL.into(),
                config: serde_json::json!({"nodes": nodes}),
            },
        }
    }

    /// §18 硬门：Event-only 端点（零 Data point）必须能 Start，
    /// 事件到达，全程不要求 PointMap。
    #[tokio::test]
    async fn event_only_endpoint_starts_without_data_points() {
        use mesa_core_types::{DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND};
        use mesa_driver_sdk::DataSink;
        use mesa_opcua_transport::{FakeOpcUaTransport, UaEventNotification};
        use opcua_types::{ByteString, DateTime, LocalizedText, UAString, Variant as V};

        let namespaces = vec![
            "http://opcfoundation.org/UA/".to_string(),
            "http://example.com/Other/".to_string(),
            "http://example.com/MyModel/".to_string(),
        ];
        let dt = |ns: i64| {
            V::DateTime(Box::new(DateTime::from(
                ns / 100 + 11644473600 * 10_000_000,
            )))
        };
        let mut fields = vec![
            V::ByteString(ByteString::from(vec![9u8, 9, 9])), // EventId
            V::NodeId(Box::new(opcua_types::NodeId::new(0, 2041u32))), // EventType
            V::NodeId(Box::new(opcua_types::NodeId::new(0, 2253u32))), // SourceNode
            V::String(UAString::from("MesaFixture")),         // SourceName
            dt(1_700_000_000_000_000_000),                    // Time
            dt(1_700_000_000_500_000_000),                    // ReceiveTime
            V::LocalizedText(Box::new(LocalizedText::new("", "event-only"))), // Message
            V::UInt16(100),                                   // Severity
        ];
        fields.extend(std::iter::repeat_n(V::Empty, 19 - fields.len()));
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(namespaces)
                .with_event_notifications(vec![UaEventNotification {
                    client_handle: 1,
                    fields,
                }]),
        );
        let mut conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: fake.clone(),
            plan: None,
            event_plan: None,
        };
        // 零 Data 任务 + 一个 generic 事件任务（configure_events 用 [] 语义清空亦可）。
        conn.configure(1, vec![]).await.unwrap();
        conn.configure_events(
            1,
            vec![EventTask {
                id: "ev-only".into(),
                mode: TaskMode::Subscribe,
                interval_ms: None,
                binding: DriverBinding {
                    kind: GENERIC_EVENT_BINDING_KIND.into(),
                    config: serde_json::json!({
                        "stream_id": "opcua.events",
                        "parameters": {},
                    }),
                },
            }],
        )
        .await
        .unwrap();
        // 注意：刻意不调 apply_point_map——Event-only 不得要求它。
        let (ctrl_tx, _ctrl_rx) =
            tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (data_tx, _data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(8);
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
        let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx).for_connection(7, 1);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        let batch = tokio::time::timeout(std::time::Duration::from_secs(5), event_rx.recv())
            .await
            .expect("事件必须到达（Event-only Start 成功）")
            .expect("通道不得关闭");
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].message.as_deref(), Some("event-only"));
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run_handle)
            .await
            .expect("run 必须退出")
            .expect("run 不 panic")
            .expect("正常 Stop 必须 Ok");
        // 清理发生且会话已断开（teardown 末端 disconnect）。
        assert_eq!(fake.deleted_subscriptions().len(), 1);
    }

    #[tokio::test]
    async fn configure_ok_and_duplicate_rejected() {
        let mut conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: None,
            event_plan: None,
        };
        let nodes = serde_json::json!([
            {"key":"a","node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"U32"},
            {"key":"b","node_id":"nsu=http://example.com/MyModel/;s=Motor.Speed","data_type":"F64"}
        ]);
        let t = task_with_nodes(nodes);
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 2);
        let dup = serde_json::json!([
            {"key":"a","node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"U32"},
            {"key":"a","node_id":"nsu=http://example.com/MyModel/;i=3","data_type":"U32"}
        ]);
        let err = conn
            .configure(2, vec![task_with_nodes(dup)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");
    }

    #[tokio::test]
    async fn invalid_node_id_rejected() {
        let mut conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: None,
            event_plan: None,
        };
        let nodes = serde_json::json!([{"key":"a","node_id":"ns=2;x=1","data_type":"U32"}]);
        let err = conn
            .configure(1, vec![task_with_nodes(nodes)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ADDRESS");
    }

    #[tokio::test]
    async fn subscribe_configure_ok() {
        let mut conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: None,
            event_plan: None,
        };
        let t = AcquisitionTask {
            id: "s1".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: BINDING_SUB.into(),
                config: serde_json::json!({"publishing_interval_ms":500,"sampling_interval_ms":250,"queue_size":10,"nodes":[{"key":"a","node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"U32"}]}),
            },
        };
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 1);
        // Subscribe 默认参数容错
        let t2 = AcquisitionTask {
            id: "s2".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: BINDING_SUB.into(),
                config: serde_json::json!({"nodes":[{"key":"b","node_id":"nsu=http://example.com/MyModel/;s=MyVar","data_type":"STRING"}]}),
            },
        };
        let descs2 = conn.configure(2, vec![t2]).await.unwrap();
        assert_eq!(descs2.len(), 1);
    }

    #[tokio::test]
    async fn configure_apply_ok() {
        let mut conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: None,
            event_plan: None,
        };
        let nodes = serde_json::json!([{"key":"a","node_id":"nsu=http://example.com/MyModel/;s=Counter","data_type":"U32"}]);
        let t = task_with_nodes(nodes);
        let descs = conn.configure(1, vec![t]).await.unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert(descs[0].point_key.clone(), 1u32);
        conn.apply_point_map(map).await.unwrap();
        // 能走到 apply 即代表 Poll 快照构建正确；run 的 DataSink 发布由集成测试覆盖
    }

    // --- P0-A.9 GOOD→BAD→GOOD / first BAD / legacy / REST 契约测试（V1.2.1 Gate） ---

    fn dv_good_f64(val: f64, ticks: i64) -> opcua_types::DataValue {
        use opcua_types::{DataValue, DateTime, StatusCode, Variant};
        DataValue {
            value: Some(Variant::Double(val)),
            status: Some(StatusCode::Good),
            source_timestamp: Some(DateTime::from(ticks)),
            source_picoseconds: None,
            server_timestamp: Some(DateTime::now()),
            server_picoseconds: None,
        }
    }

    fn dv_bad(status: opcua_types::StatusCode, ticks: i64) -> opcua_types::DataValue {
        use opcua_types::{DataValue, DateTime};
        DataValue {
            value: None,
            status: Some(status),
            source_timestamp: Some(DateTime::from(ticks)),
            source_picoseconds: None,
            server_timestamp: Some(DateTime::now()),
            server_picoseconds: None,
        }
    }

    fn point_spec_f64(key: &str) -> PointSpec {
        PointSpec {
            key: key.into(),
            addr: crate::address::parse_address("nsu=http://example.com/MyModel/;i=2").unwrap(),
            data_type: mesa_core_types::DataType::F64,
        }
    }

    #[test]
    fn first_bad_no_history_placeholder() {
        use std::collections::HashMap;
        let spec = point_spec_f64("k1");
        let mut last = HashMap::new();
        // 首次即 BAD，无历史 → Placeholder + typed neutral + source None
        const TICKS: i64 = 11644473600 * 10_000_000 + 1_000_000;
        let dv = dv_bad(opcua_types::StatusCode::BadNodeIdUnknown, TICKS);
        let pv = decode_data_value(&spec, 1, dv, &mut last);
        assert_eq!(pv.quality, Quality::Bad);
        assert_eq!(pv.value_origin, ValueOrigin::Placeholder);
        assert_eq!(
            pv.value,
            Value::typed_placeholder(mesa_core_types::DataType::F64)
        );
        assert_eq!(
            pv.source_timestamp_ns, None,
            "Placeholder 必须无 source timestamp"
        );
        assert!(pv.quality_code.is_some());
    }

    #[test]
    fn good_then_bad_last_known() {
        use std::collections::HashMap;
        let spec = point_spec_f64("k1");
        let mut last = HashMap::new();
        const TICKS_GOOD: i64 = 11644473600 * 10_000_000 + 1_000_000; // 1970+100ms
        const TICKS_BAD: i64 = TICKS_GOOD + 5_000_000; // +500ms
        let pv_good = decode_data_value(&spec, 1, dv_good_f64(12.5, TICKS_GOOD), &mut last);
        assert_eq!(pv_good.value_origin, ValueOrigin::Current);
        assert_eq!(
            pv_good.source_timestamp_ns,
            Some(ticks_to_unix_ns(TICKS_GOOD))
        );
        // BAD 时使用缓存 GOOD 的值与时间
        let pv_bad = decode_data_value(
            &spec,
            1,
            dv_bad(opcua_types::StatusCode::BadTimeout, TICKS_BAD),
            &mut last,
        );
        assert_eq!(pv_bad.value_origin, ValueOrigin::LastKnown);
        assert_eq!(pv_bad.value, Value::F64(12.5));
        assert_eq!(
            pv_bad.source_timestamp_ns,
            Some(ticks_to_unix_ns(TICKS_GOOD)),
            "LastKnown 必须携带 GOOD 的 SourceTimestamp"
        );
        assert_eq!(pv_bad.quality, Quality::Bad);
    }

    #[test]
    fn good_bad_good_restores_current() {
        use std::collections::HashMap;
        let spec = point_spec_f64("k1");
        let mut last = HashMap::new();
        const T1: i64 = 11644473600 * 10_000_000 + 1_000_000;
        const T2: i64 = T1 + 5_000_000;
        const T3: i64 = T1 + 10_000_000;
        let p1 = decode_data_value(&spec, 1, dv_good_f64(12.5, T1), &mut last);
        assert_eq!(p1.value_origin, ValueOrigin::Current);
        let p2 = decode_data_value(
            &spec,
            1,
            dv_bad(opcua_types::StatusCode::BadTimeout, T2),
            &mut last,
        );
        assert_eq!(p2.value_origin, ValueOrigin::LastKnown);
        let p3 = decode_data_value(&spec, 1, dv_good_f64(13.2, T3), &mut last);
        assert_eq!(p3.value_origin, ValueOrigin::Current);
        assert_eq!(p3.value, Value::F64(13.2));
        assert_eq!(p3.source_timestamp_ns, Some(ticks_to_unix_ns(T3)));
        assert_eq!(p3.quality, Quality::Good);
    }

    #[test]
    fn sibling_isolation_bad_does_not_poison_other() {
        use std::collections::HashMap;
        let spec_a = point_spec_f64("kA");
        let spec_b = PointSpec {
            key: "kB".into(),
            addr: crate::address::parse_address("nsu=http://example.com/MyModel/;i=3").unwrap(),
            data_type: mesa_core_types::DataType::F64,
        };
        let mut last: HashMap<u32, LastKnownSample> = HashMap::new();
        // 先给 B 一个 GOOD 缓存
        let _ = decode_data_value(
            &spec_b,
            2,
            dv_good_f64(99.0, 11644473600 * 10_000_000 + 1_000),
            &mut last,
        );
        // A 首次 BAD → Placeholder，B 不受影响仍为 LastKnown/后续 Good？
        const T_BAD: i64 = 11644473600 * 10_000_000 + 5_000_000;
        let pv_a_bad = decode_data_value(
            &spec_a,
            1,
            dv_bad(opcua_types::StatusCode::BadNodeIdUnknown, T_BAD),
            &mut last,
        );
        assert_eq!(pv_a_bad.value_origin, ValueOrigin::Placeholder);
        // B 再次 BAD 应为 LastKnown 99.0
        let pv_b_bad = decode_data_value(
            &spec_b,
            2,
            dv_bad(opcua_types::StatusCode::BadTimeout, T_BAD + 1_000),
            &mut last,
        );
        assert_eq!(pv_b_bad.value_origin, ValueOrigin::LastKnown);
        assert_eq!(pv_b_bad.value, Value::F64(99.0));
        // A 的 GOOD 不影响 B 的缓存
        let pv_a_good = decode_data_value(&spec_a, 1, dv_good_f64(1.1, T_BAD + 2_000), &mut last);
        assert_eq!(pv_a_good.value_origin, ValueOrigin::Current);
        let pv_b_bad2 = decode_data_value(
            &spec_b,
            2,
            dv_bad(opcua_types::StatusCode::BadTimeout, T_BAD + 3_000),
            &mut last,
        );
        assert_eq!(pv_b_bad2.value, Value::F64(99.0));
    }

    #[test]
    fn uncertain_valid_value_is_current_not_last_known() {
        use std::collections::HashMap;
        let spec = point_spec_f64("k1");
        let mut last = HashMap::new();
        const T_GOOD: i64 = 11644473600 * 10_000_000 + 1_000_000;
        const T_UNCERTAIN: i64 = T_GOOD + 5_000_000;
        const T_BAD: i64 = T_UNCERTAIN + 5_000_000;
        let _ = decode_data_value(&spec, 1, dv_good_f64(10.0, T_GOOD), &mut last);
        // UNCERTAIN + 有效值 → Current，质量 Uncertain，不更新 last_known
        let dv_uncertain = opcua_types::DataValue {
            value: Some(opcua_types::Variant::Double(12.8)),
            status: Some(opcua_types::StatusCode::Uncertain),
            source_timestamp: Some(opcua_types::DateTime::from(T_UNCERTAIN)),
            source_picoseconds: None,
            server_timestamp: None,
            server_picoseconds: None,
        };
        let pv_u = decode_data_value(&spec, 1, dv_uncertain, &mut last);
        assert_eq!(pv_u.quality, Quality::Uncertain);
        assert_eq!(pv_u.value_origin, ValueOrigin::Current);
        assert_eq!(pv_u.value, Value::F64(12.8));
        // 随后 BAD 应仍回退到 10.0（GOOD），而非 12.8（UNCERTAIN 未更新缓存）
        let pv_bad = decode_data_value(
            &spec,
            1,
            dv_bad(opcua_types::StatusCode::BadTimeout, T_BAD),
            &mut last,
        );
        assert_eq!(pv_bad.value, Value::F64(10.0));
        assert_eq!(pv_bad.value_origin, ValueOrigin::LastKnown);
    }

    #[test]
    fn type_mismatch_produces_bad_type_mismatch_not_good_code() {
        use std::collections::HashMap;
        let spec = PointSpec {
            key: "k1".into(),
            addr: crate::address::parse_address("nsu=http://example.com/MyModel/;s=MyString")
                .unwrap(),
            data_type: mesa_core_types::DataType::F64,
        };
        let mut last = HashMap::new();
        // GOOD 状态但 String 值与 F64 不兼容 → BadTypeMismatch，Placeholder
        let dv = opcua_types::DataValue {
            value: Some(opcua_types::Variant::String(opcua_types::UAString::from(
                "not a number",
            ))),
            status: Some(opcua_types::StatusCode::Good),
            source_timestamp: Some(opcua_types::DateTime::now()),
            source_picoseconds: None,
            server_timestamp: None,
            server_picoseconds: None,
        };
        let pv = decode_data_value(&spec, 1, dv, &mut last);
        assert_eq!(pv.quality, Quality::Bad);
        assert_eq!(
            pv.quality_code,
            Some(opcua_types::StatusCode::BadTypeMismatch.bits() as i32)
        );
        assert_eq!(pv.value_origin, ValueOrigin::Placeholder);
        assert_ne!(pv.quality_code, Some(0));
    }
}
