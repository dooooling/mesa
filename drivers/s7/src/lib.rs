//! Siemens S7 Driver — 方案 §7.1 地址型（V1 只读）。
//!
//! - 绑定种类：`s7.address-group`
//!   ```json
//!   { "items": [
//!       { "key": "motor.speed", "address": "DB10.DBD20", "data_type": "REAL" },
//!       { "key": "motor.running", "address": "DB10.DBX24.0", "data_type": "BOOL" }
//!   ]}
//!   ```
//! - 支持区域 DB / M / I / Q；类型见 `codec::S7Kind`；Core 不解析地址（硬约束）。

mod address;
mod client;
mod codec;

pub use address::{parse_address, S7Address};
pub use codec::{decode_value, parse_data_type};

use std::sync::Arc;
use std::time::Duration;

use forgelink_core_types::{
    ensure_unique_point_keys, AcquisitionTask, DataBatch, DataType, DriverMetadata, DuplicatePointKey,
    PointDescriptor, PointMap, PointValue, TaskMode, Value, now_unix_ns,
};
use forgelink_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub const BINDING_KIND: &str = "s7.address-group";

use address::AddressError;
use client::{ReadItem, S7Client, S7ConnConfig};
use codec::S7Kind;

#[derive(Default)]
pub struct S7Driver;

#[async_trait::async_trait]
impl Driver for S7Driver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: "s7".into(),
            name: "Siemens S7".into(),
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
        let v: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| SdkDriverError::configuration("BAD_CONFIG", format!("connection JSON 非法: {e}")))?;
        let s7cfg = S7ConnConfig::from_json(&v)?;
        Ok(Box::new(S7Connection { cfg: s7cfg, plan: None }))
    }
}

// ---------------------------------------------------------------------------
// 采集计划
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PointSpec {
    key: String,
    addr: S7Address,
    kind: S7Kind,
    data_type: DataType,
}

#[derive(Debug)]
struct TaskPlan {
    id: String,
    interval_ms: u64,
    point_indices: Vec<usize>,
}

#[derive(Debug)]
struct PlanSnapshot {
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    map: Option<PointMap>,
}

#[derive(Debug)]
struct S7Connection {
    cfg: S7ConnConfig,
    plan: Option<PlanSnapshot>,
}

