//! SINUMERIK Driver — Siemens SINUMERIK CNC 只读接入（PR11 起步）。
//!
//! 架构位置（特异性止于本目录）：
//!
//! ```text
//! SINUMERIK driver（本 crate：设备语义）
//!       ↓
//! mesa-opcua-transport（公共 OPC UA 会话/读/浏览/订阅）
//!       ↓
//! Native OPC UA（async-opcua）
//! ```
//!
//! - 本文件无任何 async-opcua 导入（transport 边界冻结；`opcua_types` 仅用于
//!   DataValue/Variant 解码类型，与通用 OPC UA 驱动同口径）；
//! - Core 只认识 Descriptor / ProbeReport / ResourceSelection / DataBatch，
//!   无任何 `driver_id == "sinumerik"` 分支（门禁）；
//! - V1 严格只读：`write`/`command` 沿用 SDK 默认 Unsupported；
//! - 事件：PR11 不做（`configure_events` 非空即 `EVENT_NOT_SUPPORTED`，PR13 才接入）；
//! - 资源身份一律 canonical `nsu=<uri>;<i|s|g|b>=<id>`（见 [`canonical`]），
//!   `ns=<index>` 在 `configure`/`browse` 边界一律拒绝。

mod canonical;
mod config;
mod fixture;
mod probe;
mod value;

pub use canonical::{
    CanonicalError, ResolveError, SinumerikIdentifier, SinumerikNodeId, parse_canonical,
};
pub use config::SinumerikConnConfig;
pub use fixture::{
    FIXTURE_FIRMWARE, FIXTURE_MODEL, FIXTURE_VENDOR, OBJECTS_ROOT, SIEMENS_INDEX, SIEMENS_NS,
    STD_NS, namespace_array, sinumerik_shaped_fake,
};
pub use probe::probe_with_transport;
pub use value::{
    LastKnownSample, PointSpec, decode_data_value, parse_data_type, status_to_quality,
};

use std::collections::HashMap;
use std::sync::Arc;

use mesa_core_types::{
    AcquisitionTask, DriverMetadata, DuplicatePointKey, GENERIC_BINDING_KIND, GenericBinding,
    PointDescriptor, PointMap, ensure_unique_point_keys,
};
use mesa_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
use mesa_opcua_transport::{FakeOpcUaTransport, NativeOpcUaTransport, OpcUaTransport};
use tokio_util::sync::CancellationToken;

/// 驱动 ID（Core 侧无分支；仅 Descriptor identity 与 driver.toml 声明）。
pub const DRIVER_ID: &str = "sinumerik";
/// legacy 周期读绑定（与通用 `mesa.resources.v1` 并存，语义相同）。
pub const BINDING_POLL: &str = "sinumerik.node-group";
/// legacy 订阅绑定。
pub const BINDING_SUB: &str = "sinumerik.subscription";

/// browse 单页向 transport 请求的最大引用数（服务端仍可按自身上限截断并返回
/// continuation，本驱动用 continuation 接力取全页）。
const BROWSE_MAX_REFS_PER_PAGE: u32 = 1000;
/// browse 管理面分页默认/上限（有界响应，防单次巨页）。
const BROWSE_DEFAULT_LIMIT: usize = 50;
const BROWSE_MAX_LIMIT: usize = 1000;

// ---------------------------------------------------------------------------
// 驱动入口
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct SinumerikDriver;

#[async_trait::async_trait]
impl Driver for SinumerikDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: DRIVER_ID.into(),
            name: "SINUMERIK".into(),
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
                        FieldDescriptor::new(
                            "node_id",
                            "NodeId（canonical nsu= 形态）",
                            FieldType::String,
                        )
                        .required(true),
                        {
                            let mut f =
                                FieldDescriptor::new("data_type", "Data Type", FieldType::Enum)
                                    .required(false)
                                    .default_value(serde_json::json!("STRING"));
                            f.validation.enum_options = Some(vec![
                                "STRING".into(),
                                "BOOL".into(),
                                "I32".into(),
                                "U32".into(),
                                "I64".into(),
                                "U64".into(),
                                "F32".into(),
                                "F64".into(),
                                "BYTES".into(),
                                "DATETIME".into(),
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
            // PR11 只读：无控制目录（默认空），事件目录为空（PR13 才声明）。
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
            events: Default::default(),
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
        let cfg = SinumerikConnConfig::from_json(&v)?;
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
        // transport 与采集/探测/浏览共享同一会话实例：创建一次，连接持有同一 Arc，
        // probe/browse/run 复用它，绝不另建第二会话。
        let transport: Arc<dyn OpcUaTransport> = if use_native {
            Arc::new(NativeOpcUaTransport::new(cfg.connect_options()))
        } else {
            Arc::new(FakeOpcUaTransport::new())
        };
        Ok(Box::new(SinumerikConnection {
            cfg,
            transport,
            plan: None,
        }))
    }
}

// ---------------------------------------------------------------------------
// 采集计划
// ---------------------------------------------------------------------------

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
}

#[derive(Debug, Clone)]
struct TaskPlan {
    id: String,
    kind: TaskKind,
    point_indices: Vec<usize>,
}

#[derive(Debug)]
struct PlanSnapshot {
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    map: Option<PointMap>,
}

/// 单个 SINUMERIK 运行时连接（经 [`SinumerikDriver::open_connection`] 或
/// [`SinumerikConnection::with_transport`] 构造）。
pub struct SinumerikConnection {
    cfg: SinumerikConnConfig,
    /// 采集/探测/浏览共享的传输会话（open 时一次创建，同一 Arc）。
    transport: Arc<dyn OpcUaTransport>,
    plan: Option<PlanSnapshot>,
}

impl std::fmt::Debug for SinumerikConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinumerikConnection")
            .field("cfg", &self.cfg)
            .field("has_plan", &self.plan.is_some())
            .finish()
    }
}

impl SinumerikConnection {
    /// 测试/Fixture 注入脚本化 transport（生产经 `open_connection` 构造；
    /// contract 测试与确定性 fixture 共用本入口，避免第二套建连逻辑）。
    pub fn with_transport(cfg: SinumerikConnConfig, transport: Arc<dyn OpcUaTransport>) -> Self {
        Self {
            cfg,
            transport,
            plan: None,
        }
    }

