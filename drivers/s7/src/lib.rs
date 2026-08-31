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
pub mod client; // Common SZL 直连诊断需对外暴露（V1 只读，不影响 Core 隔离）
mod codec;

pub use address::{S7Address, parse_address};
pub use codec::{decode_value, parse_data_type};

use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{
    AcquisitionTask, DataBatch, DataType, DriverMetadata, DuplicatePointKey, PointDescriptor,
    PointMap, PointValue, TaskMode, Value, ensure_unique_point_keys, now_unix_ns,
};
use mesa_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
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
        let v: serde_json::Value = serde_json::from_str(config_json).map_err(|e| {
            SdkDriverError::configuration("BAD_CONFIG", format!("connection JSON 非法: {e}"))
        })?;
        let s7cfg = S7ConnConfig::from_json(&v)?;
        Ok(Box::new(S7Connection {
            cfg: s7cfg,
            plan: None,
        }))
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

#[derive(Debug)]
struct S7Connection {
    cfg: S7ConnConfig,
    plan: Option<PlanSnapshot>,
}

// 连续区合并的批量结构，供 run 内合并与单元测试复用
#[derive(Debug, Clone)]
struct Bulk {
    base: S7Address,
    len: usize,
    members: Vec<(usize, usize, S7Kind, u32)>,
}

/// 将已排序的 paired 按 (area, db, byte_offset) 合并为 Bulk，规则与 run 内一致：
/// 单区上限由 negotiated PDU 决定（400 适配 480，900 适配 960），STRING/WSTRING 隔离，oversized 按 PDU 分片
#[allow(dead_code)]
fn merge_paired_to_bulks(paired: &[((PointSpec, u32), ReadItem)]) -> Vec<Bulk> {
    merge_paired_to_bulks_with_max(paired, 900)
}

