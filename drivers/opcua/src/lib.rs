#![allow(clippy::collapsible_if)]
//! OPC UA Driver — 方案 §7.3 节点/订阅型（V1 只读，44/44 2026-08-29）。
//!
//! - 绑定（Foundation-2 单路径）：`mesa.resources.v1`，resource `node`
//!   ```json
//!   { "selections": [
//!       { "resource_id": "node",
//!         "parameters": {"node_id":"nsu=http://example.com/MyModel/;s=Counter","data_type":"UINT32"},
//!         "outputs": [{"output":"value","point_key":"counter"}] }
//!   ]}
//!   ```
//!   node_id 一律 canonical `nsu=` 形态（`ns=<index>` 拒绝，见 `address`）；
//!   运行期每次建会话后经 NamespaceArray 换算为当前 index（漂移自愈）。
//! - 调度（TaskSchedule 单真值）：Poll 取 interval_ms；Subscribe 取
//!   publishing/sampling/queue/discard 四参数（Foundation-2 闭环）。
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
    GenericBinding, PointDescriptor, PointMap, PointValue, Quality, TaskSchedule, Value,
    ValueOrigin, ensure_unique_point_keys, now_unix_ns,
};
use mesa_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
use tokio_util::sync::CancellationToken;

/// Foundation-2 已删除的 legacy kind（`opcua.node-group` / `opcua.subscription` /
/// `opcua.browse` 统一由 `mesa.resources.v1` + TaskSchedule 替代，不再是合法
/// binding；见 ADR 0003）。此处仅保留注释说明，不再导出常量.
/// OPC UA canonical data_type 公共契约（PR3）：`mesa.resources.v1`
/// 只接受这 10 个精确大写拼写。descriptor mapping 与门禁同源。
/// NOTE：driver 只读 Value 属性；`attribute` 参数已删除（声明了但零消费
/// 的死 knob 不进契约，需要读 BrowseName 等属性时另立 feature）。
pub const CANONICAL_DATA_TYPES: [(&str, DataType); 10] = [
    ("STRING", DataType::String),
    ("INT32", DataType::I32),
    ("UINT32", DataType::U32),
    ("INT64", DataType::I64),
    ("UINT64", DataType::U64),
    ("FLOAT", DataType::F32),
    ("DOUBLE", DataType::F64),
    ("BOOL", DataType::Bool),
    ("BYTES", DataType::Bytes),
    ("DATETIME", DataType::DateTime),
];

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
            AccessMode, DriverCapabilities, DriverDescriptor, DriverIdentity, FieldDescriptor,
            FieldType, LocalizedText, OutputDescriptor, OutputTypeSpec, ResourceDescriptor,
            ResourceSelectionMethod, SchemaDescriptor,
        };
        let m = self.metadata();
        DriverDescriptor {
            contract_major: mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR,
            contract_minor: mesa_core_types::DESCRIPTOR_CONTRACT_MINOR,
            identity: DriverIdentity {
                driver_id: m.driver_id,
                name: m.name,
                version: m.version,
            },
            connection: SchemaDescriptor {
                fields: vec![
                    // C1 §4：URL 必须显式给出，不提供有效默认目标
                    //（删除原 opc.tcp://127.0.0.1:4840 默认值，避免误连本机）。
                    {
                        let mut f =
                            FieldDescriptor::new("endpoint_url", "Endpoint URL", FieldType::Url)
                                .required(true);
                        f.ui.placeholder =
                            Some("如 opc.tcp://192.0.2.10:4840（示例，需按现场填写）".into());
                        f
                    },
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
                    {
                        // C1 §6.2：Descriptor 与 Driver Parser 同一套范围。
                        let mut f =
                            FieldDescriptor::new("timeout_ms", "Timeout ms", FieldType::Duration)
                                .required(false)
                                .default_value(serde_json::json!(5000));
                        f.validation.min = Some(mesa_core_types::CONNECTION_TIMEOUT_MIN_MS as f64);
                        f.validation.max = Some(mesa_core_types::CONNECTION_TIMEOUT_MAX_MS as f64);
                        f
                    },
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
                            // FromParameter 参数必须 required（S7 同口径）：
                            // UI 可用 default 预填，但 canonical selection 必须显式携带，
                            // 否则 resolve() → None 与 configure 默认值分叉。
                            let mut f =
                                FieldDescriptor::new("data_type", "Data Type", FieldType::Enum)
                                    .required(true)
                                    .default_value(serde_json::json!("STRING"));
                            // 选项与 CANONICAL_DATA_TYPES 同源
                            f.validation.enum_options = Some(
                                CANONICAL_DATA_TYPES
                                    .iter()
                                    .map(|(k, _)| k.to_string())
                                    .collect(),
                            );
                            f
                        },
                    ],
                },
                outputs: vec![OutputDescriptor {
                    id: "value".into(),
                    label: LocalizedText::new("Value"),
                    // 类型随 data_type 参数（canonical 口径，见 CANONICAL_DATA_TYPES）。
                    type_spec: OutputTypeSpec::FromParameter {
                        parameter: "data_type".into(),
                        mapping: CANONICAL_DATA_TYPES
                            .iter()
                            .map(|(k, dt)| (k.to_string(), *dt))
                            .collect(),
                    },
                    unit: None,
                    access: AccessMode::Read,
                }],
                modes: vec![
                    // Foundation-2：canonical Subscribe 闭环（Poll + Subscribe
                    // 双模式；schedule 参数由 TaskSchedule 承载）。
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
        // C1：use_native 由 `OpcUaConnConfig::from_json` 统一严格解析，
        // 不再在此重复读取并 `unwrap_or(true)`（那会把非法类型静默当默认）。
        let use_native = cfg.use_native;
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
            plan: std::sync::RwLock::new(None),
            event_plan: std::sync::RwLock::new(None),
        }))
    }
}
// ---------------------------------------------------------------------------
// 连接配置
// ---------------------------------------------------------------------------

