//! Mesa Simulator Driver（方案附录 A）。
//!
//! 定位：Driver Framework 的参考实现与 Contract/Performance Test 基线，
//! 不属于正式设备协议范围。行为配置属于测试配置，不进入生产 DeviceProfile。
//!
//! 已实现数据源（附录 A.1 子集）：Constant / Counter / Sine / Toggle / Random。
//! binding 形如：
//!
//! ```json
//! { "points": [
//!     { "key": "sim.counter", "kind": "counter", "start": 0, "step": 1 },
//!     { "key": "sim.sine",    "kind": "sine", "amplitude": 100, "period_ms": 5000, "offset": 50 },
//!     { "key": "sim.toggle",  "kind": "toggle", "initial": true },
//!     { "key": "sim.const",   "kind": "constant", "value": 42 },
//!     { "key": "sim.rand",    "kind": "random", "min": -10, "max": 10, "seed": 7 }
//! ] }
//! ```
//!
//! 质量与故障注入（附录 A.3/A.4，Contract Test 用）：
//!
//! - 点级 `"quality": "BAD"|"UNCERTAIN"` 静态质量覆盖；
//! - 点级 `"bad_after_batches": N` / `"good_again_after": M` 实现 GOOD→BAD→GOOD 转换；
//! - connection 配置 `"faults": {"fail_after_batches": N}` 在第 N 批后连接报
//!   SIMULATED_DISCONNECT（验证 Core 重连语义）；
//! - `"faults": {"crash_after_batches": N}` 直接退出进程（仅子进程模式有意义，
//!   进程内使用会终止测试进程），验证 Driver Crash Restore。
//!
//! TODO: 附录 A 其余能力（delay/jitter/burst/silent_interval）待性能预算阶段（§22）
//! 按压测需要补齐。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mesa_core_types::{
    AcquisitionTask, ConditionTransition, DataBatch, DataType, DriverMetadata, DuplicatePointKey,
    ErrorKind, EventCondition, EventRecord, EventTask, GENERIC_BINDING_KIND, GenericBinding,
    PointDescriptor, PointMap, PointValue, Quality, TaskMode, Value, ensure_unique_point_keys,
    now_unix_ns,
};
use mesa_driver_sdk::{DataSink, Driver, DriverConnection, EventSink, SdkDriverError};
use tokio_util::sync::CancellationToken;

pub const BINDING_KIND: &str = "simulator.points";
/// 事件任务 binding kind：config 形如 `{ "stream": "sim.events.counter" }`，
/// stream 取值见 [`SIM_EVENT_STREAM_COUNTER`] / [`SIM_EVENT_STREAM_ALARM`]。
/// 语义完全由本驱动解释（Core 不懂协议）。
pub const EVENT_BINDING_KIND: &str = "simulator.events";
/// 计数器事件流：周期性瞬时事件（`counter.tick`），无 condition。
pub const SIM_EVENT_STREAM_COUNTER: &str = "sim.events.counter";
/// 报警周期流：每次 run 走一遍 Raised → Updated → Acknowledged → Cleared，
/// 四条独立 occurrence 共享同一 condition_id（`SIM-ALARM-100`）。
pub const SIM_EVENT_STREAM_ALARM: &str = "sim.events.alarm-cycle";
/// 报警流的 condition 身份（四条记录共享）。
pub const SIM_ALARM_CONDITION_ID: &str = "SIM-ALARM-100";

/// Simulator 驱动实例。无连接级共享状态——每个连接独立持有采集计划。
#[derive(Default)]
pub struct SimulatorDriver;

#[async_trait::async_trait]
impl Driver for SimulatorDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: "simulator".into(),
            name: "Mesa Simulator".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            // 与 drivers/simulator/driver.toml 保持一致
            protocol_major: 1,
            protocol_minor: 0,
        }
    }

    fn descriptor(&self) -> mesa_core_types::DriverDescriptor {
        use mesa_core_types::{
            AccessMode, DataType, DiscoveryCapabilities, DriverCapabilities, DriverDescriptor,
            DriverIdentity, EventCatalog, EventFieldDescriptor, EventStreamDescriptor,
            FieldDescriptor, FieldType, LocalizedText, OutputDescriptor, ResourceDescriptor,
            SchemaDescriptor, TaskMode,
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
                    FieldDescriptor::new("seed", "Seed", FieldType::Integer)
                        .required(false)
                        .default_value(serde_json::json!(0)),
                ],
            },
            resources: vec![
                ResourceDescriptor {
                    id: "counter".into(),
                    label: LocalizedText::new("Counter"),
                    parameters: SchemaDescriptor::default(),
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
                    id: "sine".into(),
                    label: LocalizedText::new("Sine"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            FieldDescriptor::new("amplitude", "Amplitude", FieldType::Number)
                                .required(false)
                                .default_value(serde_json::json!(100.0)),
                            FieldDescriptor::new("period_ms", "Period ms", FieldType::Integer)
                                .required(false)
                                .default_value(serde_json::json!(5000)),
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
                    id: "random".into(),
                    label: LocalizedText::new("Random"),
                    parameters: SchemaDescriptor::default(),
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
                    id: "constant".into(),
                    label: LocalizedText::new("Constant"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            FieldDescriptor::new("value", "Value", FieldType::Number)
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
            ],
            controls: mesa_core_types::ControlCatalog::default(),
            discovery: DiscoveryCapabilities {
                manual: true,
                browse: false,
                import: false,
            },
            capabilities: DriverCapabilities {
                poll: true,
                events: true,
                ..Default::default()
            },
            // Event Plane V1 §5：Simulator 是第一个 Reference Event Driver，
            // 声明 counter（瞬时）+ alarm-cycle（Condition 四态）两个事件流
            events: EventCatalog {
                streams: vec![
                    EventStreamDescriptor {
                        id: SIM_EVENT_STREAM_COUNTER.into(),
                        label: LocalizedText::new("Counter Events"),
                        modes: vec![TaskMode::Poll, TaskMode::Subscribe],
                        parameters: SchemaDescriptor::default(),
                        fields: vec![EventFieldDescriptor {
                            key: "value".into(),
                            label: LocalizedText::new("Counter value"),
                            data_type: Some("u64".into()),
                        }],
                    },
                    EventStreamDescriptor {
                        id: SIM_EVENT_STREAM_ALARM.into(),
                        label: LocalizedText::new("Alarm Cycle"),
                        modes: vec![TaskMode::Poll, TaskMode::Subscribe],
                        parameters: SchemaDescriptor::default(),
                        fields: vec![EventFieldDescriptor {
                            key: "code".into(),
                            label: LocalizedText::new("Alarm code"),
                            data_type: Some("string".into()),
                        }],
                    },
                ],
            },
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        // connection 配置仅含可选 faults 注入项；整体必须为合法 JSON
        let cfg: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| SdkDriverError::configuration("BAD_CONFIG", e.to_string()))?;
        let faults = parse_conn_faults(&cfg)?;
        Ok(Box::new(SimConnection {
            plan: None,
            faults,
            event_plan: None,
        }))
    }
}

