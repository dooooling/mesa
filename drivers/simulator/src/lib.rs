//! ForgeLink Simulator Driver（方案附录 A）。
//!
//! 定位：Driver Framework 的参考实现与 Contract/Performance Test 基线，
//! 不属于正式设备协议范围。行为配置属于测试配置，不进入生产 DeviceProfile。
//!
//! M0 实现的数据源（附录 A.1 子集）：Constant / Counter / Sine / Toggle / Random。
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
//! TODO: 附录 A 其余能力（delay/jitter/burst/silent_interval/disconnect/hang/crash 注入）
//! 在 Phase 3 补齐，用于覆盖全量 Contract Test。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use forgelink_core_types::{
    ensure_unique_point_keys, AcquisitionTask, DataBatch, DataType, DriverMetadata,
    DuplicatePointKey, ErrorKind, PointDescriptor, PointMap, PointValue, TaskMode, Value,
    now_unix_ns,
};
use forgelink_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
use tokio_util::sync::CancellationToken;

pub const BINDING_KIND: &str = "simulator.points";

/// Simulator 驱动实例。无连接级共享状态——每个连接独立持有采集计划。
#[derive(Default)]
pub struct SimulatorDriver;

#[async_trait::async_trait]
impl Driver for SimulatorDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: "simulator".into(),
            name: "ForgeLink Simulator".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            // 与 drivers/simulator/driver.toml 保持一致
            protocol_major: 1,
            protocol_minor: 0,
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        // connection 配置当前无必填项；解析仅校验 JSON 合法性
        let _: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| SdkDriverError::configuration("BAD_CONFIG", e.to_string()))?;
        Ok(Box::new(SimConnection { plan: None }))
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
}

#[derive(Debug, Clone, PartialEq)]
enum SourceSpec {
    Counter { start: f64, step: f64, wrap: Option<f64> },
    Sine { amplitude: f64, period_ms: u64, offset: f64 },
    Toggle { initial: bool },
    Constant { value: Value },
    Random { min: f64, max: f64, seed: Option<u64> },
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
                ))
            }
        };
        Ok(PointSpec { key: key.to_string(), source })
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
                if let Some(w) = wrap {
                    if self.counter > *w {
                        // 简单回绕到 0：满足附录 A "Counter 可配置回绕"
                        self.counter = 0.0;
                    }
                }
                Value::F64(self.counter)
            }
            SourceSpec::Sine { amplitude, period_ms, offset } => {
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
    id: String,
    interval_ms: u64,
    /// 指向快照内 points 数组的下标集合。
    point_indices: Vec<usize>,
}

/// configure 成功后的完整采集计划快照（§6.2 全量替换的原子单元）。
#[derive(Debug)]
struct PlanSnapshot {
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    /// ApplyPointMap 后写入；run 前必须存在。
    map: Option<PointMap>,
}

#[derive(Debug, Default)]
struct SimConnection {
    plan: Option<PlanSnapshot>,
}

#[async_trait::async_trait]
impl DriverConnection for SimConnection {
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
            if task.binding.kind != BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: binding kind `{}` unsupported, expected `{BINDING_KIND}`",
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