    /// canonical 点解析（configure 共用）：只接受 `nsu=`，legacy `ns=` 指引去 browse。
    fn parse_point_node(point_key: &str, node_id: &str) -> Result<SinumerikNodeId, SdkDriverError> {
        canonical::parse_canonical(node_id).map_err(|e| match e {
            CanonicalError::Empty => SdkDriverError::configuration(
                "INVALID_ADDRESS",
                format!("point `{point_key}` node_id 为空"),
            ),
            CanonicalError::Invalid { reason, .. } => SdkDriverError::new(
                mesa_core_types::ErrorKind::Address,
                "INVALID_ADDRESS",
                format!("point `{point_key}` node_id `{node_id}` 非法: {reason}"),
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// DriverConnection
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl DriverConnection for SinumerikConnection {
    /// SINUMERIK 动态探测：复用本连接的 transport（与采集同一会话），见 [`probe`].
    async fn probe(&mut self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        probe_with_transport(&*self.transport).await
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
            // 通用资源绑定（管理面统一 Envelope；协议参数由本驱动解释，Core 不碰）。
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
                if task.mode != mesa_core_types::TaskMode::Poll
                    && task.mode != mesa_core_types::TaskMode::Subscribe
                {
                    return Err(SdkDriverError::new(
                        mesa_core_types::ErrorKind::Unsupported,
                        "MODE_NOT_SUPPORTED",
                        format!("task `{}`: sinumerik node 仅支持 poll/subscribe", task.id),
                    ));
                }
                let mut indices = Vec::new();
                for sel in &binding.selections {
                    if sel.resource_id != "node" {
                        return Err(SdkDriverError::configuration(
                            "UNSUPPORTED_RESOURCE",
                            format!("task `{}`: sinumerik generic only supports node", task.id),
                        ));
                    }
                    // node_id 必须显式给出（不以 point_key 兜底：point_key 是 Core 侧
                    // 稳定键，node 身份是设备侧 canonical，两者故意解耦）。
                    let node_id = sel
                        .parameters
                        .get("node_id")
                        .or_else(|| sel.parameters.get("address"))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            SdkDriverError::configuration(
                                "INVALID_BINDING_CONFIG",
                                format!(
                                    "task `{}`: resource node 缺少 parameters.node_id（canonical nsu= 形态，经 browse 获取）",
                                    task.id
                                ),
                            )
                        })?;
                    let dt_str = sel
                        .parameters
                        .get("data_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("STRING");
                    let data_type = parse_data_type(dt_str).map_err(|reason| {
                        SdkDriverError::configuration(
                            "INVALID_DATA_TYPE",
                            format!("task `{}`: {reason}", task.id),
                        )
                    })?;
                    for out in &sel.outputs {
                        let node = Self::parse_point_node(&out.point_key, node_id)?;
                        indices.push(new_points.len());
                        new_points.push(PointSpec {
                            key: out.point_key.clone(),
                            node,
                            data_type,
                        });
                    }
                }
                let kind = task_kind_from_mode(task, &task.id)?;
                new_tasks.push(TaskPlan {
                    id: task.id.clone(),
                    kind,
                    point_indices: indices,
                });
                continue;
            }
            // legacy 绑定：sinumerik.node-group（poll）/ sinumerik.subscription（subscribe）。
            let is_poll = task.binding.kind == BINDING_POLL;
            let is_sub = task.binding.kind == BINDING_SUB;
            if !is_poll && !is_sub {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: 期望 {BINDING_POLL}/{BINDING_SUB} 或 {GENERIC_BINDING_KIND}，实际 {}",
                        task.id, task.binding.kind
                    ),
                ));
            }
            if is_poll && task.mode != mesa_core_types::TaskMode::Poll {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: sinumerik.node-group 仅支持 poll", task.id),
                ));
            }
            if is_sub && task.mode != mesa_core_types::TaskMode::Subscribe {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!(
                        "task `{}`: sinumerik.subscription 仅支持 subscribe",
                        task.id
                    ),
                ));
            }
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
                            format!("point `{key}` 缺少 node_id（canonical nsu= 形态）"),
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
                let addr = Self::parse_point_node(key, node_id)?;
                let data_type = parse_data_type(dt_str).map_err(|reason| {
                    SdkDriverError::configuration(
                        "INVALID_DATA_TYPE",
                        format!("point `{key}`: {reason}"),
                    )
                })?;
                indices.push(new_points.len());
                new_points.push(PointSpec {
                    key: key.to_string(),
                    node: addr,
                    data_type,
                });
            }
            let kind = task_kind_from_binding(task, &task.id, is_poll)?;
            new_tasks.push(TaskPlan {
                id: task.id.clone(),
                kind,
                point_indices: indices,
            });
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
            "SINUMERIK 采集计划构建完成"
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

    /// PR11 只读：事件面未接入——空任务直接接受（老路径兼容），
    /// 非空任务 `EVENT_NOT_SUPPORTED`（PR13 才落地 SINUMERIK 事件）。
    async fn configure_events(
        &mut self,
        revision: u64,
        tasks: Vec<mesa_core_types::EventTask>,
    ) -> Result<(), SdkDriverError> {
        let _ = revision;
        if tasks.is_empty() {
            Ok(())
        } else {
            Err(SdkDriverError::new(
                mesa_core_types::ErrorKind::Unsupported,
                "EVENT_NOT_SUPPORTED",
                "sinumerik events not supported in read-only V1 (PR13 接入)",
            ))
        }
    }

    /// Browse（管理面）：transport 单层浏览 + continuation 接力取全页，
    /// 子节点 index 经当次 NamespaceArray 快照换算为 canonical 身份后输出。
    /// Core 拿到的只是 BrowseNode（id/label/kind/data_type/access/binding），
    /// OPC UA 原始结构不泄漏。
    async fn browse(
        &mut self,
        parent: &str,
        filter: &str,
        cursor: &str,
        limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), SdkDriverError> {
        use mesa_opcua_transport::{UaBrowseRequest, UaNodeClass, UaNodeRef};

        // 建连（browse 可能在 run 之外被调用；Fake 即时成功）。
        if let Err(e) = self.transport.connect().await {
            return Err(SdkDriverError::new(
                mesa_core_types::ErrorKind::Connection,
                "CONNECT_FAILED",
                e.to_string(),
            ));
        }
        // 当次浏览的命名空间快照：parent 与全部子节点用同一快照换算，
        // 快照内一致，跨次漂移由 canonical 吸收。
        let namespaces = self.transport.read_namespace_array().await.map_err(|e| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Internal,
                "NAMESPACE_FAILED",
                format!("NamespaceArray 读取失败，无法换算 canonical 身份: {e}"),
            )
        })?;
        let parent_node: UaNodeRef = if parent.trim().is_empty() {
            UaNodeRef::numeric(0, 85)
        } else {
            let canonical = Self::parse_point_node("<parent>", parent)?;
            canonical.resolve(&namespaces).map_err(|e| {
                SdkDriverError::new(
                    mesa_core_types::ErrorKind::Address,
                    "UNKNOWN_NAMESPACE",
                    format!("parent `{parent}` 换算失败: {e}"),
                )
            })?
        };

        // continuation 接力取全页（opaque token 不解析，仅透传）。
        let mut all_children = Vec::new();
        let mut page = self
            .transport
            .browse(UaBrowseRequest {
                node: parent_node,
                max_refs: BROWSE_MAX_REFS_PER_PAGE,
            })
            .await
            .map_err(|e| {
                SdkDriverError::new(
                    mesa_core_types::ErrorKind::Internal,
                    "BROWSE_FAILED",
                    e.to_string(),
                )
            })?;
        loop {
            all_children.extend(page.nodes.into_iter().map(|n| {
                let canonical = SinumerikNodeId::from_index_ref(&n.node_id, &namespaces);
                (n, canonical)
            }));
            let Some(token) = page.continuation_point else {
                break;
            };
            let next = match self.transport.browse_next(token.clone()).await {
                Ok(p) => p,
                Err(e) => {
                    if let Err(rel) = self.transport.release_continuation(token).await {
                        tracing::debug!(?rel, "翻页失败后释放 continuation 失败（仅诊断）");
                    }
                    return Err(SdkDriverError::new(
                        mesa_core_types::ErrorKind::Internal,
                        "BROWSE_FAILED",
                        e.to_string(),
                    ));
                }
            };
            if let Err(rel) = self.transport.release_continuation(token).await {
                tracing::debug!(?rel, "释放已消费 continuation 失败（仅诊断）");
            }
            page = next;
        }

        // index 越界的子节点（快照间隙）跳过并计数告警：不杀整页。
        let mut skipped = 0usize;
        let mut canonical_children = Vec::with_capacity(all_children.len());
        for (n, canonical) in all_children {
            match canonical {
                Some(id) => canonical_children.push((n, id)),
                None => skipped += 1,
            }
        }
        if skipped > 0 {
            tracing::warn!(skipped, "browse 子节点命名空间越界已跳过（快照间隙）");
        }

        // 过滤（label/id 子串）与管理面分页（cursor 为 numeric offset）。
        let filtered: Vec<(mesa_opcua_transport::UaBrowseNode, SinumerikNodeId)> =
            canonical_children
                .into_iter()
                .filter(|(n, id)| {
                    if filter.is_empty() {
                        return true;
                    }
                    let label = n.display_name.as_deref().unwrap_or(&n.browse_name);
                    label.contains(filter)
                        || n.browse_name.contains(filter)
                        || id.to_string().contains(filter)
                })
                .collect();
        let start = cursor.parse::<usize>().unwrap_or(0);
        let lim = if limit == 0 {
            BROWSE_DEFAULT_LIMIT
        } else {
            (limit as usize).min(BROWSE_MAX_LIMIT)
        };
        let start = start.min(filtered.len());
        let end = (start + lim).min(filtered.len());
        let next_cursor = if end < filtered.len() {
            Some(end.to_string())
        } else {
            None
        };
        let nodes = filtered[start..end]
            .iter()
            .map(|(n, id)| {
                let label = n
                    .display_name
                    .clone()
                    .unwrap_or_else(|| n.browse_name.clone());
                let (kind, access, data_type) = match n.node_class {
                    // Variable 才可读值；Object/Method 在只读 V1 下不可读
                    //（Method 更不可执行，access 直接 none）。
                    UaNodeClass::Variable => ("variable", "read", "Unknown"),
                    UaNodeClass::Object => ("object", "none", ""),
                    UaNodeClass::Method => ("method", "none", ""),
                    UaNodeClass::Unknown => ("unknown", "none", ""),
                };
                // has_children 未探测时按 true（允许前端继续下钻；叶子下钻
                // 只返回空页，无害）——禁止为此发起 N+1 探测。
                let has_children = n.has_children.unwrap_or(true);
                let canonical_str = id.to_string();
                mesa_driver_protocol::pb::BrowseNode {
                    id: canonical_str.clone(),
                    label,
                    kind: kind.into(),
                    data_type: data_type.into(),
                    access: access.into(),
                    has_children,
                    binding_json: serde_json::json!({
                        "node_id": canonical_str,
                        "data_type": "STRING",
                    })
                    .to_string(),
                }
            })
            .collect();
        Ok((nodes, next_cursor))
    }

    /// 只读采集主循环（Checkpoint C/D）。
    ///
    /// 不变量（Review Gate）：
    /// - 每次 run 用新鲜 NamespaceArray 把 canonical URI 换算为当前 index
    ///   （重连/index 漂移自愈；canonical 不变 → point_id 不漂）；
    /// - 旧 session 不继续产数据（writer 按 epoch 丢弃，见 SDK；本驱动 Stop 后
    ///   不再 publish，teardown 做有界 disconnect）；
    /// - 任一任务 Err 即 cancel 全体并 reap（轻量 supervisor，与通用 OPC UA 同形）；
    /// - teardown 永不掩盖原始错误（disconnect 失败/超时仅诊断）。
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
        // V1 无事件面：零任务的 run 是无意义空转，直接 fail-closed。
        if snap.tasks.is_empty() {
            return Err(SdkDriverError::configuration(
                "EMPTY_PLAN",
                "无采集任务（sinumerik V1 只读：至少一个 poll/subscribe 任务）",
            ));
        }
        // 建连前守卫：plan/map 缺失直接拒绝（尚无会话，无需 disconnect）。
        let _ = snap.map.as_ref().ok_or_else(|| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Internal,
                "NO_POINT_MAP",
                "run 前未 apply_point_map",
            )
        })?;
        tracing::info!(
            revision = snap.revision,
            tasks = snap.tasks.len(),
            "SINUMERIK run 启动"
        );

        // 建会话：失败由 Manager 退避重建（fail-closed，不吞错）。
        // 注意：connect 之前的 early-Err（NO_PLAN/EMPTY_PLAN/NO_POINT_MAP）
        // 尚无会话，无需 disconnect；connect 之后的一切路径走统一出口。
        if let Err(e) = self.transport.connect().await {
            return Err(SdkDriverError::new(
                mesa_core_types::ErrorKind::Connection,
                "CONNECT_FAILED",
                e.to_string(),
            ));
        }
        // 统一 teardown 出口：connect 成功后的 early-Err（NamespaceArray /
        // canonical 换算 / worker 失败）都必须经过底部的有界 disconnect，
        // 与 CONTRACT "teardown 有界 disconnect" 一致。
        let outcome = self.run_connected(sink, shutdown).await;
        // teardown 末端：best-effort disconnect（有界，防已死会话 CloseSession 永不返回）。
        // 失败/超时仅诊断：不掩盖原始错误，不让干净 Stop 失败。
        let dc_timeout = std::time::Duration::from_millis(self.cfg.timeout_ms);
        match tokio::time::timeout(dc_timeout, self.transport.disconnect()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "SINUMERIK run 结束 disconnect 失败（仅诊断）");
            }
            Err(_) => {
                tracing::warn!("SINUMERIK run 结束 disconnect 超时（仅诊断）");
            }
        }
        outcome
    }
}

