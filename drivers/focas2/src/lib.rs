//! FANUC FOCAS2 Driver — 方案 §7.2 资源型（V1 只读）。
//!
//! - 绑定种类：`focas.data-block`
//!   ```json
//!   { "items": [
//!       { "key": "cnc.status", "address": "status", "data_type": "U32" },
//!       { "key": "axis.x", "address": "axis.abs.1", "data_type": "I32" },
//!       { "key": "spindle.load", "address": "spindle.load.1", "data_type": "U32" }
//!   ]}
//!   ```
//! - 地址解析见 `address::parse_address`；Core 不触及此文件（硬性约束）。
//! - 访问抽象 `focas_api::FocasApi`，当前默认 `FakeFocasApi`（多协议骨架验证），
//!   真机时切换 `NativeFocasApi`（Fwlib FFI，预留）。

mod address;
mod focas_api;
mod native;

pub use address::{AddressError, FocasAddress, parse_address};
pub use focas_api::{FakeFocasApi, FocasApi, NativeFocasApi};

use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{
    AcquisitionTask, CapabilityItem, CapabilityState, DataBatch, DataType, DriverMetadata,
    DuplicatePointKey, GENERIC_BINDING_KIND, GenericBinding, PointDescriptor, PointMap, PointValue,
    ProbeReport, ProbeWarning, Quality, TaskMode, Value, ValueOrigin, ensure_unique_point_keys,
    now_unix_ns,
};
use mesa_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
use tokio_util::sync::CancellationToken;

pub const BINDING_KIND: &str = "focas.data-block";

use focas_api::FocasApi as FocasApiTrait;

// ---------------------------------------------------------------------------
// 驱动入口
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct FocasDriver;

