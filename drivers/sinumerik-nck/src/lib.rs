//! SINUMERIK NCK Driver — 原生 NCK 只读（ADR 0001，Commit E 完成 V1 地基）。
//!
//! 架构位置：
//! ```text
//! sinumerik-nck（本 crate：NCK 设备语义，variable 资源）
//!       ↓ 不透明 var_spec（0x82/83/84，见 codec）
//! mesa-s7-transport（S7Comm 会话/ReadVar/分片）
//! ```
//!
//! - Core 只认识 Descriptor / ProbeReport / ResourceSelection / DataBatch，
//!   无任何 `driver_id == "sinumerik-nck"` 分支；
//! - V1 严格只读：`write`/`command` 沿用 SDK 默认 Unsupported；事件目录为空；
//! - probe 诚实语义：会话可达≠身份确认（family 待 anchor，真机 PR7）。

mod address;
mod browse;
mod catalog;
mod client;
mod codec;
mod config;
mod fixture;
mod probe;
mod topology;
mod value;

pub use address::{AddressError, NckArea, NckUnitMode, NckVariableRef};
pub use browse::build_tree;
pub use catalog::{CatalogError, NckCatalog, NckShape, NckVariableDefinition, NckWireDefinition};
pub use client::{NckClient, NckReadItem, NckReadResult};
pub use codec::{
    CodecError, NckWireAddress, ResolvedVariable, encode_var_spec, resolve as resolve_wire,
};
pub use config::{NCK_DEFAULT_PORT, NckConnConfig};
pub use fixture::{NckFixture, NckFixtureState};
pub use probe::{NCK_ANCHOR_PENDING, probe_with_session};
pub use topology::{NckAxis, NckChannel, NckTopology, axis_path, channel_path};
pub use value::{NckDataKind, NckSample, ValueError, decode_value};

use mesa_core_types::{AcquisitionTask, DriverMetadata, PointDescriptor, PointMap};
use mesa_driver_sdk::{Driver, DriverConnection, SdkDriverError};

/// 驱动 ID（Core 侧无分支；仅 Descriptor identity 与 driver.toml 声明）。
pub const DRIVER_ID: &str = "sinumerik-nck";
/// 通用轮询绑定（`mesa.resources.v1`，resource_id `variable`）。
pub const GENERIC_RESOURCE_ID: &str = "variable";

#[derive(Default)]
pub struct SinumerikNckDriver;