#[async_trait::async_trait]
impl DriverConnection for S7Connection {
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
                    forgelink_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: s7 仅支持 poll", task.id),
                ));
            }
            if task.binding.kind != BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!("task `{}`: 期望 {BINDING_KIND}，实际 {}", task.id, task.binding.kind),
                ));
            }
            let items = task.binding.config.get("items").and_then(|v| v.as_array()).ok_or_else(|| {
                SdkDriverError::configuration("INVALID_BINDING_CONFIG", format!("task `{}`: 缺少 items 数组", task.id))
            })?;
            if items.is_empty() {
                return Err(SdkDriverError::configuration("INVALID_BINDING_CONFIG", format!("task `{}`: items 不能为空", task.id)));
            }
            let mut indices = Vec::with_capacity(items.len());
            for item in items {
                let key = item.get("key").and_then(|v| v.as_str()).ok_or_else(|| SdkDriverError::configuration("INVALID_POINT", format!("task `{}`: point 缺少 key", task.id)))?;
                if key.trim().is_empty() {
                    return Err(SdkDriverError::configuration("INVALID_POINT", "key 不能为空"));
                }
                let addr_str = item.get("address").and_then(|v| v.as_str()).ok_or_else(|| SdkDriverError::configuration("INVALID_POINT", format!("point `{key}` 缺少 address")))?;
                let dt_str = item.get("data_type").and_then(|v| v.as_str()).ok_or_else(|| SdkDriverError::configuration("INVALID_POINT", format!("point `{key}` 缺少 data_type")))?;
                let addr = parse_address(addr_str).map_err(|e| match e {
                    AddressError::Empty => SdkDriverError::configuration("INVALID_ADDRESS", format!("point `{key}` 地址为空")),
                    AddressError::Invalid { reason, .. } => SdkDriverError::new(forgelink_core_types::ErrorKind::Address, "INVALID_ADDRESS", format!("point `{key}` 地址 `{addr_str}` 非法: {reason}")),
                })?;
                let (data_type, kind) = parse_data_type(dt_str)?;
                // BOOL 必须带位
                if kind == S7Kind::Bool && addr.bit_offset.is_none() {
                    return Err(SdkDriverError::configuration("INVALID_ADDRESS", format!("point `{key}` BOOL 必须使用位地址如 DB10.DBX0.0 或 M0.0")));
                }
                if kind != S7Kind::Bool && addr.bit_offset.is_some() {
                    return Err(SdkDriverError::configuration("INVALID_ADDRESS", format!("point `{key}` 非 BOOL 不应带位偏移")));
                }
                indices.push(new_points.len());
                new_points.push(PointSpec { key: key.to_string(), addr, kind, data_type });
            }
            let interval = task.interval_ms.expect("validated");
            new_tasks.push(TaskPlan { id: task.id.clone(), interval_ms: interval, point_indices: indices });
        }

        let descriptors: Vec<PointDescriptor> = new_points
            .iter()
            .map(|p| PointDescriptor { point_key: p.key.clone(), data_type: p.data_type, unit: None })
            .collect();
        ensure_unique_point_keys(&descriptors).map_err(|DuplicatePointKey(k)| {
            SdkDriverError::configuration("DUPLICATE_POINT_KEY", format!("`{k}` 重复"))
        })?;

        tracing::info!(revision, points = new_points.len(), tasks = new_tasks.len(), "S7 采集计划构建完成");
        self.plan = Some(PlanSnapshot { revision, points: new_points, tasks: new_tasks, map: None });
        Ok(descriptors)
    }

    async fn apply_point_map(&mut self, map: PointMap) -> Result<(), SdkDriverError> {
        let snap = self.plan.as_mut().ok_or_else(|| SdkDriverError::new(forgelink_core_types::ErrorKind::Internal, "NOT_CONFIGURED", "apply 在 configure 之前"))?;
        for p in &snap.points {
            if !map.contains_key(&p.key) {
                return Err(SdkDriverError::configuration("MISSING_POINT_ID", format!("point `{}` 缺少映射", p.key)));
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
        let snap = self.plan.as_ref().ok_or_else(|| SdkDriverError::new(forgelink_core_types::ErrorKind::Internal, "NO_PLAN", "run 前未 configure+apply"))?;
        let map = snap.map.as_ref().ok_or_else(|| SdkDriverError::new(forgelink_core_types::ErrorKind::Internal, "NO_POINT_MAP", "run 前未 apply_point_map"))?;

        // 建立 S7 会话（若失败则直接返回，由 Manager 按 §11.1 退避重连；PUT/GET 未启用等配置类错误直接 Failed）
        let mut client = S7Client::connect(self.cfg.clone()).await.map_err(|e| {
            tracing::error!(error=%e, "S7 连接失败");
            e
        })?;
        let client = Arc::new(Mutex::new(client));

        // 为每个 Task 起独立轮询协程，共享同一 S7 会话（串行化读）
        use std::sync::atomic::{AtomicU64, Ordering};
        let seq = Arc::new(AtomicU64::new(1));

        let mut handles = Vec::with_capacity(snap.tasks.len());
        for task in &snap.tasks {
            let indices = task.point_indices.clone();
            let points: Vec<(PointSpec, u32)> = indices.iter().map(|&i| {
                let p = snap.points[i].clone();
                let pid = map[&p.key];
                (p, pid)
            }).collect();
            let sink = sink.clone();
            let shutdown = shutdown.clone();
            let seq = Arc::clone(&seq);
            let client = Arc::clone(&client);
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
                    // 组装本任务的读项：BOOL 按所在字节的 BYTE 读取后本地取位，避免 BIT 传输的兼容性差异
                    let items: Vec<ReadItem> = points.iter().map(|(spec, _)| {
                        if spec.kind == S7Kind::Bool {
                            let mut addr = spec.addr.clone();
                            addr.bit_offset = None;
                            ReadItem { addr, kind: S7Kind::Byte }
                        } else {
                            ReadItem { addr: spec.addr.clone(), kind: spec.kind }
                        }
                    }).collect();
                    let raw_vec = {
                        let mut guard = client.lock().await;
                        match guard.read_vars(&items).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!(task=%task_id, error=%e, "S7 读失败");
                                return Err(e);
                            }
                        }
                    };
                    // 解码并发布：BOOL 需按位提取
                    let values: Vec<PointValue> = points.iter().zip(raw_vec).filter_map(|((spec, pid), raw)| {
                        let vt = if spec.kind == S7Kind::Bool {
                            let bit = spec.addr.bit_offset.unwrap_or(0) as usize;
                            let b = raw.first().copied().unwrap_or(0);
                            Ok(Value::Bool(((b >> bit) & 1) != 0))
                        } else {
                            decode_value(&raw, spec.kind)
                        };
                        match vt {
                            Ok(v) => Some(PointValue::good(*pid, v)),
                            Err(e) => {
                                tracing::warn!(key=%spec.key, error=%e, "解码失败");
                                None
                            }
                        }
                    }).collect();
                    if values.is_empty() { continue; }
                    sink.publish(DataBatch {
                        connection_handle: 0,
                        stream_epoch: 0,
                        sequence: seq.fetch_add(1, Ordering::Relaxed),
                        timestamp_ns: now_unix_ns(),
                        values,
                    }).await;
                }
                Ok::<(), SdkDriverError>(())
            }));
        }

        // 等待任一任务失败或全部被取消
        let mut final_err: Option<SdkDriverError> = None;
        for h in handles {
            match h.await {
                Ok(Ok(())) => {},
                Ok(Err(e)) => {
                    if final_err.is_none() { final_err = Some(e); }
                },
                Err(join_err) => {
                    tracing::error!(%join_err, "S7 任务 panic");
                    if final_err.is_none() {
                        final_err = Some(SdkDriverError::new(forgelink_core_types::ErrorKind::Internal, "TASK_PANIC", join_err.to_string()));
                    }
                }
            }
            // 若已有失败，取消其余
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
    use forgelink_core_types::{AcquisitionTask, DriverBinding, TaskMode};

    fn task_with_items(items: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: "t1".into(),
            mode: TaskMode::Poll,
            interval_ms: Some(100),
            binding: DriverBinding { kind: BINDING_KIND.into(), config: serde_json::json!({"items": items}) },
        }
    }

    #[tokio::test]
    async fn configure_ok_and_duplicate_rejected() {
        let mut conn = S7Connection { cfg: S7ConnConfig::default(), plan: None };
        let items = serde_json::json!([
            {"key":"a","address":"DB10.DBD0","data_type":"REAL"},
            {"key":"b","address":"DB10.DBX0.0","data_type":"BOOL"}
        ]);
        let t = task_with_items(items);
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 2);
        assert_eq!(descs[0].data_type, forgelink_core_types::DataType::F32);
        // 重复 key
        let dup = serde_json::json!([
            {"key":"a","address":"DB10.DBD0","data_type":"REAL"},
            {"key":"a","address":"DB10.DBD4","data_type":"REAL"}
        ]);
        let err = conn.configure(2, vec![task_with_items(dup)]).await.unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");
    }

    #[tokio::test]
    async fn bool_requires_bit() {
        let mut conn = S7Connection { cfg: S7ConnConfig::default(), plan: None };
        let items = serde_json::json!([{"key":"a","address":"DB10.DBD0","data_type":"BOOL"}]);
        let err = conn.configure(1, vec![task_with_items(items)]).await.unwrap_err();
        assert_eq!(err.code, "INVALID_ADDRESS");
    }
}