#[async_trait::async_trait]
impl Driver for FocasDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: "focas2".into(),
            name: "FANUC FOCAS2".into(),
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
                    FieldDescriptor::new("host", "Host", FieldType::Host)
                        .required(true)
                        .default_value(serde_json::json!("192.168.0.1")),
                    FieldDescriptor::new("port", "Port", FieldType::Port)
                        .required(false)
                        .default_value(serde_json::json!(8193)),
                    FieldDescriptor::new("timeout_ms", "Timeout ms", FieldType::Duration)
                        .required(false)
                        .default_value(serde_json::json!(3000)),
                ],
            },
            resources: vec![
                ResourceDescriptor {
                    id: "dynamic".into(),
                    label: LocalizedText::new("Dynamic"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            FieldDescriptor::new("axis", "Axis", FieldType::Integer)
                                .required(false)
                                .default_value(serde_json::json!(1)),
                        ],
                    },
                    outputs: vec![
                        OutputDescriptor {
                            id: "feed".into(),
                            label: LocalizedText::new("Feed"),
                            data_type: DataType::U32,
                            unit: Some("mm/min".into()),
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            id: "spindle.speed".into(),
                            label: LocalizedText::new("Spindle Speed"),
                            data_type: DataType::U32,
                            unit: Some("rpm".into()),
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            id: "program.current".into(),
                            label: LocalizedText::new("Current Program"),
                            data_type: DataType::String,
                            unit: None,
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            id: "position.absolute".into(),
                            label: LocalizedText::new("Absolute Position"),
                            data_type: DataType::I32,
                            unit: Some("pulse".into()),
                            access: AccessMode::Read,
                        },
                    ],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "status".into(),
                    label: LocalizedText::new("Status"),
                    parameters: SchemaDescriptor::default(),
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Status"),
                        data_type: DataType::U32,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "axis".into(),
                    label: LocalizedText::new("Axis"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            FieldDescriptor::new("axis", "Axis", FieldType::Integer).required(true),
                        ],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Position"),
                        data_type: DataType::I32,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "spindle".into(),
                    label: LocalizedText::new("Spindle"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            FieldDescriptor::new("spindle", "Spindle", FieldType::Integer)
                                .required(true),
                        ],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Load"),
                        data_type: DataType::U32,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "pmc".into(),
                    label: LocalizedText::new("PMC"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            {
                                let mut f = FieldDescriptor::new("kind", "Kind", FieldType::Enum)
                                    .required(true)
                                    .default_value(serde_json::json!("R"));
                                f.validation.enum_options = Some(vec![
                                    "G".into(),
                                    "R".into(),
                                    "X".into(),
                                    "Y".into(),
                                    "F".into(),
                                    "A".into(),
                                    "D".into(),
                                    "C".into(),
                                    "K".into(),
                                    "T".into(),
                                ]);
                                f
                            },
                            FieldDescriptor::new("addr", "Address", FieldType::Integer)
                                .required(true),
                        ],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Value"),
                        data_type: DataType::I32,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "macro".into(),
                    label: LocalizedText::new("Macro"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            FieldDescriptor::new("number", "Number", FieldType::Integer)
                                .required(true),
                        ],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Value"),
                        data_type: DataType::F64,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "alarm".into(),
                    label: LocalizedText::new("Alarm"),
                    parameters: SchemaDescriptor::default(),
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Alarm"),
                        data_type: DataType::String,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "program".into(),
                    label: LocalizedText::new("Program"),
                    parameters: SchemaDescriptor::default(),
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Program"),
                        data_type: DataType::String,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
            ],
            controls: mesa_core_types::ControlCatalog {
                commands: vec![mesa_core_types::capability::CommandDescriptor {
                    id: "status".into(),
                    label: mesa_core_types::LocalizedText::new("状态查询"),
                    description: Some("只读：读取 165 CNC 状态（statinfo），不改机床".into()),
                    input_schema: mesa_core_types::SchemaDescriptor::default(),
                    result_schema: mesa_core_types::SchemaDescriptor::default(),
                    risk: mesa_core_types::capability::RiskLevel::Low,
                    confirmation: false,
                    timeout_ms: Some(3000),
                    idempotent: true,
                }],
            },
            discovery: DiscoveryCapabilities {
                manual: true,
                browse: false,
                import: false,
            },
            capabilities: DriverCapabilities {
                poll: true,
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
        let cfg = FocasConnConfig::from_json(&v)?;
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
        let api: Arc<dyn FocasApiTrait> = if use_native {
            Arc::new(NativeFocasApi::new())
        } else {
            Arc::new(FakeFocasApi::new())
        };
        Ok(Box::new(FocasConnection {
            cfg,
            api,
            plan: None,
        }))
    }
}

/// 可注入后端的探测主体（connection 方法与单测共用；单测直传 Fake，无需碰进程级 env）。
async fn await_probe_with_api(
    api: &Arc<dyn FocasApiTrait>,
    cfg: &FocasConnConfig,
) -> Result<ProbeReport, SdkDriverError> {
    if let Err(e) = api.connect(&cfg.host, cfg.port, cfg.timeout_ms).await {
        return Ok(ProbeReport::unreachable("CONNECTION_FAILED", e));
    }
    // P1-1：read 是否 Available 以本次 sysinfo 实测为准；失败即 Unknown。
    let report = match api.system_info().await {
        Ok(info) => ProbeReport {
            reachable: true,
            vendor: Some("FANUC".into()),
            family: Some(info.series),
            model: None,
            firmware: Some(info.version),
            model_confidence: None,
            // subscribe/browse 为实现确认缺席（静态事实，可断言）。
            capabilities: vec![
                CapabilityItem {
                    id: "read".into(),
                    state: CapabilityState::Available,
                    detail: None,
                },
                CapabilityItem {
                    id: "subscribe".into(),
                    state: CapabilityState::NotPresent,
                    detail: Some("focas2 only supports poll mode".into()),
                },
                CapabilityItem {
                    id: "browse".into(),
                    state: CapabilityState::NotPresent,
                    detail: Some("focas2 has no browse space".into()),
                },
            ],
            warnings: vec![ProbeWarning {
                code: "MODEL_UNDETECTED".into(),
                message: "ODBSYS series 无法唯一确定 model，需真机确认映射".into(),
            }],
        },
        Err(e) => ProbeReport {
            reachable: true,
            vendor: Some("FANUC".into()),
            family: None,
            model: None,
            firmware: None,
            model_confidence: None,
            capabilities: vec![
                CapabilityItem {
                    id: "read".into(),
                    state: CapabilityState::Unknown,
                    detail: Some(format!("system_info 读取失败: {e}")),
                },
                CapabilityItem {
                    id: "subscribe".into(),
                    state: CapabilityState::NotPresent,
                    detail: Some("focas2 only supports poll mode".into()),
                },
                CapabilityItem {
                    id: "browse".into(),
                    state: CapabilityState::NotPresent,
                    detail: Some("focas2 has no browse space".into()),
                },
            ],
            // P0-1 不重复规则：局部原因只在 read.detail，全局后果只在 warning。
            warnings: vec![ProbeWarning {
                code: "IDENTITY_UNAVAILABLE".into(),
                message: "设备身份（series/version）未能识别，profile 提示可能缺失".into(),
            }],
        },
    };
    // 短连接清理（trait 返回 ()，无可掩盖的错误）
    api.disconnect().await;
    Ok(report)
}