#[async_trait::async_trait]
impl Driver for SinumerikNckDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            driver_id: DRIVER_ID.into(),
            name: "SINUMERIK NCK".into(),
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
                        .default_value(serde_json::json!(102)),
                    // TSAP 显式必填（NCK 无 rack/slot 推导；远端值由 profile + 真机 Gate 定）。
                    FieldDescriptor::new("local_tsap", "Local TSAP", FieldType::Integer)
                        .required(true),
                    FieldDescriptor::new("remote_tsap", "Remote TSAP", FieldType::Integer)
                        .required(true),
                    FieldDescriptor::new("timeout_ms", "Timeout ms", FieldType::Duration)
                        .required(false)
                        .default_value(serde_json::json!(5000)),
                    FieldDescriptor::new("pdu_length", "PDU length", FieldType::Integer)
                        .required(false)
                        .default_value(serde_json::json!(480)),
                ],
            },
            resources: vec![ResourceDescriptor {
                id: GENERIC_RESOURCE_ID.into(),
                label: LocalizedText::new("NC Variable"),
                parameters: SchemaDescriptor {
                    fields: vec![
                        {
                            let mut f = FieldDescriptor::new("area", "Area", FieldType::Enum)
                                .required(true)
                                .default_value(serde_json::json!("C"));
                            f.validation.enum_options = Some(vec![
                                "N".into(),
                                "B".into(),
                                "C".into(),
                                "A".into(),
                                "T".into(),
                                "V".into(),
                                "H".into(),
                            ]);
                            f
                        },
                        FieldDescriptor::new("area_no", "Area No.", FieldType::Integer)
                            .required(false),
                        FieldDescriptor::new("block", "Block", FieldType::String).required(true),
                        FieldDescriptor::new("variable", "Variable", FieldType::String)
                            .required(true),
                        FieldDescriptor::new("line", "Line", FieldType::Integer).required(false),
                        FieldDescriptor::new("column", "Column", FieldType::Integer)
                            .required(false),
                        FieldDescriptor::new("count", "Count", FieldType::Integer)
                            .required(false)
                            .default_value(serde_json::json!(1)),
                        {
                            let mut f =
                                FieldDescriptor::new("unit_mode", "Unit mode", FieldType::Enum)
                                    .required(false)
                                    .default_value(serde_json::json!("current"));
                            f.validation.enum_options =
                                Some(vec!["current".into(), "metric".into(), "inch".into()]);
                            f
                        },
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
            }],
            controls: mesa_core_types::ControlCatalog::default(),
            discovery: DiscoveryCapabilities {
                manual: true,
                // Commit E：Catalog 虚拟树就位（空 catalog 即空根，不伪造内容）。
                browse: true,
                import: false,
            },
            capabilities: DriverCapabilities {
                poll: true,
                // 不伪造 Subscribe（ReadVar 本质是请求/响应）。
                subscribe: false,
                browse: true,
                ..Default::default()
            },
            // Event Plane：NCK V1 无事件目录即 empty（Major 不升级）。
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
        let cfg = NckConnConfig::from_json(&v)?;
        let catalog = NckCatalog::load_shipped().map_err(|e| {
            SdkDriverError::configuration("BAD_CONFIG", format!("NCK catalog 装载失败: {e}"))
        })?;
        Ok(Box::new(NckConnection {
            cfg,
            catalog,
            topology: None,
            plan: None,
        }))
    }
}

#[derive(Debug, Clone)]
struct PointSpec {
    key: String,
    /// 用户语义回显（canonical key 进诊断）。
    var_ref: NckVariableRef,
    wire: NckWireAddress,
    kind: NckDataKind,
    expected_len: usize,
    /// 响应 transport 期望（P1-4 逐项校验）。
    expected_transport_size: u8,
    data_type: mesa_core_types::DataType,
    unit: Option<String>,
}

#[derive(Debug, Clone)]
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

pub struct NckConnection {
    cfg: NckConnConfig,
    catalog: NckCatalog,
    /// 拓扑快照（probe 回填；V1 恒 None，browse 退化为纯 Catalog 树）。
    topology: Option<NckTopology>,
    plan: Option<PlanSnapshot>,
}

impl NckConnection {
    /// 测试/现场覆盖 catalog 注入（随仓 catalog 为空，真机回填前用合成目录验证数据面）。
    pub fn with_catalog(cfg: NckConnConfig, catalog: NckCatalog) -> Self {
        Self {
            cfg,
            catalog,
            topology: None,
            plan: None,
        }
    }
}