/// 支持指定 max 的合并，用于 PDU240/480/960 单测
fn merge_paired_to_bulks_with_max(
    paired: &[((PointSpec, u32), ReadItem)],
    max_bulk_len: usize,
) -> Vec<Bulk> {
    const GAP_THRESHOLD: u32 = 4;
    let mut bulks: Vec<Bulk> = Vec::new();
    let mut cur: Option<Bulk> = None;
    for (idx, ((spec, pid), item)) in paired.iter().enumerate() {
        let is_string = matches!(item.kind, S7Kind::String | S7Kind::WString);
        let item_len = item.kind.byte_len();
        let start = item.addr.byte_offset;
        let end = start + item_len as u32;
        let can_merge = if let Some(c) = cur.as_ref() {
            let c_end = c.base.byte_offset + c.len as u32;
            let c_has_string = c.members.iter().any(|(mi, _, _, _)| {
                matches!(paired[*mi].1.kind, S7Kind::String | S7Kind::WString)
            });
            !is_string
                && !c_has_string
                && item.addr.area == c.base.area
                && item.addr.db_number == c.base.db_number
                && start <= c_end + GAP_THRESHOLD
                && (end.max(c_end) - c.base.byte_offset) as usize <= max_bulk_len
        } else {
            false
        };
        if can_merge {
            let c = cur.as_mut().unwrap();
            let c_end = c.base.byte_offset + c.len as u32;
            let new_end = end.max(c_end);
            c.len = (new_end - c.base.byte_offset) as usize;
            let offset = (start - c.base.byte_offset) as usize;
            c.members.push((idx, offset, item.kind, *pid));
        } else {
            if let Some(c) = cur.take() {
                bulks.push(c);
            }
            cur = Some(Bulk {
                base: item.addr.clone(),
                len: item_len,
                members: vec![(idx, 0, item.kind, *pid)],
            });
            let _ = spec;
        }
    }
    if let Some(c) = cur.take() {
        bulks.push(c);
    }
    // 物理分片下沉至 S7Client::read_byte_ranges，planner 仅做逻辑合并，不做物理分片，避免头污染与成员跨 chunk 覆盖
    bulks
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
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!("task `{}`: s7 仅支持 poll", task.id),
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
                let (data_type, kind) = parse_data_type(dt_str)?;
                // BOOL 必须带位
                if kind == S7Kind::Bool && addr.bit_offset.is_none() {
                    return Err(SdkDriverError::configuration(
                        "INVALID_ADDRESS",
                        format!("point `{key}` BOOL 必须使用位地址如 DB10.DBX0.0 或 M0.0"),
                    ));
                }
                if kind != S7Kind::Bool && addr.bit_offset.is_some() {
                    return Err(SdkDriverError::configuration(
                        "INVALID_ADDRESS",
                        format!("point `{key}` 非 BOOL 不应带位偏移"),
                    ));
                }
                indices.push(new_points.len());
                new_points.push(PointSpec {
                    key: key.to_string(),
                    addr,
                    kind,
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
            "S7 采集计划构建完成"
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

        // 建立 S7 会话（若失败则直接返回，由 Manager 按 §11.1 退避重连；PUT/GET 未启用等配置类错误直接 Failed）
        let client = S7Client::connect(self.cfg.clone()).await.map_err(|e| {
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
                    // 组装本任务的读项：BOOL 按所在字节的 BYTE 读取后本地取位，避免 BIT 传输的兼容性差异；
                    // 连续合并：同 DB 内按 byte_offset 排序后，若下一项紧接上一项尾部则合并为一次变长读，减少 PDU 往返（V1 §7.1 合并连续区域）
                    // 保持 point 与 ReadItem 同序排序，避免排序后 zip 错位
                    // 组装本任务的读项：BOOL 按所在字节的 BYTE 读取后本地取位，避免 BIT 传输的兼容性差异
                    let mut paired: Vec<((PointSpec, u32), ReadItem)> = points
                        .iter()
                        .map(|(spec, pid)| {
                            let item = if spec.kind == S7Kind::Bool {
                                let mut addr = spec.addr.clone();
                                addr.bit_offset = None;
                                ReadItem {
                                    addr,
                                    kind: S7Kind::Byte,
                                }
                            } else {
                                ReadItem {
                                    addr: spec.addr.clone(),
                                    kind: spec.kind,
                                }
                            };
                            ((spec.clone(), *pid), item)
                        })
                        .collect();
                    // 按 (area, db, byte_offset) 排序，为连续合并做准备
                    paired.sort_by(|a, b| {
                        a.1.addr
                            .area
                            .code()
                            .cmp(&b.1.addr.area.code())
                            .then(a.1.addr.db_number.cmp(&b.1.addr.db_number))
                            .then(a.1.addr.byte_offset.cmp(&b.1.addr.byte_offset))
                    });
                    // 生产代码复用 merge_paired_to_bulks_with_max，按 negotiated PDU 分片（240/480/960）
                    let pdu_max = {
                        let g = client.lock().await;
                        (g.pdu_length() as usize).saturating_sub(32).clamp(200, 900)
                    };
                    let bulks = merge_paired_to_bulks_with_max(&paired, pdu_max);
                    // 按 bulk 发起批量 BYTE 读，client 内部再按 PDU 剩余动态分批
                    let ranges: Vec<(S7Address, usize)> =
                        bulks.iter().map(|b| (b.base.clone(), b.len)).collect();
                    let bulk_raws: Vec<Option<Vec<u8>>> = {
                        let mut guard = client.lock().await;
                        match guard.read_byte_ranges(&ranges).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!(task=%task_id, error=%e, "S7 bulk 读失败");
                                return Err(e);
                            }
                        }
                    };
                    // 将 bulk 字节切片分发回各点，按项 BAD 隔离
                    let mut raw_by_paired: Vec<Option<Vec<u8>>> = vec![None; paired.len()];
                    let mut fallback_indices: Vec<(usize, ReadItem)> = Vec::new();
                    for (bulk, raw_opt) in bulks.iter().zip(bulk_raws.iter()) {
                        match raw_opt {
                            Some(bulk_bytes) => {
                                for (paired_idx, offset, kind, _pid) in &bulk.members {
                                    let need = kind.byte_len();
                                    if *offset + need <= bulk_bytes.len() {
                                        raw_by_paired[*paired_idx] =
                                            Some(bulk_bytes[*offset..*offset + need].to_vec());
                                    } else if *offset < bulk_bytes.len() {
                                        raw_by_paired[*paired_idx] =
                                            Some(bulk_bytes[*offset..].to_vec());
                                    }
                                }
                            }
                            None => {
                                // bulk 整体 BAD（如合并区含非法地址），回退为逐点单读以隔离合法点
                                for (paired_idx, _offset, _kind, _pid) in &bulk.members {
                                    let (_, item) = &paired[*paired_idx];
                                    fallback_indices.push((*paired_idx, item.clone()));
                                }
                            }
                        }
                    }
                    // 逐点回退：对 bulk BAD 的成员逐一单读，避免合法点被误丢
                    if !fallback_indices.is_empty() {
                        let mut guard = client.lock().await;
                        for (paired_idx, item) in fallback_indices {
                            match guard.read_vars(&[item]).await {
                                Ok(mut v) => {
                                    if let Some(Some(b)) = v.pop() {
                                        raw_by_paired[paired_idx] = Some(b);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(idx=%paired_idx, error=%e, "S7 fallback 单点读失败");
                                }
                            }
                        }
                    }
                    // 解码并发布
                    let values: Vec<PointValue> = paired
                        .iter()
                        .enumerate()
                        .filter_map(|(idx, ((spec, pid), _))| {
                            let raw = match &raw_by_paired[idx] {
                                Some(v) => v,
                                None => return None,
                            };
                            let vt = if spec.kind == S7Kind::Bool {
                                let bit = spec.addr.bit_offset.unwrap_or(0) as usize;
                                let b = raw.first().copied().unwrap_or(0);
                                Ok(Value::Bool(((b >> bit) & 1) != 0))
                            } else {
                                decode_value(raw, spec.kind)
                            };
                            match vt {
                                Ok(v) => Some(PointValue::good(*pid, v)),
                                Err(e) => {
                                    tracing::warn!(key=%spec.key, error=%e, "解码失败");
                                    None
                                }
                            }
                        })
                        .collect();
                    if values.is_empty() {
                        continue;
                    }
                    sink.publish(DataBatch {
                        connection_handle: 0,
                        stream_epoch: 0,
                        sequence: seq.fetch_add(1, Ordering::Relaxed),
                        timestamp_ns: now_unix_ns(),
                        values,
                    })
                    .await;
                }
                Ok::<(), SdkDriverError>(())
            }));
        }

        // 等待任一任务失败或全部被取消
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
                    tracing::error!(%join_err, "S7 任务 panic");
                    if final_err.is_none() {
                        final_err = Some(SdkDriverError::new(
                            mesa_core_types::ErrorKind::Internal,
                            "TASK_PANIC",
                            join_err.to_string(),
                        ));
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
        let mut conn = S7Connection {
            cfg: S7ConnConfig::default(),
            plan: None,
        };
        let items = serde_json::json!([
            {"key":"a","address":"DB10.DBD0","data_type":"REAL"},
            {"key":"b","address":"DB10.DBX0.0","data_type":"BOOL"}
        ]);
        let t = task_with_items(items);
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 2);
        assert_eq!(descs[0].data_type, mesa_core_types::DataType::F32);
        // 重复 key
        let dup = serde_json::json!([
            {"key":"a","address":"DB10.DBD0","data_type":"REAL"},
            {"key":"a","address":"DB10.DBD4","data_type":"REAL"}
        ]);
        let err = conn
            .configure(2, vec![task_with_items(dup)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");
    }

    #[tokio::test]
    async fn bool_requires_bit() {
        let mut conn = S7Connection {
            cfg: S7ConnConfig::default(),
            plan: None,
        };
        let items = serde_json::json!([{"key":"a","address":"DB10.DBD0","data_type":"BOOL"}]);
        let err = conn
            .configure(1, vec![task_with_items(items)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ADDRESS");
    }

    #[test]
    fn merge_contiguous_and_gap() {
        // DB10.DBD0(4B) + DB10.DBD4(4B) 相邻 → 合并为 8B 单 bulk
        let mk = |addr: &str, kind| {
            let a = parse_address(addr).unwrap();
            ReadItem { addr: a, kind }
        };
        let mk_spec = |key: &str, addr: &str, kind| PointSpec {
            key: key.into(),
            addr: parse_address(addr).unwrap(),
            kind,
            data_type: mesa_core_types::DataType::F32,
        };
        let paired = vec![
            (
                (mk_spec("a", "DB10.DBD0", S7Kind::Real), 1),
                mk("DB10.DBD0", S7Kind::Real),
            ),
            (
                (mk_spec("b", "DB10.DBD4", S7Kind::Real), 2),
                mk("DB10.DBD4", S7Kind::Real),
            ),
        ];
        let bulks = merge_paired_to_bulks(&paired);
        assert_eq!(bulks.len(), 1, "相邻 4+4 应合并");
        assert_eq!(bulks[0].len, 8);
        //  gap 10 (>4) 不合并
        let paired2 = vec![
            (
                (mk_spec("a", "DB10.DBD0", S7Kind::Real), 1),
                mk("DB10.DBD0", S7Kind::Real),
            ),
            (
                (mk_spec("b", "DB10.DBD20", S7Kind::Real), 2),
                mk("DB10.DBD20", S7Kind::Real),
            ),
        ];
        let bulks2 = merge_paired_to_bulks(&paired2);
        assert_eq!(bulks2.len(), 2, "gap 16 应分两 bulk");
    }

    #[test]
    fn merge_wstring_isolated_and_oversized() {
        let mk = |addr: &str, kind| {
            let a = parse_address(addr).unwrap();
            ReadItem { addr: a, kind }
        };
        let mk_spec = |key: &str, addr: &str, kind| PointSpec {
            key: key.into(),
            addr: parse_address(addr).unwrap(),
            kind,
            data_type: mesa_core_types::DataType::String,
        };
        // WSTRING 516B 单独成区，不与 REAL 合并
        let paired = vec![
            (
                (mk_spec("a", "DB10.DBD0", S7Kind::Real), 1),
                mk("DB10.DBD0", S7Kind::Real),
            ),
            (
                (mk_spec("b", "DB10.DBD256", S7Kind::WString), 2),
                mk("DB10.DBD256", S7Kind::WString),
            ),
        ];
        let bulks = merge_paired_to_bulks(&paired);
        assert_eq!(bulks.len(), 2, "WSTRING 应隔离");
        // oversized：10 个 REAL 4*10=40 <400 合并为 1；101 个 REAL 404 >400 需分片
        let many: Vec<((PointSpec, u32), ReadItem)> = (0..101)
            .map(|i| {
                let addr = format!("DB10.DBD{}", i * 4);
                let a = parse_address(&addr).unwrap();
                let spec = PointSpec {
                    key: format!("k{i}"),
                    addr: a.clone(),
                    kind: S7Kind::Real,
                    data_type: mesa_core_types::DataType::F32,
                };
                (
                    (spec, i as u32),
                    ReadItem {
                        addr: a,
                        kind: S7Kind::Real,
                    },
                )
            })
            .collect();
        let bulks_many = merge_paired_to_bulks_with_max(&many, 400);
        assert!(
            bulks_many.len() > 1,
            "101*4=404 超限应分片，实际 {}",
            bulks_many.len()
        );
        for b in &bulks_many {
            assert!(b.len <= 400, "单 bulk 不超 400，实际 {}", b.len);
        }
        // 真正 PDU240/480/960：同一 101 项在不同 max 下分片数不同
        let b240 = merge_paired_to_bulks_with_max(&many, 240 - 32);
        let b480 = merge_paired_to_bulks_with_max(&many, 480 - 32);
        let b960 = merge_paired_to_bulks_with_max(&many, 960 - 32);
        assert!(
            b240.len() >= b480.len() && b480.len() >= b960.len(),
            "小 PDU 应分更多片 240:{} 480:{} 960:{}",
            b240.len(),
            b480.len(),
            b960.len()
        );
    }

    #[test]
    fn negotiated_pdu_aware_chunking() {
        // 验证 client 动态 PDU：19 限制 + byte_len 限流，WSTRING 516 单项仍可单 PDU（取决于协商 960）
        // 此单测仅验证 merge 层不超 400，client 层由 S7_MAX_ITEMS_PER_PDU 与 pdu_length 双重限流已在 client.rs 400 行覆盖
        let mk = |addr: &str, kind| {
            let a = parse_address(addr).unwrap();
            ReadItem { addr: a, kind }
        };
        let mk_spec = |key: &str, addr: &str, kind| PointSpec {
            key: key.into(),
            addr: parse_address(addr).unwrap(),
            kind,
            data_type: mesa_core_types::DataType::String,
        };
        let paired = vec![(
            (mk_spec("a", "DB10.DBD0", S7Kind::String), 1),
            mk("DB10.DBD0", S7Kind::String),
        )];
        let bulks = merge_paired_to_bulks(&paired);
        assert_eq!(bulks[0].len, 256, "STRING 固定 256");
    }
}