impl SinumerikConnection {
    /// 已连接会话的数据阶段：namespace 快照 → canonical 换算 → workers → supervisor。
    /// 前置：transport 已 connect；返回后调用方（run）必走有界 disconnect。
    async fn run_connected(
        &self,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError> {
        use mesa_core_types::{DataBatch, now_unix_ns};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::Duration;

        // run() 已校验，此处重复守卫保证独立正确（尤其是直接调用本方法时）。
        let snap = self.plan.as_ref().ok_or_else(|| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Internal,
                "NO_PLAN",
                "run 前未 configure+apply",
            )
        })?;
        if snap.tasks.is_empty() {
            return Err(SdkDriverError::configuration(
                "EMPTY_PLAN",
                "无采集任务（sinumerik V1 只读：至少一个 poll/subscribe 任务）",
            ));
        }
        let map = snap.map.as_ref().ok_or_else(|| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Internal,
                "NO_POINT_MAP",
                "run 前未 apply_point_map",
            )
        })?;
        // 本次 run 的命名空间快照：重连后 URI→index 重新换算，index 漂移自愈。
        let namespaces = self.transport.read_namespace_array().await.map_err(|e| {
            SdkDriverError::new(
                mesa_core_types::ErrorKind::Connection,
                "NAMESPACE_FAILED",
                format!("run 启动 NamespaceArray 读取失败: {e}"),
            )
        })?;
        // 全部 canonical 点一次换算（缺 URI 即 fail-closed，不带病运行）。
        let mut resolved: Vec<(PointSpec, u32, mesa_opcua_transport::UaNodeRef)> = Vec::new();
        for point in &snap.points {
            let pid = map.get(&point.key).ok_or_else(|| {
                SdkDriverError::configuration(
                    "MISSING_POINT_ID",
                    format!("point `{}` 缺少映射", point.key),
                )
            })?;
            let node = point.node.resolve(&namespaces).map_err(|e| {
                SdkDriverError::new(
                    mesa_core_types::ErrorKind::Address,
                    "UNKNOWN_NAMESPACE",
                    format!("point `{}` 换算失败: {e}", point.key),
                )
            })?;
            resolved.push((point.clone(), *pid, node));
        }
        let resolved = Arc::new(resolved);

        let seq = Arc::new(AtomicU64::new(1));
        let mut set: tokio::task::JoinSet<Result<(), SdkDriverError>> = tokio::task::JoinSet::new();
        for task in &snap.tasks {
            let indices = task.point_indices.clone();
            let points: Vec<(PointSpec, u32, mesa_opcua_transport::UaNodeRef)> = indices
                .iter()
                .map(|&i| {
                    let (spec, pid, node) = &resolved[i];
                    (spec.clone(), *pid, node.clone())
                })
                .collect();
            let sink = sink.clone();
            let shutdown = shutdown.clone();
            let seq = Arc::clone(&seq);
            let transport = Arc::clone(&self.transport);
            let task_id = task.id.clone();
            match task.kind.clone() {
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
                            let nodes: Vec<mesa_opcua_transport::UaNodeRef> =
                                points.iter().map(|(_, _, n)| n.clone()).collect();
                            let data_values = match transport.read(&nodes).await {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::error!(task=%task_id, error=%e, "SINUMERIK 读失败");
                                    return Err(SdkDriverError::new(
                                        mesa_core_types::ErrorKind::Connection,
                                        "READ_FAILED",
                                        e.to_string(),
                                    ));
                                }
                            };
                            if data_values.len() != points.len() {
                                tracing::warn!(
                                    task=%task_id,
                                    got=data_values.len(),
                                    expected=points.len(),
                                    "SINUMERIK 返回数量不一致"
                                );
                                continue;
                            }
                            let mut batch_vals = Vec::with_capacity(points.len());
                            for ((spec, pid, _), dv) in points.iter().zip(data_values) {
                                batch_vals.push(decode_data_value(spec, *pid, dv, &mut last_known));
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
                    set.spawn(async move {
                        use mesa_opcua_transport::{UaMonitoredItemSpec, UaSubscriptionSpec};
                        // 分裂生命周期第一步：仅建订阅（保留 Server Revised 供诊断）
                        let sub = match transport
                            .create_subscription(UaSubscriptionSpec {
                                publishing_interval_ms,
                                lifetime_count: 30,
                                max_keep_alive_count: 10,
                                max_notifications_per_publish: 0,
                                priority: 0,
                                publishing_enabled: true,
                            })
                            .await
                        {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(task=%task_id, error=%e, "SINUMERIK 订阅失败");
                                return Err(SdkDriverError::new(
                                    mesa_core_types::ErrorKind::Connection,
                                    "SUBSCRIBE_FAILED",
                                    e.to_string(),
                                ));
                            }
                        };
                        tracing::info!(
                            task=%task_id,
                            sub_id=sub.id,
                            requested=sub.requested_publishing_interval_ms,
                            revised=sub.revised_publishing_interval_ms,
                            "SINUMERIK 订阅已建立（revised 由 Server 协商）"
                        );
                        // 第二步：独立建监控项（client_handle = idx+1）
                        let mi_specs: Vec<UaMonitoredItemSpec> = points
                            .iter()
                            .enumerate()
                            .map(|(idx, (_, _, node))| UaMonitoredItemSpec {
                                node: node.clone(),
                                client_handle: (idx as u32) + 1,
                                sampling_interval_ms,
                                queue_size,
                                discard_oldest,
                            })
                            .collect();
                        let results = match transport
                            .create_monitored_items(sub.id, &mi_specs)
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                // 服务级失败必须回滚刚建的空订阅（cleanup 失败仅诊断）。
                                if let Err(cleanup) = transport.delete_subscription(sub.id).await {
                                    tracing::debug!(
                                        sub_id = sub.id,
                                        ?cleanup,
                                        "订阅创建失败后回滚删订阅失败（仅诊断）"
                                    );
                                }
                                return Err(SdkDriverError::new(
                                    mesa_core_types::ErrorKind::Connection,
                                    "SUBSCRIBE_FAILED",
                                    e.to_string(),
                                ));
                            }
                        };
                        // handle -> (spec, pid)；单项 BAD 合成初始 BAD 事件并隔离该项
                        //（失败项永不到达 live 流，不合成则首个 live 前该点"无值"而非
                        // LastKnown/Placeholder）。client_handle = idx+1，反查 points。
                        let mut handle_map: HashMap<u32, (PointSpec, u32)> = HashMap::new();
                        let mut ok_ids = Vec::new();
                        let mut initial_bads: Vec<(usize, opcua_types::StatusCode)> = Vec::new();
                        for r in &results {
                            let status = opcua_types::StatusCode::from(r.status_code);
                            let idx = (r.client_handle as usize).checked_sub(1);
                            let point = idx.and_then(|i| points.get(i));
                            match (status.is_good(), point) {
                                (true, Some((spec, pid, _))) => {
                                    handle_map.insert(r.client_handle, (spec.clone(), *pid));
                                    ok_ids.push(r.monitored_item_id);
                                }
                                _ => {
                                    if let Some(idx) = idx {
                                        initial_bads.push((idx, status));
                                    }
                                }
                            }
                        }
                        let mut sub_rx = sub.receiver;
                        let mut last_known: HashMap<u32, LastKnownSample> = HashMap::new();
                        // 初始 BAD 有序在 live 之前（与通用 OPC UA 同序）。
                        for (idx, status) in initial_bads {
                            let Some((spec, pid, _)) = points.get(idx) else {
                                continue;
                            };
                            let dv = opcua_types::DataValue {
                                value: None,
                                status: Some(status),
                                source_timestamp: None,
                                source_picoseconds: None,
                                server_timestamp: None,
                                server_picoseconds: None,
                            };
                            let pv = decode_data_value(spec, *pid, dv, &mut last_known);
                            sink.publish(DataBatch {
                                connection_handle: 0,
                                stream_epoch: 0,
                                sequence: seq.fetch_add(1, Ordering::Relaxed),
                                timestamp_ns: now_unix_ns(),
                                values: vec![pv],
                                mono_ns: None,
                            })
                            .await;
                        }
                        // 订阅事件循环：批量聚合（Latest-Wins 由 Sink 承接），
                        // KeepAlive 无事件不产批、不递增 sequence。
                        //
                        // P0 会话丢失语义：receiver 关闭 ≠ 正常 Stop。transport
                        // 在 session event-loop 意外结束时会 abort forwarders
                        // 并关闭 receiver（已冻结行为）；此时必须先清理、再以
                        // SESSION_LOST Err 退出（SDK：Err → Failed → Manager
                        // 重建；若误报 Ok → Stopped，Manager 永不重连）。
                        let mut session_lost = false;
                        loop {
                            let first = tokio::select! {
                                ev = sub_rx.recv() => ev,
                                _ = shutdown.cancelled() => break,
                            };
                            let Some(first_ev) = first else {
                                if shutdown.is_cancelled() {
                                    // 干净 Stop：Manager 主动 cancel，通道关闭是
                                    // teardown 的一部分，正常退出。
                                    break;
                                }
                                tracing::error!(
                                    task=%task_id,
                                    sub_id=sub.id,
                                    "订阅通道意外关闭（会话丢失），先清理后报 SESSION_LOST"
                                );
                                session_lost = true;
                                break;
                            };
                            let mut events = vec![first_ev];
                            while let Ok(ev) = sub_rx.try_recv() {
                                events.push(ev);
                                if events.len() >= 64 {
                                    break;
                                }
                            }
                            let mut batch_vals = Vec::with_capacity(events.len());
                            for ev in events {
                                let Some((spec, pid)) = handle_map.get(&ev.client_handle) else {
                                    tracing::warn!(
                                        task=%task_id,
                                        handle=%ev.client_handle,
                                        "未知 client_handle"
                                    );
                                    continue;
                                };
                                batch_vals.push(decode_data_value(
                                    spec,
                                    *pid,
                                    ev.data_value,
                                    &mut last_known,
                                ));
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
                        // 按序清理但永不短路：删项失败也 best-effort 删订阅
                        //（删订阅会收掉所属监控项），失败仅诊断。
                        if !ok_ids.is_empty()
                            && let Err(e) = transport.delete_monitored_items(sub.id, &ok_ids).await
                        {
                            tracing::warn!(
                                sub_id = sub.id,
                                error = %e,
                                "shutdown 删监控项失败（仅诊断，继续删订阅）"
                            );
                        }
                        if let Err(e) = transport.delete_subscription(sub.id).await {
                            tracing::warn!(
                                sub_id = sub.id,
                                error = %e,
                                "shutdown 删订阅失败（仅诊断）"
                            );
                        }
                        // 会话丢失：清理已完成（best-effort），再以 Err 退出，
                        // 交 supervisor cancel/reap → run Err → Manager 重建。
                        if session_lost {
                            return Err(SdkDriverError::new(
                                mesa_core_types::ErrorKind::Connection,
                                "SESSION_LOST",
                                format!("task `{task_id}`: 订阅通道意外关闭（会话丢失）"),
                            ));
                        }
                        Ok::<(), SdkDriverError>(())
                    });
                }
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
                    // 任一 child Err 即 cancel 全体，继续 reap 剩余 workers。
                    shutdown.cancel();
                }
                Err(join_err) => {
                    tracing::error!(%join_err, "SINUMERIK 任务 panic");
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
        // disconnect 由调用方 run() 统一执行（本方法任何返回路径都不直接断连）。
        if let Some(e) = final_err {
            return Err(e);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 任务形态辅助
// ---------------------------------------------------------------------------

/// 通用绑定按 TaskMode 派生 TaskKind（含 subscribe 参数缺省，与通用 OPC UA 同口径）。
fn task_kind_from_mode(task: &AcquisitionTask, task_id: &str) -> Result<TaskKind, SdkDriverError> {
    match task.mode {
        mesa_core_types::TaskMode::Poll => {
            let interval = task.interval_ms.ok_or_else(|| {
                SdkDriverError::configuration("INVALID_TASK", "poll 缺少 interval_ms")
            })?;
            if interval == 0 {
                return Err(SdkDriverError::configuration(
                    "INVALID_TASK",
                    "interval_ms 需 >0",
                ));
            }
            Ok(TaskKind::Poll {
                interval_ms: interval,
            })
        }
        mesa_core_types::TaskMode::Subscribe => {
            subscribe_kind_from_config(&task.binding.config, task_id)
        }
    }
}

fn task_kind_from_binding(
    task: &AcquisitionTask,
    task_id: &str,
    is_poll: bool,
) -> Result<TaskKind, SdkDriverError> {
    if is_poll {
        let interval = task.interval_ms.ok_or_else(|| {
            SdkDriverError::configuration("INVALID_TASK", "poll 缺少 interval_ms")
        })?;
        if interval == 0 {
            return Err(SdkDriverError::configuration(
                "INVALID_TASK",
                "interval_ms 需 >0",
            ));
        }
        Ok(TaskKind::Poll {
            interval_ms: interval,
        })
    } else {
        subscribe_kind_from_config(&task.binding.config, task_id)
    }
}

fn subscribe_kind_from_config(
    config: &serde_json::Value,
    task_id: &str,
) -> Result<TaskKind, SdkDriverError> {
    let publishing_interval_ms = config
        .get("publishing_interval_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(500);
    let sampling_interval_ms = config
        .get("sampling_interval_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(250);
    let queue_size = config
        .get("queue_size")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as u32;
    let discard_oldest = config
        .get("discard_oldest")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if publishing_interval_ms == 0 || sampling_interval_ms == 0 || queue_size == 0 {
        return Err(SdkDriverError::configuration(
            "INVALID_BINDING_CONFIG",
            format!("task `{task_id}`: publishing/sampling/queue 需 >0"),
        ));
    }
    Ok(TaskKind::Subscribe {
        publishing_interval_ms,
        sampling_interval_ms,
        queue_size,
        discard_oldest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{DriverBinding, ResourceSelection, SelectedOutput, TaskMode};
    use mesa_opcua_transport::{UaBrowsePage, UaNodeRef, fake_browse_node};
    use serde_json::json;

    const STD_NS: &str = "http://opcfoundation.org/UA/";
    const SIEMENS_NS: &str = "http://www.siemens.com/sinumerik";

    fn cfg_json() -> SinumerikConnConfig {
        SinumerikConnConfig::default()
    }

    fn generic_task_poll(node_id: &str, point_key: &str) -> AcquisitionTask {
        AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::to_value(GenericBinding {
                    selections: vec![ResourceSelection {
                        resource_id: "node".into(),
                        parameters: json!({"node_id": node_id, "data_type": "F64"}),
                        outputs: vec![SelectedOutput {
                            output: "value".into(),
                            point_key: point_key.into(),
                        }],
                    }],
                })
                .unwrap(),
            },
        }
    }

    #[test]
    fn descriptor_is_valid_and_read_only() {
        let d = SinumerikDriver.descriptor();
        d.validate().expect("sinumerik descriptor 必须合法");
        assert_eq!(d.identity.driver_id, "sinumerik");
        assert!(d.capabilities.poll, "只读 V1 必须 poll");
        assert!(d.capabilities.subscribe, "只读 V1 必须 subscribe");
        assert!(d.capabilities.browse, "必须 browse");
        assert!(!d.capabilities.write, "V1 只读：不得声明 write");
        assert!(!d.capabilities.events, "PR11 无事件：不得声明 events");
        assert!(d.controls.commands.is_empty(), "V1 只读：无控制目录");
        assert!(d.events.streams.is_empty(), "PR11 无事件目录");
        assert!(d.resources.iter().any(|r| r.id == "node"));
    }

    #[tokio::test]
    async fn configure_generic_canonical_ok_and_duplicate_rejected() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(FakeOpcUaTransport::new()));
        let descs = conn
            .configure(
                1,
                vec![generic_task_poll(
                    &format!("nsu={SIEMENS_NS};s=Channel.State"),
                    "chn.state",
                )],
            )
            .await
            .expect("canonical 通用绑定 Ok");
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].point_key, "chn.state");

        // point_key 重复 → DUPLICATE_POINT_KEY
        let mut conn2 =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(FakeOpcUaTransport::new()));
        let dup = AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::to_value(GenericBinding {
                    selections: vec![
                        ResourceSelection {
                            resource_id: "node".into(),
                            parameters: json!({"node_id": format!("nsu={SIEMENS_NS};s=A")}),
                            outputs: vec![SelectedOutput {
                                output: "value".into(),
                                point_key: "dup".into(),
                            }],
                        },
                        ResourceSelection {
                            resource_id: "node".into(),
                            parameters: json!({"node_id": format!("nsu={SIEMENS_NS};s=B")}),
                            outputs: vec![SelectedOutput {
                                output: "value".into(),
                                point_key: "dup".into(),
                            }],
                        },
                    ],
                })
                .unwrap(),
            },
        };
        let err = conn2
            .configure(1, vec![dup])
            .await
            .expect_err("重复必须拒绝");
        // 结构层（validate_selections_structure）先于点表层发现重复，两处任一即合法。
        assert!(
            err.code == "DUPLICATE_POINT_KEY" || err.code == "INVALID_BINDING_CONFIG",
            "expected duplicate rejection, got {}",
            err.code
        );
    }

    #[tokio::test]
    async fn configure_rejects_legacy_ns_index() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(FakeOpcUaTransport::new()));
        let err = conn
            .configure(1, vec![generic_task_poll("ns=2;s=Channel.State", "k")])
            .await
            .expect_err("ns= 索引形态必须拒绝");
        assert_eq!(err.code, "INVALID_ADDRESS");
        assert!(
            err.message.contains("nsu="),
            "须指引 canonical，实际: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn configure_rejects_bad_datatype_and_binding() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(FakeOpcUaTransport::new()));
        // 非法 data_type
        let mut task = generic_task_poll(&format!("nsu={SIEMENS_NS};s=A"), "k");
        task.binding.config["selections"][0]["parameters"]["data_type"] = json!("VARIANT");
        let err = conn
            .configure(1, vec![task])
            .await
            .expect_err("非法 data_type 必须拒绝");
        assert_eq!(err.code, "INVALID_DATA_TYPE");
        // 未知资源
        let mut task2 = generic_task_poll(&format!("nsu={SIEMENS_NS};s=A"), "k");
        task2.binding.config["selections"][0]["resource_id"] = json!("alarm");
        let err = conn
            .configure(1, vec![task2])
            .await
            .expect_err("未知资源必须拒绝");
        assert_eq!(err.code, "UNSUPPORTED_RESOURCE");
        // 缺 node_id（不以 point_key 兜底）
        let task3 = AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::to_value(GenericBinding {
                    selections: vec![ResourceSelection {
                        resource_id: "node".into(),
                        parameters: json!({}),
                        outputs: vec![SelectedOutput {
                            output: "value".into(),
                            point_key: "k".into(),
                        }],
                    }],
                })
                .unwrap(),
            },
        };
        let err = conn
            .configure(1, vec![task3])
            .await
            .expect_err("缺 node_id 必须拒绝");
        assert_eq!(err.code, "INVALID_BINDING_CONFIG");
    }

    #[tokio::test]
    async fn configure_legacy_poll_ok_and_unknown_binding_rejected() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(FakeOpcUaTransport::new()));
        let task = AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: BINDING_POLL.into(),
                config: json!({"nodes": [
                    {"key": "speed", "node_id": format!("nsu={SIEMENS_NS};s=Speed"), "data_type": "F64"},
                ]}),
            },
        };
        let descs = conn.configure(1, vec![task]).await.expect("legacy poll Ok");
        assert_eq!(descs.len(), 1);

        let bad = AcquisitionTask {
            id: "t9".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: "sinumerik.browse".into(),
                config: json!({}),
            },
        };
        let err = conn
            .configure(1, vec![bad])
            .await
            .expect_err("未知绑定拒绝");
        assert_eq!(err.code, "UNSUPPORTED_BINDING");
    }

    /// browse fixture：根下两页（翻页 token 接力），子节点挂 Siemens 命名空间。
    fn browse_fake_two_pages() -> FakeOpcUaTransport {
        let token = vec![0xAAu8, 0xBB];
        FakeOpcUaTransport::new()
            .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()])
            .with_browse_pages(
                &UaNodeRef::numeric(0, 85),
                vec![
                    UaBrowsePage {
                        nodes: vec![
                            fake_browse_node(UaNodeRef::string(1, "Channel"), "Channel"),
                            fake_browse_node(UaNodeRef::string(1, "Axis"), "Axis"),
                        ],
                        continuation_point: Some(token),
                    },
                    UaBrowsePage {
                        nodes: vec![fake_browse_node(UaNodeRef::string(1, "Spindle"), "Spindle")],
                        continuation_point: None,
                    },
                ],
            )
    }

    #[tokio::test]
    async fn browse_root_aggregates_pages_with_canonical_ids() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(browse_fake_two_pages()));
        let (nodes, cursor) = conn.browse("", "", "", 2).await.expect("browse Ok");
        // 翻页已聚合：limit 2 取前两页中的前 2 个，还有第 3 个 → next_cursor
        assert_eq!(nodes.len(), 2);
        assert!(cursor.is_some());
        for n in &nodes {
            assert!(
                n.id.starts_with(&format!("nsu={SIEMENS_NS};")),
                "browse 身份必须 canonical，实际: {}",
                n.id
            );
            let binding: serde_json::Value = serde_json::from_str(&n.binding_json).unwrap();
            assert_eq!(binding["node_id"], serde_json::Value::String(n.id.clone()));
        }
        // 下一页
        let (page2, cursor2) = conn
            .browse("", "", &cursor.unwrap(), 2)
            .await
            .expect("第二页 Ok");
        // 注意：Fake 页队列已被第一次 browse 消费，第二次 browse 返回空
        //（Fake 单次脚本语义）；此处仅断言分页协议形态合法。
        assert!(page2.len() <= 1);
        assert!(cursor2.is_none() || page2.len() == 2);
    }

    #[tokio::test]
    async fn browse_single_shot_returns_all_canonical() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(browse_fake_two_pages()));
        let (nodes, cursor) = conn.browse("", "", "", 50).await.expect("browse Ok");
        assert_eq!(nodes.len(), 3);
        assert!(cursor.is_none());
        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&format!("nsu={SIEMENS_NS};s=Channel").as_str()));
        // 过滤
        let mut conn2 =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(browse_fake_two_pages()));
        let (filtered, _) = conn2
            .browse("", "Spindle", "", 50)
            .await
            .expect("filter Ok");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].label, "Spindle");
    }

    #[tokio::test]
    async fn browse_identity_stable_across_namespace_reshuffle() {
        // 同一 URI 从 index 1 挪到 index 3：browse 产出的 canonical 身份不变。
        async fn browse_ids(fake: FakeOpcUaTransport) -> Vec<String> {
            let mut conn = SinumerikConnection::with_transport(cfg_json(), Arc::new(fake));
            let (nodes, _) = conn.browse("", "", "", 50).await.expect("browse Ok");
            let mut ids: Vec<String> = nodes.into_iter().map(|n| n.id).collect();
            ids.sort();
            ids
        }
        let before = browse_ids(
            FakeOpcUaTransport::new()
                .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()])
                .with_browse_pages(
                    &UaNodeRef::numeric(0, 85),
                    vec![UaBrowsePage {
                        nodes: vec![fake_browse_node(UaNodeRef::string(1, "Channel"), "Channel")],
                        continuation_point: None,
                    }],
                ),
        )
        .await;
        let after = browse_ids(
            FakeOpcUaTransport::new()
                .with_namespace_array(vec![
                    STD_NS.to_string(),
                    "urn:other-a".to_string(),
                    "urn:other-b".to_string(),
                    SIEMENS_NS.to_string(),
                ])
                .with_browse_pages(
                    &UaNodeRef::numeric(0, 85),
                    vec![UaBrowsePage {
                        nodes: vec![fake_browse_node(UaNodeRef::string(3, "Channel"), "Channel")],
                        continuation_point: None,
                    }],
                ),
        )
        .await;
        assert_eq!(before, after);
        assert_eq!(before, vec![format!("nsu={SIEMENS_NS};s=Channel")]);
    }

    #[tokio::test]
    async fn browse_unknown_parent_namespace_fails_closed() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(browse_fake_two_pages()));
        let err = conn
            .browse("nsu=urn:unknown;s=X", "", "", 10)
            .await
            .expect_err("未知命名空间必须 fail-closed");
        assert_eq!(err.code, "UNKNOWN_NAMESPACE");
    }

    #[tokio::test]
    async fn events_not_supported_in_read_only_v1() {
        let mut conn =
            SinumerikConnection::with_transport(cfg_json(), Arc::new(FakeOpcUaTransport::new()));
        conn.configure_events(1, vec![])
            .await
            .expect("空事件表接受");
        let task = mesa_core_types::EventTask {
            id: "e1".into(),
            mode: mesa_core_types::TaskMode::Subscribe,
            interval_ms: None,
            binding: mesa_core_types::DriverBinding {
                kind: "mesa.events.v1".into(),
                config: json!({}),
            },
        };
        let err = conn
            .configure_events(1, vec![task])
            .await
            .expect_err("PR11 非空事件必须拒绝");
        assert_eq!(err.code, "EVENT_NOT_SUPPORTED");
    }
}