/// 单点解码：GOOD + 足长 → Current（更新 last_known）；BAD/短包 → 有缓存
/// LastKnown、无缓存 Placeholder；quality_code 携带 NCK 返回码（解码失败为 None）。
/// `source_timestamp_ns` 恒 None（协议无此语义，不伪造）。
fn decode_point(
    spec: &PointSpec,
    point_id: u32,
    raw: Option<&[u8]>,
    return_code: Option<u8>,
    last_known: &mut std::collections::HashMap<u32, mesa_core_types::Value>,
) -> mesa_core_types::PointValue {
    use mesa_core_types::{PointValue, Quality, Value, ValueOrigin};
    let good_value: Option<Value> = raw.and_then(|bytes| {
        let elem = spec.kind.byte_len();
        let count = spec.expected_len / elem;
        // P1-4：client 已保证 exact，此处再收一道（exact，不猜不补）。
        if bytes.len() != spec.expected_len || count == 0 {
            tracing::warn!(key = %spec.key, canonical = %spec.var_ref.canonical_key(), got = bytes.len(), need = spec.expected_len, "NCK 数据长度不符，按 BAD 隔离");
            return None;
        }
        let mut elems = Vec::with_capacity(count);
        for i in 0..count {
            match decode_value(&bytes[i * elem..(i + 1) * elem], spec.kind) {
                Ok(v) => elems.push(v),
                Err(e) => {
                    tracing::warn!(key = %spec.key, canonical = %spec.var_ref.canonical_key(), error = %e, "NCK 元素解码失败，按 BAD 隔离");
                    return None;
                }
            }
        }
        Some(if count == 1 {
            elems.into_iter().next().expect("count=1 必有一元素")
        } else {
            pack_array(spec.kind, elems)
        })
    });
    match good_value {
        Some(v) => {
            last_known.insert(point_id, v.clone());
            PointValue {
                point_id,
                value: v,
                quality: Quality::Good,
                quality_code: None,
                source_timestamp_ns: None,
                value_origin: ValueOrigin::Current,
            }
        }
        None => {
            let (val, origin) = match last_known.get(&point_id) {
                Some(cached) => (cached.clone(), ValueOrigin::LastKnown),
                None => (
                    Value::typed_placeholder(spec.data_type),
                    ValueOrigin::Placeholder,
                ),
            };
            PointValue {
                point_id,
                value: val,
                quality: Quality::Bad,
                quality_code: return_code.map(|c| c as i32),
                source_timestamp_ns: None,
                value_origin: origin,
            }
        }
    }
}

/// 多元素打包为 typed array（count=1 永不进此分支）。
fn pack_array(kind: NckDataKind, elems: Vec<mesa_core_types::Value>) -> mesa_core_types::Value {
    use mesa_core_types::Value;
    match kind {
        NckDataKind::F64 => Value::F64Array(
            elems
                .into_iter()
                .map(|v| match v {
                    Value::F64(x) => x,
                    _ => f64::NAN,
                })
                .collect(),
        ),
        NckDataKind::F32 => Value::F32Array(
            elems
                .into_iter()
                .map(|v| match v {
                    Value::F32(x) => x,
                    _ => f32::NAN,
                })
                .collect(),
        ),
        NckDataKind::I32 => Value::I32Array(
            elems
                .into_iter()
                .map(|v| match v {
                    Value::I32(x) => x,
                    _ => 0,
                })
                .collect(),
        ),
        NckDataKind::U32 => Value::U32Array(
            elems
                .into_iter()
                .map(|v| match v {
                    Value::U32(x) => x,
                    _ => 0,
                })
                .collect(),
        ),
        NckDataKind::Bool => Value::BoolArray(
            elems
                .into_iter()
                .map(|v| matches!(v, Value::Bool(true)))
                .collect(),
        ),
    }
}

#[async_trait::async_trait]
impl DriverConnection for NckConnection {
    /// 探测：S7Comm 会话可达性（reachable），身份恒待确认
    /// （family/model 为 None + `NCK_ANCHOR_PENDING`，见 probe）。
    async fn probe(&mut self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        Ok(probe_with_session(&self.cfg).await)
    }