/// Secret marker 形状判定（`{"secret_set": true}`）。
///
/// 为什么驱动需要认识它：marker 只能由 Core 的 Secret 物化路径解析成明文，
/// 一旦原样传到 Driver，说明启动/恢复链路漏了物化。此处识别它是为了给出
/// **明确的 fail-closed 诊断**，而不是把它当字符串密码或静默降级为匿名连接。
fn is_secret_marker_shape(v: &serde_json::Value) -> bool {
    v.as_object()
        .map(|m| m.get("secret_set") == Some(&serde_json::Value::Bool(true)))
        .unwrap_or(false)
}

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
    /// 是否使用 Native 客户端（C1：由本解析器统一严格校验，
    /// 不再在 `open_connection` 里用 `unwrap_or(true)` 静默兜底）。
    use_native: bool,
}

impl Default for OpcUaConnConfig {
    fn default() -> Self {
        Self {
            // NOTE: 该默认值仅供测试夹具构造，**不是**可接受的连接目标：
            // `from_json` 要求显式 `endpoint_url`，绝不回落到本机回环。
            endpoint_url: "opc.tcp://127.0.0.1:4840".into(),
            timeout_ms: 5000,
            security_policy: "None".into(),
            security_mode: "None".into(),
            username: None,
            password: None,
            use_native: true,
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

    /// 从 JSON 解析连接参数（C1 §6.2「缺席 vs 存在但非法」契约）。
    ///
    /// 兼容别名（有明确定义的 legacy 形态）：
    /// `endpoint_url` / `endpoint` / `url` / `host`+`port`；`timeout` = `timeout_ms`。
    /// 规则：
    /// - 主字段**缺席**时才允许读取别名；
    /// - 主字段存在但类型非法 → 直接 `BAD_CONFIG`，**不得**退回别名或默认 URL；
    /// - 多个等价字段同时存在且取值冲突 → 拒绝（不做无说明的任选）；
    /// - `endpoint_url` 必须显式给出，绝不回落 `opc.tcp://127.0.0.1:4840`；
    /// - 端口先判范围再窄化，禁止 `as u16` 截断。
    fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        fn bad(message: impl Into<String>) -> SdkDriverError {
            SdkDriverError::configuration("BAD_CONFIG", message)
        }
        let object = v.as_object().ok_or_else(|| bad("connection 需为对象"))?;

        // ---- endpoint_url（别名 endpoint / url / host+port）----
        let mut url_candidates: Vec<(&str, String)> = Vec::new();
        for key in ["endpoint_url", "endpoint", "url"] {
            if let Some(value) = object.get(key) {
                let s = value
                    .as_str()
                    .ok_or_else(|| bad(format!("{key} 必须为字符串")))?;
                url_candidates.push((key, s.to_string()));
            }
        }
        // host+port 形态：host 存在但类型非法同样拒绝（不静默当作缺失）。
        if let Some(value) = object.get("host") {
            let host = value
                .as_str()
                .ok_or_else(|| bad("host 必须为字符串"))?
                .trim();
            if host.is_empty() {
                return Err(bad("host 不能为空"));
            }
            let port = match object.get("port") {
                None => DEFAULT_OPCUA_PORT as u64,
                Some(p) => {
                    let n = p
                        .as_u64()
                        .ok_or_else(|| bad("port 必须为 1..=65535 的整数"))?;
                    if !(1..=65535).contains(&n) {
                        return Err(bad(format!("port {n} 非法，需 1..=65535")));
                    }
                    n
                }
            };
            url_candidates.push(("host+port", format!("opc.tcp://{host}:{port}")));
        }
        // 冲突检测：同一语义给出多个不同取值时拒绝，而不是任选其一。
        let endpoint_url = match url_candidates.first() {
            None => {
                return Err(bad(
                    "endpoint_url 必填（不从默认目标 opc.tcp://127.0.0.1:4840 隐式连接）",
                ));
            }
            Some((_, first)) => {
                if let Some((key, other)) = url_candidates.iter().skip(1).find(|(_, u)| u != first)
                {
                    return Err(bad(format!(
                        "连接目标字段冲突：`{key}`=`{other}` 与首字段取值不一致，拒绝隐式任选"
                    )));
                }
                first.clone()
            }
        };

        // ---- timeout_ms（别名 timeout）----
        let timeout_ms = match (object.get("timeout_ms"), object.get("timeout")) {
            (None, None) => 5000u64,
            (Some(value), None) => value
                .as_u64()
                .ok_or_else(|| bad("timeout_ms 必须为非负整数"))?,
            (None, Some(value)) => value
                .as_u64()
                .ok_or_else(|| bad("timeout 必须为非负整数"))?,
            (Some(m), Some(a)) => {
                let mv = m.as_u64().ok_or_else(|| bad("timeout_ms 必须为非负整数"))?;
                let av = a.as_u64().ok_or_else(|| bad("timeout 必须为非负整数"))?;
                if mv != av {
                    return Err(bad(format!(
                        "timeout_ms 与别名 timeout 同时存在但取值冲突（{mv} vs {av}），拒绝隐式任选"
                    )));
                }
                mv
            }
        };

        // ---- 枚举字段：存在但类型非法即拒绝，不静默当默认 ----
        let security_policy = match object.get("security_policy") {
            None => "None".to_string(),
            Some(value) => value
                .as_str()
                .ok_or_else(|| bad("security_policy 必须为字符串"))?
                .to_string(),
        };
        let security_mode = match object.get("security_mode") {
            None => "None".to_string(),
            Some(value) => value
                .as_str()
                .ok_or_else(|| bad("security_mode 必须为字符串"))?
                .to_string(),
        };
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
        if timeout_ms < mesa_core_types::CONNECTION_TIMEOUT_MIN_MS {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!(
                    "timeout_ms {timeout_ms} 非法，最小值为 {}",
                    mesa_core_types::CONNECTION_TIMEOUT_MIN_MS
                ),
            ));
        }
        if timeout_ms > mesa_core_types::CONNECTION_TIMEOUT_MAX_MS {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!(
                    "timeout_ms {timeout_ms} 超出支持上限 {}（Probe 分阶段预算按该值推导）",
                    mesa_core_types::CONNECTION_TIMEOUT_MAX_MS
                ),
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
        // ---- 凭据：类型非法即拒绝；Secret marker 明确报错（不得当密码用） ----
        let username = match object.get("username") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| bad("username 必须为字符串"))?
                    .to_string(),
            ),
        };
        let password = match object.get("password") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => {
                if value.is_string() {
                    Some(value.as_str().unwrap_or_default().to_string())
                } else if crate::is_secret_marker_shape(value) {
                    // P0 诊断：marker 只能由 Core 的 Secret 物化路径解析，
                    // 传到 Driver 说明恢复/启动链路漏了物化。明确 fail-closed，
                    // 绝不把 marker 当密码、也不静默降级为匿名连接。
                    return Err(bad(
                        "password 收到 Secret marker 而非明文：调用方必须先经 Secret 物化（缺 Secret 时 fail-closed），驱动不接受 marker",
                    ));
                } else {
                    return Err(bad("password 必须为字符串"));
                }
            }
        };
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
        // use_native：存在但类型非法即拒绝（此前 `unwrap_or(true)` 会静默当默认）。
        let use_native = match object.get("use_native") {
            None => true,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| bad("use_native 必须为布尔值"))?,
        };
        // 禁止生产默认忽略校验：若 policy/mode 为 None 则 trust_server_certs 需显式，但此处仅校验，实际连接由证书目录决定
        Ok(Self {
            endpoint_url,
            timeout_ms,
            security_policy,
            security_mode,
            username,
            password,
            use_native,
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
    // Foundation-2 已删除 legacy `opcua.browse`（周期浏览任务）；
    // Browse 能力保留为 ResourceSelectionMethod（选点方式），不再是任务类型。
}

#[derive(Debug, Clone)]
struct TaskPlan {
    id: String,
    kind: TaskKind,
    point_indices: Vec<usize>,
}

#[allow(dead_code)]
/// control-handle 并发模型下 run 入口克隆整个快照，故需 Clone。
#[derive(Debug, Clone)]
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
    // 采集计划（control-handle 并发模型）：configure/apply 写锁替换，
    // run 读锁克隆快照后释放；api/transport 本就 Arc 共享。
    plan: std::sync::RwLock<Option<PlanSnapshot>>,
    /// 事件计划快照（Stage ③）：`configure_events` 原子替换，供 run() 启动
    /// Event workers；空表/None = 无事件订阅。
    event_plan: std::sync::RwLock<Option<OpcUaEventPlanSnapshot>>,
}