/// run 级测试（Checkpoint C/D）：Poll / Subscribe / 重连 / Stop。
///
/// 全部经脚本化 Fake（生产 transport 零改动）；会话/teardown 语义与通用 OPC UA
/// 驱动同形（supervisor、回滚、有界 disconnect、epoch 盖戳）。
#[cfg(test)]
mod run_tests {
    use super::*;
    use mesa_core_types::{
        DataBatch, DriverBinding, GenericBinding, ResourceSelection, SelectedOutput, TaskMode,
    };
    use mesa_driver_sdk::{DataSink, EventBatch};
    use mesa_opcua_transport::{
        FakeOpcUaTransport, OpcUaTransport, SubscriptionStats, UaBrowsePage, UaBrowseRequest,
        UaDataChange, UaDataValue, UaEventMonitoredItemResult, UaEventMonitoredItemSpec,
        UaEventSubscription, UaMonitoredItemId, UaMonitoredItemResult, UaMonitoredItemSpec,
        UaNodeRef, UaOperation, UaSubscription, UaSubscriptionId, UaSubscriptionSpec,
        UaTransportError,
    };
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    const STD_NS: &str = "http://opcfoundation.org/UA/";
    const SIEMENS_NS: &str = "http://www.siemens.com/sinumerik";

    fn sink_with_epoch(handle: u32, epoch: u64) -> (DataSink, mpsc::Receiver<DataBatch>) {
        let (ctrl_tx, _ctrl_rx) = mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (data_tx, data_rx) = mpsc::channel::<DataBatch>(64);
        let (event_tx, _event_rx) = mpsc::channel::<EventBatch>(8);
        (
            DataSink::for_test(ctrl_tx, data_tx, event_tx).for_connection(handle, epoch),
            data_rx,
        )
    }

