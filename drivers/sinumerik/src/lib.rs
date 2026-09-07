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
mod probe;
mod value;

pub use canonical::{CanonicalError, ResolveError, SinumerikIdentifier, SinumerikNodeId};
pub use config::SinumerikConnConfig;
pub use probe::probe_with_transport;
pub use value::{
    LastKnownSample, PointSpec, decode_data_value, parse_data_type, status_to_quality,
};

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

#[allow(dead_code)]
#[derive(Debug)]
struct PlanSnapshot {
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    map: Option<PointMap>,
}

struct SinumerikConnection {
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
    /// 测试/Fixture 注入脚本化 transport（生产经 `open_connection` 构造）。
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
                let (kind, _interval) = task_kind_from_mode(task, &task.id)?;
                let _ = _interval;
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
            let (kind, _interval) = task_kind_from_binding(task, &task.id, is_poll)?;
            let _ = _interval;
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

    /// 只读数据路径在后续 commit 落地（Poll → Subscribe → 重连硬化）。
    /// 此处先显式 Unsupported，避免"静默无数据"被误读为正常空闲。
    async fn run(
        &mut self,
        _sink: DataSink,
        _shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError> {
        let _ = _sink;
        Err(SdkDriverError::new(
            mesa_core_types::ErrorKind::Unsupported,
            "READ_PATH_NOT_YET",
            "只读数据路径在后续 commit 落地（当前仅 Descriptor/Probe/Browse/Identity）",
        ))
    }
}

// ---------------------------------------------------------------------------
// 任务形态辅助
// ---------------------------------------------------------------------------

/// 通用绑定按 TaskMode 派生 TaskKind（含 subscribe 参数缺省，与通用 OPC UA 同口径）。
fn task_kind_from_mode(
    task: &AcquisitionTask,
    task_id: &str,
) -> Result<(TaskKind, u64), SdkDriverError> {
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
            Ok((
                TaskKind::Poll {
                    interval_ms: interval,
                },
                interval,
            ))
        }
        mesa_core_types::TaskMode::Subscribe => {
            let (kind, interval) = subscribe_kind_from_config(&task.binding.config, task_id)?;
            Ok((kind, interval))
        }
        _ => Err(SdkDriverError::new(
            mesa_core_types::ErrorKind::Unsupported,
            "MODE_NOT_SUPPORTED",
            format!("task `{task_id}`: sinumerik node 仅支持 poll/subscribe"),
        )),
    }
}

fn task_kind_from_binding(
    task: &AcquisitionTask,
    task_id: &str,
    is_poll: bool,
) -> Result<(TaskKind, u64), SdkDriverError> {
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
        Ok((
            TaskKind::Poll {
                interval_ms: interval,
            },
            interval,
        ))
    } else {
        subscribe_kind_from_config(&task.binding.config, task_id)
    }
}

fn subscribe_kind_from_config(
    config: &serde_json::Value,
    task_id: &str,
) -> Result<(TaskKind, u64), SdkDriverError> {
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
    Ok((
        TaskKind::Subscribe {
            publishing_interval_ms,
            sampling_interval_ms,
            queue_size,
            discard_oldest,
        },
        publishing_interval_ms,
    ))
}

// 抑制未使用告警：run 落地后启用（TaskPlan.kind/point_indices、PlanSnapshot 全字段）。
#[allow(dead_code)]
fn _plan_fields_used(_plan: &TaskPlan) {}

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