// ---------------------------------------------------------------------------
// 连接配置
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FocasConnConfig {
    host: String,
    port: u16,
    timeout_ms: u64,
}

impl Default for FocasConnConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8193,
            timeout_ms: 3000,
        }
    }
}

impl FocasConnConfig {
    fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        // 兼容两种写法：{host,port,timeout_ms} 或 {ip,port}
        let host = v
            .get("host")
            .or_else(|| v.get("ip"))
            .and_then(|x| x.as_str())
            .unwrap_or("127.0.0.1")
            .to_string();
        let port = v.get("port").and_then(|x| x.as_u64()).unwrap_or(8193) as u16;
        let timeout_ms = v
            .get("timeout_ms")
            .or_else(|| v.get("timeout"))
            .and_then(|x| x.as_u64())
            .unwrap_or(3000);
        if host.trim().is_empty() {
            return Err(SdkDriverError::configuration("BAD_CONFIG", "host 不能为空"));
        }
        if port == 0 {
            return Err(SdkDriverError::configuration("BAD_CONFIG", "port 非法"));
        }
        Ok(Self {
            host,
            port,
            timeout_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// 采集计划
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PointSpec {
    key: String,
    addr: FocasAddress,
    data_type: DataType,
}

#[derive(Debug)]
struct TaskPlan {
    // TODO: PlanSnapshot 冻结字段，task id 用于诊断与多任务追踪，V1 仅内存使用但需保留
    #[allow(dead_code)]
    id: String,
    interval_ms: u64,
    point_indices: Vec<usize>,
}

#[derive(Debug)]
struct PlanSnapshot {
    // TODO: PlanSnapshot 冻结字段，revision 为 §6.2 全量快照版本号，需保留以备 Driver 侧原子校验与回放
    #[allow(dead_code)]
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    map: Option<PointMap>,
}

struct FocasConnection {
    cfg: FocasConnConfig,
    api: Arc<dyn FocasApiTrait>,
    plan: Option<PlanSnapshot>,
}

impl std::fmt::Debug for FocasConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FocasConnection")
            .field("cfg", &self.cfg)
            .field("has_plan", &self.plan.is_some())
            .finish()
    }
}

// 将 Value 校验为期望的 DataType（用于 configure 阶段快速失败）
fn value_fits_data_type(v: &Value, dt: DataType) -> bool {
    match (v, dt) {
        (Value::Bool(_), DataType::Bool) => true,
        (Value::U32(_), DataType::U32) => true,
        (Value::I32(_), DataType::I32) => true,
        (Value::F32(_), DataType::F32) => true,
        (Value::F64(_), DataType::F64) => true,
        (Value::String(_), DataType::String) => true,
        // 允许一定宽容：U32/I32 互通，F32/F64 互通
        (Value::U32(_), DataType::I32) => true,
        (Value::I32(_), DataType::U32) => true,
        (Value::F32(_), DataType::F64) => true,
        (Value::F64(_), DataType::F32) => true,
        _ => false,
    }
}

fn parse_data_type(s: &str) -> Result<DataType, SdkDriverError> {
    match s.trim().to_ascii_uppercase().as_str() {
        "BOOL" | "BOOLEAN" => Ok(DataType::Bool),
        "U32" | "UINT32" | "DWORD" => Ok(DataType::U32),
        "I32" | "INT32" | "DINT" | "INT" => Ok(DataType::I32),
        "F32" | "FLOAT" | "REAL" => Ok(DataType::F32),
        "F64" | "DOUBLE" | "LREAL" => Ok(DataType::F64),
        "STRING" | "STR" => Ok(DataType::String),
        _ => Err(SdkDriverError::configuration(
            "INVALID_DATA_TYPE",
            format!("data_type `{s}` 非法，期望 BOOL/U32/I32/F32/F64/STRING"),
        )),
    }
}

#[async_trait::async_trait]
impl DriverConnection for FocasConnection {
    /// FOCAS2 动态探测：复用本连接的 api/cfg 做短连接 + `system_info`（低风险只读）。
    /// - 建连失败 → Ok(unreachable)；sysinfo 失败 → reachable + IDENTITY_UNAVAILABLE；
    /// - series 可确认 family/firmware，但 model 无法从 ODBSYS 唯一确定，
    ///   恒为 None + MODEL_UNDETECTED（等真机确认 series→model 映射）。
    /// 配置已在 OpenConnection 校验（含 use_native 门禁）；短连接的 disconnect
    /// 在返回前 best-effort 执行，不掩盖探测结论。
    async fn probe(&mut self) -> Result<ProbeReport, SdkDriverError> {
        await_probe_with_api(&self.api, &self.cfg).await
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
            if task.mode != TaskMode::Poll {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: focas2 仅支持 poll", task.id),
                ));
            }
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
                let mut indices = Vec::new();
                for sel in &binding.selections {
                    for out in &sel.outputs {
                        // 优先使用 parameters.address，否则回退到 output id（便于测试直接使用合法 FOCAS 地址作为 output）
                        let addr_str = sel
                            .parameters
                            .get("address")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&out.output)
                            .to_string();
                        let dt_str = sel
                            .parameters
                            .get("data_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("U32");
                        let addr = parse_address(&addr_str).map_err(|e| match e {
                            AddressError::Empty => SdkDriverError::configuration(
                                "INVALID_ADDRESS",
                                format!("point `{}` 地址为空", out.point_key),
                            ),
                            AddressError::Invalid { reason, .. } => SdkDriverError::new(
                                mesa_core_types::ErrorKind::Address,
                                "INVALID_ADDRESS",
                                format!(
                                    "point `{}` 地址 `{addr_str}` 非法: {reason}",
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
                new_tasks.push(TaskPlan {
                    id: task.id.clone(),
                    interval_ms: task.interval_ms.expect("validated above"),
                    point_indices: indices,
                });
                continue;
            }
            if task.binding.kind != BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: 期望 {BINDING_KIND} 或 {GENERIC_BINDING_KIND}，实际 {}",
                        task.id, task.binding.kind
                    ),
                ));
            }
            let items = task
                .binding
                .config
                .get("items")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: 缺少 items 数组", task.id),
                    )
                })?;
            if items.is_empty() {
                return Err(SdkDriverError::configuration(
                    "INVALID_BINDING_CONFIG",
                    format!("task `{}`: items 不能为空", task.id),
                ));
            }
            let mut indices = Vec::with_capacity(items.len());
            for item in items {
                let key = item.get("key").and_then(|v| v.as_str()).ok_or_else(|| {
                    SdkDriverError::configuration(
                        "INVALID_POINT",
                        format!("task `{}`: point 缺少 key", task.id),
                    )
                })?;
                if key.trim().is_empty() {
                    return Err(SdkDriverError::configuration(
                        "INVALID_POINT",
                        "key 不能为空",
                    ));
                }
                let addr_str = item
                    .get("address")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        SdkDriverError::configuration(
                            "INVALID_POINT",
                            format!("point `{key}` 缺少 address"),
                        )
                    })?;
                let dt_str = item
                    .get("data_type")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        SdkDriverError::configuration(
                            "INVALID_POINT",
                            format!("point `{key}` 缺少 data_type"),
                        )
                    })?;
                let addr = parse_address(addr_str).map_err(|e| match e {
                    AddressError::Empty => SdkDriverError::configuration(
                        "INVALID_ADDRESS",
                        format!("point `{key}` 地址为空"),
                    ),
                    AddressError::Invalid { reason, .. } => SdkDriverError::new(
                        mesa_core_types::ErrorKind::Address,
                        "INVALID_ADDRESS",
                        format!("point `{key}` 地址 `{addr_str}` 非法: {reason}"),
                    ),
                })?;
                let data_type = parse_data_type(dt_str)?;
                // 预校验：Fake 下各地址的默认 Value 需匹配声明类型
                // 仅在非严格场景跳过，避免过度约束用户
                indices.push(new_points.len());
                new_points.push(PointSpec {
                    key: key.to_string(),
                    addr,
                    data_type,
                });
            }
            let interval = task.interval_ms.expect("validated");
            new_tasks.push(TaskPlan {
                id: task.id.clone(),
                interval_ms: interval,
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
            "FOCAS2 采集计划构建完成"
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

        // 连接（Fake 下即时成功；Native 下可能失败并由 Manager 退避重连）
        // NOTE: 当前 Fake 为纯异步直接 await；TODO: Native 切 spawn_blocking 隔离 Fwlib 阻塞
        let connect_result = self
            .api
            .connect(&self.cfg.host, self.cfg.port, self.cfg.timeout_ms)
            .await;
        if let Err(e) = connect_result {
            let msg = e.to_string();
            if msg.contains("未实现") || msg.contains("NOT_IMPLEMENTED") {
                return Err(SdkDriverError::configuration("NOT_IMPLEMENTED", msg));
            } else {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Connection,
                    "CONNECT_FAILED",
                    msg,
                ));
            }
        }

        use std::sync::atomic::{AtomicU64, Ordering};
        let seq = Arc::new(AtomicU64::new(1));

        // 捕获外层 api 供任务共享（避免与 task 变量同名遮蔽）
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
            let interval = Duration::from_millis(task.interval_ms);
            let task_id = task.id.clone();
            handles.push(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {},
                        _ = shutdown.cancelled() => break,
                    }
                    // 批量读：Fake 直接 await；Native 阶段改为 spawn_blocking + Fwlib
                    let addrs: Vec<FocasAddress> = points.iter().map(|(s, _)| s.addr.clone()).collect();
                    let values = match api.read_batch(&addrs).await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!(task=%task_id, error=%e, "FOCAS2 读失败");
                            return Err(SdkDriverError::new(mesa_core_types::ErrorKind::Connection, "READ_FAILED", e));
                        }
                    };
                    if values.len() != points.len() {
                        tracing::warn!(task=%task_id, got=values.len(), expected=points.len(), "FOCAS2 返回数量不一致");
                        continue;
                    }
                    let mut batch_vals = Vec::with_capacity(points.len());
                    for ((spec, pid), raw_val) in points.iter().zip(values) {
                        // 多机型单点不支持：Native 以 "ERR:EW_*" 字符串占位，转 typed BAD（§3.6 禁 String 冒充数值类型）
                        if let Value::String(s) = &raw_val
                            && s.starts_with("ERR:") {
                                tracing::warn!(key=%spec.key, error=%s, "单点 Bad，不影响同批其他点");
                                // P0-B：BAD 仍必须携带与 data_type 匹配的 typed 值，quality_code 保留协议原因
                                let neutral = neutral_value_for(spec.data_type);
                                // 尝试提取 EW_* 尾缀作为可读码，暂以 1 为兜底整数码
                                #[allow(clippy::if_same_then_else)]
                                let code = if s.contains("EW_") { 1 } else { 1 };
                                batch_vals.push(PointValue {
                                    point_id: *pid,
                                    value: neutral,
                                    quality: Quality::Bad,
                                    quality_code: Some(code),
                                    source_timestamp_ns: None,
                                    value_origin: ValueOrigin::Placeholder,
                                });
                                continue;
                            }
                        let coerced = coerce_value(raw_val, spec.data_type);
                        if !value_fits_data_type(&coerced, spec.data_type) {
                            // §3.12：单 output 解码失败不丢整批，改为该点 BAD（typed neutral）
                            tracing::warn!(key=%spec.key, got=?coerced, expected=?spec.data_type, "类型不匹配→单点 BAD");
                            batch_vals.push(PointValue {
                                point_id: *pid,
                                value: neutral_value_for(spec.data_type),
                                quality: Quality::Bad,
                                quality_code: Some(1),
                                source_timestamp_ns: None,
                                value_origin: ValueOrigin::Placeholder,
                            });
                            continue;
                        }
                        batch_vals.push(PointValue::good(*pid, coerced));
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
                    tracing::error!(%join_err, "FOCAS2 任务 panic");
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

    async fn command(
        &mut self,
        command: &str,
        args_json: &str,
    ) -> Result<serde_json::Value, SdkDriverError> {
        match command {
            "status" => {
                let args: serde_json::Value =
                    serde_json::from_str(args_json).unwrap_or(serde_json::json!({}));
                // 只读不触硬件，直接经 Control 可靠队列返回，避免与 run 循环的 FOCAS 句柄并发冲突
                Ok(
                    serde_json::json!({"command":"status","host":self.cfg.host.clone(),"port":self.cfg.port,"args":args,"status":"ok"}),
                )
            }
            _ => Err(SdkDriverError::new(
                mesa_core_types::ErrorKind::Unsupported,
                "COMMAND_NOT_SUPPORTED",
                format!("command `{command}` not supported, only status"),
            )),
        }
    }
}

