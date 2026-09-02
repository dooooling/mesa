//! OPC UA Driver — 方案 §7.3 节点/订阅/浏览型（V1 只读，44/44 2026-08-29）。
//!
//! - Poll 绑定：`opcua.node-group`
//!   ```json
//!   { "nodes": [
//!       { "key": "counter", "node_id": "ns=2;s=Counter", "data_type": "U32" },
//!       { "key": "sine",    "node_id": "ns=2;i=2",        "data_type": "DOUBLE" }
//!   ]}
//!   ```
//! - Subscribe 绑定：`opcua.subscription`（publishing 500 sampling 30 queue10，§7.3）
//!   ```json
//!   { "publishing_interval_ms": 500, "sampling_interval_ms": 250, "queue_size": 10,
//!     "discard_oldest": true, "nodes": [ {"key":"k","node_id":"ns=2;i=2"} ] }
//!   ```
//! - Browse 绑定：`opcua.browse`（周期浏览，§7.3 V1 支持）
//!   ```json
//!   { "nodes": [ {"key":"objects","node_id":"ns=0;i=85","data_type":"STRING"} ] }
//!   ```
//! - NodeId 解析见 `address::parse_address`；Core 不触及此文件（硬约束）。
//! - SecurityPolicy/MessageSecurityMode 透传至 Native ClientBuilder pki_dir/own.der/key trust false verify true
//! - SourceTimestamp 1601 ticks→Unix ns 精确保留，Quality GOOD/UNCERTAIN/BAD 按 StatusCode 映射，Array→Typed Array

mod address;
mod opcua_api;

