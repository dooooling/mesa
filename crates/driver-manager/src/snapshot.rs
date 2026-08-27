//! Core 进程内共享状态：驱动清单、Endpoint 状态与最新值缓存。
//!
//! M0 阶段全部驻留内存；SQLite 持久化（ConfigStore）在 Phase 1 引入，
//! 届时 LatestValueCache 仍保持内存形态——热路径不落盘。

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;

/// Value 的 JSON 视图：带类型标签，避免 REST 消费方猜测数值含义。
#[derive(Debug, Clone, Serialize)]
pub struct ValueJson {
    #[serde(rename = "type")]
    pub type_name: String,
    pub value: serde_json::Value,
}

fn value_to_json(v: &forgelink_core_types::Value) -> ValueJson {
    use forgelink_core_types::Value as V;
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
        | V::DateTimeArray(_)) =>
        {
            use forgelink_core_types::Value::*;
            let (name, arr): (&str, Vec<serde_json::Value>) = match arrays {
                BoolArray(xs) => ("bool[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                I32Array(xs) => ("i32[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                U32Array(xs) => ("u32[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                I64Array(xs) => ("i64[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                U64Array(xs) => ("u64[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                F32Array(xs) => ("f32[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                F64Array(xs) => ("f64[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                StringArray(xs) => ("string[]", xs.iter().map(|b| serde_json::json!(b)).collect()),
                DateTimeArray(xs) => {
                    ("datetime_ns[]", xs.iter().map(|b| serde_json::json!(b)).collect())
                }
                _ => unreachable!("matched above"),
            };
            (name, serde_json::Value::Array(arr))
        }
    };
    ValueJson { type_name: type_name.into(), value }
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
    pub key: String,
    #[serde(flatten)]
    pub value: ValueJson,
    pub quality: String,
    /// 批次时间戳（UTC Unix ns）；断线置 BAD 后保留最后值的旧时间戳。
    pub timestamp_ns: i64,
}

/// 进程级共享快照。锁粒度按用途拆分，REST 读路径互不阻塞。
#[derive(Default)]
pub struct Snapshot {
    drivers: Mutex<Vec<DriverInfo>>,
    endpoints: Mutex<HashMap<String, EndpointStatus>>,
    latest: Mutex<HashMap<(String, u32), LatestEntry>>,
    /// point_key -> data_type，用于 latest 输出补全类型信息。
    keys: Mutex<HashMap<(String, u32), String>>,
}

impl Snapshot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_drivers(&self, infos: Vec<DriverInfo>) {
        *self.drivers.lock().unwrap() = infos;
    }

    pub fn drivers(&self) -> Vec<DriverInfo> {
        self.drivers.lock().unwrap().clone()
    }

    pub fn upsert_endpoint(&self, status: EndpointStatus) {
        self.endpoints.lock().unwrap().insert(status.endpoint_id.clone(), status);
    }

    pub fn endpoints(&self) -> Vec<EndpointStatus> {
        let mut v: Vec<_> = self.endpoints.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.endpoint_id.cmp(&b.endpoint_id));
        v
    }

    pub fn endpoint(&self, id: &str) -> Option<EndpointStatus> {
        self.endpoints.lock().unwrap().get(id).cloned()
    }

    /// 记录点元数据（ApplyPointMap 时）：point_id -> point_key，
    /// 供 latest 输出回填可读键名；数据类型由值本身携带（ValueJson.type）。
    pub fn register_points(
        &self,
        endpoint_id: &str,
        defs: &[forgelink_core_types::PointDefinition],
    ) {
        let mut keys = self.keys.lock().unwrap();
        for d in defs {
            keys.insert((endpoint_id.to_string(), d.point_id), d.point_key.clone());
        }
    }

    /// 应用一个批次到 LatestValueCache。同点覆盖即"最新值胜出"的 Core 侧体现。
    pub fn apply_batch(&self, batch: &forgelink_core_types::DataBatch, endpoint_id: &str) {
        let mut latest = self.latest.lock().unwrap();
        for pv in &batch.values {
            let entry = LatestEntry {
                endpoint_id: endpoint_id.to_string(),
                point_id: pv.point_id,
                key: self
                    .keys
                    .lock()
                    .unwrap()
                    .get(&(endpoint_id.to_string(), pv.point_id))
                    .cloned()
                    .unwrap_or_default(),
                value: value_to_json(&pv.value),
                quality: pv.quality.as_str().to_string(),
                timestamp_ns: batch.timestamp_ns,
            };
            latest.insert((endpoint_id.to_string(), pv.point_id), entry);
        }
    }

    /// 断线标记（§11）：将该 Endpoint 全部已知点置 BAD/COMMUNICATION_LOST，
    /// 不生成虚假采样、不改动原值。
    pub fn mark_communication_lost(&self, endpoint_id: &str) {
        let mut latest = self.latest.lock().unwrap();
        for ((ep, _pid), entry) in latest.iter_mut() {
            if ep == endpoint_id && entry.quality != "BAD" {
                entry.quality = "BAD".into();
                entry.value.value = serde_json::Value::Null;
            }
        }
    }

    pub fn latest_all(&self) -> Vec<LatestEntry> {
        let mut v: Vec<_> = self.latest.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| {
            a.endpoint_id.cmp(&b.endpoint_id).then(a.point_id.cmp(&b.point_id))
        });
        v
    }
}
