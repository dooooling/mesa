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
    AcquisitionTask, DataBatch, DataType, DriverMetadata, DuplicatePointKey, PointDescriptor,
    PointMap, PointValue, Quality, TaskMode, Value, ensure_unique_point_keys, now_unix_ns,
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

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        let v: serde_json::Value = serde_json::from_str(config_json).map_err(|e| {
            SdkDriverError::configuration("BAD_CONFIG", format!("connection JSON 非法: {e}"))
        })?;
        let cfg = FocasConnConfig::from_json(&v)?;
        // 已支持 use_native=true 切 Native（44/44 2026-08-29 165 11 GOOD），默认 Fake 用于 CI
        let use_native = v
            .get("use_native")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
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
            if task.binding.kind != BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: 期望 {BINDING_KIND}，实际 {}",
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
}