impl std::fmt::Debug for OpcUaConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcUaConnection")
            .field("cfg", &self.cfg)
            .field("has_plan", &self.plan.read().unwrap().is_some())
            .finish()
    }
}

/// Foundation-2 已删除 legacy `nodes[]` 信封的数据类型别名解析；
/// canonical 路径直接查 CANONICAL_DATA_TYPES 表（configure 内联）。
/// 本函数保留供单测锁定解析语义（地址解析内部能力），不进产品契约。
#[allow(dead_code)]
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
    async fn probe(&self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        crate::probe::probe_with_transport(&*self.transport).await
    }

    async fn configure(
        &self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        let mut new_points: Vec<PointSpec> = Vec::new();
        let mut new_tasks: Vec<TaskPlan> = Vec::new();

        for task in &tasks {
            task.validate()
                .map_err(|e| SdkDriverError::configuration("INVALID_TASK", e.to_string()))?;
            // Foundation-2 单路径：仅接受 mesa.resources.v1（legacy 已删除）。
            if task.binding.kind != GENERIC_BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: 期望 {GENERIC_BINDING_KIND}，实际 {}",
                        task.id, task.binding.kind
                    ),
                ));
            }
            {
                let binding: GenericBinding = serde_json::from_value(task.binding.config.clone())
                    .map_err(|e| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: invalid generic binding: {e}", task.id),
                    )
                })?;
                mesa_core_types::validate_selections_structure(&binding.selections)
                    .map_err(|e| SdkDriverError::configuration("INVALID_BINDING_CONFIG", e))?;
                // Foundation-2 Subscribe 闭环：schedule 单真值（Poll 取 interval，
                // Subscribe 取订阅四参数；validate 已保证合法）。
                enum ScheduleKind {
                    Poll {
                        interval_ms: u64,
                    },
                    Subscribe {
                        publishing_interval_ms: u64,
                        sampling_interval_ms: u64,
                        queue_size: u32,
                        discard_oldest: bool,
                    },
                }
                let schedule_kind = match task.schedule {
                    TaskSchedule::Poll { interval_ms } => ScheduleKind::Poll { interval_ms },
                    TaskSchedule::Subscribe {
                        publishing_interval_ms,
                        sampling_interval_ms,
                        queue_size,
                        discard_oldest,
                    } => ScheduleKind::Subscribe {
                        publishing_interval_ms,
                        sampling_interval_ms,
                        queue_size,
                        discard_oldest,
                    },
                };
                let mut indices = Vec::new();
                for sel in &binding.selections {
                    if sel.resource_id != "node" {
                        return Err(SdkDriverError::configuration(
                            "UNSUPPORTED_RESOURCE",
                            format!("task `{}`: opcua generic only supports node", task.id),
                        ));
                    }
                    for out in &sel.outputs {
                        // PR3 canonical：只接受 node_id（address 回退已删除）；
                        // node_id 缺失即 INVALID（descriptor 要求必填）。
                        let node_id = sel
                            .parameters
                            .get("node_id")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                SdkDriverError::configuration(
                                    "INVALID_POINT",
                                    format!("point `{}` 缺少 node_id", out.point_key),
                                )
                            })?
                            .to_string();
                        // data_type 必填（descriptor required）：缺失即 INVALID_POINT，
                        // 不得回落 STRING（回落即与 validate_instance/required 双真值）。
                        let dt_str = sel
                            .parameters
                            .get("data_type")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                SdkDriverError::configuration(
                                    "INVALID_POINT",
                                    format!("point `{}` 缺少 data_type", out.point_key),
                                )
                            })?;
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
                        let data_type = CANONICAL_DATA_TYPES
                            .iter()
                            .find(|(k, _)| *k == dt_str)
                            .map(|(_, dt)| *dt)
                            .ok_or_else(|| {
                                SdkDriverError::configuration(
                                    "INVALID_DATA_TYPE",
                                    format!(
                                        "point `{}` data_type `{dt_str}` 非 canonical（仅接受 {:?}）",
                                        out.point_key,
                                        CANONICAL_DATA_TYPES.map(|(k, _)| k),
                                    ),
                                )
                            })?;
                        indices.push(new_points.len());
                        new_points.push(PointSpec {
                            key: out.point_key.clone(),
                            addr,
                            data_type,
                        });
                    }
                }
                match schedule_kind {
                    ScheduleKind::Poll { interval_ms } => {
                        new_tasks.push(TaskPlan {
                            id: task.id.clone(),
                            kind: TaskKind::Poll { interval_ms },
                            point_indices: indices,
                        });
                    }
                    ScheduleKind::Subscribe {
                        publishing_interval_ms,
                        sampling_interval_ms,
                        queue_size,
                        discard_oldest,
                    } => {
                        if publishing_interval_ms == 0
                            || sampling_interval_ms == 0
                            || queue_size == 0
                        {
                            return Err(SdkDriverError::configuration(
                                "INVALID_TASK",
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
            }
        }

        let descriptors: Vec<PointDescriptor> = new_points
            .iter()
            .map(|p| PointDescriptor {
                point_key: p.key.clone(),
                data_type: p.data_type,
                unit: None,
                // P1 未实现：回落 None（UI 显示技术坐标），不阻塞。
                source_label: None,
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
        // 写锁内整体替换（§6.2 原子切换；失败路径在锁外早返回）。
        *self.plan.write().unwrap() = Some(PlanSnapshot {
            revision,
            points: new_points,
            tasks: new_tasks,
            map: None,
        });
        Ok(descriptors)
    }

    async fn apply_point_map(&self, map: PointMap) -> Result<(), SdkDriverError> {
        let mut guard = self.plan.write().unwrap();
        let snap = guard.as_mut().ok_or_else(|| {
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
        &self,
        revision: u64,
        tasks: Vec<mesa_core_types::EventTask>,
    ) -> Result<(), SdkDriverError> {
        let plans = event::parse_event_tasks(&tasks)?;
        tracing::info!(revision, tasks = plans.len(), "opcua event plan built");
        *self.event_plan.write().unwrap() = Some(OpcUaEventPlanSnapshot {
            revision,
            tasks: plans,
        });
        Ok(())
    }

    async fn browse(
        &self,
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
                // browse 发出的类型必须是 canonical（回填 selection 可直接过门禁）
                data_type: "STRING".into(),
                access: "read".into(),
                has_children: true,
                binding_json: serde_json::json!({"node_id": node_id, "data_type": "STRING"})
                    .to_string(),
            })
            .collect();
        Ok((nodes, next_cursor))
    }

    async fn run(&self, sink: DataSink, shutdown: CancellationToken) -> Result<(), SdkDriverError> {
        // §18：一等 Event-only——Data 计划缺席/空表不再是错误；PointMap 只在
        // 有 Data 任务时要求。Event-only 端点无需任何 Data point 即可 Start。
        // 读锁下克隆快照后释放（await 期间不持 std 锁；namespace resolve 的
        // 写回在独立短临界区内完成，见下）。
        let plan_snapshot: Option<PlanSnapshot> = self.plan.read().unwrap().clone();
        let has_data_tasks = plan_snapshot
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
        // namespace resolve 写回：克隆快照→改地址→写锁整体换回
        //（短临界区；resolve 失败即 fail-closed，不同 plan 混跑由 revision 门保证）。
        let resolved_snapshot: Option<PlanSnapshot> = {
            let mut owned = plan_snapshot;
            if let Some(snap) = owned.as_mut() {
                for p in &mut snap.points {
                    p.addr.resolve(&namespaces).map_err(|e| {
                        SdkDriverError::new(
                            mesa_core_types::ErrorKind::Address,
                            "UNKNOWN_NAMESPACE",
                            format!("point `{}`: {e}", p.key),
                        )
                    })?;
                }
                // 写回表内（run 期后续 configure 会整体替换，不冲突）。
                *self.plan.write().unwrap() = Some(snap.clone());
            }
            owned
        };
        let namespaces = Arc::new(namespaces);

        let data_plan = if has_data_tasks {
            Some(resolved_snapshot.as_ref().ok_or_else(|| {
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
            .read()
            .unwrap()
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
    use mesa_core_types::{AcquisitionTask, DriverBinding};

    // -----------------------------------------------------------------------
    // C1 §6.2 / P0：连接字段「缺席 vs 存在但非法」+ Secret marker 诊断
    // -----------------------------------------------------------------------

    /// endpoint_url 必填：缺席/类型非法/空一律拒绝，绝不回落本机回环默认值。
    #[test]
    fn endpoint_url_is_explicit_and_never_defaults_to_loopback() {
        assert_eq!(
            OpcUaConnConfig::from_json(&serde_json::json!({}))
                .unwrap_err()
                .code,
            "BAD_CONFIG",
            "缺 endpoint_url 必须拒绝（不得连 opc.tcp://127.0.0.1:4840）"
        );
        for bad in [
            serde_json::json!(123),
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!(""),
            serde_json::json!("   "),
            serde_json::json!("http://host:4840"),
        ] {
            assert_eq!(
                OpcUaConnConfig::from_json(&serde_json::json!({"endpoint_url": bad}))
                    .unwrap_err()
                    .code,
                "BAD_CONFIG",
                "endpoint_url={bad} 必须拒绝"
            );
        }
    }

    /// 等价目标字段冲突必须拒绝，而不是任选其一。
    #[test]
    fn conflicting_target_fields_are_rejected() {
        assert_eq!(
            OpcUaConnConfig::from_json(&serde_json::json!({
                "endpoint_url": "opc.tcp://a:4840",
                "endpoint": "opc.tcp://b:4840",
            }))
            .unwrap_err()
            .code,
            "BAD_CONFIG"
        );
        // host+port 与 endpoint_url 冲突同样拒绝。
        assert_eq!(
            OpcUaConnConfig::from_json(&serde_json::json!({
                "endpoint_url": "opc.tcp://a:4840",
                "host": "b",
                "port": 4840,
            }))
            .unwrap_err()
            .code,
            "BAD_CONFIG"
        );
        // 取值一致 → 接受。
        assert!(
            OpcUaConnConfig::from_json(&serde_json::json!({
                "endpoint_url": "opc.tcp://a:4840",
                "endpoint": "opc.tcp://a:4840",
            }))
            .is_ok()
        );
    }

    /// 端口先判范围再窄化；类型非法一律拒绝。
    #[test]
    fn host_port_form_validates_port_before_narrowing() {
        for bad in [
            serde_json::json!(0),
            serde_json::json!(70000),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("4840"),
            serde_json::json!(true),
            serde_json::json!(null),
        ] {
            assert!(
                OpcUaConnConfig::from_json(&serde_json::json!({"host": "a", "port": bad})).is_err(),
                "port={bad} 必须拒绝"
            );
        }
        let cfg =
            OpcUaConnConfig::from_json(&serde_json::json!({"host": "a", "port": 4840})).unwrap();
        assert_eq!(cfg.endpoint_url, "opc.tcp://a:4840");
    }

    /// Secret marker 传到 Driver 必须 fail-closed 并给出明确诊断，
    /// 绝不当作密码、也不静默降级为匿名连接（C1 P0）。
    #[test]
    fn secret_marker_reaching_driver_is_fail_closed_with_diagnosis() {
        let err = OpcUaConnConfig::from_json(&serde_json::json!({
            "endpoint_url": "opc.tcp://a:4840",
            "username": "u",
            "password": {"secret_set": true},
        }))
        .unwrap_err();
        assert_eq!(err.code, "BAD_CONFIG");
        assert!(
            err.message.contains("Secret marker"),
            "错误消息必须指明 marker 未物化，实际: {}",
            err.message
        );
        // 只有 marker、没有 username 时同样不能匿名降级。
        assert_eq!(
            OpcUaConnConfig::from_json(&serde_json::json!({
                "endpoint_url": "opc.tcp://a:4840",
                "password": {"secret_set": true},
            }))
            .unwrap_err()
            .code,
            "BAD_CONFIG"
        );
    }

    /// 凭据类型非法 / use_native 类型非法一律拒绝（不静默用默认值）。
    #[test]
    fn ill_typed_credential_and_use_native_are_rejected() {
        for bad in [
            serde_json::json!(123),
            serde_json::json!(true),
            serde_json::json!({}),
        ] {
            assert!(
                OpcUaConnConfig::from_json(&serde_json::json!({
                    "endpoint_url": "opc.tcp://a:4840",
                    "password": bad,
                }))
                .is_err(),
                "password={bad} 必须拒绝"
            );
        }
        assert!(
            OpcUaConnConfig::from_json(&serde_json::json!({
                "endpoint_url": "opc.tcp://a:4840",
                "use_native": "yes",
            }))
            .is_err()
        );
        // 枚举字段类型非法同样拒绝。
        assert!(
            OpcUaConnConfig::from_json(&serde_json::json!({
                "endpoint_url": "opc.tcp://a:4840",
                "security_policy": 7,
            }))
            .is_err()
        );
    }

    /// timeout 范围与 Descriptor 同源，超限在网络前拒绝。
    #[test]
    fn timeout_bounds_match_descriptor() {
        use mesa_driver_sdk::Driver;
        let with = |t: serde_json::Value| {
            OpcUaConnConfig::from_json(
                &serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "timeout_ms": t}),
            )
        };
        assert!(
            with(serde_json::json!(
                mesa_core_types::CONNECTION_TIMEOUT_MIN_MS - 1
            ))
            .is_err()
        );
        assert!(
            with(serde_json::json!(
                mesa_core_types::CONNECTION_TIMEOUT_MAX_MS + 1
            ))
            .is_err()
        );
        assert!(
            with(serde_json::json!(
                mesa_core_types::CONNECTION_TIMEOUT_MAX_MS
            ))
            .is_ok()
        );
        let desc = OpcUaDriver.descriptor();
        let f = desc
            .connection
            .fields
            .iter()
            .find(|f| f.key == "timeout_ms")
            .expect("Descriptor 必须有 timeout_ms");
        assert_eq!(
            f.validation.min,
            Some(mesa_core_types::CONNECTION_TIMEOUT_MIN_MS as f64)
        );
        assert_eq!(
            f.validation.max,
            Some(mesa_core_types::CONNECTION_TIMEOUT_MAX_MS as f64)
        );
        let url = desc
            .connection
            .fields
            .iter()
            .find(|f| f.key == "endpoint_url")
            .expect("Descriptor 必须有 endpoint_url");
        assert!(url.required);
        assert_eq!(
            url.default, None,
            "endpoint_url 不得有有效默认目标（C1 §4 硬要求）"
        );
    }

    fn generic_task(selections: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: "t1".into(),
            schedule: TaskSchedule::Poll { interval_ms: 100 },
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::json!({"selections": selections}),
            },
        }
    }

    fn generic_subscribe_task(selections: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: "s1".into(),
            schedule: TaskSchedule::Subscribe {
                publishing_interval_ms: 500,
                sampling_interval_ms: 250,
                queue_size: 10,
                discard_oldest: true,
            },
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::json!({"selections": selections}),
            },
        }
    }

    fn node_selection(point_key: &str, params: serde_json::Value) -> serde_json::Value {
        serde_json::json!([{
            "resource_id": "node",
            "parameters": params,
            "outputs": [{"output": "value", "point_key": point_key}],
        }])
    }

    async fn test_conn() -> Box<dyn DriverConnection> {
        // C1 后 endpoint_url 必填：测试夹具显式给目标（不再依赖默认回环值）。
        OpcUaDriver
            .open_connection("ep1", r#"{"endpoint_url":"opc.tcp://127.0.0.1:4840"}"#)
            .await
            .expect("open")
    }

    /// PR3 mode 门：descriptor 声明的每个 mode 都必须 generic configure 成功
    ///（Foundation-2：Poll + Subscribe 双模式闭环，报执行不了即 Descriptor lie）。
    #[tokio::test]
    async fn declared_modes_all_configurable() {
        use mesa_core_types::TaskMode;
        let d = OpcUaDriver.descriptor();
        let node = d.resources.iter().find(|r| r.id == "node").unwrap();
        assert_eq!(node.modes, vec![TaskMode::Poll, TaskMode::Subscribe]);
        let sel = node_selection(
            "k",
            serde_json::json!({
                "node_id": "nsu=http://example.com/MyModel/;i=2",
                "data_type": "STRING",
            }),
        );
        for mode in &node.modes {
            let task = match mode {
                TaskMode::Poll => generic_task(sel.clone()),
                TaskMode::Subscribe => generic_subscribe_task(sel.clone()),
            };
            let conn = test_conn().await;
            let descs = conn.configure(1, vec![task]).await.unwrap();
            assert_eq!(descs.len(), 1);
        }
    }

    /// PR3 闭环：10 个 canonical data_type → validate_instance PASS →
    /// generic configure PASS → PointDescriptor == resolve()。
    #[tokio::test]
    async fn generic_canonical_loop_closed_for_all_types() {
        let d = OpcUaDriver.descriptor();
        d.validate().expect("descriptor 必须合法");
        let node = d.resources.iter().find(|r| r.id == "node").unwrap();
        assert!(
            node.parameters.fields.iter().all(|f| f.key != "attribute"),
            "attribute 死参数必须已删除"
        );
        for (dt, expected) in CANONICAL_DATA_TYPES {
            let params = serde_json::json!({
                "node_id": "nsu=http://example.com/MyModel/;i=2",
                "data_type": dt,
            });
            let issues = node.parameters.validate_instance("parameters", &params);
            assert!(issues.is_empty(), "{dt}: {issues:?}");
            let pmap = params.as_object().unwrap().clone();
            assert_eq!(
                node.outputs[0].type_spec.resolve(&pmap),
                Some(expected),
                "{dt}"
            );
            let conn = test_conn().await;
            let descs = conn
                .configure(1, vec![generic_task(node_selection("k", params))])
                .await
                .unwrap();
            assert_eq!(descs.len(), 1);
            assert_eq!(descs[0].data_type, expected, "{dt}");
        }
    }

    /// generic 拒绝：address 回退、缺 node_id、小写 alias、旧 U32 拼写。
    #[tokio::test]
    async fn generic_rejects_non_canonical() {
        let cases: Vec<(&str, serde_json::Value, &str)> = vec![
            (
                "address 回退不接受",
                serde_json::json!({"address": "nsu=http://example.com/MyModel/;i=2"}),
                "INVALID_POINT",
            ),
            (
                "缺 node_id",
                serde_json::json!({"data_type": "STRING"}),
                "INVALID_POINT",
            ),
            (
                "缺 data_type（descriptor required）",
                serde_json::json!({"node_id": "nsu=http://example.com/MyModel/;i=2"}),
                "INVALID_POINT",
            ),
            (
                "小写 alias 不接受",
                serde_json::json!({"node_id": "nsu=http://example.com/MyModel/;i=2", "data_type": "string"}),
                "INVALID_DATA_TYPE",
            ),
            (
                "旧 U32 拼写不接受（canonical 为 UINT32）",
                serde_json::json!({"node_id": "nsu=http://example.com/MyModel/;i=2", "data_type": "U32"}),
                "INVALID_DATA_TYPE",
            ),
        ];
        for (name, params, code) in cases {
            let conn = test_conn().await;
            let err = conn
                .configure(1, vec![generic_task(node_selection("k", params))])
                .await
                .unwrap_err();
            assert_eq!(err.code, code, "{name}");
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
        let conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: fake.clone(),
            plan: std::sync::RwLock::new(None),
            event_plan: std::sync::RwLock::new(None),
        };
        // 零 Data 任务 + 一个 generic 事件任务（configure_events 用 [] 语义清空亦可）。
        conn.configure(1, vec![]).await.unwrap();
        conn.configure_events(
            1,
            vec![EventTask {
                id: "ev-only".into(),
                mode: mesa_core_types::TaskMode::Subscribe,
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
        let conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: std::sync::RwLock::new(None),
            event_plan: std::sync::RwLock::new(None),
        };
        // generic 双点（canonical node_id + data_type）
        let both = serde_json::json!([
            {"resource_id":"node","parameters":{"node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"UINT32"},"outputs":[{"output":"value","point_key":"a"}]},
            {"resource_id":"node","parameters":{"node_id":"nsu=http://example.com/MyModel/;s=Motor.Speed","data_type":"DOUBLE"},"outputs":[{"output":"value","point_key":"b"}]},
        ]);
        let descs = conn.configure(1, vec![generic_task(both)]).await.unwrap();
        assert_eq!(descs.len(), 2);
        // 跨 task 重复 point_key → DUPLICATE_POINT_KEY（同一 task 内重复则
        // 早于此被结构级拒绝，见 generic 路径 validate_selections_structure）
        let dup_a = serde_json::json!([{"resource_id":"node","parameters":{"node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"UINT32"},"outputs":[{"output":"value","point_key":"a"}]}]);
        let dup_b = serde_json::json!([{"resource_id":"node","parameters":{"node_id":"nsu=http://example.com/MyModel/;i=3","data_type":"UINT32"},"outputs":[{"output":"value","point_key":"a"}]}]);
        let mut t1 = generic_task(dup_a);
        t1.id = "t1".into();
        let mut t2 = generic_task(dup_b);
        t2.id = "t2".into();
        let err = conn.configure(2, vec![t1, t2]).await.unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");
    }

    #[tokio::test]
    async fn invalid_node_id_rejected() {
        let conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: std::sync::RwLock::new(None),
            event_plan: std::sync::RwLock::new(None),
        };
        // 非 canonical（ns= 拒绝）
        let sel = serde_json::json!([{"resource_id":"node","parameters":{"node_id":"ns=2;x=1","data_type":"UINT32"},"outputs":[{"output":"value","point_key":"a"}]}]);
        let err = conn
            .configure(1, vec![generic_task(sel)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ADDRESS");
    }

    #[tokio::test]
    async fn subscribe_configure_ok() {
        let conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: std::sync::RwLock::new(None),
            event_plan: std::sync::RwLock::new(None),
        };
        // Foundation-2：Subscribe 参数由 schedule 承载（非 binding config）。
        let sel_a = serde_json::json!([{"resource_id":"node","parameters":{"node_id":"nsu=http://example.com/MyModel/;i=2","data_type":"UINT32"},"outputs":[{"output":"value","point_key":"a"}]}]);
        let descs = conn
            .configure(1, vec![generic_subscribe_task(sel_a)])
            .await
            .unwrap();
        assert_eq!(descs.len(), 1);
        let sel_b = serde_json::json!([{"resource_id":"node","parameters":{"node_id":"nsu=http://example.com/MyModel/;s=MyVar","data_type":"STRING"},"outputs":[{"output":"value","point_key":"b"}]}]);
        let descs2 = conn
            .configure(2, vec![generic_subscribe_task(sel_b)])
            .await
            .unwrap();
        assert_eq!(descs2.len(), 1);
    }

    #[tokio::test]
    async fn configure_apply_ok() {
        let conn = OpcUaConnection {
            cfg: OpcUaConnConfig::default(),
            api: Arc::new(FakeOpcUaApi::new()),
            transport: Arc::new(mesa_opcua_transport::FakeOpcUaTransport::new()),
            plan: std::sync::RwLock::new(None),
            event_plan: std::sync::RwLock::new(None),
        };
        let sel = serde_json::json!([{"resource_id":"node","parameters":{"node_id":"nsu=http://example.com/MyModel/;s=Counter","data_type":"UINT32"},"outputs":[{"output":"value","point_key":"a"}]}]);
        let descs = conn.configure(1, vec![generic_task(sel)]).await.unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert(descs[0].point_key.clone(), 1u32);
        conn.apply_point_map(map).await.unwrap();
        // 能走到 apply 即代表 Poll 快照构建正确；run 的 DataSink 发布由集成测试覆盖
    }

    // --- P0-A.9 GOOD→BAD→GOOD / first BAD / 兼容别名 / REST 契约测试（V1.2.1 Gate） ---

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