        tracing::info!(revision, points = new_points.len(), tasks = new_tasks.len(), "plan built");
        self.plan = Some(PlanSnapshot { revision, points: new_points, tasks: new_tasks, map: None });
        Ok(descriptors)
    }

    async fn apply_point_map(&mut self, map: PointMap) -> Result<(), SdkDriverError> {
        let snapshot = self.plan.as_mut().ok_or_else(|| {
            SdkDriverError::new(ErrorKind::Internal, "NOT_CONFIGURED", "apply before configure")
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
        self: Box<Self>,
        sink: DataSink,
        shutdown: CancellationToken,
    ) -> Result<(), SdkDriverError> {
        let snapshot = self.plan.as_ref().ok_or_else(|| {
            SdkDriverError::new(ErrorKind::Internal, "NO_PLAN", "run before configure+apply")
        })?;
        let map = snapshot.map.as_ref().ok_or_else(|| {
            SdkDriverError::new(ErrorKind::Internal, "NO_POINT_MAP", "run before apply_point_map")
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
                    RuntimePoint { point_id, state: SourceState::new(&spec.source, local as u64), spec }
                })
                .collect()
        };

        // sequence 按 (connection, epoch) 从 1 递增（§10）；原子计数保证多任务唯一
        let seq = Arc::new(AtomicU64::new(1));
        let started = std::time::Instant::now();

        let mut handles = Vec::with_capacity(snapshot.tasks.len());
        for plan in &snapshot.tasks {
            let mut runtime = build_runtime(&plan.point_indices);
            let sink = sink.clone();
            let shutdown = shutdown.clone();
            let seq = Arc::clone(&seq);
            let interval = Duration::from_millis(plan.interval_ms);
            handles.push(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {}
                        _ = shutdown.cancelled() => return,
                    }
                    let t_ms = started.elapsed().as_millis() as u64;
                    let values: Vec<PointValue> = runtime
                        .iter_mut()
                        .map(|rp| PointValue::good(rp.point_id, rp.state.sample(&rp.spec.source, t_ms)))
                        .collect();
                    // NOTE: publish 内部做 Latest-Wins 合并，sequence 可能产生缺口（§12）
                    sink.publish(DataBatch {
                        // handle/epoch 由 SDK 盖戳，驱动侧无需感知分配细节
                        connection_handle: 0,
                        stream_epoch: 0,
                        sequence: seq.fetch_add(1, Ordering::Relaxed),
                        timestamp_ns: now_unix_ns(),
                        values,
                    })
                    .await;
                }
            }));
        }
        for h in handles {
            let _ = h.await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgelink_core_types::{AcquisitionTask, DriverBinding};

    fn poll_task(id: &str, interval: u64, points: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: id.into(),
            mode: TaskMode::Poll,
            interval_ms: Some(interval),
            binding: DriverBinding { kind: BINDING_KIND.into(), config: points },
        }
    }

    #[tokio::test]
    async fn configure_rejects_duplicate_and_unknown_sources() {
        let mut conn = SimConnection::default();
        let dup = poll_task("t", 100, serde_json::json!({
            "points": [
                {"key":"a","kind":"counter"},
                {"key":"a","kind":"counter"}
            ]
        }));
        let err = conn.configure(1, vec![dup]).await.unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");

        let unknown = poll_task("t", 100, serde_json::json!({
            "points": [{"key":"a","kind":"quantum_flux"}]
        }));
        let err = conn.configure(1, vec![unknown]).await.unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED_SOURCE_KIND");
    }

    #[test]
    fn counter_advances_and_wraps() {
        // 语义：本次累加后超过 wrap 上限，则该次采样直接输出回绕后的 0
        let spec = SourceSpec::Counter { start: 8.0, step: 3.0, wrap: Some(10.0) };
        let mut st = SourceState::new(&spec, 0);
        assert_eq!(st.sample(&spec, 0), Value::F64(0.0)); // 8+3=11 > 10 → 回绕
        assert_eq!(st.sample(&spec, 0), Value::F64(3.0)); // 从 0 继续累加
    }

    #[test]
    fn sine_hits_extremes_at_quarter_periods() {
        let spec = SourceSpec::Sine { amplitude: 10.0, period_ms: 400, offset: 5.0 };
        let mut st = SourceState::new(&spec, 0);
        assert_eq!(st.sample(&spec, 100), Value::F64(15.0)); // T*1/4 → 峰值
        assert_eq!(st.sample(&spec, 300), Value::F64(-5.0)); // T*3/4 → 谷值
    }

    #[test]
    fn random_with_fixed_seed_is_deterministic_and_in_range() {
        let mk = || {
            let spec = SourceSpec::Random { min: -1.0, max: 1.0, seed: Some(42) };
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

    /// 高价值路径：configure -> apply -> 单次采样产出带正确 point_id 的值。
    #[tokio::test]
    async fn full_plan_produces_mapped_point_ids() {
        let mut conn = SimConnection::default();
        let task = poll_task("t", 100, serde_json::json!({
            "points": [
                {"key":"k.a","kind":"constant","value":7},
                {"key":"k.b","kind":"toggle","initial":false}
            ]
        }));
        let descriptors = conn.configure(3, vec![task]).await.unwrap();
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].data_type, DataType::F64);
        assert_eq!(descriptors[1].data_type, DataType::Bool);

        conn.apply_point_map(PointMap::from([("k.a".to_string(), 11), ("k.b".to_string(), 22)]))
            .await
            .unwrap();

        // 直接驱动内部状态机验证映射结果（不起 IPC）
        let snapshot = conn.plan.as_ref().unwrap();
        let mut states: Vec<(u32, SourceSpec, SourceState)> = snapshot
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (snapshot.map.as_ref().unwrap()[&p.key], p.source.clone(), SourceState::new(&p.source, i as u64))
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