// ---------------------------------------------------------------------------
// 点位规格与数据源
// ---------------------------------------------------------------------------

/// 单个模拟点的静态规格（configure 阶段从 binding 解析）。
#[derive(Debug, Clone, PartialEq)]
struct PointSpec {
    key: String,
    source: SourceSpec,
    quality: QualitySpec,
}

/// 质量注入规格（附录 A.4）：静态覆盖 + 按批次数的 GOOD→BAD→GOOD 转换。
#[derive(Debug, Clone, PartialEq)]
struct QualitySpec {
    /// 缺省质量；未配置任何转换时恒定输出。
    base: Quality,
    /// 第 N 批（1 起）起转为 BAD。
    bad_after_batches: Option<u64>,
    /// 第 M 批起恢复 GOOD（须大于 bad_after_batches 才有转换窗口）。
    good_again_after: Option<u64>,
}

impl Default for QualitySpec {
    fn default() -> Self {
        Self {
            base: Quality::Good,
            bad_after_batches: None,
            good_again_after: None,
        }
    }
}

impl QualitySpec {
    fn from_json(v: &serde_json::Value) -> Result<Self, SdkDriverError> {
        let mut spec = Self::default();
        if let Some(q) = v.get("quality").and_then(|q| q.as_str()) {
            spec.base = match q.to_ascii_uppercase().as_str() {
                "GOOD" => Quality::Good,
                "UNCERTAIN" => Quality::Uncertain,
                "BAD" => Quality::Bad,
                other => {
                    return Err(bad_point(
                        "<quality>",
                        &format!("unknown quality `{other}`"),
                    ));
                }
            };
        }
        spec.bad_after_batches = parse_batch_threshold(v, "bad_after_batches")?;
        spec.good_again_after = parse_batch_threshold(v, "good_again_after")?;
        Ok(spec)
    }

    /// 计算第 `batch_no` 批（1 起）应输出的质量。
    fn at(&self, batch_no: u64) -> Quality {
        if let Some(bad_from) = self.bad_after_batches
            && batch_no >= bad_from
        {
            return match self.good_again_after {
                Some(good_from) if batch_no >= good_from => Quality::Good,
                _ => Quality::Bad,
            };
        }
        self.base
    }
}

fn parse_batch_threshold(
    v: &serde_json::Value,
    field: &str,
) -> Result<Option<u64>, SdkDriverError> {
    match v.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(x) => x.as_u64().map(Some).ok_or_else(|| {
            bad_point(
                field,
                &format!("`{field}` must be a positive integer batch count"),
            )
        }),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum SourceSpec {
    Counter {
        start: f64,
        step: f64,
        wrap: Option<f64>,
    },
    Sine {
        amplitude: f64,
        period_ms: u64,
        offset: f64,
    },
    Toggle {
        initial: bool,
    },
    Constant {
        value: Value,
    },
    Random {
        min: f64,
        max: f64,
        seed: Option<u64>,
    },
}

impl SourceSpec {
    fn parse(key: &str, v: &serde_json::Value) -> Result<PointSpec, SdkDriverError> {
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .ok_or_else(|| bad_point(key, "missing `kind`"))?;
        let get_f = |name: &str| v.get(name).and_then(|x| x.as_f64());
        let source = match kind {
            "counter" => SourceSpec::Counter {
                start: get_f("start").unwrap_or(0.0),
                step: get_f("step").unwrap_or(1.0),
                wrap: get_f("wrap"),
            },
            "sine" => SourceSpec::Sine {
                amplitude: get_f("amplitude").unwrap_or(100.0),
                period_ms: (get_f("period_ms").unwrap_or(5000.0)).max(1.0) as u64,
                offset: get_f("offset").unwrap_or(0.0),
            },
            "toggle" => SourceSpec::Toggle {
                initial: v.get("initial").and_then(|b| b.as_bool()).unwrap_or(false),
            },
            "constant" => {
                let raw = v.get("value").cloned().unwrap_or(serde_json::json!(0));
                let value = if let Some(f) = raw.as_f64() {
                    Value::F64(f)
                } else if let Some(s) = raw.as_str() {
                    Value::String(s.to_string())
                } else {
                    return Err(bad_point(key, "`constant.value` must be number or string"));
                };
                SourceSpec::Constant { value }
            }
            "random" => SourceSpec::Random {
                min: get_f("min").unwrap_or(-100.0),
                max: get_f("max").unwrap_or(100.0),
                seed: v.get("seed").and_then(|s| s.as_u64()),
            },
            other => {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_SOURCE_KIND",
                    format!("point `{key}`: unknown kind `{other}`"),
                ));
            }
        };
        let quality = QualitySpec::from_json(v)?;
        Ok(PointSpec {
            key: key.to_string(),
            source,
            quality,
        })
    }

    fn data_type(&self) -> DataType {
        match self {
            SourceSpec::Toggle { .. } => DataType::Bool,
            SourceSpec::Constant { value } => value.data_type(),
            _ => DataType::F64,
        }
    }
}