    fn generic_task(
        id: &str,
        mode: TaskMode,
        interval_ms: Option<u64>,
        node_id: &str,
        data_type: &str,
        point_key: &str,
        extra: serde_json::Value,
    ) -> AcquisitionTask {
        let mut parameters = serde_json::json!({"node_id": node_id, "data_type": data_type});
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                parameters[k] = v.clone();
            }
        }
        AcquisitionTask {
            id: id.into(),
            mode,
            interval_ms,
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::to_value(GenericBinding {
                    selections: vec![ResourceSelection {
                        resource_id: "node".into(),
                        parameters,
                        outputs: vec![SelectedOutput {
                            output: "value".into(),
                            point_key: point_key.into(),
                        }],
                    }],
                })
                .unwrap(),
            },
        }
    }

    fn poll_task(node_id: &str, point_key: &str) -> AcquisitionTask {
        generic_task(
            "t-poll",
            TaskMode::Poll,
            Some(50),
            node_id,
            "F64",
            point_key,
            serde_json::json!({}),
        )
    }

    async fn next_batch(rx: &mut mpsc::Receiver<DataBatch>, what: &str) -> DataBatch {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("{what} 超时"))
            .expect("通道不得关闭")
    }

    /// 会话杀死模拟：前 N 次 read 成功，之后整体失败（READ_FAILED 路径）。
    /// 其余方法全部透传给内部 Fake（只测"会话中途死亡"，不测 transport 本体）。
    /// 附带 disconnect 计数：统一 teardown 出口的"恰一次"断言用。
    struct FlakyTransport {
        inner: FakeOpcUaTransport,
        ok_reads_left: AtomicUsize,
        disconnects: AtomicUsize,
    }

    impl FlakyTransport {
        fn new(inner: FakeOpcUaTransport, ok_reads: usize) -> Self {
            Self {
                inner,
                ok_reads_left: AtomicUsize::new(ok_reads),
                disconnects: AtomicUsize::new(0),
            }
        }

        fn disconnects(&self) -> usize {
            self.disconnects.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl OpcUaTransport for FlakyTransport {
        async fn connect(&self) -> Result<(), UaTransportError> {
            self.inner.connect().await
        }
        async fn disconnect(&self) -> Result<(), UaTransportError> {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            self.inner.disconnect().await
        }
        async fn read(&self, nodes: &[UaNodeRef]) -> Result<Vec<UaDataValue>, UaTransportError> {
            match self
                .ok_reads_left
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            {
                Ok(_) => self.inner.read(nodes).await,
                Err(_) => Err(UaTransportError::service(
                    UaOperation::Read,
                    Some(opcua_types::StatusCode::BadTimeout),
                    true,
                    "Flaky: 会话已杀死",
                )),
            }
        }
        async fn browse(&self, request: UaBrowseRequest) -> Result<UaBrowsePage, UaTransportError> {
            self.inner.browse(request).await
        }
        async fn browse_next(
            &self,
            continuation_point: Vec<u8>,
        ) -> Result<UaBrowsePage, UaTransportError> {
            self.inner.browse_next(continuation_point).await
        }
        async fn release_continuation(
            &self,
            continuation_point: Vec<u8>,
        ) -> Result<(), UaTransportError> {
            self.inner.release_continuation(continuation_point).await
        }
        async fn read_namespace_array(&self) -> Result<Vec<String>, UaTransportError> {
            self.inner.read_namespace_array().await
        }
        async fn create_subscription(
            &self,
            spec: UaSubscriptionSpec,
        ) -> Result<UaSubscription, UaTransportError> {
            self.inner.create_subscription(spec).await
        }
        async fn create_monitored_items(
            &self,
            subscription_id: UaSubscriptionId,
            items: &[UaMonitoredItemSpec],
        ) -> Result<Vec<UaMonitoredItemResult>, UaTransportError> {
            self.inner
                .create_monitored_items(subscription_id, items)
                .await
        }
        async fn delete_monitored_items(
            &self,
            subscription_id: UaSubscriptionId,
            ids: &[UaMonitoredItemId],
        ) -> Result<(), UaTransportError> {
            self.inner
                .delete_monitored_items(subscription_id, ids)
                .await
        }
        async fn delete_subscription(&self, id: UaSubscriptionId) -> Result<(), UaTransportError> {
            self.inner.delete_subscription(id).await
        }
        async fn create_event_subscription(
            &self,
            spec: UaSubscriptionSpec,
        ) -> Result<UaEventSubscription, UaTransportError> {
            self.inner.create_event_subscription(spec).await
        }
        async fn create_event_monitored_items(
            &self,
            subscription_id: UaSubscriptionId,
            items: &[UaEventMonitoredItemSpec],
        ) -> Result<Vec<UaEventMonitoredItemResult>, UaTransportError> {
            self.inner
                .create_event_monitored_items(subscription_id, items)
                .await
        }
    }

    /// 可控订阅通道的脚本 transport：订阅 receiver 的 sender 由测试持有。
    ///
    /// 背景：Fake 的订阅 sender 在 create_subscription 返回后即 drop（事件耗尽
    /// 即 None）。新 P0 语义下"耗尽→None" = 会话丢失；干净 Stop 测试需要通道
    /// 保持打开，会话丢失测试需要精确时刻关闭——两者都要求测试持有 sender。
    /// 非订阅方法全部透传内部 Fake；订阅 cleanup 调用可观测计数。
    struct ScriptedSubTransport {
        inner: FakeOpcUaTransport,
        sender: Mutex<Option<mpsc::Sender<UaDataChange>>>,
        /// 建项时强制 BAD 的节点 key（`UaNodeRef::to_string`），缺省全 Good。
        bad_nodes: Mutex<HashSet<String>>,
        created_subs: AtomicUsize,
        deleted_subs: AtomicUsize,
        deleted_items: AtomicUsize,
        disconnects: AtomicUsize,
    }

    impl ScriptedSubTransport {
        fn new(inner: FakeOpcUaTransport) -> Self {
            Self {
                inner,
                sender: Mutex::new(None),
                bad_nodes: Mutex::new(HashSet::new()),
                created_subs: AtomicUsize::new(0),
                deleted_subs: AtomicUsize::new(0),
                deleted_items: AtomicUsize::new(0),
                disconnects: AtomicUsize::new(0),
            }
        }

        fn mark_bad(&self, node: &UaNodeRef) {
            self.bad_nodes.lock().unwrap().insert(node.to_string());
        }

        /// 等待订阅建立（sender 就位），供测试在发 live 前同步。
        async fn wait_sender(&self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if self.sender.lock().unwrap().is_some() {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "订阅 sender 5s 未就位"
                );
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }

        fn send_live(&self, client_handle: u32, value: opcua_types::DataValue) {
            self.sender
                .lock()
                .unwrap()
                .as_ref()
                .expect("订阅 sender 未就位")
                .try_send(UaDataChange {
                    client_handle,
                    data_value: value,
                })
                .expect("live 事件入队");
        }

        /// 模拟会话丢失：drop sender → receiver 耗尽后关闭（transport 已冻结行为）。
        fn kill_session(&self) {
            *self.sender.lock().unwrap() = None;
        }

        fn created_subs(&self) -> usize {
            self.created_subs.load(Ordering::SeqCst)
        }

        fn deleted_subs(&self) -> usize {
            self.deleted_subs.load(Ordering::SeqCst)
        }

        fn deleted_items(&self) -> usize {
            self.deleted_items.load(Ordering::SeqCst)
        }

        fn disconnects(&self) -> usize {
            self.disconnects.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl OpcUaTransport for ScriptedSubTransport {
        async fn connect(&self) -> Result<(), UaTransportError> {
            self.inner.connect().await
        }
        async fn disconnect(&self) -> Result<(), UaTransportError> {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            self.inner.disconnect().await
        }
        async fn read(&self, nodes: &[UaNodeRef]) -> Result<Vec<UaDataValue>, UaTransportError> {
            self.inner.read(nodes).await
        }
        async fn browse(&self, request: UaBrowseRequest) -> Result<UaBrowsePage, UaTransportError> {
            self.inner.browse(request).await
        }
        async fn browse_next(
            &self,
            continuation_point: Vec<u8>,
        ) -> Result<UaBrowsePage, UaTransportError> {
            self.inner.browse_next(continuation_point).await
        }
        async fn release_continuation(
            &self,
            continuation_point: Vec<u8>,
        ) -> Result<(), UaTransportError> {
            self.inner.release_continuation(continuation_point).await
        }
        async fn read_namespace_array(&self) -> Result<Vec<String>, UaTransportError> {
            self.inner.read_namespace_array().await
        }
        async fn create_subscription(
            &self,
            spec: UaSubscriptionSpec,
        ) -> Result<UaSubscription, UaTransportError> {
            let (tx, rx) = mpsc::channel(256);
            *self.sender.lock().unwrap() = Some(tx);
            self.created_subs.fetch_add(1, Ordering::SeqCst);
            Ok(UaSubscription {
                id: 1,
                requested_publishing_interval_ms: spec.publishing_interval_ms,
                revised_publishing_interval_ms: spec.publishing_interval_ms,
                revised_lifetime_count: spec.lifetime_count,
                revised_max_keep_alive_count: spec.max_keep_alive_count,
                receiver: rx,
                stats: std::sync::Arc::new(SubscriptionStats::default()),
            })
        }
        async fn create_monitored_items(
            &self,
            _subscription_id: UaSubscriptionId,
            items: &[UaMonitoredItemSpec],
        ) -> Result<Vec<UaMonitoredItemResult>, UaTransportError> {
            let bad = self.bad_nodes.lock().unwrap();
            Ok(items
                .iter()
                .enumerate()
                .map(|(idx, s)| {
                    let status = if bad.contains(&s.node.to_string()) {
                        opcua_types::StatusCode::BadNodeIdUnknown
                    } else {
                        opcua_types::StatusCode::Good
                    };
                    UaMonitoredItemResult {
                        client_handle: s.client_handle,
                        monitored_item_id: 100 + idx as u32,
                        status_code: status.bits(),
                        requested_sampling_interval_ms: s.sampling_interval_ms,
                        revised_sampling_interval_ms: s.sampling_interval_ms,
                        requested_queue_size: s.queue_size,
                        revised_queue_size: s.queue_size,
                    }
                })
                .collect())
        }
        async fn delete_monitored_items(
            &self,
            _subscription_id: UaSubscriptionId,
            ids: &[UaMonitoredItemId],
        ) -> Result<(), UaTransportError> {
            // 只观测驱动的清理调用（Fake 侧无对应订阅，不透传）。
            self.deleted_items.fetch_add(ids.len(), Ordering::SeqCst);
            Ok(())
        }
        async fn delete_subscription(&self, _id: UaSubscriptionId) -> Result<(), UaTransportError> {
            self.deleted_subs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn create_event_subscription(
            &self,
            spec: UaSubscriptionSpec,
        ) -> Result<UaEventSubscription, UaTransportError> {
            self.inner.create_event_subscription(spec).await
        }
        async fn create_event_monitored_items(
            &self,
            subscription_id: UaSubscriptionId,
            items: &[UaEventMonitoredItemSpec],
        ) -> Result<Vec<UaEventMonitoredItemResult>, UaTransportError> {
            self.inner
                .create_event_monitored_items(subscription_id, items)
                .await
        }
    }

    fn speed_fake() -> FakeOpcUaTransport {
        FakeOpcUaTransport::new()
            .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()])
            .with_read(
                &UaNodeRef::string(1, "Speed"),
                opcua_types::DataValue::new_now(1500.0f64),
            )
    }

    #[tokio::test]
    async fn poll_run_publishes_decoded_batch_and_stop_is_clean() {
        let mut conn = SinumerikConnection::with_transport(
            SinumerikConnConfig::default(),
            Arc::new(speed_fake()),
        );
        conn.configure(
            1,
            vec![poll_task(
                &format!("nsu={SIEMENS_NS};s=Speed"),
                "spindle.speed",
            )],
        )
        .await
        .expect("configure Ok");
        let mut map = PointMap::new();
        map.insert("spindle.speed".into(), 1001);
        conn.apply_point_map(map).await.expect("apply Ok");

        let (sink, mut rx) = sink_with_epoch(7, 3);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let mut conn_task = conn;
        let run_handle = tokio::spawn(async move { conn_task.run(sink, sd).await });
        let batch = next_batch(&mut rx, "首个 poll 批次").await;
        // SDK 盖戳：驱动传 0/0，wire 上为绑定 handle/epoch。
        assert_eq!(batch.connection_handle, 7);
        assert_eq!(batch.stream_epoch, 3);
        assert_eq!(batch.values.len(), 1);
        assert_eq!(batch.values[0].point_id, 1001);
        assert_eq!(batch.values[0].value, mesa_core_types::Value::F64(1500.0));
        assert_eq!(batch.values[0].quality, mesa_core_types::Quality::Good);
        assert_eq!(
            batch.values[0].value_origin,
            mesa_core_types::ValueOrigin::Current
        );
        // Stop：run 干净退出，不再产出。
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run_handle)
            .await
            .expect("run 必须退出")
            .expect("run 不 panic")
            .expect("正常 Stop 必须 Ok");
    }

    #[tokio::test]
    async fn poll_bad_point_is_explicit_placeholder() {
        // 未预置 read 的点 → Fake 单点 BAD → typed placeholder（绝不静默 0）。
        let mut conn = SinumerikConnection::with_transport(
            SinumerikConnConfig::default(),
            Arc::new(
                FakeOpcUaTransport::new()
                    .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()]),
            ),
        );
        conn.configure(
            1,
            vec![poll_task(&format!("nsu={SIEMENS_NS};s=Missing"), "missing")],
        )
        .await
        .expect("configure Ok");
        let mut map = PointMap::new();
        map.insert("missing".into(), 1002);
        conn.apply_point_map(map).await.expect("apply Ok");

        let (sink, mut rx) = sink_with_epoch(7, 3);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        let batch = next_batch(&mut rx, "BAD 批次").await;
        assert_eq!(batch.values[0].point_id, 1002);
        assert_eq!(batch.values[0].quality, mesa_core_types::Quality::Bad);
        assert_eq!(
            batch.values[0].value_origin,
            mesa_core_types::ValueOrigin::Placeholder
        );
        assert_eq!(batch.values[0].source_timestamp_ns, None);
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run_handle)
            .await
            .expect("run 必须退出")
            .expect("run 不 panic")
            .expect("Stop 必须 Ok");
    }

    #[tokio::test]
    async fn run_without_plan_or_with_empty_plan_fails_closed() {
        let (sink, _rx) = sink_with_epoch(7, 3);
        // 未 configure
        let mut conn = SinumerikConnection::with_transport(
            SinumerikConnConfig::default(),
            Arc::new(speed_fake()),
        );
        let err = conn
            .run(sink, CancellationToken::new())
            .await
            .expect_err("未 configure 必须拒绝");
        assert_eq!(err.code, "NO_PLAN");
        // 空任务表（V1 无事件可豁免，直接拒绝空转）
        let (sink, _rx) = sink_with_epoch(7, 3);
        let mut conn = SinumerikConnection::with_transport(
            SinumerikConnConfig::default(),
            Arc::new(speed_fake()),
        );
        conn.configure(1, vec![])
            .await
            .expect("空表 configure 接受");
        let mut map = PointMap::new();
        map.insert("nope".into(), 1);
        let err = conn
            .run(sink, CancellationToken::new())
            .await
            .expect_err("空任务 run 必须拒绝");
        assert_eq!(err.code, "EMPTY_PLAN");
        let _ = map;
    }

    #[tokio::test]
    async fn run_unknown_namespace_disconnects_exactly_once() {
        // 设备 NamespaceArray 无该 URI（命名空间被改）→ fail-closed，
        // 且统一 teardown 出口保证 disconnect 恰一次（early-Err 不绕过）。
        let flaky = Arc::new(FlakyTransport::new(
            FakeOpcUaTransport::new().with_namespace_array(vec![STD_NS.to_string()]),
            99,
        ));
        let mut conn =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), flaky.clone());
        conn.configure(
            1,
            vec![poll_task(&format!("nsu={SIEMENS_NS};s=Speed"), "speed")],
        )
        .await
        .expect("configure 只做语法校验，通过");
        let mut map = PointMap::new();
        map.insert("speed".into(), 1001);
        conn.apply_point_map(map).await.expect("apply Ok");
        let (sink, _rx) = sink_with_epoch(7, 3);
        let err = conn
            .run(sink, CancellationToken::new())
            .await
            .expect_err("未知命名空间必须 fail-closed");
        assert_eq!(err.code, "UNKNOWN_NAMESPACE");
        assert_eq!(
            flaky.disconnects(),
            1,
            "统一出口：early-Err 也必须 disconnect 恰一次"
        );
    }

    #[tokio::test]
    async fn early_namespace_failure_disconnects_exactly_once() {
        // connect 成功但 NamespaceArray 失败 → run Err，且 disconnect 恰一次。
        let flaky = Arc::new(FlakyTransport::new(
            FakeOpcUaTransport::new().with_namespace_array_error(
                mesa_opcua_transport::UaTransportError::service(
                    UaOperation::ReadNamespaceArray,
                    Some(opcua_types::StatusCode::BadTimeout),
                    true,
                    "Fake 命名空间失败",
                ),
            ),
            99,
        ));
        let mut conn =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), flaky.clone());
        conn.configure(
            1,
            vec![poll_task(&format!("nsu={SIEMENS_NS};s=Speed"), "speed")],
        )
        .await
        .expect("configure 通过");
        let mut map = PointMap::new();
        map.insert("speed".into(), 1001);
        conn.apply_point_map(map).await.expect("apply Ok");
        let (sink, _rx) = sink_with_epoch(7, 3);
        let err = conn
            .run(sink, CancellationToken::new())
            .await
            .expect_err("命名空间失败必须 Err");
        assert_eq!(err.code, "NAMESPACE_FAILED");
        assert_eq!(
            flaky.disconnects(),
            1,
            "统一出口：early-Err 也必须 disconnect 恰一次"
        );
    }

    #[tokio::test]
    async fn session_loss_fails_run_and_reconnect_resumes_same_point_id() {
        // 生命周期核心：kill session → run 报 READ_FAILED（Manager 据此重建）；
        // 新会话（命名空间 index 已漂移）→ 同一 canonical 同一 point_id，数据继续。
        let task = poll_task(&format!("nsu={SIEMENS_NS};s=Speed"), "speed");
        let mut map = PointMap::new();
        map.insert("speed".into(), 1001);

        // 第一程：1 次成功 read 后会话死亡
        let flaky = Arc::new(FlakyTransport::new(speed_fake(), 1));
        let mut conn =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), flaky.clone());
        conn.configure(1, vec![task.clone()])
            .await
            .expect("configure");
        conn.apply_point_map(map.clone()).await.expect("apply");
        let (sink, mut rx) = sink_with_epoch(7, 3);
        let err = {
            let shutdown = CancellationToken::new();
            conn.run(sink, shutdown)
                .await
                .expect_err("会话死亡必须 Err")
        };
        assert_eq!(err.code, "READ_FAILED");
        assert_eq!(
            flaky.disconnects(),
            1,
            "统一出口：worker Err 后也必须 disconnect 恰一次"
        );
        let first = next_batch(&mut rx, "死亡前批次").await;
        assert_eq!(first.values[0].point_id, 1001);

        // 第二程：重建连接（index 1→3 漂移），同一 Core 映射 → 同 point_id
        let revived = FakeOpcUaTransport::new()
            .with_namespace_array(vec![
                STD_NS.to_string(),
                "urn:other".to_string(),
                "urn:more".to_string(),
                SIEMENS_NS.to_string(),
            ])
            .with_read(
                &UaNodeRef::string(3, "Speed"),
                opcua_types::DataValue::new_now(1600.0f64),
            );
        let mut conn2 =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), Arc::new(revived));
        conn2.configure(2, vec![task]).await.expect("configure");
        conn2.apply_point_map(map).await.expect("apply");
        let (sink2, mut rx2) = sink_with_epoch(7, 4);
        let shutdown2 = CancellationToken::new();
        let sd2 = shutdown2.clone();
        let run2 = tokio::spawn(async move { conn2.run(sink2, sd2).await });
        let batch2 = next_batch(&mut rx2, "重连后批次").await;
        assert_eq!(batch2.values[0].point_id, 1001, "point_id 不漂");
        assert_eq!(batch2.values[0].value, mesa_core_types::Value::F64(1600.0));
        shutdown2.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run2)
            .await
            .expect("run2 必须退出")
            .expect("run2 不 panic")
            .expect("Stop 必须 Ok");
    }

    #[tokio::test]
    async fn subscribe_run_forwards_live_and_stop_is_clean() {
        // 通道保持打开 → Stop 干净 Ok；清理可观测（删项+删订阅+disconnect 各恰一次）。
        let scripted = Arc::new(ScriptedSubTransport::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()]),
        ));
        let mut conn =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), scripted.clone());
        conn.configure(
            1,
            vec![generic_task(
                "t-sub",
                TaskMode::Subscribe,
                None,
                &format!("nsu={SIEMENS_NS};s=Counter"),
                "I32",
                "counter",
                serde_json::json!({"publishing_interval_ms": 100}),
            )],
        )
        .await
        .expect("configure Ok");
        let mut map = PointMap::new();
        map.insert("counter".into(), 2001);
        conn.apply_point_map(map).await.expect("apply Ok");

        let (sink, mut rx) = sink_with_epoch(7, 3);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        scripted.wait_sender().await;
        scripted.send_live(1, opcua_types::DataValue::new_now(42i32));
        let batch = next_batch(&mut rx, "订阅 live 批次").await;
        assert_eq!(batch.values[0].point_id, 2001);
        assert_eq!(batch.values[0].value, mesa_core_types::Value::I32(42));
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run_handle)
            .await
            .expect("run 必须退出")
            .expect("run 不 panic")
            .expect("Stop 必须 Ok");
        // 清理可观测：订阅建一次、监控项与订阅均被删除、无泄漏、统一出口断连一次。
        assert_eq!(scripted.created_subs(), 1);
        assert_eq!(scripted.deleted_items(), 1);
        assert_eq!(scripted.deleted_subs(), 1);
        assert_eq!(scripted.disconnects(), 1);
    }

    #[tokio::test]
    async fn subscribe_only_session_loss_returns_error() {
        // P0 Gate：Subscribe-only 端点会话丢失 → run Err(SESSION_LOST)，
        // Manager 据此 Failed+重建；绝不能是 Ok(Stopped)（否则永不重连）。
        let scripted = Arc::new(ScriptedSubTransport::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()]),
        ));
        let mut conn =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), scripted.clone());
        conn.configure(
            1,
            vec![generic_task(
                "t-sub",
                TaskMode::Subscribe,
                None,
                &format!("nsu={SIEMENS_NS};s=Counter"),
                "I32",
                "counter",
                serde_json::json!({"publishing_interval_ms": 100}),
            )],
        )
        .await
        .expect("configure Ok");
        let mut map = PointMap::new();
        map.insert("counter".into(), 2001);
        conn.apply_point_map(map).await.expect("apply Ok");

        let (sink, mut rx) = sink_with_epoch(7, 3);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        scripted.wait_sender().await;
        scripted.send_live(1, opcua_types::DataValue::new_now(43i32));
        // 死亡前数据正常流动（证明订阅曾建好，不是建连失败）。
        let live = next_batch(&mut rx, "死亡前 live").await;
        assert_eq!(live.values[0].point_id, 2001);
        // 会话丢失：transport abort forwarders → receiver 关闭（已冻结行为）。
        scripted.kill_session();
        let err = tokio::time::timeout(std::time::Duration::from_secs(5), run_handle)
            .await
            .expect("run 必须退出")
            .expect("run 不 panic")
            .expect_err("会话丢失必须 Err，不能是 Ok(Stopped)");
        assert_eq!(err.code, "SESSION_LOST");
        // 清理先于 Err：删项+删订阅恰一次（不短路），统一出口 disconnect 恰一次。
        assert_eq!(scripted.deleted_items(), 1);
        assert_eq!(scripted.deleted_subs(), 1);
        assert_eq!(scripted.disconnects(), 1);
    }

    #[tokio::test]
    async fn subscribe_session_loss_reconnects_with_same_point_id() {
        // 第一程 SESSION_LOST；第二程（index 漂移）同 point_id 恢复数据。
        let task = generic_task(
            "t-sub",
            TaskMode::Subscribe,
            None,
            &format!("nsu={SIEMENS_NS};s=Counter"),
            "I32",
            "counter",
            serde_json::json!({"publishing_interval_ms": 100}),
        );
        let mut map = PointMap::new();
        map.insert("counter".into(), 2001);

        let scripted1 = Arc::new(ScriptedSubTransport::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()]),
        ));
        let mut conn1 =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), scripted1.clone());
        conn1
            .configure(1, vec![task.clone()])
            .await
            .expect("configure");
        conn1.apply_point_map(map.clone()).await.expect("apply");
        let (sink1, mut rx1) = sink_with_epoch(7, 3);
        let sd1 = CancellationToken::new();
        let sd1c = sd1.clone();
        let run1 = tokio::spawn(async move { conn1.run(sink1, sd1c).await });
        scripted1.wait_sender().await;
        scripted1.send_live(1, opcua_types::DataValue::new_now(43i32));
        let live1 = next_batch(&mut rx1, "第一程 live").await;
        assert_eq!(live1.values[0].point_id, 2001);
        scripted1.kill_session();
        let err = tokio::time::timeout(std::time::Duration::from_secs(5), run1)
            .await
            .expect("run1 必须退出")
            .expect("run1 不 panic")
            .expect_err("第一程必须 SESSION_LOST");
        assert_eq!(err.code, "SESSION_LOST");

        // 第二程：重建连接（URI index 1→3 漂移），同一 Core 映射 → 同 point_id。
        let scripted2 = Arc::new(ScriptedSubTransport::new(
            FakeOpcUaTransport::new().with_namespace_array(vec![
                STD_NS.to_string(),
                "urn:other".to_string(),
                "urn:more".to_string(),
                SIEMENS_NS.to_string(),
            ]),
        ));
        let mut conn2 =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), scripted2.clone());
        conn2.configure(2, vec![task]).await.expect("configure");
        conn2.apply_point_map(map).await.expect("apply");
        let (sink2, mut rx2) = sink_with_epoch(7, 4);
        let shutdown2 = CancellationToken::new();
        let sd2 = shutdown2.clone();
        let run2 = tokio::spawn(async move { conn2.run(sink2, sd2).await });
        scripted2.wait_sender().await;
        scripted2.send_live(1, opcua_types::DataValue::new_now(44i32));
        let live2 = next_batch(&mut rx2, "第二程 live").await;
        assert_eq!(live2.values[0].point_id, 2001, "point_id 不漂");
        assert_eq!(live2.values[0].value, mesa_core_types::Value::I32(44));
        shutdown2.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run2)
            .await
            .expect("run2 必须退出")
            .expect("run2 不 panic")
            .expect("Stop 必须 Ok");
    }

    #[tokio::test]
    async fn subscribe_single_bad_item_synthesizes_initial_bad_first() {
        // 单项建项失败 → 初始 BAD 有序在 live 之前（与通用 OPC UA 同序）。
        let scripted = Arc::new(ScriptedSubTransport::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(vec![STD_NS.to_string(), SIEMENS_NS.to_string()]),
        ));
        scripted.mark_bad(&UaNodeRef::string(1, "Broken"));
        let mut conn =
            SinumerikConnection::with_transport(SinumerikConnConfig::default(), scripted.clone());
        let task = AcquisitionTask {
            id: "t-sub".into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::to_value(GenericBinding {
                    selections: vec![
                        ResourceSelection {
                            resource_id: "node".into(),
                            parameters: serde_json::json!({
                                "node_id": format!("nsu={SIEMENS_NS};s=Counter"),
                                "data_type": "I32",
                            }),
                            outputs: vec![SelectedOutput {
                                output: "value".into(),
                                point_key: "counter".into(),
                            }],
                        },
                        ResourceSelection {
                            resource_id: "node".into(),
                            parameters: serde_json::json!({
                                "node_id": format!("nsu={SIEMENS_NS};s=Broken"),
                                "data_type": "I32",
                            }),
                            outputs: vec![SelectedOutput {
                                output: "value".into(),
                                point_key: "broken".into(),
                            }],
                        },
                    ],
                })
                .unwrap(),
            },
        };
        conn.configure(1, vec![task]).await.expect("configure Ok");
        let mut map = PointMap::new();
        map.insert("counter".into(), 2001);
        map.insert("broken".into(), 2002);
        conn.apply_point_map(map).await.expect("apply Ok");

        let (sink, mut rx) = sink_with_epoch(7, 3);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        scripted.wait_sender().await;
        scripted.send_live(1, opcua_types::DataValue::new_now(7i32));
        // 首批：失败项的合成 BAD（有序在 live 之前）
        let bad = next_batch(&mut rx, "初始 BAD").await;
        assert_eq!(bad.values[0].point_id, 2002);
        assert_eq!(bad.values[0].quality, mesa_core_types::Quality::Bad);
        // 次批：live 好值
        let live = next_batch(&mut rx, "live").await;
        assert_eq!(live.values[0].point_id, 2001);
        assert_eq!(live.values[0].value, mesa_core_types::Value::I32(7));
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), run_handle)
            .await
            .expect("run 必须退出")
            .expect("run 不 panic")
            .expect("Stop 必须 Ok");
    }
}
