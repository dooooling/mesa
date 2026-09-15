//! Core 进程内共享状态：驱动清单、Endpoint 状态与最新值缓存。
//!
//! 全部驻留内存，不落盘——热路径最新值仅内存可达，持久化由 ConfigStore 负责。

use std::collections::{HashMap, VecDeque};
use std::sync::{
    RwLock,
    atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

/// Value 的 JSON 视图：带类型标签，避免 REST 消费方猜测数值含义。
#[derive(Debug, Clone, Serialize)]
pub struct ValueJson {
    #[serde(rename = "type")]
    pub type_name: String,
    pub value: serde_json::Value,
}

fn value_to_json(v: &mesa_core_types::Value) -> ValueJson {
    use mesa_core_types::Value as V;
    let (type_name, value) = match v {
        V::Bool(b) => ("bool", serde_json::json!(b)),
        V::I32(x) => ("i32", serde_json::json!(x)),
        V::U32(x) => ("u32", serde_json::json!(x)),
        V::I64(x) => ("i64", serde_json::json!(x)),
        V::U64(x) => ("u64", serde_json::json!(x)),
        V::F32(x) => ("f32", serde_json::json!(x)),
        V::F64(x) => ("f64", serde_json::json!(x)),
        V::String(s) => ("string", serde_json::json!(s)),
        V::Bytes(b) => ("bytes", serde_json::json!(b)),
        // DateTime 以 UTC Unix ns 数值暴露，格式化交给前端
        V::DateTime(ns) => ("datetime_ns", serde_json::json!(ns)),
        arrays @ (V::BoolArray(_)
        | V::I32Array(_)
        | V::U32Array(_)
        | V::I64Array(_)
        | V::U64Array(_)
        | V::F32Array(_)
        | V::F64Array(_)
        | V::StringArray(_)
        | V::DateTimeArray(_)) => {
            use mesa_core_types::Value::*;
            let (name, arr): (&str, Vec<serde_json::Value>) = match arrays {
                BoolArray(xs) => ("bool[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                I32Array(xs) => ("i32[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                U32Array(xs) => ("u32[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                I64Array(xs) => ("i64[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                U64Array(xs) => ("u64[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                F32Array(xs) => ("f32[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                F64Array(xs) => ("f64[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                StringArray(xs) => (
                    "string[]",
                    xs.iter().map(|b| serde_json::json!(b)).collect(),
                ),
                DateTimeArray(xs) => (
                    "datetime_ns[]",
                    xs.iter().map(|b| serde_json::json!(b)).collect(),
                ),
                _ => unreachable!("matched above"),
            };
            (name, serde_json::Value::Array(arr))
        }
    };
    ValueJson {
        type_name: type_name.into(),
        value,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DriverInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: String,
    pub launchable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointStatus {
    pub endpoint_id: String,
    pub driver_id: String,
    pub state: String,
    pub detail: String,
    pub revision: u64,
    /// 当前快照注册的点数。
    pub points: usize,
    /// 最近一次成功 Start 的 stream_epoch；0 表示尚未运行过。
    /// Driver 重启恢复后必然变化，是"新数据流已建立"的判据（§10/§17）。
    pub epoch: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatestEntry {
    pub endpoint_id: String,
    pub point_id: u32,
    #[serde(alias = "key")]
    pub point_key: String,
    // 兼容：同时输出 key，避免旧 UI 读取 point_key 为 undefined
    pub key: String,
    /// P1 Point Presentation Metadata：Driver 给的人类可读来源（如 S7
    /// `DB10.DBD20`）。None 即未支持，UI 诚实显示未提供。纯展示，非 identity。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    /// P2 用户展示名（registry 回填，configure 不产生）。None = 未设置，
    /// UI 回落 point_key；改名不改变 point_id/key/label。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(flatten)]
    pub value: ValueJson,
    pub quality: String,
    /// 协议/ Mesa 原因码（§3.7）：GOOD 时 None；BAD 时如 `COMMUNICATION_LOST` / `EW_*`。
    /// 断线场景固定为 `COMMUNICATION_LOST`，保持可测契约。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_code: Option<String>,
    /// 批次时间戳（UTC Unix ns）；断线置 BAD 后保留最后值的旧时间戳，不生成虚假采样。
    pub timestamp_ns: i64,
    /// 值的来源语义（§5.5）：CURRENT / LAST_KNOWN / PLACEHOLDER（REST/UI 区分 Stale/No valid value）
    pub value_origin: String,
    /// 协议原始 SourceTimestamp（UTC ns），Placeholder 时为 None，杜绝 0.0@19:04 伪语义
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_timestamp_ns: Option<i64>,
}

/// P2 后点元数据：(point_key, source_label, display_name)。热路径 apply_batch
/// 一次读锁同时取三者，不新增锁（50K/s 锁争用敏感）。
type PointMeta = (String, Option<String>, Option<String>);

/// 进程级共享快照。锁粒度按用途拆分，REST 读路径互不阻塞；latest/meta 采用 RwLock
/// 以支持高频 DataBatch 写入与 REST 并发读取（§22 50K/s 下 apply_batch 与 latest_all 争用显著）。
pub struct Snapshot {
    drivers: RwLock<Vec<DriverInfo>>,
    endpoints: RwLock<HashMap<String, EndpointStatus>>,
    latest: RwLock<HashMap<(String, u32), LatestEntry>>,
    /// (endpoint_id, point_id) -> (point_key, source_label)，register 时 replace。
    point_meta: RwLock<HashMap<(String, u32), PointMeta>>,
    /// §22 精确计数：自启动以来的 envelope / point_value 总数（单调递增，跨 batch 累加）
    envelopes_total: AtomicU64,
    point_value_total: AtomicU64,
    /// 诊断用：Snapshot apply 阶段的单调延迟样本（环形缓冲 4096，O(1)），用于 p50/p95/p99
    snapshot_apply_latencies_ns: RwLock<VecDeque<u64>>,
    /// P1：IPC/E2E 单调延迟样本（Driver mono_ns → Core 收到时的 wall 差值，>10s 视为不可比丢弃）
    ipc_latencies_ns: RwLock<VecDeque<u64>>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            drivers: RwLock::new(Vec::new()),
            endpoints: RwLock::new(HashMap::new()),
            latest: RwLock::new(HashMap::new()),
            point_meta: RwLock::new(HashMap::new()),
            envelopes_total: AtomicU64::new(0),
            point_value_total: AtomicU64::new(0),
            snapshot_apply_latencies_ns: RwLock::new(VecDeque::with_capacity(4096)),
            ipc_latencies_ns: RwLock::new(VecDeque::with_capacity(4096)),
        }
    }
}

impl Snapshot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_drivers(&self, infos: Vec<DriverInfo>) {
        *self.drivers.write().unwrap() = infos;
    }

    pub fn drivers(&self) -> Vec<DriverInfo> {
        self.drivers.read().unwrap().clone()
    }

    pub fn upsert_endpoint(&self, status: EndpointStatus) {
        self.endpoints
            .write()
            .unwrap()
            .insert(status.endpoint_id.clone(), status);
    }

    pub fn endpoints(&self) -> Vec<EndpointStatus> {
        let mut v: Vec<_> = self.endpoints.read().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.endpoint_id.cmp(&b.endpoint_id));
        v
    }

    pub fn endpoint(&self, id: &str) -> Option<EndpointStatus> {
        self.endpoints.read().unwrap().get(id).cloned()
    }

    /// 记录点元数据（ApplyPointMap 时）：point_id -> (point_key, source_label, display_name)，
    /// 供 latest 输出回填可读键名、来源与展示名；数据类型由值本身携带（ValueJson.type）。
    /// 语义为 replace：先清理该 endpoint 的旧映射，再插入新集合，避免减少任务后旧点残留。
    /// P1：source_label 同步 replace（含 None 清空，与 registry 快照语义一致）；
    /// 已有 LatestEntry 的 key/label 立即同步刷新（不等下一批 DataBatch，
    /// 否则改地址后 Start 失败/离线期间 UI 长期显示旧来源）。
    /// P2：display_name 同步 replace；已有 LatestEntry 的展示名同样立即刷新
    ///（改名后不等下一批即生效）。
    pub fn register_points(&self, endpoint_id: &str, defs: &[mesa_core_types::PointDefinition]) {
        let mut meta = self.point_meta.write().unwrap();
        meta.retain(|(ep, _), _| ep != endpoint_id);
        for d in defs {
            meta.insert(
                (endpoint_id.to_string(), d.point_id),
                (
                    d.point_key.clone(),
                    d.source_label.clone(),
                    d.display_name.clone(),
                ),
            );
        }
        // 同步清理 latest 中已不在新点集的旧点；保留的立即刷新 key/label/name
        let mut latest = self.latest.write().unwrap();
        latest.retain(|(ep, pid), _| ep != endpoint_id || meta.contains_key(&(ep.clone(), *pid)));
        for ((ep, _), entry) in latest.iter_mut() {
            if ep == endpoint_id
                && let Some((k, label, name)) = meta.get(&(ep.clone(), entry.point_id))
            {
                entry.point_key = k.clone();
                entry.key = k.clone();
                entry.source_label = label.clone();
                entry.display_name = name.clone();
            }
        }
    }

    /// 应用一个批次到 LatestValueCache。同点覆盖即"最新值胜出"的 Core 侧体现。
    /// 优化：预先快照 point_meta 的读锁，避免在持有 latest 写锁期间逐点再次加锁，显著降低 50K/s 下的锁争用（原实现为 latest 锁内嵌 keys 锁）。
    /// 同时以单调时钟记录 envelopes/points 计数，供 §22 精确吞吐与延迟百分位使用。
    pub fn apply_batch(&self, batch: &mesa_core_types::DataBatch, endpoint_id: &str) {
        // 业务时间戳为 UTC ns，性能用单调时钟（此处以 apply 时刻的 Instant 采样，非跨进程相减）
        let _mono_ns = std::time::Instant::now().elapsed().as_nanos() as u64; // 占位：实际延迟由 DataSink 单调埋点传入，此处仅计数
        self.envelopes_total.fetch_add(1, Ordering::Relaxed);
        self.point_value_total
            .fetch_add(batch.values.len() as u64, Ordering::Relaxed);
        // 预构建本批次所需的 key+label+name 映射快照（仅一次读锁，不新增锁）。
        let meta_snapshot: HashMap<(String, u32), PointMeta> = {
            let meta = self.point_meta.read().unwrap();
            batch
                .values
                .iter()
                .map(|pv| {
                    let k = (endpoint_id.to_string(), pv.point_id);
                    let v = meta.get(&k).cloned().unwrap_or_default();
                    (k, v)
                })
                .collect()
        };
        let mut latest = self.latest.write().unwrap();
        for pv in &batch.values {
            let k = (endpoint_id.to_string(), pv.point_id);
            let (point_key, source_label, display_name) =
                meta_snapshot.get(&k).cloned().unwrap_or_default();
            // V1.2.1：透传 value_origin + source_timestamp，Placeholder 强制 source=None（已在解码层保证）
            let origin = pv
                .value_origin
                .normalize_unspecified(pv.quality)
                .as_str()
                .to_string();
            let src = if origin == "PLACEHOLDER" {
                None
            } else {
                pv.source_timestamp_ns
            };
            let entry = LatestEntry {
                endpoint_id: endpoint_id.to_string(),
                point_id: pv.point_id,
                point_key: point_key.clone(),
                key: point_key,
                source_label,
                display_name,
                value: value_to_json(&pv.value),
                quality: pv.quality.as_str().to_string(),
                quality_code: pv.quality_code.map(|c| c.to_string()).or_else(|| {
                    if pv.quality == mesa_core_types::Quality::Bad {
                        Some("DEVICE_ERROR".into())
                    } else {
                        None
                    }
                }),
                timestamp_ns: batch.timestamp_ns,
                value_origin: origin,
                source_timestamp_ns: src,
            };
            latest.insert(k, entry);
        }
    }

    /// 记录 Snapshot apply 单调延迟样本，O(1)
    pub fn record_snapshot_apply_latency_ns(&self, ns: u64) {
        let mut v = self.snapshot_apply_latencies_ns.write().unwrap();
        if v.len() >= 4096 {
            v.pop_front();
        }
        v.push_back(ns);
    }

    /// 记录 IPC/E2E 单调延迟样本，O(1)；>10s 视为时钟不可比丢弃由调用方保证
    pub fn record_ipc_latency_ns(&self, ns: u64) {
        let mut v = self.ipc_latencies_ns.write().unwrap();
        if v.len() >= 4096 {
            v.pop_front();
        }
        v.push_back(ns);
    }

    /// 兼容旧命名，实际为 snapshot_apply
    pub fn record_latency_ns(&self, ns: u64) {
        self.record_snapshot_apply_latency_ns(ns)
    }

    pub fn envelopes_total(&self) -> u64 {
        self.envelopes_total.load(Ordering::Relaxed)
    }
    pub fn point_value_total(&self) -> u64 {
        self.point_value_total.load(Ordering::Relaxed)
    }
    pub fn snapshot_apply_latencies_snapshot(&self) -> Vec<u64> {
        self.snapshot_apply_latencies_ns
            .read()
            .unwrap()
            .iter()
            .copied()
            .collect()
    }

    pub fn ipc_latencies_snapshot(&self) -> Vec<u64> {
        self.ipc_latencies_ns
            .read()
            .unwrap()
            .iter()
            .copied()
            .collect()
    }

    /// 兼容旧命名
    pub fn latencies_snapshot(&self) -> Vec<u64> {
        self.snapshot_apply_latencies_snapshot()
    }

    /// 断线标记（§3.11 / §11）：将该 Endpoint 全部已知点置 BAD/COMMUNICATION_LOST，
    /// 保留最后一个 typed value 与原 timestamp，不生成虚假采样（P0-A 冻结契约）。
    /// ValueOrigin 跃迁：PLACEHOLDER 保持 PLACEHOLDER（source=None），其余 CURRENT/LAST_KNOWN → LAST_KNOWN
    pub fn mark_communication_lost(&self, endpoint_id: &str) {
        let mut latest = self.latest.write().unwrap();
        for ((ep, _pid), entry) in latest.iter_mut() {
            if ep == endpoint_id {
                entry.quality = "BAD".into();
                entry.quality_code = Some("COMMUNICATION_LOST".into());
                if entry.value_origin != "PLACEHOLDER" {
                    entry.value_origin = "LAST_KNOWN".into();
                }
                if entry.value_origin == "PLACEHOLDER" {
                    entry.source_timestamp_ns = None;
                }
                // 保留 entry.value（typed last value）与 timestamp_ns 不变，不置 Null
                // 契约：无论之前是 GOOD 还是 BAD/DECODE_FAILED，断线后统一置为 COMMUNICATION_LOST
            }
        }
    }

    pub fn remove_endpoint(&self, endpoint_id: &str) {
        self.point_meta
            .write()
            .unwrap()
            .retain(|(ep, _), _| ep != endpoint_id);
        self.latest
            .write()
            .unwrap()
            .retain(|(ep, _), _| ep != endpoint_id);
        self.endpoints.write().unwrap().remove(endpoint_id);
    }

    /// P2 改名同步（编辑 display_name 后调用）：更新 point_meta 与已有
    /// LatestEntry 的展示名，point_id/key/label 不动，不等下一批即生效。
    /// `name=None` = 清除（回落 point_key）。点不存在时静默无操作
    ///（registry 已先校验活跃点，运行时缺席即尚未 register）。
    pub fn update_display_name(&self, endpoint_id: &str, point_id: u32, name: Option<String>) {
        {
            let mut meta = self.point_meta.write().unwrap();
            if let Some(m) = meta.get_mut(&(endpoint_id.to_string(), point_id)) {
                m.2 = name.clone();
            }
        }
        let mut latest = self.latest.write().unwrap();
        if let Some(e) = latest.get_mut(&(endpoint_id.to_string(), point_id)) {
            e.display_name = name;
        }
    }

    pub fn latest_all(&self) -> Vec<LatestEntry> {
        let mut v: Vec<_> = self.latest.read().unwrap().values().cloned().collect();
        v.sort_by(|a, b| {
            a.endpoint_id
                .cmp(&b.endpoint_id)
                .then(a.point_id.cmp(&b.point_id))
        });
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{
        DataBatch, DataType, PointDefinition, PointValue, Quality, Value, ValueOrigin,
    };

    #[test]
    fn placeholder_visible_in_rest_and_no_source_timestamp() {
        let snap = Snapshot::new();
        snap.register_points(
            "ep1",
            &[PointDefinition {
                point_id: 1,
                point_key: "k1".into(),
                data_type: DataType::F64,
                unit: None,
                source_label: None,
                display_name: None,
            }],
        );
        let batch = DataBatch {
            connection_handle: 1,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: 1_000_000,
            values: vec![PointValue {
                point_id: 1,
                value: Value::F64(0.0),
                quality: Quality::Bad,
                quality_code: Some(0x80340000u32 as i32),
                source_timestamp_ns: Some(999_999),
                value_origin: ValueOrigin::Placeholder,
            }],
            mono_ns: None,
        };
        snap.apply_batch(&batch, "ep1");
        let e = &snap.latest_all()[0];
        assert_eq!(e.value_origin, "PLACEHOLDER");
        assert_eq!(e.source_timestamp_ns, None, "Placeholder 必须无 source");
        assert_eq!(e.quality, "BAD");
        // REST JSON 必须包含 value_origin
        let json = serde_json::to_value(e).unwrap();
        assert_eq!(json["value_origin"], "PLACEHOLDER");
    }

    #[test]
    fn last_known_rest_and_communication_lost_transition() {
        let snap = Snapshot::new();
        snap.register_points(
            "ep1",
            &[PointDefinition {
                point_id: 1,
                point_key: "k1".into(),
                data_type: DataType::F64,
                unit: None,
                source_label: None,
                display_name: None,
            }],
        );
        let pv_current = PointValue {
            point_id: 1,
            value: Value::F64(12.5),
            quality: Quality::Good,
            quality_code: None,
            source_timestamp_ns: Some(1_000),
            value_origin: ValueOrigin::Current,
        };
        snap.apply_batch(
            &DataBatch {
                connection_handle: 1,
                stream_epoch: 1,
                sequence: 1,
                timestamp_ns: 1_000_000,
                values: vec![pv_current],
                mono_ns: None,
            },
            "ep1",
        );
        let pv_last = PointValue {
            point_id: 1,
            value: Value::F64(12.5),
            quality: Quality::Bad,
            quality_code: Some(1),
            source_timestamp_ns: Some(1_000),
            value_origin: ValueOrigin::LastKnown,
        };
        snap.apply_batch(
            &DataBatch {
                connection_handle: 1,
                stream_epoch: 1,
                sequence: 2,
                timestamp_ns: 2_000_000,
                values: vec![pv_last],
                mono_ns: None,
            },
            "ep1",
        );
        let e = snap.latest_all()[0].clone();
        assert_eq!(e.value_origin, "LAST_KNOWN");
        assert_eq!(e.source_timestamp_ns, Some(1_000));
        snap.mark_communication_lost("ep1");
        let e2 = &snap.latest_all()[0];
        assert_eq!(e2.quality, "BAD");
        assert_eq!(e2.quality_code.as_deref(), Some("COMMUNICATION_LOST"));
        assert_eq!(e2.value_origin, "LAST_KNOWN");
        assert_eq!(e2.source_timestamp_ns, Some(1_000));
        // Placeholder 断线保持 Placeholder
        let snap2 = Snapshot::new();
        snap2.register_points(
            "ep2",
            &[PointDefinition {
                point_id: 2,
                point_key: "k2".into(),
                data_type: DataType::I32,
                unit: None,
                source_label: None,
                display_name: None,
            }],
        );
        snap2.apply_batch(
            &DataBatch {
                connection_handle: 1,
                stream_epoch: 1,
                sequence: 1,
                timestamp_ns: 1_000_000,
                values: vec![PointValue {
                    point_id: 2,
                    value: Value::I32(0),
                    quality: Quality::Bad,
                    quality_code: Some(1),
                    source_timestamp_ns: Some(123),
                    value_origin: ValueOrigin::Placeholder,
                }],
                mono_ns: None,
            },
            "ep2",
        );
        snap2.mark_communication_lost("ep2");
        let e3 = &snap2.latest_all()[0];
        assert_eq!(e3.value_origin, "PLACEHOLDER");
        assert_eq!(e3.source_timestamp_ns, None);
    }

    #[test]
    fn source_label_flows_from_register_to_latest() {
        // P1：register 的 source_label 经 apply_batch 透出到 LatestEntry，
        // 未注册 label 的点为 None（UI 回落技术坐标）。
        let snap = Snapshot::new();
        snap.register_points(
            "ep1",
            &[
                PointDefinition {
                    point_id: 1,
                    point_key: "k1".into(),
                    data_type: DataType::F64,
                    unit: None,
                    source_label: Some("DB10.DBD20".into()),
                    display_name: None,
                },
                PointDefinition {
                    point_id: 2,
                    point_key: "k2".into(),
                    data_type: DataType::F64,
                    unit: None,
                    source_label: None,
                    display_name: None,
                },
            ],
        );
        snap.apply_batch(
            &DataBatch {
                connection_handle: 1,
                stream_epoch: 1,
                sequence: 1,
                timestamp_ns: 1_000_000,
                values: vec![
                    PointValue {
                        point_id: 1,
                        value: Value::F64(1.0),
                        quality: Quality::Good,
                        quality_code: None,
                        source_timestamp_ns: None,
                        value_origin: ValueOrigin::Current,
                    },
                    PointValue {
                        point_id: 2,
                        value: Value::F64(2.0),
                        quality: Quality::Good,
                        quality_code: None,
                        source_timestamp_ns: None,
                        value_origin: ValueOrigin::Current,
                    },
                ],
                mono_ns: None,
            },
            "ep1",
        );
        let all = snap.latest_all();
        let e1 = all.iter().find(|e| e.point_id == 1).unwrap();
        let e2 = all.iter().find(|e| e.point_id == 2).unwrap();
        assert_eq!(e1.source_label.as_deref(), Some("DB10.DBD20"));
        assert_eq!(e2.source_label, None);
        let json = serde_json::to_value(e1).unwrap();
        assert_eq!(json["source_label"], "DB10.DBD20");
    }

    #[test]
    fn register_refreshes_existing_latest_labels_without_new_batch() {
        // 收2：改地址后已有 LatestEntry 必须立即同步（不等下一批），
        // 含 Some 覆盖与 None 清空。
        use mesa_core_types::PointDefinition;
        let snap = Snapshot::new();
        let def = |label: Option<&str>| {
            vec![PointDefinition {
                point_id: 1,
                point_key: "motor.speed".into(),
                data_type: DataType::F64,
                unit: None,
                source_label: label.map(|s| s.to_string()),
                display_name: None,
            }]
        };
        let batch = || DataBatch {
            connection_handle: 1,
            stream_epoch: 1,
            sequence: 1,
            timestamp_ns: 1_000_000,
            values: vec![PointValue {
                point_id: 1,
                value: Value::F64(1500.0),
                quality: Quality::Good,
                quality_code: None,
                source_timestamp_ns: None,
                value_origin: ValueOrigin::Current,
            }],
            mono_ns: None,
        };
        snap.register_points("ep1", &def(Some("DB10.DBD20")));
        snap.apply_batch(&batch(), "ep1");
        assert_eq!(
            snap.latest_all()[0].source_label.as_deref(),
            Some("DB10.DBD20")
        );
        // 改地址：无新 batch，latest 立即 = 新 label
        snap.register_points("ep1", &def(Some("DB20.DBD40")));
        assert_eq!(
            snap.latest_all()[0].source_label.as_deref(),
            Some("DB20.DBD40")
        );
        // Driver 回 None：立即清空
        snap.register_points("ep1", &def(None));
        assert_eq!(snap.latest_all()[0].source_label, None);
        // 值本身不受影响
        assert_eq!(snap.latest_all()[0].value.value, serde_json::json!(1500.0));
    }
}