fn bad_point(key: &str, why: &str) -> SdkDriverError {
    SdkDriverError::configuration("INVALID_POINT_SPEC", format!("point `{key}`: {why}"))
}

/// 连接级故障注入（附录 A.3）。从 connection 配置的 `faults` 对象解析。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ConnFaults {
    /// 第 N 批发布后连接以 ConnectionError/SIMULATED_DISCONNECT 结束，
    /// 驱动进程存活——用于验证 Core 侧重连与断线数据语义。
    fail_after_batches: Option<u64>,
    /// 第 N 批发布后进程直接退出（exit code 101）。仅子进程模式可观测，
    /// 用于端到端验证 Driver Crash Restore。
    crash_after_batches: Option<u64>,
}

fn parse_conn_faults(cfg: &serde_json::Value) -> Result<ConnFaults, SdkDriverError> {
    let mut f = ConnFaults::default();
    let Some(faults) = cfg.get("faults") else {
        return Ok(f);
    };
    if !faults.is_object() {
        return Err(SdkDriverError::configuration(
            "BAD_CONFIG",
            "`faults` must be an object".to_string(),
        ));
    }
    f.fail_after_batches = parse_batch_threshold(faults, "fail_after_batches")
        .map_err(|e| SdkDriverError::configuration("BAD_CONFIG", e.message))?;
    f.crash_after_batches = parse_batch_threshold(faults, "crash_after_batches")
        .map_err(|e| SdkDriverError::configuration("BAD_CONFIG", e.message))?;
    Ok(f)
}

/// 运行期可变状态。与点位一一对应，由各任务循环独占持有，无跨任务共享。
#[derive(Debug)]
struct SourceState {
    counter: f64,
    toggle: bool,
    lcg: u64,
}

impl SourceState {
    fn new(spec: &SourceSpec, index: u64) -> Self {
        // LCG 种子：显式 seed 优先；否则用时间+下标混合，保证多点位不相关
        let time_seed = now_unix_ns() as u64;
        let lcg = match spec {
            SourceSpec::Random { seed: Some(s), .. } => *s,
            _ => time_seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        };
        Self {
            counter: match spec {
                SourceSpec::Counter { start, .. } => *start,
                _ => 0.0,
            },
            toggle: matches!(spec, SourceSpec::Toggle { initial: true }),
            lcg,
        }
    }