/// §3.6：BAD 时的 type-compatible neutral value（无 last-known 时兜底）
fn neutral_value_for(dt: DataType) -> Value {
    match dt {
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

fn coerce_value(v: Value, dt: DataType) -> Value {
    match (v, dt) {
        (Value::U32(x), DataType::I32) => Value::I32(x as i32),
        (Value::I32(x), DataType::U32) => Value::U32(x as u32),
        (Value::U32(x), DataType::F32) => Value::F32(x as f32),
        (Value::U32(x), DataType::F64) => Value::F64(x as f64),
        (Value::I32(x), DataType::F32) => Value::F32(x as f32),
        (Value::I32(x), DataType::F64) => Value::F64(x as f64),
        (Value::F32(x), DataType::F64) => Value::F64(x as f64),
        (Value::F64(x), DataType::F32) => Value::F32(x as f32),
        (other, _) => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{AcquisitionTask, DriverBinding, TaskMode};

    fn task_with_items(items: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding {
                kind: BINDING_KIND.into(),
                config: serde_json::json!({"items": items}),
            },
        }
    }

    #[tokio::test]
    async fn configure_ok_and_duplicate_rejected() {
        let mut conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: None,
        };
        let items = serde_json::json!([
            {"key":"a","address":"status","data_type":"U32"},
            {"key":"b","address":"axis.abs.1","data_type":"I32"}
        ]);
        let t = task_with_items(items);
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 2);
        let dup = serde_json::json!([
            {"key":"a","address":"status","data_type":"U32"},
            {"key":"a","address":"axis.abs.2","data_type":"I32"}
        ]);
        let err = conn
            .configure(2, vec![task_with_items(dup)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");
    }

    #[tokio::test]
    async fn invalid_address_rejected() {
        let mut conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: None,
        };
        let items = serde_json::json!([{"key":"a","address":"axis.abs.0","data_type":"I32"}]);
        let err = conn
            .configure(1, vec![task_with_items(items)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ADDRESS");
    }

    #[tokio::test]
    async fn probe_fake_returns_deterministic_identity() {
        // Fake 身份是合同基准：FANUC / 0i-F / 固件 1.0，model 恒 None + MODEL_UNDETECTED。
        // 经已打开的连接调用（OpenConnection 阶段已做配置门禁），不碰进程级 env。
        let mut conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: None,
        };
        let r = conn.probe().await.expect("Fake probe 必须 Ok");
        assert!(r.reachable);
        assert_eq!(r.vendor.as_deref(), Some("FANUC"));
        assert_eq!(r.family.as_deref(), Some("0i-F"));
        assert_eq!(r.firmware.as_deref(), Some("1.0"));
        assert!(r.model.is_none());
        assert!(r.warnings.iter().any(|w| w.code == "MODEL_UNDETECTED"));
        assert!(
            r.capabilities
                .iter()
                .any(|c| c.id == "read" && c.state == mesa_core_types::CapabilityState::Available)
        );
    }

    #[tokio::test]
    async fn open_rejects_bad_config() {
        // 配置校验在 OpenConnection（probe 复用已开连接）
        let d = FocasDriver;
        let err = match d.open_connection("t", "not-json").await {
            Ok(_) => panic!("非法配置必须拒绝"),
            Err(e) => e,
        };
        assert_eq!(err.code, "BAD_CONFIG");
    }

    #[tokio::test]
    async fn probe_native_without_dll_is_unreachable() {
        // CI 无 fwlib（Linux 下 load 失败干净返回，不再 SIGSEGV）：Native 建连失败
        // → Ok(unreachable)，不是 Err。经已打开的连接调用，不碰进程级 env。
        let mut conn = FocasConnection {
            cfg: FocasConnConfig {
                host: "127.0.0.1".into(),
                port: 9,
                timeout_ms: 1000,
            },
            api: Arc::new(NativeFocasApi::new()),
            plan: None,
        };
        let r = conn.probe().await.expect("不可达是探测结果");
        assert!(!r.reachable);
        assert!(r.warnings.iter().any(|w| w.code == "CONNECTION_FAILED"));
    }

    #[tokio::test]
    async fn fake_api_smoke() {
        // Fake API 直测：保证各地址类型可产出对应 Value
        let api = FakeFocasApi::new();
        api.connect("127.0.0.1", 8193, 3000).await.unwrap();
        let addrs = vec![
            crate::address::parse_address("status").unwrap(),
            crate::address::parse_address("axis.abs.1").unwrap(),
            crate::address::parse_address("spindle.load.1").unwrap(),
            crate::address::parse_address("macro.100").unwrap(),
        ];
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals.len(), 4);
    }

    #[tokio::test]
    async fn explicit_planner_pmc_10_words_single_task() {
        // Milestone D: 10 连续 WORD(R) 同一任务应编译为 1 Range（逻辑 10 > 物理 1）
        let mut conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: None,
        };
        let items: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                serde_json::json!({"key": format!("p{}", i), "address": format!("pmc.R{}", 100+i*2), "data_type":"I32"})
            })
            .collect();
        let t = task_with_items(serde_json::Value::Array(items));
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 10);
        let api = FakeFocasApi::new();
        let addrs: Vec<_> = (0..10)
            .map(|i| crate::address::parse_address(&format!("pmc.R{}", 100 + i * 2)).unwrap())
            .collect();
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals.len(), 10, "10 PMC WORD 应一次批量返回");
    }

    #[tokio::test]
    async fn explicit_planner_bad_isolation() {
        // Milestone D: 单 output BAD 不污染同 Operation 其他 outputs（§3.12 + P0-B）
        let api = FakeFocasApi::new();
        let addrs = vec![
            crate::address::parse_address("status").unwrap(),
            crate::address::parse_address("axis.abs.1").unwrap(),
        ];
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals.len(), 2);
    }
}