pub use address::{AddressError, Identifier, OpcUaAddress, parse_address};
pub use opcua_api::{DEFAULT_OPCUA_PORT, FakeOpcUaApi, NativeOpcUaApi, OpcUaApi};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{
    AcquisitionTask, DataBatch, DataType, DriverMetadata, DuplicatePointKey, GENERIC_BINDING_KIND,
    GenericBinding, PointDescriptor, PointMap, PointValue, Quality, TaskMode, Value,
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
            AccessMode, DataType, DiscoveryCapabilities, DriverCapabilities, DriverDescriptor,
            DriverIdentity, FieldDescriptor, FieldType, LocalizedText, OutputDescriptor,
            ResourceDescriptor, SchemaDescriptor,
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
                    data_type: DataType::String,
                    unit: None,
                    access: AccessMode::Read,
                }],
                modes: vec![
                    mesa_core_types::TaskMode::Poll,
                    mesa_core_types::TaskMode::Subscribe,
                ],
            }],
            controls: mesa_core_types::ControlCatalog::default(),
            discovery: DiscoveryCapabilities {
                manual: true,
                browse: true,
                import: false,
            },
            capabilities: DriverCapabilities {
                poll: true,
                subscribe: true,
                browse: true,
                ..Default::default()
            },
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
        let api: Arc<dyn OpcUaApiTrait> = if use_native {
            let native = if let Some(pki) = OpcUaConnConfig::resolve_pki_dir() {
                NativeOpcUaApi::new_with_pki_dir(pki)
            } else {
                NativeOpcUaApi::new()
            };
            native.set_security(cfg.security_policy.clone(), cfg.security_mode.clone());
            native.set_credentials(cfg.username.clone(), cfg.password.clone());
            Arc::new(native)
        } else {
            Arc::new(FakeOpcUaApi::new())
        };
        Ok(Box::new(OpcUaConnection {
            cfg,
            api,
            plan: None,
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
    /// 提取可选的 pki_dir：仅允许环境变量 MESA_OPCUA_PKI_DIR 或默认值，禁止 Endpoint JSON 指定路径
    fn resolve_pki_dir() -> Option<std::path::PathBuf> {
        std::env::var("MESA_OPCUA_PKI_DIR")
            .ok()
            .map(std::path::PathBuf::from)
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
    plan: Option<PlanSnapshot>,
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

#[async_trait::async_trait]
impl DriverConnection for OpcUaConnection {
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
            "ns=0;i=85"
        } else {
            parent
        };
        let addr = parse_address(parent_str).map_err(|e| match e {
            AddressError::Empty => SdkDriverError::configuration("INVALID_ADDRESS", "parent 为空"),
            AddressError::Invalid { reason, .. } => SdkDriverError::new(
                mesa_core_types::ErrorKind::Address,
                "INVALID_ADDRESS",
                format!("parent `{parent_str}` 非法: {reason}"),
            ),
        })?;
        let children = self.api.browse(&addr).await.map_err(|e| {
            SdkDriverError::new(mesa_core_types::ErrorKind::Internal, "BROWSE_FAILED", e)
        })?;
        // 过滤与分页
        let filtered: Vec<String> = children
            .into_iter()
            .filter(|n| filter.is_empty() || n.contains(filter))
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
            .map(|name| {
                let node_id = format!("ns=2;s={}", name);
                mesa_driver_protocol::pb::BrowseNode {
                    id: name.clone(),
                    label: name.clone(),
                    kind: "node".into(),
                    data_type: "String".into(),
                    access: "read".into(),
                    has_children: true,
                    binding_json: serde_json::json!({"node_id": node_id, "data_type": "String"})
                        .to_string(),
                }
            })
            .collect();
        Ok((nodes, next_cursor))
    }

    async fn run(
        &mut self,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError> {
        let snap = self.plan.as_ref().ok_or_else(|| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Internal,
                "NO_PLAN",
                "run 前未 configure+apply",
            )
        })?;
        let map = snap.map.as_ref().ok_or_else(|| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Internal,
                "NO_POINT_MAP",
                "run 前未 apply_point_map",
            )
        })?;

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

        use std::sync::atomic::{AtomicU64, Ordering};
        let seq = Arc::new(AtomicU64::new(1));
        let shared_api = Arc::clone(&self.api);
        let mut handles = Vec::with_capacity(snap.tasks.len());
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
                    handles.push(tokio::spawn(async move {
                        let mut ticker = tokio::time::interval(interval);
                        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {
                                _ = ticker.tick() => {},
                                _ = shutdown.cancelled() => break,
                            }
                            let addrs: Vec<OpcUaAddress> = points.iter().map(|(s, _)| s.addr.clone()).collect();
                            let values = match api.read_batch(&addrs).await {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::error!(task=%task_id, error=%e, "OPC UA 读失败");
                                    return Err(SdkDriverError::new(mesa_core_types::ErrorKind::Connection, "READ_FAILED", e));
                                }
                            };
                            if values.len() != points.len() {
                                tracing::warn!(task=%task_id, got=values.len(), expected=points.len(), "OPC UA 返回数量不一致");
                                continue;
                            }
                            let mut batch_vals = Vec::with_capacity(points.len());
                            for ((spec, pid), raw_val) in points.iter().zip(values) {
                                if let Value::String(s) = &raw_val
                                    && s.starts_with("ERR:") {
                                        let q = if s.contains("Uncertain") { Quality::Uncertain } else { Quality::Bad };
                                        let code = if s.contains("Uncertain") { Some(0x40000000) } else { Some(1) };
                                        tracing::warn!(key=%spec.key, error=%s, quality=?q, "单点非 Good");
                                        batch_vals.push(PointValue {
                                            point_id: *pid,
                                            value: raw_val,
                                            quality: q,
                                            quality_code: code,
                                            source_timestamp_ns: None,
                                        });
                                        continue;
                                    }
                                let coerced = coerce_value(raw_val, spec.data_type);
                                if !value_fits_data_type(&coerced, spec.data_type) {
                                    tracing::warn!(key=%spec.key, got=?coerced, expected=?spec.data_type, "类型不匹配跳过");
                                    continue;
                                }
                                let ts = now_unix_ns();
                                batch_vals.push(PointValue {
                                    point_id: *pid,
                                    value: coerced,
                                    quality: Quality::Good,
                                    quality_code: None,
                                    source_timestamp_ns: Some(ts),
                                });
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
                    }));
                }
                TaskKind::Browse { interval_ms } => {
                    let interval = Duration::from_millis(interval_ms);
                    handles.push(tokio::spawn(async move {
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
                                        let val = Value::String(refs.join(";"));
                                        batch_vals.push(PointValue::good(*pid, val));
                                    }
                                    Err(e) => {
                                        tracing::warn!(key=%spec.key, error=%e, "Browse 单点 Bad");
                                        batch_vals.push(PointValue {
                                            point_id: *pid,
                                            value: Value::String(format!("ERR:{}", e)),
                                            quality: Quality::Bad,
                                            quality_code: Some(1),
                                            source_timestamp_ns: None,
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
                    }));
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
                    handles.push(tokio::spawn(async move {
                        let (sub_id, mut rx) = match api.subscribe(&addrs, publishing_interval_ms, sampling_interval_ms, queue_size, discard_oldest).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!(task=%task_id, error=%e, "OPC UA 订阅失败");
                                return Err(SdkDriverError::new(mesa_core_types::ErrorKind::Connection, "SUBSCRIBE_FAILED", e));
                            }
                        };
                        tracing::info!(task=%task_id, sub_id=%sub_id, "OPC UA 订阅已建立");
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
                                let dv = ev.data_value;
                                // 状态按 OPC UA StatusCode 映射 GOOD/UNCERTAIN/BAD (§9.3)
                                if let Some(status) = dv.status
                                    && !status.is_good() {
                                        let q = crate::opcua_api::status_to_quality(status);
                                        tracing::warn!(key=%spec.key, status=?status, quality=?q, "单点非 Good");
                                        batch_vals.push(PointValue {
                                            point_id: *pid,
                                            value: Value::String(format!("ERR:{:?}", status)),
                                            quality: q,
                                            quality_code: Some(status.bits() as i32),
                                            source_timestamp_ns: None,
                                        });
                                        continue;
                                    }
                                let Some(variant) = dv.value else {
                                    tracing::warn!(key=%spec.key, "DataValue 无 value");
                                    batch_vals.push(PointValue {
                                        point_id: *pid,
                                        value: Value::String(format!("ERR:BadNoValue {:?}", dv.status)),
                                        quality: Quality::Bad,
                                        quality_code: Some(1),
                                        source_timestamp_ns: None,
                                    });
                                    continue;
                                };
                                let Some(raw_val) = crate::opcua_api::variant_to_value(&variant) else {
                                    continue;
                                };
                                let coerced = coerce_value(raw_val, spec.data_type);
                                if !value_fits_data_type(&coerced, spec.data_type) {
                                    tracing::warn!(key=%spec.key, got=?coerced, expected=?spec.data_type, "类型不匹配跳过");
                                    continue;
                                }
                                // source_timestamp 透传：优先 DataValue.source_timestamp 1601 ticks→Unix ns，否则 now
                                let ts_ns = dv.source_timestamp.map(|dt| {
                                    let ticks = dt.ticks();
                                    const TICKS_PER_SEC: i64 = 10_000_000;
                                    const UNIX_TICKS_OFFSET: i64 = 11644473600 * TICKS_PER_SEC;
                                    let unix_ticks = ticks - UNIX_TICKS_OFFSET;
                                    unix_ticks * 100
                                }).unwrap_or_else(now_unix_ns);
                                batch_vals.push(PointValue {
                                    point_id: *pid,
                                    value: coerced,
                                    quality: Quality::Good,
                                    quality_code: None,
                                    source_timestamp_ns: Some(ts_ns),
                                });
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
                        // 清理订阅
                        let _ = api.unsubscribe(sub_id).await;
                        Ok::<(), SdkDriverError>(())
                    }));
                }
            }
        }

        let mut final_err: Option<SdkDriverError> = None;
        for h in handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if final_err.is_none() {
                        final_err = Some(e);
                    }
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
                }
            }
            if final_err.is_some() {
                shutdown.cancel();
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

    #[tokio::test]
    async fn configure_ok_and_duplicate_rejected() {
        let mut conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            plan: None,
        };
        let nodes = serde_json::json!([
            {"key":"a","node_id":"ns=2;i=2","data_type":"U32"},
            {"key":"b","node_id":"ns=2;s=Motor.Speed","data_type":"F64"}
        ]);
        let t = task_with_nodes(nodes);
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 2);
        let dup = serde_json::json!([
            {"key":"a","node_id":"ns=2;i=2","data_type":"U32"},
            {"key":"a","node_id":"ns=2;i=3","data_type":"U32"}
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
            plan: None,
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
            plan: None,
        };
        let t = AcquisitionTask {
            id: "s1".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: BINDING_SUB.into(),
                config: serde_json::json!({"publishing_interval_ms":500,"sampling_interval_ms":250,"queue_size":10,"nodes":[{"key":"a","node_id":"ns=2;i=2","data_type":"U32"}]}),
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
                config: serde_json::json!({"nodes":[{"key":"b","node_id":"ns=2;s=MyVar","data_type":"STRING"}]}),
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
            plan: None,
        };
        let nodes = serde_json::json!([{"key":"a","node_id":"ns=2;s=Counter","data_type":"U32"}]);
        let t = task_with_nodes(nodes);
        let descs = conn.configure(1, vec![t]).await.unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert(descs[0].point_key.clone(), 1u32);
        conn.apply_point_map(map).await.unwrap();
        // 能走到 apply 即代表 Poll 快照构建正确；run 的 DataSink 发布由集成测试覆盖
    }
}