    /// 计算第 `t_ms` 毫秒时刻的值。
    fn sample(&mut self, spec: &SourceSpec, t_ms: u64) -> Value {
        match spec {
            SourceSpec::Counter { step, wrap, .. } => {
                self.counter += *step;
                if let Some(w) = wrap
                    && self.counter > *w
                {
                    // 简单回绕到 0：满足附录 A "Counter 可配置回绕"
                    self.counter = 0.0;
                }
                Value::F64(self.counter)
            }
            SourceSpec::Sine {
                amplitude,
                period_ms,
                offset,
            } => {
                let phase = (t_ms % period_ms) as f64 / *period_ms as f64;
                Value::F64(offset + amplitude * (std::f64::consts::TAU * phase).sin())
            }
            SourceSpec::Toggle { .. } => {
                self.toggle = !self.toggle;
                Value::Bool(self.toggle)
            }
            SourceSpec::Constant { value } => value.clone(),
            SourceSpec::Random { min, max, .. } => {
                // xorshift64* 生成 [0,1)，映射到 [min,max)
                let mut x = self.lcg;
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                self.lcg = x;
                let unit =
                    (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64;
                Value::F64(min + unit * (max - min))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 连接生命周期
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TaskPlan {
    // TODO: PlanSnapshot 冻结字段，task id 用于诊断与任务级追踪，V1 仅内存使用未序列化但需保留
    #[allow(dead_code)]
    id: String,
    interval_ms: u64,
    /// 指向快照内 points 数组的下标集合。
    point_indices: Vec<usize>,
    /// 每次 tick 连续发布的批次数（附录 A.2 burst）。Windows 定时器精度约
    /// 15.6ms，小间隔 tick 不可靠；压测/背压场景以 burst 保证产出速率。
    burst: u64,
}

/// configure 成功后的完整采集计划快照（§6.2 全量替换的原子单元）。
#[derive(Debug)]
struct PlanSnapshot {
    // TODO: PlanSnapshot 冻结字段，revision 为 §6.2 全量快照版本号，当前仅内存校验未持久化到 Driver 但需保留以备回放校验
    #[allow(dead_code)]
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    /// ApplyPointMap 后写入；run 前必须存在。
    map: Option<PointMap>,
}

#[derive(Debug, Default)]
struct SimConnection {
    plan: Option<PlanSnapshot>,
    faults: ConnFaults,
    /// 事件订阅计划（Event Plane V1 §6 全量快照，原子替换，与数据计划独立）。
    /// None/空 = 不发射任何事件；run() 只在有任务时起事件循环。
    event_plan: Option<EventPlanSnapshot>,
}

/// configure_events 成功后的事件订阅快照（revision 对应 ConfigureEventTasks.revision）。
#[derive(Debug)]
struct EventPlanSnapshot {
    #[allow(dead_code)]
    revision: u64,
    tasks: Vec<SimEventTask>,
}

/// 单个事件任务的驱动内解释形态：stream 决定发射器，interval 决定周期。
#[derive(Debug)]
struct SimEventTask {
    #[allow(dead_code)]
    id: String,
    stream: SimEventStream,
    interval_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimEventStream {
    Counter,
    AlarmCycle,
}

impl SimEventStream {
    fn parse(stream: &str) -> Option<Self> {
        match stream {
            SIM_EVENT_STREAM_COUNTER => Some(Self::Counter),
            SIM_EVENT_STREAM_ALARM => Some(Self::AlarmCycle),
            _ => None,
        }
    }
}

/// run() 调用计数（进程级单调）：alarm-cycle 每次 run 走一遍四态，
/// event_id 必须跨 run 唯一（Endpoint 内去重键），用 run 序号区分。
static SIM_EVENT_RUN_SEQ: AtomicU64 = AtomicU64::new(0);

#[async_trait::async_trait]
impl DriverConnection for SimConnection {
    /// Simulator 探测：无真实设备，返回确定性事实（合同基准）。
    /// 复用本连接（OpenConnection 已校验配置），不启动任何采集。
    async fn probe(&mut self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        use mesa_core_types::{CapabilityItem, CapabilityState};
        Ok(mesa_core_types::ProbeReport {
            reachable: true,
            vendor: Some("Mesa".into()),
            family: Some("Simulator".into()),
            model: Some("Basic".into()),
            firmware: Some("1.0".into()),
            model_confidence: Some("high".into()),
            capabilities: vec![
                CapabilityItem {
                    id: "read".into(),
                    state: CapabilityState::Available,
                    detail: None,
                },
                CapabilityItem {
                    id: "subscribe".into(),
                    state: CapabilityState::NotPresent,
                    detail: Some("simulator only supports poll mode".into()),
                },
                CapabilityItem {
                    id: "browse".into(),
                    state: CapabilityState::NotPresent,
                    detail: Some("simulator has no browse space".into()),
                },
            ],
            warnings: vec![],
        })
    }

    async fn configure(
        &mut self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        // 全量快照替换：先完整构建新计划，成功后才整体替换（§6.2 原子切换）；
        // 任一步失败即返回错误且旧计划保持不变
        let mut new_points: Vec<PointSpec> = Vec::new();
        let mut new_tasks: Vec<TaskPlan> = Vec::new();

        for task in &tasks {
            task.validate()
                .map_err(|e| SdkDriverError::configuration("INVALID_TASK", e.to_string()))?;
            // Simulator 仅支持 Poll；Subscribe 属于 OPC UA 能力（§5.5）
            if task.mode != TaskMode::Poll {
                return Err(SdkDriverError::new(
                    ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: simulator only supports poll mode", task.id),
                ));
            }
            if task.binding.kind == GENERIC_BINDING_KIND {
                // 通用 ResourceSelection 路径（§15）：selections -> points
                let binding: GenericBinding = serde_json::from_value(task.binding.config.clone())
                    .map_err(|e| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: invalid generic binding: {e}", task.id),
                    )
                })?;
                // 结构级校验（point_key 唯一等）
                mesa_core_types::validate_selections_structure(&binding.selections)
                    .map_err(|e| SdkDriverError::configuration("INVALID_BINDING_CONFIG", e))?;
                let mut indices = Vec::new();
                for sel in &binding.selections {
                    // Simulator 资源映射：resource_id 即 SourceKind，parameters 即源参数
                    for out in &sel.outputs {
                        // 构造与 Legacy 兼容的 SourceSpec 解析输入：合并 kind 与 parameters
                        let mut src_json = sel.parameters.clone();
                        if src_json.is_null() {
                            src_json = serde_json::json!({});
                        }
                        if let Some(obj) = src_json.as_object_mut() {
                            obj.insert("kind".into(), serde_json::json!(sel.resource_id));
                        } else {
                            src_json = serde_json::json!({"kind": sel.resource_id});
                        }
                        indices.push(new_points.len());
                        new_points.push(SourceSpec::parse(&out.point_key, &src_json)?);
                    }
                }
                // 泛型路径的 indices 已收集，此处直接创建任务计划
                new_tasks.push(TaskPlan {
                    id: task.id.clone(),
                    interval_ms: task.interval_ms.expect("validated above"),
                    point_indices: indices,
                    burst: task
                        .binding
                        .config
                        .get("burst")
                        .and_then(|b| b.as_u64())
                        .unwrap_or(1)
                        .max(1),
                });
                continue;
            }
            if task.binding.kind != BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: binding kind `{}` unsupported, expected `{BINDING_KIND}` or `{GENERIC_BINDING_KIND}`",
                        task.id, task.binding.kind
                    ),
                ));
            }
            let cfg_points = task
                .binding
                .config
                .get("points")
                .and_then(|p| p.as_array())
                .ok_or_else(|| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: missing array `points`", task.id),
                    )
                })?;
            let mut indices = Vec::with_capacity(cfg_points.len());
            for p in cfg_points {
                let key = p
                    .get("key")
                    .and_then(|k| k.as_str())
                    .ok_or_else(|| bad_point("<unnamed>", "missing `key`"))?;
                indices.push(new_points.len());
                new_points.push(SourceSpec::parse(key, p)?);
            }
            new_tasks.push(TaskPlan {
                id: task.id.clone(),
                interval_ms: task.interval_ms.expect("validated above"),
                point_indices: indices,
                burst: task
                    .binding
                    .config
                    .get("burst")
                    .and_then(|b| b.as_u64())
                    .unwrap_or(1)
                    .max(1),
            });
        }

        let descriptors: Vec<PointDescriptor> = new_points
            .iter()
            .map(|p| PointDescriptor {
                point_key: p.key.clone(),
                data_type: p.source.data_type(),
                unit: None,
            })
            .collect();
        ensure_unique_point_keys(&descriptors).map_err(|DuplicatePointKey(k)| {
            SdkDriverError::configuration(
                "DUPLICATE_POINT_KEY",
                format!("`{k}` appears more than once in the snapshot"),
            )
        })?;

        tracing::info!(
            revision,
            points = new_points.len(),
            tasks = new_tasks.len(),
            "plan built"
        );
        self.plan = Some(PlanSnapshot {
            revision,
            points: new_points,
            tasks: new_tasks,
            map: None,
        });
        Ok(descriptors)
    }

    /// 事件任务配置（Event Plane V1 §6）：空快照清空订阅；非空快照逐条校验
    /// （任务结构 + binding kind + stream 存在性），任一非法即整个 revision
    /// 失败且旧订阅保持不变（§35 原子切换）。
    async fn configure_events(
        &mut self,
        revision: u64,
        tasks: Vec<EventTask>,
    ) -> Result<(), SdkDriverError> {
        use mesa_core_types::EventTaskError;
        let mut new_tasks = Vec::with_capacity(tasks.len());
        for task in &tasks {
            task.validate().map_err(|e| match e {
                EventTaskError::EmptyTaskId => SdkDriverError::configuration(
                    "INVALID_EVENT_TASK",
                    format!("event task id 不能为空 (revision {revision})"),
                ),
                EventTaskError::PollRequiresInterval { task } => SdkDriverError::configuration(
                    "INVALID_EVENT_TASK",
                    format!("event task `{task}`: Poll 模式必须提供正整数 interval_ms"),
                ),
            })?;
            if task.binding.kind != EVENT_BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_EVENT_BINDING",
                    format!(
                        "event task `{}`: binding kind `{}` unsupported, expected `{EVENT_BINDING_KIND}`",
                        task.id, task.binding.kind
                    ),
                ));
            }
            let stream_name = task
                .binding
                .config
                .get("stream")
                .and_then(|s| s.as_str())
                .ok_or_else(|| {
                    SdkDriverError::configuration(
                        "INVALID_EVENT_BINDING_CONFIG",
                        format!(
                            "event task `{}`: missing string `stream` (expected `{SIM_EVENT_STREAM_COUNTER}` or `{SIM_EVENT_STREAM_ALARM}`)",
                            task.id
                        ),
                    )
                })?;
            let stream = SimEventStream::parse(stream_name).ok_or_else(|| {
                SdkDriverError::configuration(
                    "UNKNOWN_EVENT_STREAM",
                    format!("event task `{}`: unknown stream `{stream_name}`", task.id),
                )
            })?;
            // Poll 用任务自带周期；Subscribe 用驱动默认 100ms 节奏
            let interval_ms = match task.mode {
                TaskMode::Poll => task.interval_ms.expect("validated above"),
                TaskMode::Subscribe => task.interval_ms.unwrap_or(100).max(1),
            };
            new_tasks.push(SimEventTask {
                id: task.id.clone(),
                stream,
                interval_ms,
            });
        }
        tracing::info!(revision, tasks = new_tasks.len(), "event plan built");
        self.event_plan = Some(EventPlanSnapshot {
            revision,
            tasks: new_tasks,
        });
        Ok(())
    }

    async fn apply_point_map(&mut self, map: PointMap) -> Result<(), SdkDriverError> {
        let snapshot = self.plan.as_mut().ok_or_else(|| {
            SdkDriverError::new(
                ErrorKind::Internal,
                "NOT_CONFIGURED",
                "apply before configure",
            )
        })?;
        // 映射必须覆盖全部已注册点，否则该点将永远无 ID 可发
        for p in &snapshot.points {
            if !map.contains_key(&p.key) {
                return Err(SdkDriverError::configuration(
                    "MISSING_POINT_ID",
                    format!("point `{}` has no id in applied map", p.key),
                ));
            }
        }
        snapshot.map = Some(map);
        Ok(())
    }

    async fn run(
        &mut self,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError> {
        let snapshot = self.plan.as_ref().ok_or_else(|| {
            SdkDriverError::new(ErrorKind::Internal, "NO_PLAN", "run before configure+apply")
        })?;
        let map = snapshot.map.as_ref().ok_or_else(|| {
            SdkDriverError::new(
                ErrorKind::Internal,
                "NO_POINT_MAP",
                "run before apply_point_map",
            )
        })?;

        /// 组装某任务循环的 owned 运行单元（id + 规格 + 状态），不借用连接对象。
        struct RuntimePoint {
            point_id: u32,
            spec: PointSpec,
            state: SourceState,
        }
        let build_runtime = |indices: &[usize]| -> Vec<RuntimePoint> {
            indices
                .iter()
                .enumerate()
                .map(|(local, &idx)| {
                    let spec = snapshot.points[idx].clone();
                    let point_id = map[&spec.key];
                    RuntimePoint {
                        point_id,
                        state: SourceState::new(&spec.source, local as u64),
                        spec,
                    }
                })
                .collect()
        };

        // sequence 按 (connection, epoch) 从 1 递增（§10）；原子计数保证多任务唯一
        let seq = Arc::new(AtomicU64::new(1));
        // 连接级已发布批次数：故障注入（fail/crash_after_batches）的触发依据
        let published = Arc::new(AtomicU64::new(0));
        let started = std::time::Instant::now();
        // 本次 run 的事件序号：alarm event_id 跨 run 唯一（Endpoint 内去重键）
        let event_run_seq = SIM_EVENT_RUN_SEQ.fetch_add(1, Ordering::Relaxed);

        let mut handles = Vec::with_capacity(snapshot.tasks.len());
        for plan in &snapshot.tasks {
            let mut runtime = build_runtime(&plan.point_indices);
            let sink = sink.clone();
            let shutdown = shutdown.clone();
            let seq = Arc::clone(&seq);
            let published = Arc::clone(&published);
            let faults = self.faults;
            let interval = Duration::from_millis(plan.interval_ms);
            let plan_burst = plan.burst;
            handles.push(tokio::spawn(async move {
                // 本任务循环自身的批次序号：质量转换按"该任务第几批"计。
                // 闭包返回 Result 以承载 SIMULATED_DISCONNECT 故障路径。
                let run: Result<(), SdkDriverError> = async {
                    let mut batch_no: u64 = 0;
                    let mut ticker = tokio::time::interval(interval);
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tokio::select! {
                            _ = ticker.tick() => {}
                            _ = shutdown.cancelled() => return Ok(()),
                        }
                        let t_ms = started.elapsed().as_millis() as u64;
                        // burst：单次 tick 连续发布多批，绕开 Windows 定时器精度限制
                        for _ in 0..plan_burst {
                            batch_no += 1;
                            let values: Vec<PointValue> = runtime
                                .iter_mut()
                                .map(|rp| {
                                    let mut pv = PointValue::good(
                                        rp.point_id,
                                        rp.state.sample(&rp.spec.source, t_ms),
                                    );
                                    pv.quality = rp.spec.quality.at(batch_no);
                                    pv
                                })
                                .collect();
                            // NOTE: publish 内部做 Latest-Wins 合并，sequence 可能产生缺口（§12）
                            sink.publish(DataBatch {
                                // handle/epoch 由 SDK 盖戳，驱动侧无需感知分配细节
                                connection_handle: 0,
                                stream_epoch: 0,
                                sequence: seq.fetch_add(1, Ordering::Relaxed),
                                timestamp_ns: now_unix_ns(),
                                values,
                                        mono_ns: None,
        })
                            .await;

                            if !faults_eq_default(faults) {
                                let n = published.fetch_add(1, Ordering::Relaxed) + 1;
                                if faults.crash_after_batches == Some(n) {
                                    eprintln!(
                                        "simulator: fault injection crash_after_batches={n}, exiting"
                                    );
                                    std::process::exit(101);
                                }
                                if faults.fail_after_batches == Some(n) {
                                    return Err(SdkDriverError::new(
                                        ErrorKind::Connection,
                                        "SIMULATED_DISCONNECT",
                                        format!("fault injection: disconnected after {n} batches"),
                                    ));
                                }
                            }
                        }
                    }
                }
                .await;
                run
            }));
        }
        // 事件循环：仅当配置了非空事件计划才启动（无订阅不发射，
        // 避免无消费者的批次占用 Event Queue）。
        if let Some(eplan) = &self.event_plan {
            for task in &eplan.tasks {
                let events = sink.events();
                let shutdown = shutdown.clone();
                let stream = task.stream;
                let interval = Duration::from_millis(task.interval_ms);
                handles.push(tokio::spawn(async move {
                    run_sim_event_task(events, shutdown, stream, event_run_seq, interval).await;
                    Ok::<(), SdkDriverError>(())
                }));
            }
        }
        for h in handles {
            let _ = h.await;
        }
        Ok(())
    }

    async fn write(
        &mut self,
        target: &str,
        _value: Value,
        _expected: Option<Value>,
    ) -> Result<(), SdkDriverError> {
        // Simulator 控制：若已配置则校验 target 合法性，否则直接成功（用于 probe 前的轻量校验）
        if let Some(plan) = &self.plan
            && !plan.points.iter().any(|p| p.key == target)
        {
            return Err(SdkDriverError::new(
                ErrorKind::Internal,
                "TARGET_NOT_FOUND",
                format!("target `{target}` not found"),
            ));
        }
        Ok(())
    }

    async fn command(
        &mut self,
        command: &str,
        args_json: &str,
    ) -> Result<serde_json::Value, SdkDriverError> {
        match command {
            "reset" | "start" | "stop" | "fault" | "writable" => {
                let args: serde_json::Value =
                    serde_json::from_str(args_json).unwrap_or(serde_json::json!({}));
                Ok(serde_json::json!({"command": command, "args": args, "status": "ok"}))
            }
            _ => Err(SdkDriverError::new(
                ErrorKind::Unsupported,
                "COMMAND_NOT_SUPPORTED",
                format!("command `{command}` not supported"),
            )),
        }
    }
}