    /// 通用绑定 `mesa.resources.v1`（resource_id `variable`）→ Catalog →
    /// PointDescriptor。未知变量/非法参数即配置拒绝（fail-closed，不进运行期）。
    async fn configure(
        &mut self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        use mesa_core_types::{
            DuplicatePointKey, GENERIC_BINDING_KIND, GenericBinding, TaskMode,
            ensure_unique_point_keys, validate_selections_structure,
        };
        let mut new_points: Vec<PointSpec> = Vec::new();
        let mut new_tasks: Vec<TaskPlan> = Vec::new();
        for task in &tasks {
            task.validate()
                .map_err(|e| SdkDriverError::configuration("INVALID_TASK", e.to_string()))?;
            if task.binding.kind != GENERIC_BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: sinumerik-nck 只接受 {GENERIC_BINDING_KIND}",
                        task.id
                    ),
                ));
            }
            let binding: GenericBinding = serde_json::from_value(task.binding.config.clone())
                .map_err(|e| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: invalid generic binding: {e}", task.id),
                    )
                })?;
            validate_selections_structure(&binding.selections)
                .map_err(|e| SdkDriverError::configuration("INVALID_BINDING_CONFIG", e))?;
            if task.mode != TaskMode::Poll {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!(
                        "task `{}`: sinumerik-nck 只支持 poll（不伪造 subscribe）",
                        task.id
                    ),
                ));
            }
            let mut indices = Vec::new();
            for sel in &binding.selections {
                if sel.resource_id != GENERIC_RESOURCE_ID {
                    return Err(SdkDriverError::configuration(
                        "UNSUPPORTED_RESOURCE",
                        format!("task `{}`: sinumerik-nck 只支持 variable", task.id),
                    ));
                }
                for out in &sel.outputs {
                    let params = sel.parameters.as_object().ok_or_else(|| {
                        SdkDriverError::configuration(
                            "INVALID_VARIABLE",
                            format!("point `{}`: parameters 需为对象", out.point_key),
                        )
                    })?;
                    let var_ref = NckVariableRef::from_parameters(params).map_err(|e| {
                        SdkDriverError::new(
                            mesa_core_types::ErrorKind::Address,
                            "INVALID_VARIABLE",
                            format!("point `{}`: {e}", out.point_key),
                        )
                    })?;
                    let def = self
                        .catalog
                        .lookup(var_ref.area, &var_ref.block, &var_ref.variable)
                        .map_err(|e| {
                            SdkDriverError::new(
                                mesa_core_types::ErrorKind::Address,
                                "INVALID_VARIABLE",
                                format!("point `{}`: {e}", out.point_key),
                            )
                        })?;
                    let rv = resolve_wire(&var_ref, def).map_err(|e| {
                        SdkDriverError::new(
                            mesa_core_types::ErrorKind::Address,
                            "INVALID_VARIABLE",
                            format!("point `{}`: {e}", out.point_key),
                        )
                    })?;
                    indices.push(new_points.len());
                    tracing::debug!(
                        key = %out.point_key,
                        canonical = %var_ref.canonical_key(),
                        "NCK 点位已解析",
                    );
                    new_points.push(PointSpec {
                        key: out.point_key.clone(),
                        var_ref,
                        wire: rv.wire,
                        kind: rv.kind,
                        expected_len: rv.expected_data_len,
                        expected_transport_size: rv.expected_transport_size,
                        data_type: rv.kind.core_type(),
                        unit: def.unit.clone(),
                    });
                }
            }
            let interval = task.interval_ms.ok_or_else(|| {
                SdkDriverError::configuration(
                    "INVALID_TASK",
                    format!("task `{}`: Poll 模式必须提供 interval_ms", task.id),
                )
            })?;
            if interval == 0 {
                return Err(SdkDriverError::configuration(
                    "INVALID_TASK",
                    format!("task `{}`: interval_ms 需 >0", task.id),
                ));
            }
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
                unit: p.unit.clone(),
            })
            .collect();
        ensure_unique_point_keys(&descriptors).map_err(|DuplicatePointKey(k)| {
            SdkDriverError::configuration("DUPLICATE_POINT_KEY", format!("`{k}` 重复"))
        })?;
        tracing::info!(
            revision,
            points = new_points.len(),
            tasks = new_tasks.len(),
            "NCK 采集计划构建完成"
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

    /// Poll 数据面：建会话 → 每任务独立 ticker → MultiRead → 解码发布。
    /// 会话/整包失败即 Err（fail 当前 attempt，Manager 重建）；单点 BAD 隔离。
    async fn run(
        &mut self,
        sink: mesa_driver_sdk::DataSink,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<(), SdkDriverError> {
        use mesa_core_types::{DataBatch, PointValue, Value, now_unix_ns};
        use std::sync::atomic::{AtomicU64, Ordering};
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
        let client = NckClient::connect(&self.cfg).await.map_err(|e| {
            tracing::error!(error = %e, "NCK 连接失败");
            e
        })?;
        tracing::info!(
            revision = snap.revision,
            points = snap.points.len(),
            tasks = snap.tasks.len(),
            "NCK 数据面启动",
        );
        let client = std::sync::Arc::new(tokio::sync::Mutex::new(client));
        let seq = std::sync::Arc::new(AtomicU64::new(1));
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
            let seq = std::sync::Arc::clone(&seq);
            let client = std::sync::Arc::clone(&client);
            let interval = std::time::Duration::from_millis(task.interval_ms);
            let task_id = task.id.clone();
            handles.push(tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut last_known: std::collections::HashMap<u32, Value> =
                    std::collections::HashMap::new();
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {},
                        _ = shutdown.cancelled() => break,
                    }
                    let items: Vec<NckReadItem> = points
                        .iter()
                        .map(|(spec, _)| NckReadItem {
                            wire: spec.wire,
                            expected_data_len: spec.expected_len,
                            expected_transport_size: spec.expected_transport_size,
                        })
                        .collect();
                    let results = {
                        let mut guard = client.lock().await;
                        match guard.read_vars(&items).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!(task = %task_id, error = %e, "NCK 批量读失败");
                                return Err(e);
                            }
                        }
                    };
                    let values: Vec<PointValue> = points
                        .iter()
                        .zip(results.iter())
                        .map(|((spec, pid), r)| {
                            decode_point(
                                spec,
                                *pid,
                                r.data.as_deref(),
                                Some(r.return_code),
                                &mut last_known,
                            )
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
                        mono_ns: None,
                    })
                    .await;
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
                    tracing::error!(%join_err, "NCK 任务 panic");
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

    /// 浏览：Catalog 虚拟树（纯函数，无需会话；topology 由 probe 回填，
    /// V1 为空即纯 Catalog 树）。未知 parent → 空页。
    async fn browse(
        &mut self,
        parent: &str,
        filter: &str,
        cursor: &str,
        limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), SdkDriverError> {
        Ok(build_tree(
            &self.catalog,
            self.topology.as_ref(),
            parent,
            filter,
            cursor,
            limit,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_valid_and_read_only() {
        let d = SinumerikNckDriver.descriptor();
        d.validate().expect("sinumerik-nck descriptor 必须合法");
        assert_eq!(d.identity.driver_id, "sinumerik-nck");
        assert!(d.capabilities.poll, "NCK V1 必须 poll");
        assert!(!d.capabilities.subscribe, "NCK 不伪造 subscribe");
        assert!(d.capabilities.browse, "NCK browse（Catalog 树）已就位");
        assert!(d.discovery.browse, "discovery browse 已就位");
        assert!(!d.capabilities.write, "NCK V1 只读");
        assert!(!d.capabilities.method, "NCK V1 无 command");
        assert!(!d.capabilities.events, "NCK V1 无事件");
        // variable 资源参数与 address.rs 口径一致。
        let res = d
            .resources
            .iter()
            .find(|r| r.id == "variable")
            .expect("variable 资源");
        let keys: Vec<_> = res
            .parameters
            .fields
            .iter()
            .map(|f| f.key.as_str())
            .collect();
        for k in [
            "area",
            "area_no",
            "block",
            "variable",
            "line",
            "column",
            "count",
            "unit_mode",
        ] {
            assert!(keys.contains(&k), "缺参数 {k}");
        }
    }

    #[tokio::test]
    async fn open_connection_validates_config() {
        async fn err_of(d: &SinumerikNckDriver, json: &str) -> SdkDriverError {
            match d.open_connection("t", json).await {
                Ok(_) => panic!("非法配置必须拒绝: {json}"),
                Err(e) => e,
            }
        }
        let d = SinumerikNckDriver;
        assert_eq!(err_of(&d, "not-json").await.code, "BAD_CONFIG");
        // 缺 TSAP 拒绝。
        assert_eq!(
            err_of(&d, r#"{"host":"10.0.0.5"}"#).await.code,
            "BAD_CONFIG"
        );
        let ok = d
            .open_connection(
                "t",
                r#"{"host":"10.0.0.5","local_tsap":256,"remote_tsap":258}"#,
            )
            .await;
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn probe_honest_and_browse_empty_catalog() {
        // 空 catalog：browse 根为空页（不伪造内容）；probe 可达但身份待确认。
        let mut conn = NckConnection::with_catalog(
            NckConnConfig::from_json(&serde_json::json!({
                "host": "10.0.0.5", "local_tsap": 256, "remote_tsap": 258,
            }))
            .unwrap(),
            NckCatalog::empty(),
        );
        let (nodes, next) = conn.browse("", "", "", 0).await.unwrap();
        assert!(nodes.is_empty());
        assert!(next.is_none());
        // probe 打不可达桩（10.0.0.5 无真机）→ 不可达规范报告。
        let report = conn.probe().await.unwrap();
        assert!(!report.reachable, "无真机时 probe 必须不可达");
    }

    #[tokio::test]
    async fn browse_synthetic_tree_and_probe_reachable() {
        // 合成 catalog：browse 树可导航；fixture 可达且身份待确认。
        let fx = NckFixture::spawn(NckFixtureState::default()).await;
        let mut conn = NckConnection::with_catalog(
            NckConnConfig::from_json(&serde_json::json!({
                "host": "127.0.0.1", "local_tsap": 256, "remote_tsap": 258,
            }))
            .unwrap(),
            synthetic_catalog(),
        );
        conn.cfg.port = fx.addr.port();
        let (root, _) = conn.browse("", "", "", 0).await.unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].id, "nck://C");
        let (leaves, _) = conn.browse("nck://C/SEMA", "", "", 0).await.unwrap();
        assert_eq!(leaves.len(), 1);
        assert!(leaves[0].id.starts_with("nck://C/"));
        let report = conn.probe().await.unwrap();
        assert!(report.reachable);
        assert!(report.family.is_none(), "无 anchor 不得断言 family");
        assert!(report.warnings.iter().any(|w| w.code == NCK_ANCHOR_PENDING));
    }

    /// 合成 catalog（脚手架数值，非 Siemens 语义； wired module/column 仅测机器）。
    fn synthetic_catalog() -> NckCatalog {
        NckCatalog::from_json(&serde_json::json!({"variables": [
            {"area": "C", "block": "SEMA", "variable": "actFeedRate",
             "data_type": "F64", "shape": "lines", "unit": "mm/min",
             "wire": {"module": 18, "column": 42, "transport_size": 4, "element_size": 8},
             "supported_families": ["test"]},
        ]}))
        .expect("合成 catalog 合法")
    }

    fn conn_with_synthetic() -> NckConnection {
        NckConnection::with_catalog(
            NckConnConfig::from_json(&serde_json::json!({
                "host": "127.0.0.1", "local_tsap": 256, "remote_tsap": 258,
            }))
            .unwrap(),
            synthetic_catalog(),
        )
    }

    fn poll_task(id: &str, selections: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: id.into(),
            mode: mesa_core_types::TaskMode::Poll,
            interval_ms: Some(50),
            binding: mesa_core_types::DriverBinding {
                kind: mesa_core_types::GENERIC_BINDING_KIND.into(),
                config: serde_json::json!({"selections": selections}),
            },
        }
    }

    fn speed_selection(point_key: &str) -> serde_json::Value {
        serde_json::json!([{
            "resource_id": "variable",
            "parameters": {
                "area": "C", "area_no": 1, "block": "SEMA",
                "variable": "actFeedRate", "line": 3,
            },
            "outputs": [{"output": "value", "point_key": point_key}],
        }])
    }

    #[tokio::test]
    async fn configure_ok_and_rejects() {
        let mut conn = conn_with_synthetic();
        // 合法：descriptor 类型 F64 + unit 回填。
        let descs = conn
            .configure(1, vec![poll_task("t1", speed_selection("axis3.speed"))])
            .await
            .unwrap();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].point_key, "axis3.speed");
        assert_eq!(descs[0].data_type, mesa_core_types::DataType::F64);
        assert_eq!(descs[0].unit.as_deref(), Some("mm/min"));

        // 未知变量 fail-closed。
        let mut bad_sel = speed_selection("k");
        bad_sel[0]["parameters"]["variable"] = serde_json::json!("noSuchVar");
        let err = conn
            .configure(2, vec![poll_task("t2", bad_sel)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_VARIABLE");

        // 非 Poll 模式拒绝（不伪造 subscribe）。
        let mut t = poll_task("t3", speed_selection("k"));
        t.mode = mesa_core_types::TaskMode::Subscribe;
        t.interval_ms = None;
        let err = conn.configure(3, vec![t]).await.unwrap_err();
        assert_eq!(err.code, "MODE_NOT_SUPPORTED");

        // 非 variable 资源拒绝。
        let mut sel = speed_selection("k");
        sel[0]["resource_id"] = serde_json::json!("node");
        let err = conn
            .configure(4, vec![poll_task("t4", sel)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED_RESOURCE");

        // 重复 point_key：同任务内由结构校验先拦（INVALID_BINDING_CONFIG），
        // 跨任务由快照唯一性拦（DUPLICATE_POINT_KEY）。
        let dup = serde_json::json!([
            speed_selection("k")[0].clone(),
            speed_selection("k")[0].clone(),
        ]);
        let err = conn
            .configure(5, vec![poll_task("t5", dup)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_BINDING_CONFIG");
        let err = conn
            .configure(
                6,
                vec![
                    poll_task("t6a", speed_selection("k")),
                    poll_task("t6b", speed_selection("k")),
                ],
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");
    }

    #[tokio::test]
    async fn run_poll_e2e_ok_stop_and_restart() {
        use mesa_driver_sdk::DataSink;
        // fixture：F64 pattern（全字节序号 tag → 非法 F64？pattern 字节解为
        // f64 仍是确定值，断言 GOOD + 值等于 tag 字节解码）。
        let fx = NckFixture::spawn(NckFixtureState {
            element_size: 8,
            ..Default::default()
        })
        .await;
        let mut conn = conn_with_synthetic();
        conn.cfg.port = fx.addr.port();
        conn.configure(1, vec![poll_task("t1", speed_selection("axis3.speed"))])
            .await
            .unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("axis3.speed".to_string(), 7u32);
        conn.apply_point_map(map).await.unwrap();

        // run → 收一批 → cancel → run 必须 Ok 退出（Stop 生命周期）。
        let (data_tx, mut data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(16);
        let (ctrl_tx, _ctrl_rx) =
            tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
        let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx);
        // NOTE: DataSink.for_test 的 data 通道是否直通 publish，见下断言；
        // 若 SDK 侧聚合则改断言（此处先按直通收一批）。
        let shutdown = tokio_util::sync::CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        let batch = tokio::time::timeout(std::time::Duration::from_secs(10), data_rx.recv())
            .await
            .expect("首批必须到达")
            .expect("通道不断");
        assert_eq!(batch.values.len(), 1);
        let pv = &batch.values[0];
        assert_eq!(pv.point_id, 7);
        assert_eq!(pv.quality, mesa_core_types::Quality::Good);
        assert_eq!(pv.source_timestamp_ns, None, "NCK 永不伪造 source 时间戳");
        // tag=1 的 8 字节解为 F64（确定值，只断言 GOOD + 类型）。
        assert!(
            matches!(pv.value, mesa_core_types::Value::F64(_)),
            "实际: {:?}",
            pv.value
        );
        shutdown.cancel();
        run_handle
            .await
            .expect("join")
            .expect("run cancel 后 Ok 退出");

        // restart：同一连接再 run，会话重建后仍出数（Stop → Start 生命周期）。
        let (data_tx2, mut data_rx2) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(16);
        let (ctrl_tx2, _ctrl_rx2) =
            tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (event_tx2, _event_rx2) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
        // NOTE: conn 已被上一个 spawn move，此处重建同配连接（等价于 Manager 重建）。
        let mut conn2 = conn_with_synthetic();
        conn2.cfg.port = fx.addr.port();
        conn2
            .configure(1, vec![poll_task("t1", speed_selection("axis3.speed"))])
            .await
            .unwrap();
        let mut map2 = std::collections::HashMap::new();
        map2.insert("axis3.speed".to_string(), 7u32);
        conn2.apply_point_map(map2).await.unwrap();
        let sink2 = DataSink::for_test(ctrl_tx2, data_tx2, event_tx2);
        let sd2 = tokio_util::sync::CancellationToken::new();
        let sd2c = sd2.clone();
        let h2 = tokio::spawn(async move { conn2.run(sink2, sd2c).await });
        let batch2 = tokio::time::timeout(std::time::Duration::from_secs(10), data_rx2.recv())
            .await
            .expect("重启后首批必须到达")
            .expect("通道不断");
        assert_eq!(batch2.values.len(), 1);
        sd2.cancel();
        h2.await.expect("join").expect("重启 run Ok 退出");
    }

    #[tokio::test]
    async fn run_bad_then_heal_lastknown() {
        use mesa_driver_sdk::DataSink;
        let fx = NckFixture::spawn(NckFixtureState {
            element_size: 8,
            ..Default::default()
        })
        .await;
        let mut conn = conn_with_synthetic();
        conn.cfg.port = fx.addr.port();
        conn.configure(1, vec![poll_task("t1", speed_selection("axis3.speed"))])
            .await
            .unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("axis3.speed".to_string(), 7u32);
        conn.apply_point_map(map).await.unwrap();
        let (data_tx, mut data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(16);
        let (ctrl_tx, _ctrl_rx) =
            tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
        let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let sd = shutdown.clone();
        let h = tokio::spawn(async move { conn.run(sink, sd).await });
        // 首批 GOOD（建立 last_known）。
        let b1 = tokio::time::timeout(std::time::Duration::from_secs(10), data_rx.recv())
            .await
            .expect("首批")
            .expect("通道不断");
        assert_eq!(
            b1.values[0].value_origin,
            mesa_core_types::ValueOrigin::Current
        );
        // 注入 BAD → 下一批 LastKnown + quality Bad + code 0x05。
        fx.set_fail_items(&[0]);
        let b2 = tokio::time::timeout(std::time::Duration::from_secs(10), data_rx.recv())
            .await
            .expect("BAD 批")
            .expect("通道不断");
        assert_eq!(
            b2.values[0].value_origin,
            mesa_core_types::ValueOrigin::LastKnown
        );
        assert_eq!(b2.values[0].quality, mesa_core_types::Quality::Bad);
        assert_eq!(b2.values[0].quality_code, Some(0x05));
        assert_eq!(b2.values[0].value, b1.values[0].value);
        // 撤除注入 → 恢复 Current。
        fx.set_fail_items(&[]);
        let b3 = tokio::time::timeout(std::time::Duration::from_secs(10), data_rx.recv())
            .await
            .expect("恢复批")
            .expect("通道不断");
        assert_eq!(
            b3.values[0].value_origin,
            mesa_core_types::ValueOrigin::Current
        );
        shutdown.cancel();
        h.await.expect("join").expect("run Ok 退出");
    }

    #[tokio::test]
    async fn run_session_loss_fails_attempt() {
        use mesa_driver_sdk::DataSink;
        let fx = NckFixture::spawn(NckFixtureState {
            element_size: 8,
            ..Default::default()
        })
        .await;
        let mut conn = conn_with_synthetic();
        conn.cfg.port = fx.addr.port();
        conn.configure(1, vec![poll_task("t1", speed_selection("axis3.speed"))])
            .await
            .unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("axis3.speed".to_string(), 7u32);
        conn.apply_point_map(map).await.unwrap();
        let (data_tx, _data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(16);
        let (ctrl_tx, _ctrl_rx) =
            tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
        let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx);
        let sd = tokio_util::sync::CancellationToken::new();
        // 中途致命 → run 必须 Err（Manager 据此重建会话，而非静默停数）。
        fx.set_fail_reads(true);
        let r = conn.run(sink, sd).await;
        let err = r.expect_err("会话丢失必须 fail attempt");
        assert_eq!(err.code, "READ_SHORT");
    }
}