/// 仅在有注入项时才走计数路径，避免正常采集承担原子开销。
fn faults_eq_default(f: ConnFaults) -> bool {
    f == ConnFaults::default()
}

// ---------------------------------------------------------------------------
// 事件发射器（Event Plane V1 Reference 实现，PR6）
// ---------------------------------------------------------------------------

/// 单个事件任务的发射循环（与数据采集循环独立，共享 shutdown）。
/// Counter：按 interval 周期发射瞬时事件；AlarmCycle：每次 run 走一遍
/// Raised → Updated → Acknowledged → Cleared 四态后结束（周期性重复由
/// Stop → Start 驱动，保证测试确定性）。
async fn run_sim_event_task(
    events: EventSink,
    shutdown: CancellationToken,
    stream: SimEventStream,
    run_seq: u64,
    interval: Duration,
) {
    match stream {
        SimEventStream::Counter => {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut n: u64 = 0;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = shutdown.cancelled() => return,
                }
                n += 1;
                let record = EventRecord {
                    event_id: format!("sim.counter:{run_seq}:{n}"),
                    category: "message".into(),
                    kind: "counter.tick".into(),
                    source: "sim".into(),
                    severity: 0,
                    code: None,
                    message: Some(format!("counter tick {n}")),
                    message_locale: Some("en".into()),
                    // 瞬时事件无设备时间：occurred_at_ns 为 None（禁止伪造 now）
                    occurred_at_ns: None,
                    condition: None,
                    correlation_id: None,
                    attributes: std::collections::BTreeMap::from([("value".into(), Value::U64(n))]),
                };
                if let Err(e) = events.publish(vec![record]).await {
                    // 背压可见：队列满说明 Core 消费端已死，记 warn 后继续
                    // （序号不回退，下游 gap 可观测；由 Core 侧 fail-closed 兜底）。
                    tracing::warn!(error = %e, tick = n, "sim event publish failed");
                }
            }
        }
        SimEventStream::AlarmCycle => {
            // 四态：独立 event_id、共享 condition_id（§1 生命周期语义）
            let steps = [
                (
                    ConditionTransition::Raised,
                    true,
                    None,
                    700u16,
                    "alarm raised",
                ),
                (
                    ConditionTransition::Updated,
                    true,
                    None,
                    700,
                    "alarm updated",
                ),
                (
                    ConditionTransition::Acknowledged,
                    true,
                    Some(true),
                    400,
                    "alarm acknowledged",
                ),
                (
                    ConditionTransition::Cleared,
                    false,
                    None,
                    200,
                    "alarm cleared",
                ),
            ];
            for (i, (transition, active, acknowledged, severity, message)) in
                steps.into_iter().enumerate()
            {
                // 首条立即发射，后续间隔 50ms（给 Core 留出逐批到达的时序）
                if i > 0 {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                        _ = shutdown.cancelled() => return,
                    }
                }
                let name = format!("{transition:?}").to_lowercase();
                let record = EventRecord {
                    event_id: format!("sim.alarm100:{run_seq}:{name}"),
                    category: "alarm".into(),
                    kind: "alarm.condition".into(),
                    source: "Channel1".into(),
                    severity,
                    code: Some("SIM-100".into()),
                    message: Some(message.into()),
                    message_locale: Some("en".into()),
                    occurred_at_ns: Some(now_unix_ns()),
                    condition: Some(EventCondition {
                        condition_id: SIM_ALARM_CONDITION_ID.into(),
                        transition,
                        active: Some(active),
                        acknowledged,
                        confirmed: None,
                        retain: Some(true),
                    }),
                    correlation_id: None,
                    attributes: std::collections::BTreeMap::from([(
                        "code".into(),
                        Value::String("SIM-100".into()),
                    )]),
                };
                if let Err(e) = events.publish(vec![record]).await {
                    tracing::warn!(error = %e, step = i, "sim alarm publish failed");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{AcquisitionTask, DriverBinding};

    fn poll_task(id: &str, interval: u64, points: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: id.into(),
            mode: TaskMode::Poll,
            interval_ms: Some(interval),
            binding: DriverBinding {
                kind: BINDING_KIND.into(),
                config: points,
            },
        }
    }

    #[tokio::test]
    async fn configure_rejects_duplicate_and_unknown_sources() {
        let mut conn = SimConnection::default();
        let dup = poll_task(
            "t",
            100,
            serde_json::json!({
                "points": [
                    {"key":"a","kind":"counter"},
                    {"key":"a","kind":"counter"}
                ]
            }),
        );
        let err = conn.configure(1, vec![dup]).await.unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");

        let unknown = poll_task(
            "t",
            100,
            serde_json::json!({
                "points": [{"key":"a","kind":"quantum_flux"}]
            }),
        );
        let err = conn.configure(1, vec![unknown]).await.unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED_SOURCE_KIND");
    }

    #[test]
    fn counter_advances_and_wraps() {
        // 语义：本次累加后超过 wrap 上限，则该次采样直接输出回绕后的 0
        let spec = SourceSpec::Counter {
            start: 8.0,
            step: 3.0,
            wrap: Some(10.0),
        };
        let mut st = SourceState::new(&spec, 0);
        assert_eq!(st.sample(&spec, 0), Value::F64(0.0)); // 8+3=11 > 10 → 回绕
        assert_eq!(st.sample(&spec, 0), Value::F64(3.0)); // 从 0 继续累加
    }

    #[test]
    fn sine_hits_extremes_at_quarter_periods() {
        let spec = SourceSpec::Sine {
            amplitude: 10.0,
            period_ms: 400,
            offset: 5.0,
        };
        let mut st = SourceState::new(&spec, 0);
        assert_eq!(st.sample(&spec, 100), Value::F64(15.0)); // T*1/4 → 峰值
        assert_eq!(st.sample(&spec, 300), Value::F64(-5.0)); // T*3/4 → 谷值
    }

    #[test]
    fn random_with_fixed_seed_is_deterministic_and_in_range() {
        let mk = || {
            let spec = SourceSpec::Random {
                min: -1.0,
                max: 1.0,
                seed: Some(42),
            };
            let state = SourceState::new(&spec, 0);
            (spec, state)
        };
        let (spec, mut a) = mk();
        let (spec2, mut b) = mk();
        for t in [0u64, 7, 13] {
            let va = a.sample(&spec, t);
            let vb = b.sample(&spec2, t);
            assert_eq!(va, vb, "same seed must reproduce sequence");
            if let Value::F64(f) = va {
                assert!((-1.0..1.0).contains(&f));
            } else {
                panic!("random must produce F64");
            }
        }
    }

    #[test]
    fn quality_spec_static_and_transitions() {
        // 静态 BAD：恒定输出
        let q = QualitySpec::from_json(&serde_json::json!({"quality": "BAD"})).unwrap();
        assert_eq!(q.at(1), Quality::Bad);
        assert_eq!(q.at(100), Quality::Bad);

        // GOOD→BAD→GOOD 转换窗口：第 3 批起坏，第 5 批起恢复
        let q = QualitySpec::from_json(
            &serde_json::json!({"bad_after_batches": 3, "good_again_after": 5}),
        )
        .unwrap();
        assert_eq!(q.at(1), Quality::Good);
        assert_eq!(q.at(2), Quality::Good);
        assert_eq!(q.at(3), Quality::Bad);
        assert_eq!(q.at(4), Quality::Bad);
        assert_eq!(q.at(5), Quality::Good);
        assert_eq!(q.at(9), Quality::Good);

        // 只有 bad_after、无恢复点：持续 BAD
        let q = QualitySpec::from_json(&serde_json::json!({"bad_after_batches": 2})).unwrap();
        assert_eq!(q.at(1), Quality::Good);
        assert_eq!(q.at(2), Quality::Bad);
        assert_eq!(q.at(50), Quality::Bad);

        // 非法质量名与非法阈值必须被拒绝
        assert!(QualitySpec::from_json(&serde_json::json!({"quality": "PERFECT"})).is_err());
        assert!(QualitySpec::from_json(&serde_json::json!({"bad_after_batches": -1})).is_err());
    }

    #[tokio::test]
    async fn conn_faults_parse_and_default_config_ok() {
        // 无 faults 字段的空配置照常工作（兼容空配置）
        let conn = SimulatorDriver.open_connection("e", "{}").await.unwrap();
        let _ = conn;

        let f = parse_conn_faults(
            &serde_json::json!({"faults": {"fail_after_batches": 4, "crash_after_batches": 10}}),
        )
        .unwrap();
        assert_eq!(f.fail_after_batches, Some(4));
        assert_eq!(f.crash_after_batches, Some(10));

        // faults 非对象 / 阈值非法 → 结构化配置错误
        // （Box<dyn DriverConnection> 非 Debug，不能用 unwrap_err，需手动匹配）
        let err = match SimulatorDriver
            .open_connection("e", "{\"faults\": []}")
            .await
        {
            Err(e) => e,
            Ok(_) => panic!("non-object faults must be rejected"),
        };
        assert_eq!(err.code, "BAD_CONFIG");
        let err = match SimulatorDriver
            .open_connection("e", "{\"faults\": {\"crash_after_batches\": \"x\"}}")
            .await
        {
            Err(e) => e,
            Ok(_) => panic!("invalid fault threshold must be rejected"),
        };
        assert_eq!(err.code, "BAD_CONFIG");
    }

    /// 高价值路径：configure -> apply -> 单次采样产出带正确 point_id 的值。
    #[tokio::test]
    async fn full_plan_produces_mapped_point_ids() {
        let mut conn = SimConnection::default();
        let task = poll_task(
            "t",
            100,
            serde_json::json!({
                "points": [
                    {"key":"k.a","kind":"constant","value":7},
                    {"key":"k.b","kind":"toggle","initial":false}
                ]
            }),
        );
        let descriptors = conn.configure(3, vec![task]).await.unwrap();
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].data_type, DataType::F64);
        assert_eq!(descriptors[1].data_type, DataType::Bool);

        conn.apply_point_map(PointMap::from([
            ("k.a".to_string(), 11),
            ("k.b".to_string(), 22),
        ]))
        .await
        .unwrap();

        // 直接驱动内部状态机验证映射结果（不起 IPC）
        let snapshot = conn.plan.as_ref().unwrap();
        let mut states: Vec<(u32, SourceSpec, SourceState)> = snapshot
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    snapshot.map.as_ref().unwrap()[&p.key],
                    p.source.clone(),
                    SourceState::new(&p.source, i as u64),
                )
            })
            .collect();
        let vals: Vec<(u32, Value)> = states
            .iter_mut()
            .map(|(id, spec, st)| (*id, st.sample(spec, 0)))
            .collect();
        assert_eq!(vals[0], (11, Value::F64(7.0)));
        assert_eq!(vals[1], (22, Value::Bool(true)));
    }
}
