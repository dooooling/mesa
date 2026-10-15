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
mod client;
mod codec;
mod config;
mod fixture;
mod identity;
mod identity_spec;
mod native;
mod presets;
mod probe;
mod value;

pub use address::{AddressError, NckArea, NckUnitMode};
pub use client::{NckClient, NckReadItem, NckReadResult};
pub use codec::{CodecError, NckWireAddress, encode_var_spec};
pub use config::{NCK_DEFAULT_PORT, NckConnConfig};
pub use fixture::{NckFixture, NckFixtureState};
pub use identity_spec::{
    AXIS_CTRLOUT_MODULE_NR, AXIS_CTRLOUT_TYPE, AXIS_ENC_MODULE_NR, AXIS_NAME, AXIS_SPIND_ASSIGN,
    CHANNEL_MACHAX_USED, DRIVE_OBJECT_NAME, DRIVE_OBJECT_NAME_M130, IdentityProbeSpec, PROBES_828D,
    ProbeSlot, probes_for_family,
};
pub use native::{
    DECODE_NAMES, InvalidRule, MAX_AREA_NO, MAX_COUNT, NativeError, NativeNckAddress,
    ResolvedPoint, TRANSPORT_SIZES, parse_transport_size, resolve_native, transport_size_name,
    unit_mode_name,
};
pub use presets::{PresetRegistry, PresetTemplate};
pub use probe::{NCK_ANCHOR_PENDING, probe_with_session};
pub use value::{NckDataKind, NckSample, ValueError, decode_value};

use mesa_core_types::{AcquisitionTask, DriverMetadata, PointDescriptor, PointMap};
use mesa_driver_sdk::{Driver, DriverConnection, SdkDriverError};

/// 驱动 ID（Core 侧无分支；仅 Descriptor identity 与 driver.toml 声明）。
pub const DRIVER_ID: &str = "sinumerik-nck";
/// 通用轮询绑定（`mesa.resources.v1`，resource_id `variable`）。
pub const GENERIC_RESOURCE_ID: &str = "variable";
/// 已声明系列（与 Descriptor Enum 同源）。
///
/// `family` **不是**读取许可：任何结构合法的原生地址都能 Configure，
/// 与系列无关。但未声明的系列意味着模板与身份探针都不可用，必须在连接
/// 阶段明确拒绝，而不是静默接受后什么都不显示。
pub const DECLARED_FAMILIES: [&str; 2] = ["828d", "840d-sl"];

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
            AccessMode, DriverCapabilities, DriverDescriptor, DriverIdentity, FieldDescriptor,
            FieldType, LocalizedText, OutputDescriptor, OutputTypeSpec, ResourceDescriptor,
            ResourceSelectionMethod, SchemaDescriptor,
        };
        let m = self.metadata();
        DriverDescriptor {
            contract_major: mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR,
            contract_minor: mesa_core_types::DESCRIPTOR_CONTRACT_MINOR,
            identity: DriverIdentity {
                driver_id: m.driver_id,
                name: m.name,
                version: m.version,
            },
            connection: SchemaDescriptor {
                fields: vec![
                    {
                        let mut f =
                            FieldDescriptor::new("host", "Host", FieldType::Host).required(true);
                        f.ui.placeholder = Some("输入控制器 IP 或主机名".into());
                        f
                    },
                    FieldDescriptor::new("port", "Port", FieldType::Port)
                        .required(false)
                        .default_value(serde_json::json!(102)),
                    // P1-3：系列显式必填（common + 单 family，无静默默认；
                    // 选项与 `DECLARED_FAMILIES` 对应，新增系列需同步加枚举与模板）。
                    {
                        let mut f = FieldDescriptor::new("family", "Family", FieldType::Enum)
                            .required(true);
                        f.validation.enum_options =
                            Some(DECLARED_FAMILIES.iter().map(|s| (*s).to_string()).collect());
                        f
                    },
                    // TSAP 显式必填（NCK 无 rack/slot 推导；远端值由真机 Gate 定）。
                    // Integer 契约保持：前端文本编辑态接受十进制（256）或
                    // 十六进制（0x0100/0100h），编辑完成规范化为整数提交；
                    // Core schema 只见 number；后端 from_json 双格式兜底
                    // （直接 API 调 string 亦可）。默认值待真机 Gate 冻结，
                    // 此处不预填（placeholder 只作格式示例）。
                    {
                        let mut f =
                            FieldDescriptor::new("local_tsap", "Local TSAP", FieldType::Integer)
                                .required(true);
                        f.validation.min = Some(0.0);
                        f.validation.max = Some(65535.0);
                        f.ui.placeholder = Some("如 0x0100 或 256".into());
                        // 归入「高级连接参数」：TSAP 是 S7 通信的 COTP 层参数，
                        // 只有明确知道控制器侧配置的用户才改得动；放进高级区可避免
                        // 常规配置被它干扰。**仅改分组，不提供默认值**——单台 828D
                        // 的实测值（0x0400/0x0D04）只是可选示例，由前端在用户主动
                        // 点击「应用实测示例」时写入，绝不静默预填（见
                        // apps/mesa-web/src/connection/connectionProfiles.ts）。
                        f.ui.advanced = Some(true);
                        f
                    },
                    {
                        let mut f =
                            FieldDescriptor::new("remote_tsap", "Remote TSAP", FieldType::Integer)
                                .required(true);
                        f.validation.min = Some(0.0);
                        f.validation.max = Some(65535.0);
                        f.ui.placeholder = Some("如 0x0102 或 258".into());
                        // 同 local_tsap：只归入高级区，不加默认值。
                        f.ui.advanced = Some(true);
                        f
                    },
                    {
                        // C1 §6.2：Descriptor 与 Driver Parser 同一套范围。
                        let mut f =
                            FieldDescriptor::new("timeout_ms", "Timeout ms", FieldType::Duration)
                                .required(false)
                                .default_value(serde_json::json!(5000));
                        f.validation.min = Some(mesa_core_types::CONNECTION_TIMEOUT_MIN_MS as f64);
                        f.validation.max = Some(mesa_core_types::CONNECTION_TIMEOUT_MAX_MS as f64);
                        f
                    },
                    FieldDescriptor::new("pdu_length", "PDU length", FieldType::Integer)
                        .required(false)
                        .default_value(serde_json::json!(480)),
                    {
                        let mut f = FieldDescriptor::new(
                            "max_read_items",
                            "每次读取项数上限",
                            FieldType::Integer,
                        )
                        .required(false)
                        .default_value(serde_json::json!(19));
                        f.validation.min = Some(1.0);
                        f.validation.max = Some(19.0);
                        f
                    },
                ],
            },
            resources: vec![ResourceDescriptor {
                id: GENERIC_RESOURCE_ID.into(),
                label: LocalizedText::new("NC Variable"),
                parameters: SchemaDescriptor {
                    // 原生只读地址（V2.0 §2.2）：字段即协议字段，不存在
                    // 变量名目录、不存在 access_path 入口切换、不存在
                    // 按名称推导地址的回退。旧 block/variable/access_path
                    // 由 Core Schema（未知字段）与 Driver 双重拒绝。
                    fields: vec![
                        {
                            let mut f = FieldDescriptor::new("area", "Area", FieldType::Enum)
                                .required(true);
                            f.validation.enum_options = Some(vec![
                                "N".into(),
                                "B".into(),
                                "C".into(),
                                "A".into(),
                                "T".into(),
                                "V".into(),
                                "H".into(),
                            ]);
                            f.ui.placeholder = Some("NCK Area（M/MMC 尚未确认，不开放）".into());
                            f
                        },
                        {
                            let mut f =
                                FieldDescriptor::new("area_no", "Area No.", FieldType::Integer)
                                    .required(true);
                            f.validation.min = Some(0.0);
                            f.validation.max = Some(31.0);
                            f.ui.placeholder = Some("VarSpec unit 低 5 位（0..31）".into());
                            f
                        },
                        {
                            let mut f =
                                FieldDescriptor::new("module", "Module", FieldType::Integer)
                                    .required(true);
                            f.validation.min = Some(0.0);
                            f.validation.max = Some(255.0);
                            f.ui.placeholder = Some("VarSpec module（0..255）".into());
                            f
                        },
                        {
                            let mut f =
                                FieldDescriptor::new("column", "Column", FieldType::Integer)
                                    .required(true);
                            f.validation.min = Some(0.0);
                            f.validation.max = Some(65535.0);
                            f.ui.placeholder = Some("VarSpec column（0..65535，含 0）".into());
                            f
                        },
                        {
                            let mut f = FieldDescriptor::new("line", "Line", FieldType::Integer)
                                .required(true);
                            f.validation.min = Some(0.0);
                            f.validation.max = Some(65535.0);
                            f.ui.placeholder = Some("VarSpec line（0..65535，含 0）".into());
                            f
                        },
                        {
                            // count 是线缆 line_count（元素个数），不是字节数。
                            let mut f = FieldDescriptor::new("count", "Count", FieldType::Integer)
                                .required(true)
                                .default_value(serde_json::json!(1));
                            f.validation.min = Some(1.0);
                            f.validation.max = Some(255.0);
                            f.ui.placeholder = Some("线缆 line_count（元素个数，非字节数）".into());
                            f
                        },
                        {
                            let mut f =
                                FieldDescriptor::new("unit_mode", "Unit mode", FieldType::Enum)
                                    .required(true)
                                    .default_value(serde_json::json!("current"));
                            f.validation.enum_options =
                                Some(vec!["current".into(), "metric".into(), "inch".into()]);
                            f.ui.placeholder = Some("选择 NCK syntax 0x82/0x83/0x84".into());
                            f
                        },
                        {
                            // 规范词表：显式字节序与宽度，不接受 REAL/DINT 等别名。
                            let mut f = FieldDescriptor::new("decode", "Decode", FieldType::Enum)
                                .required(true);
                            f.validation.enum_options = Some(
                                crate::native::DECODE_NAMES
                                    .iter()
                                    .map(|s| (*s).to_string())
                                    .collect(),
                            );
                            f.ui.placeholder =
                                Some("显式大小端与类型；协议读取成功不等于工程量已确认".into());
                            f
                        },
                        {
                            // 响应 transport 期望：用枚举字符串让十六进制语义显式可见。
                            let mut f = FieldDescriptor::new(
                                "transport_size",
                                "Transport size",
                                FieldType::Enum,
                            )
                            .required(true);
                            f.validation.enum_options = Some(vec![
                                "0x05".into(),
                                "0x06".into(),
                                "0x07".into(),
                                "0x09".into(),
                            ]);
                            f.ui.placeholder = Some("期望设备响应 transport 字段".into());
                            f
                        },
                        {
                            // 无效工程值哨兵：显式规则，不再从目录隐式继承。
                            let mut f = FieldDescriptor::new(
                                "invalid_value",
                                "Invalid value",
                                FieldType::Number,
                            )
                            .required(false);
                            f.ui.placeholder =
                                Some("可选；与 invalid_tolerance 成对（如 -200）".into());
                            f
                        },
                        {
                            let mut f = FieldDescriptor::new(
                                "invalid_tolerance",
                                "Invalid tolerance",
                                FieldType::Number,
                            )
                            .required(false);
                            f.validation.min = Some(0.0);
                            f.validation.max = Some(1.0);
                            f.ui.placeholder = Some("可选；0..=1，与 invalid_value 成对".into());
                            f
                        },
                        {
                            let mut f = FieldDescriptor::new("unit", "Unit", FieldType::String)
                                .required(false);
                            f.ui.placeholder =
                                Some("可选展示单位（仅注释，不作为工程语义已核验的证据）".into());
                            f
                        },
                    ],
                },
                outputs: vec![OutputDescriptor {
                    id: "value".into(),
                    label: LocalizedText::new("Value"),
                    // 类型来自地址上的 `decode`，只能 Configure 时确定。
                    type_spec: OutputTypeSpec::DriverResolved,
                    unit: None,
                    access: AccessMode::Read,
                }],
                modes: vec![mesa_core_types::TaskMode::Poll],
            }],
            controls: mesa_core_types::ControlCatalog::default(),
            resource_selection_methods: vec![
                ResourceSelectionMethod::Manual,
                ResourceSelectionMethod::Browse,
            ],
            capabilities: DriverCapabilities {
                poll: true,
                // 不伪造 Subscribe（ReadVar 本质是请求/响应）。
                subscribe: false,
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
        // 连接只决定「连哪台设备」（host/TSAP/family/PDU），不再装载任何
        // 变量目录：地址合法性完全由 Configure 的原生字段校验决定。
        // 内嵌模板解析失败只影响 Browse，不阻塞采集（PresetRegistry 自带降级）。
        if !DECLARED_FAMILIES.contains(&cfg.family.as_str()) {
            return Err(SdkDriverError::configuration(
                "BAD_CONFIG",
                format!(
                    "未声明的 family `{}`（已声明：{}）",
                    cfg.family,
                    DECLARED_FAMILIES.join("/")
                ),
            ));
        }
        Ok(Box::new(NckConnection::new(cfg)))
    }
}

#[derive(Debug, Clone)]
struct PointSpec {
    key: String,
    /// 稳定地址标识（诊断/回显用）。PR-A 起不再持有 `NckVariableRef`：
    /// 采集面只需要标签字符串，地址模型可独立演进为原生字段。
    source_label: String,
    wire: NckWireAddress,
    kind: NckDataKind,
    expected_len: usize,
    /// 响应 transport 期望（P1-4 逐项校验）。
    expected_transport_size: u8,
    data_type: mesa_core_types::DataType,
    unit: Option<String>,
    /// 显式无效工程值规则（可选；不再从目录隐式继承）。
    invalid_rule: Option<InvalidRule>,
}

#[derive(Debug, Clone)]
struct TaskPlan {
    id: String,
    interval_ms: u64,
    point_indices: Vec<usize>,
}

/// control-handle 并发模型下 run 入口克隆整个快照，故需 Clone。
#[derive(Debug, Clone)]
struct PlanSnapshot {
    revision: u64,
    points: Vec<PointSpec>,
    tasks: Vec<TaskPlan>,
    map: Option<PointMap>,
}

pub struct NckConnection {
    cfg: NckConnConfig,
    /// 可选配置预填模板（只影响 Browse；不参与 Configure/Poll 准入）。
    presets: PresetRegistry,
    // 采集计划（control-handle 并发模型）：configure/apply 写锁替换，
    // run 读锁克隆快照后释放；presets 只读。
    plan: std::sync::RwLock<Option<PlanSnapshot>>,
}

impl NckConnection {
    /// 生产构造：内嵌模板。
    pub fn new(cfg: NckConnConfig) -> Self {
        Self::with_presets(cfg, PresetRegistry::embedded())
    }

    /// 注入模板集合（测试模拟模板缺失/非法场景；不影响采集路径）。
    pub fn with_presets(cfg: NckConnConfig, presets: PresetRegistry) -> Self {
        Self {
            cfg,
            presets,
            plan: std::sync::RwLock::new(None),
        }
    }
}

/// 单点解码：GOOD + 足长 → Current（更新 last_known）；BAD/短包 → 有缓存
/// LastKnown、无缓存 Placeholder。
/// `quality_code` 只携带设备原生返回码（`return_code != 0xFF` 时）；
/// Mesa 本地校验失败（transport/长度 mismatch，此时 return_code 为 0xFF）
/// 填 None——`0xFF` 本意是设备 item 成功，写成 quality_code 会误导诊断，
/// 原因以 warn 日志为准。
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
            tracing::warn!(key = %spec.key, canonical = %spec.source_label, got = bytes.len(), need = spec.expected_len, "NCK 数据长度不符，按 BAD 隔离");
            return None;
        }
        if matches!(spec.kind, NckDataKind::Utf8NulLast32 | NckDataKind::Utf8NulFirst66 | NckDataKind::Utf16Be) {
            // 目录声明的文本组成行整体解码；无效UTF-8仍按项BAD。
            return decode_value(bytes, spec.kind).ok();
        }
        let mut elems = Vec::with_capacity(count);
        for i in 0..count {
            match decode_value(&bytes[i * elem..(i + 1) * elem], spec.kind) {
                Ok(v) => elems.push(v),
                Err(e) => {
                    tracing::warn!(key = %spec.key, canonical = %spec.source_label, error = %e, "NCK 元素解码失败，按 BAD 隔离");
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
    let good_value = good_value.filter(|value| {
        // 无效工程值规则覆盖数值标量、数值数组与可安全投影的整型
        // （整数按 f64 比较；i32/u32 在 f64 精度内无损）。
        let numbers: Vec<f64> = match value {
            Value::F32(v) => vec![*v as f64],
            Value::F64(v) => vec![*v],
            Value::I32(v) => vec![*v as f64],
            Value::U32(v) => vec![*v as f64],
            Value::F32Array(values) => values.iter().map(|v| *v as f64).collect(),
            Value::F64Array(values) => values.clone(),
            Value::I32Array(values) => values.iter().map(|v| *v as f64).collect(),
            Value::U32Array(values) => values.iter().map(|v| *v as f64).collect(),
            _ => Vec::new(),
        };
        let rule = spec.invalid_rule;
        let valid = numbers
            .iter()
            .all(|v| v.is_finite() && !rule.map(|r| r.hits(*v)).unwrap_or(false));
        if !valid {
            tracing::warn!(key=%spec.key, "NCK返回设备无效工程值标记，按BAD处理，不伪造协议错误码");
        }
        valid
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
                // P2a：仅设备原生错误码进 quality_code；本地校验失败
                // （return_code 0xFF）填 None，不伪造协议成功码为诊断码。
                quality_code: return_code
                    .filter(|&c| c != mesa_s7_transport::S7_ITEM_OK)
                    .map(|c| c as i32),
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
        NckDataKind::Utf8NulLast32 | NckDataKind::Utf8NulFirst66 | NckDataKind::Utf16Be => {
            unreachable!("文本组成行必须在pack_array前完成投影")
        }
        NckDataKind::F64 | NckDataKind::F64Le => Value::F64Array(
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
        NckDataKind::I32 | NckDataKind::I32Le | NckDataKind::I16Be => Value::I32Array(
            elems
                .into_iter()
                .map(|v| match v {
                    Value::I32(x) => x,
                    _ => 0,
                })
                .collect(),
        ),
        NckDataKind::U32 | NckDataKind::U16Le | NckDataKind::U16Be | NckDataKind::U8 => {
            Value::U32Array(
                elems
                    .into_iter()
                    .map(|v| match v {
                        Value::U32(x) => x,
                        _ => 0,
                    })
                    .collect(),
            )
        }
        NckDataKind::Utf8Fixed16 => Value::StringArray(
            elems
                .into_iter()
                .map(|v| match v {
                    Value::String(s) => s,
                    _ => unreachable!("轴名元素已按目录解码为字符串"),
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
    async fn probe(&self) -> Result<mesa_core_types::ProbeReport, SdkDriverError> {
        Ok(probe_with_session(&self.cfg).await)
    }

    /// 通用绑定 `mesa.resources.v1`（resource_id `variable`）→ 原生地址解析 →
    /// PointDescriptor。未知变量/非法参数/单项超 PDU 即配置拒绝
    /// （fail-closed，不进运行期；不查任何目录）。
    async fn configure(
        &self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        use mesa_core_types::{
            DuplicatePointKey, GENERIC_BINDING_KIND, GenericBinding, TaskSchedule,
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
            // Foundation-2 单真值：schedule 已由 validate 保证 Poll interval 合法
            let TaskSchedule::Poll { interval_ms } = task.schedule else {
                return Err(SdkDriverError::new(
                    mesa_core_types::ErrorKind::Unsupported,
                    "MODE_NOT_SUPPORTED",
                    format!(
                        "task `{}`: sinumerik-nck 只支持 poll（不伪造 subscribe）",
                        task.id
                    ),
                ));
            };
            let binding: GenericBinding = serde_json::from_value(task.binding.config.clone())
                .map_err(|e| {
                    SdkDriverError::configuration(
                        "INVALID_BINDING_CONFIG",
                        format!("task `{}`: invalid generic binding: {e}", task.id),
                    )
                })?;
            validate_selections_structure(&binding.selections)
                .map_err(|e| SdkDriverError::configuration("INVALID_BINDING_CONFIG", e))?;
            let mut indices = Vec::new();
            for sel in &binding.selections {
                if sel.resource_id != GENERIC_RESOURCE_ID {
                    return Err(SdkDriverError::configuration(
                        "UNSUPPORTED_RESOURCE",
                        format!("task `{}`: sinumerik-nck 只支持 variable", task.id),
                    ));
                }
                for out in &sel.outputs {
                    // 纵深校验：Driver 不依赖 UI 自动填充，也不依赖 Core
                    // 前置 Schema 校验；输出必须正好是 value。
                    if out.output != "value" {
                        return Err(SdkDriverError::configuration(
                            "UNSUPPORTED_OUTPUT",
                            format!(
                                "point `{}`: sinumerik-nck 只支持 output `value`",
                                out.point_key
                            ),
                        ));
                    }
                    let params = sel.parameters.as_object().ok_or_else(|| {
                        SdkDriverError::configuration(
                            "INVALID_VARIABLE",
                            format!("point `{}`: parameters 需为对象", out.point_key),
                        )
                    })?;
                    // 唯一解析链：原生字段 → 校验 → wire/类型/规则。
                    // 不查目录、不按名称推导、不做旧配置迁移。
                    let addr = NativeNckAddress::from_parameters(params).map_err(|e| {
                        SdkDriverError::new(
                            mesa_core_types::ErrorKind::Address,
                            "INVALID_VARIABLE",
                            format!("point `{}`: {e}", out.point_key),
                        )
                    })?;
                    let rp = resolve_native(addr).map_err(|e| {
                        SdkDriverError::new(
                            mesa_core_types::ErrorKind::Address,
                            "INVALID_VARIABLE",
                            format!("point `{}`: {e}", out.point_key),
                        )
                    })?;
                    // P2-1：Configure 期提前拦截「单项装不下」的配置，避免
                    // 「配置成功、启动后才在读取时报 READ_ITEM_TOO_LARGE」。
                    //
                    // 判定用**请求的** PDU：协商只向下取 min（见 s7-transport
                    // `parse_setup_ack`），所以这里判失败 ⇒ 协商后必然也失败，
                    // 是可靠的必要条件；判通过仍可能在协商到更小 PDU 后失败，
                    // 权威检查因此保留在读取时（fail-closed，不静默发超长请求）。
                    mesa_s7_transport::read_var::check_single_item_fits(
                        &mesa_s7_transport::S7ReadVarItem {
                            var_spec: encode_var_spec(&rp.wire),
                            expected_data_len: rp.expected_data_len,
                        },
                        self.cfg.requested_pdu_length,
                        false,
                    )
                    .map_err(|e| {
                        SdkDriverError::new(
                            mesa_core_types::ErrorKind::Configuration,
                            e.code,
                            format!(
                                "point `{}`: count={} × {} 字节装不进请求 PDU {}（{}）；\
                                 请减小 count 或增大连接 `pdu_length`——协商 PDU 只会更小，\
                                 该配置在当前连接参数下无法读取",
                                out.point_key,
                                rp.wire.line_count,
                                rp.kind.byte_len(),
                                self.cfg.requested_pdu_length,
                                e.message
                            ),
                        )
                    })?;
                    indices.push(new_points.len());
                    tracing::debug!(
                        key = %out.point_key,
                        canonical = %rp.address.source_label(),
                        "NCK 原生地址已解析",
                    );
                    new_points.push(PointSpec {
                        key: out.point_key.clone(),
                        source_label: rp.address.source_label(),
                        wire: rp.wire,
                        kind: rp.kind,
                        expected_len: rp.expected_data_len,
                        expected_transport_size: rp.expected_transport_size,
                        data_type: rp.data_type,
                        unit: rp.address.unit.clone(),
                        invalid_rule: rp.invalid_rule,
                    });
                }
            }
            new_tasks.push(TaskPlan {
                id: task.id.clone(),
                interval_ms,
                point_indices: indices,
            });
        }
        let descriptors: Vec<PointDescriptor> = new_points
            .iter()
            .map(|p| PointDescriptor {
                point_key: p.key.clone(),
                data_type: p.data_type,
                unit: p.unit.clone(),
                // 通用地址回显仅用于诊断，不能据此断言业务轴身份。
                source_label: Some(p.source_label.clone()),
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
        // 写锁内整体替换（§6.2 原子切换；失败路径在锁外早返回）。
        *self.plan.write().unwrap() = Some(PlanSnapshot {
            revision,
            points: new_points,
            tasks: new_tasks,
            map: None,
        });
        Ok(descriptors)
    }

    async fn apply_point_map(&self, map: PointMap) -> Result<(), SdkDriverError> {
        let mut guard = self.plan.write().unwrap();
        let snap = guard.as_mut().ok_or_else(|| {
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
        &self,
        sink: mesa_driver_sdk::DataSink,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<(), SdkDriverError> {
        use mesa_core_types::{DataBatch, PointValue, Value, now_unix_ns};
        use std::sync::atomic::{AtomicU64, Ordering};
        // 读锁下克隆整个快照后释放（await 期间不持 std 锁）。
        let snapshot: PlanSnapshot = {
            let guard = self.plan.read().unwrap();
            guard
                .as_ref()
                .ok_or_else(|| {
                    SdkDriverError::new(
                        mesa_core_types::ErrorKind::Internal,
                        "NO_PLAN",
                        "run 前未 configure+apply",
                    )
                })?
                .clone()
        };
        let map = snapshot.map.clone().ok_or_else(|| {
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
            revision = snapshot.revision,
            points = snapshot.points.len(),
            tasks = snapshot.tasks.len(),
            "NCK 数据面启动",
        );
        let client = std::sync::Arc::new(tokio::sync::Mutex::new(client));
        let seq = std::sync::Arc::new(AtomicU64::new(1));
        // P1-1：JoinSet（哪个 task 先结束就先观察哪个）。顺序 await Vec
        // 在多 task 下会饿死错误传播：健康的无限 Poll task 排在前面时，
        // 后面 task 的 Err 永远等不到 cancel，run() 不返回，Manager 不重建。
        let mut set: tokio::task::JoinSet<Result<(), SdkDriverError>> = tokio::task::JoinSet::new();
        for task in &snapshot.tasks {
            let indices = task.point_indices.clone();
            let points: Vec<(PointSpec, u32)> = indices
                .iter()
                .map(|&i| {
                    let p = snapshot.points[i].clone();
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
            set.spawn(async move {
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
            });
        }
        let mut final_err: Option<SdkDriverError> = None;
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if final_err.is_none() {
                        final_err = Some(e);
                    }
                    // 任一 task Err 即 cancel 全体，继续 reap 剩余 task。
                    shutdown.cancel();
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
                    shutdown.cancel();
                }
            }
        }
        if let Some(e) = final_err {
            return Err(e);
        }
        Ok(())
    }

    /// 浏览：只有两类入口——**可选模板预填**与**只读身份核对**。
    ///
    /// 不再有 Catalog 虚拟树：模板缺失/非法只让模板入口显示诊断，
    /// 手动原生地址与身份核对都不依赖它。未知 parent → 空页。
    async fn browse(
        &self,
        parent: &str,
        filter: &str,
        cursor: &str,
        limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), SdkDriverError> {
        let identity_ok = identity::supported(&self.cfg.family);
        if identity_ok
            && (parent == identity::ROOT || parent.starts_with(&format!("{}/", identity::ROOT)))
        {
            // 临时Browse连接无跨请求缓存；按页/节点读取并保留整次操作时限。
            return tokio::time::timeout(
                std::time::Duration::from_secs(15),
                identity::browse(&self.cfg, &self.presets, parent, filter, cursor, limit),
            )
            .await
            .map_err(|_| {
                SdkDriverError::new(
                    mesa_core_types::ErrorKind::Timeout,
                    "IDENTITY_TIMEOUT",
                    "目标页或对象核对超过15秒",
                )
            })?;
        }
        if parent.is_empty() {
            let mut nodes = vec![presets::folder_node(
                PRESETS_ROOT,
                "常用预设模板（可选预填，需确认实例与行）",
            )];
            if identity_ok {
                nodes.push(identity::root_node());
            } else {
                nodes.push(presets::diagnostic(format!(
                    "该系列（{}）暂无可信身份探针，身份浏览待验证；仍可手动填写原生地址",
                    self.cfg.family
                )));
            }
            return Ok(paginate(nodes, filter, cursor, limit));
        }
        if parent == PRESETS_ROOT {
            let mut nodes: Vec<_> = self
                .presets
                .for_family(&self.cfg.family)
                .into_iter()
                .map(presets::leaf_node)
                .collect();
            // 模板不可用时明确提示，而不是伪装成“没有模板”。
            if let Some(err) = self.presets.error() {
                nodes.insert(0, presets::diagnostic(err.to_string()));
            }
            if nodes.is_empty() && self.presets.error().is_none() {
                nodes.push(presets::diagnostic(format!(
                    "系列 {} 暂无预填模板；可直接手动填写原生地址",
                    self.cfg.family
                )));
            }
            return Ok(paginate(nodes, filter, cursor, limit));
        }
        Ok((Vec::new(), None))
    }
}

/// 模板入口根（身份入口为 `identity::ROOT`）。
const PRESETS_ROOT: &str = "nck://presets";

/// 本地节点分页（纯函数；顺序稳定，游标为十进制偏移）。
fn paginate(
    mut nodes: Vec<mesa_driver_protocol::pb::BrowseNode>,
    filter: &str,
    cursor: &str,
    limit: u32,
) -> (Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>) {
    if !filter.is_empty() {
        nodes.retain(|node| node.label.contains(filter) || node.id.contains(filter));
    }
    let start = cursor.parse::<usize>().unwrap_or(0).min(nodes.len());
    let end = (start
        + if limit == 0 {
            50
        } else {
            limit.min(500) as usize
        })
    .min(nodes.len());
    (
        nodes[start..end].to_vec(),
        (end < nodes.len()).then(|| end.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无效工程值规则由**绑定显式携带**（不再从目录继承）。
    /// 同一地址未配置哨兵时不得假装具有同等工程语义保护。
    fn r0035_spec(invalid: bool) -> PointSpec {
        let mut params = serde_json::json!({
            "area": "V", "area_no": 1, "module": 26, "column": 35, "line": 1,
            "count": 1, "unit_mode": "current", "decode": "F32_BE",
            "transport_size": "0x07",
        });
        if invalid {
            params["invalid_value"] = serde_json::json!(-200.0);
            params["invalid_tolerance"] = serde_json::json!(0.001);
        }
        let addr = NativeNckAddress::from_parameters(params.as_object().unwrap()).unwrap();
        let rp = resolve_native(addr).unwrap();
        PointSpec {
            key: "temperature".into(),
            source_label: rp.address.source_label(),
            wire: rp.wire,
            kind: rp.kind,
            expected_len: rp.expected_data_len,
            expected_transport_size: rp.expected_transport_size,
            data_type: rp.data_type,
            unit: rp.address.unit.clone(),
            invalid_rule: rp.invalid_rule,
        }
    }

    #[test]
    fn invalid_temperature_is_bad_and_does_not_forge_a_protocol_error() {
        let spec = r0035_spec(true);
        let mut cache = std::collections::HashMap::new();
        let good = decode_point(&spec, 1, Some(&[0x41, 0x98, 0, 0]), Some(0xff), &mut cache);
        assert_eq!(good.quality, mesa_core_types::Quality::Good);
        assert_eq!(good.value, mesa_core_types::Value::F32(19.0));
        // 实体机独立响应值接近-200；不能因为返回FF就显示为正常电机温度。
        let invalid = decode_point(
            &spec,
            1,
            Some(&[0xc3, 0x47, 0xff, 0xfe]),
            Some(0xff),
            &mut cache,
        );
        assert_eq!(invalid.quality, mesa_core_types::Quality::Bad);
        assert_eq!(invalid.quality_code, None);
        assert_eq!(
            invalid.value_origin,
            mesa_core_types::ValueOrigin::LastKnown
        );
        assert_eq!(invalid.value, mesa_core_types::Value::F32(19.0));
        let negative = decode_point(
            &spec,
            1,
            Some(&(-20.0f32).to_be_bytes()),
            Some(0xff),
            &mut cache,
        );
        assert_eq!(
            negative.quality,
            mesa_core_types::Quality::Good,
            "正常负温度不能误判"
        );
    }

    /// §6.3 诚实边界：未配置哨兵的同地址不享受无效值保护（仍按协议 GOOD）。
    #[test]
    fn same_address_without_explicit_rule_has_no_invalid_value_protection() {
        let spec = r0035_spec(false);
        assert!(spec.invalid_rule.is_none());
        let mut cache = std::collections::HashMap::new();
        let v = decode_point(
            &spec,
            1,
            Some(&[0xc3, 0x47, 0xff, 0xfe]),
            Some(0xff),
            &mut cache,
        );
        assert_eq!(
            v.quality,
            mesa_core_types::Quality::Good,
            "未声明规则时不做工程语义判定"
        );
    }

    /// 无效值规则覆盖整型与数组（此前只检查浮点）。
    #[test]
    fn invalid_rule_covers_integers_and_arrays() {
        let mut params = serde_json::json!({
            "area": "V", "area_no": 1, "module": 26, "column": 35, "line": 1,
            "count": 2, "unit_mode": "current", "decode": "I32_BE",
            "transport_size": "0x07",
            "invalid_value": -200.0, "invalid_tolerance": 0.5,
        });
        params["decode"] = serde_json::json!("I32_BE");
        let addr = NativeNckAddress::from_parameters(params.as_object().unwrap()).unwrap();
        let rp = resolve_native(addr).unwrap();
        let spec = PointSpec {
            key: "ints".into(),
            source_label: rp.address.source_label(),
            wire: rp.wire,
            kind: rp.kind,
            expected_len: rp.expected_data_len,
            expected_transport_size: rp.expected_transport_size,
            data_type: rp.data_type,
            unit: None,
            invalid_rule: rp.invalid_rule,
        };
        assert_eq!(spec.data_type, mesa_core_types::DataType::I32Array);
        let mut cache = std::collections::HashMap::new();
        // 两个元素都正常 → GOOD。
        let mut ok = Vec::new();
        ok.extend_from_slice(&10i32.to_be_bytes());
        ok.extend_from_slice(&(-20i32).to_be_bytes());
        assert_eq!(
            decode_point(&spec, 1, Some(&ok), Some(0xff), &mut cache).quality,
            mesa_core_types::Quality::Good
        );
        // 数组任一元素命中哨兵 → BAD。
        let mut bad = Vec::new();
        bad.extend_from_slice(&10i32.to_be_bytes());
        bad.extend_from_slice(&(-200i32).to_be_bytes());
        assert_eq!(
            decode_point(&spec, 1, Some(&bad), Some(0xff), &mut cache).quality,
            mesa_core_types::Quality::Bad
        );
    }

    #[test]
    fn descriptor_is_valid_and_read_only() {
        let d = SinumerikNckDriver.descriptor();
        d.validate().expect("sinumerik-nck descriptor 必须合法");
        assert_eq!(d.identity.driver_id, "sinumerik-nck");
        assert!(d.capabilities.poll, "NCK V1 必须 poll");
        assert!(!d.capabilities.subscribe, "NCK 不伪造 subscribe");
        // Browse 是 resource_selection_methods 的唯一真值
        assert!(
            d.resource_selection_methods
                .contains(&mesa_core_types::ResourceSelectionMethod::Browse)
        );
        // P1-3：连接 family 显式必填（选项与 `DECLARED_FAMILIES` 对应）。
        let conn_keys: Vec<_> = d.connection.fields.iter().map(|f| f.key.as_str()).collect();
        assert!(conn_keys.contains(&"family"), "缺 family");
        let fam = d
            .connection
            .fields
            .iter()
            .find(|f| f.key == "family")
            .unwrap();
        assert!(fam.required);
        let opts = fam.validation.enum_options.clone().unwrap();
        assert_eq!(
            opts,
            DECLARED_FAMILIES
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
        // TSAP 双格式：Integer 契约保持（前端文本态→整数提交），
        // 0..=65535 范围 + 占位示例；无默认值（待真机 Gate 冻结）。
        for key in ["local_tsap", "remote_tsap"] {
            let f = d
                .connection
                .fields
                .iter()
                .find(|f| f.key == key)
                .unwrap_or_else(|| panic!("缺 {key}"));
            assert!(f.required, "{key} 必须必填");
            assert_eq!(f.field_type, mesa_core_types::FieldType::Integer);
            assert_eq!(f.validation.min, Some(0.0));
            assert_eq!(f.validation.max, Some(65535.0));
            assert!(f.default.is_none(), "{key} 不得预填默认值");
            // 归入高级连接参数（前端据此默认折叠），但不得因此弱化必填/校验。
            assert_eq!(f.ui.advanced, Some(true), "{key} 应归入高级参数");
        }
        assert!(!d.capabilities.write, "NCK V1 只读");
        assert!(!d.capabilities.method, "NCK V1 无 command");
        assert!(!d.capabilities.events, "NCK V1 无事件");
        // variable 资源参数与 native.rs 口径一致：只有原生协议字段，
        // 不含 block/variable/access_path 等旧模型字段。
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
            "module",
            "column",
            "line",
            "count",
            "unit_mode",
            "decode",
            "transport_size",
            "invalid_value",
            "invalid_tolerance",
            "unit",
        ] {
            assert!(keys.contains(&k), "缺参数 {k}");
        }
        for legacy in ["block", "variable", "access_path"] {
            assert!(
                !keys.contains(&legacy),
                "旧字段 {legacy} 不得出现在新 Schema"
            );
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
                r#"{"host":"10.0.0.5","family":"840d-sl","local_tsap":256,"remote_tsap":258}"#,
            )
            .await;
        assert!(ok.is_ok());
        // 十六进制字符串等价（0x0100 == 256；直接调 API 亦可）。
        let ok_hex = d
            .open_connection(
                "t",
                r#"{"host":"10.0.0.5","family":"840d-sl","local_tsap":"0x0100","remote_tsap":"0x0102"}"#,
            )
            .await;
        assert!(ok_hex.is_ok());
        // 集成回归：Descriptor(Integer 契约) → Core validate_instance →
        // NCK from_json 全链（hex 文本→整数提交→解析同值）。
        {
            let desc = SinumerikNckDriver.descriptor();
            // 前端提交形态（整数，已由 TsapField 规范化）。
            let conn = serde_json::json!({
                "host": "10.0.0.5", "family": "840d-sl",
                "local_tsap": 256, "remote_tsap": 258,
            });
            let issues = desc.connection.validate_instance("connection", &conn);
            assert!(issues.is_empty(), "整数 TSAP 必须过 Core 门：{issues:?}");
            let cfg = NckConnConfig::from_json(&conn).expect("Core 通过后 NCK 必须解析");
            assert_eq!((cfg.local_tsap, cfg.remote_tsap), (256, 258));
            // 旧整数配置修改重载链（向后兼容：整数直通不变）。
            let issues2 = desc.connection.validate_instance("connection", &conn);
            assert!(issues2.is_empty());
        }
        // 未知 family 拒绝（P1-3：绝不全系列合并）。
        assert_eq!(
            err_of(
                &d,
                r#"{"host":"10.0.0.5","family":"nope","local_tsap":256,"remote_tsap":258}"#
            )
            .await
            .code,
            "BAD_CONFIG"
        );
    }

    /// Browse 根在“模板缺失/非法”时仍可导航：模板入口给诊断，系列无探针时
    /// 明确提示身份浏览待验证——绝不伪装成空目录，也绝不阻塞采集。
    #[tokio::test]
    async fn browse_root_degrades_honestly_without_presets() {
        let conn = NckConnection::with_presets(
            NckConnConfig::from_json(&serde_json::json!({
                "host": "10.0.0.5", "family": "840d-sl",
                "local_tsap": 256, "remote_tsap": 258,
            }))
            .unwrap(),
            PresetRegistry::load("{not json"),
        );
        let (root, next) = conn.browse("", "", "", 0).await.unwrap();
        assert_eq!(root.len(), 2, "模板入口 + 系列提示");
        assert_eq!(root[0].id, "nck://presets");
        assert_eq!(root[1].kind, "diagnostic", "无探针系列必须明示");
        assert!(next.is_none());
        // 模板入口给出加载诊断，而不是假装“没有模板”。
        let (leaves, _) = conn.browse("nck://presets", "", "", 0).await.unwrap();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].kind, "diagnostic");
        // probe 打不可达桩（10.0.0.5 无真机）→ 不可达规范报告。
        let report = conn.probe().await.unwrap();
        assert!(!report.reachable, "无真机时 probe 必须不可达");
    }

    /// 828d：模板入口可列举候选模板（仍是 candidate 且缺必填）。
    #[tokio::test]
    async fn browse_presets_lists_candidates_and_probe_reachable() {
        let fx = NckFixture::spawn(NckFixtureState::default()).await;
        let mut conn = NckConnection::with_presets(
            NckConnConfig::from_json(&serde_json::json!({
                "host": "127.0.0.1", "family": "828d",
                "local_tsap": 256, "remote_tsap": 258,
            }))
            .unwrap(),
            PresetRegistry::embedded(),
        );
        conn.cfg.port = fx.addr.port();
        let (root, _) = conn.browse("", "", "", 0).await.unwrap();
        assert_eq!(root.len(), 2);
        assert_eq!(root[0].id, "nck://presets");
        assert_eq!(root[1].id, "nck://identity");
        let (leaves, next) = conn.browse("nck://presets", "", "", 0).await.unwrap();
        assert!(!leaves.is_empty());
        assert!(next.is_some(), "模板多于 50 条时必须分页");
        assert!(leaves.iter().all(|n| n.label.contains("candidate")));
        assert!(leaves.iter().all(|n| n.label.contains("待确认")));
        let report = conn.probe().await.unwrap();
        assert!(report.reachable);
        assert!(report.family.is_none(), "无 anchor 不得断言 family");
        assert!(report.warnings.iter().any(|w| w.code == NCK_ANCHOR_PENDING));
    }

    /// 采集用连接：注入模板集合（Gate B：模板缺失/非法**不得**影响数据面）。
    fn conn_with_presets(presets: PresetRegistry) -> NckConnection {
        NckConnection::with_presets(
            NckConnConfig::from_json(&serde_json::json!({
                "host": "127.0.0.1", "family": "840d-sl",
                "local_tsap": 256, "remote_tsap": 258,
            }))
            .unwrap(),
            presets,
        )
    }

    fn conn_with_synthetic() -> NckConnection {
        conn_with_presets(PresetRegistry::embedded())
    }

    fn poll_task(id: &str, selections: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: id.into(),
            schedule: mesa_core_types::TaskSchedule::Poll { interval_ms: 50 },
            binding: mesa_core_types::DriverBinding {
                kind: mesa_core_types::GENERIC_BINDING_KIND.into(),
                config: serde_json::json!({"selections": selections}),
            },
        }
    }

    /// 原生地址选择项（脚手架数值，非 Siemens 语义：C/1/module18/column42，
    /// line=3，F64_BE/0x09，与 fixture 缺省响应自洽）。
    fn speed_selection(point_key: &str) -> serde_json::Value {
        serde_json::json!([{
            "resource_id": "variable",
            "parameters": {
                "area": "C", "area_no": 1, "module": 18, "column": 42,
                "line": 3, "count": 1, "unit_mode": "current",
                "decode": "F64_BE", "transport_size": "0x09",
                "unit": "mm/min",
            },
            "outputs": [{"output": "value", "point_key": point_key}],
        }])
    }

    /// DriverResolved 闭环：原生 `decode + count` → configure →
    /// PointDescriptor；count=1 标量、count>1 同口径 array（pack_array 一致）。
    #[tokio::test]
    async fn driver_resolved_type_matches_decode_and_count() {
        let d = SinumerikNckDriver.descriptor();
        d.validate().expect("descriptor 必须合法");
        let var = d.resources.iter().find(|r| r.id == "variable").unwrap();
        assert_eq!(
            var.outputs[0].type_spec,
            mesa_core_types::OutputTypeSpec::DriverResolved
        );
        // count=1 → F64（decode=F64_BE）
        let conn = conn_with_synthetic();
        let params = serde_json::json!({
            "area": "C", "area_no": 1, "module": 18, "column": 42,
            "line": 3, "count": 1, "unit_mode": "current",
            "decode": "F64_BE", "transport_size": "0x09",
        });
        let issues = var.parameters.validate_instance("parameters", &params);
        assert!(issues.is_empty(), "{issues:?}");
        let descs = conn
            .configure(1, vec![poll_task("t1", speed_selection("k1"))])
            .await
            .unwrap();
        assert_eq!(descs[0].data_type, mesa_core_types::DataType::F64);
        // count=3 → F64Array（与 pack_array 同口径）
        let mut sel3 = speed_selection("k3");
        sel3[0]["parameters"]["count"] = serde_json::json!(3);
        let params3 = sel3[0]["parameters"].clone();
        let issues3 = var.parameters.validate_instance("parameters", &params3);
        assert!(issues3.is_empty(), "{issues3:?}");
        let descs3 = conn
            .configure(2, vec![poll_task("t2", sel3)])
            .await
            .unwrap();
        assert_eq!(descs3[0].data_type, mesa_core_types::DataType::F64Array);
    }

    #[tokio::test]
    async fn configure_ok_and_rejects() {
        let conn = conn_with_synthetic();
        // 合法：descriptor 类型 F64 + unit 回填。
        let descs = conn
            .configure(1, vec![poll_task("t1", speed_selection("axis3.speed"))])
            .await
            .unwrap();
        assert_eq!(descs.len(), 1);
        assert_eq!(descs[0].point_key, "axis3.speed");
        assert_eq!(descs[0].data_type, mesa_core_types::DataType::F64);
        assert_eq!(descs[0].unit.as_deref(), Some("mm/min"));

        // 旧模型字段 fail-closed（不迁移、不隐式推导）。
        let mut bad_sel = speed_selection("k");
        bad_sel[0]["parameters"]["variable"] = serde_json::json!("noSuchVar");
        let err = conn
            .configure(2, vec![poll_task("t2", bad_sel)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_VARIABLE");

        // 非法原生字段（未知 transport）同样 fail-closed。
        let mut bad_ts = speed_selection("k");
        bad_ts[0]["parameters"]["transport_size"] = serde_json::json!("0x04");
        let err = conn
            .configure(7, vec![poll_task("t7", bad_ts)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_VARIABLE");

        // output 不是 value：Driver 纵深拒绝。
        let mut bad_out = speed_selection("k");
        bad_out[0]["outputs"][0]["output"] = serde_json::json!("other");
        let err = conn
            .configure(8, vec![poll_task("t8", bad_out)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED_OUTPUT");

        // 非 Poll 模式拒绝（不伪造 subscribe）。
        let mut t = poll_task("t3", speed_selection("k"));
        t.schedule = mesa_core_types::TaskSchedule::default_subscribe();
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

    #[tokio::test]
    async fn run_transport_mismatch_is_bad_without_fake_success_code() {
        // P2a：设备 return_code 0xFF 但 transport 不符 → BAD，且 quality_code
        // 必须为 None（0xFF 本意成功，写成诊断码会误导）。
        use mesa_driver_sdk::DataSink;
        let fx = NckFixture::spawn(NckFixtureState {
            element_size: 8,
            wrong_transport: true,
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
        let b = tokio::time::timeout(std::time::Duration::from_secs(10), data_rx.recv())
            .await
            .expect("BAD 批必须到达")
            .expect("通道不断");
        assert_eq!(b.values[0].quality, mesa_core_types::Quality::Bad);
        assert_eq!(
            b.values[0].quality_code, None,
            "本地校验失败不得伪造 255 为诊断码"
        );
        assert_eq!(
            b.values[0].value_origin,
            mesa_core_types::ValueOrigin::Placeholder
        );
        shutdown.cancel();
        h.await.expect("join").expect("run Ok 退出");
    }

    /// P2-1 回归：单项装不进 PDU 的配置必须在 **Configure 期**被拒绝，
    /// 而不是"配置成功、启动后才在第一次读取时报错"。
    ///
    /// 判定基准是**请求的** PDU：协商只向下取 min，故此处判失败 ⇒ 协商后
    /// 必然也失败（可靠的必要条件）。
    #[tokio::test]
    async fn oversized_count_is_rejected_at_configure() {
        let conn = conn_with_synthetic();
        let mut huge = speed_selection("big");
        // F64 × 255 = 2040 字节，远超缺省请求 PDU 480。
        huge[0]["parameters"]["count"] = serde_json::json!(255);
        let err = conn
            .configure(1, vec![poll_task("tB", huge)])
            .await
            .expect_err("超 PDU 的 count 必须在 Configure 期拒绝");
        assert_eq!(err.code, "READ_ITEM_TOO_LARGE");
        assert!(
            err.message.contains("count=255"),
            "错误须点明 count：{}",
            err.message
        );
        assert!(
            err.message.contains("480"),
            "错误须点明请求 PDU：{}",
            err.message
        );
        // 同一配置里合法点位不受影响：拒绝是整包 fail-closed，不留下半份计划。
        let ok = conn
            .configure(1, vec![poll_task("tA", speed_selection("healthy"))])
            .await;
        assert!(ok.is_ok(), "合法配置仍必须可提交");
    }

    #[tokio::test]
    async fn nck_second_poll_task_failure_fails_whole_attempt() {
        // P1-1 回归：task A（前）永久健康 + task B（后）每 tick 致命。
        // 顺序 await 下 B 的 Err 会被 A 的无限 Poll 饿死，run 永不返回；
        // JoinSet 下 run 必须 bounded time 内 Err。
        //
        // 致命机制用「协商 PDU < 请求 PDU」：count 在配置期按请求值 960 合法，
        // 协商到 240 后单项装不下 → 读取时 fail-closed。这同时说明 Configure
        // 期检查是**必要非充分**条件，权威判定仍在读取时。
        use mesa_driver_sdk::DataSink;
        let fx = NckFixture::spawn(NckFixtureState {
            element_size: 8,
            max_pdu: 240,
            ..Default::default()
        })
        .await;
        let mut conn = conn_with_synthetic();
        conn.cfg.port = fx.addr.port();
        conn.cfg.requested_pdu_length = mesa_s7_transport::S7_PDU_MAX;
        let mut huge_sel = speed_selection("big");
        // 12(var_spec) + 8×40 + 32(余量) = 364：装得进 960，装不进 240。
        huge_sel[0]["parameters"]["count"] = serde_json::json!(40);
        conn.configure(
            1,
            vec![
                poll_task("tA", speed_selection("healthy")),
                poll_task("tB", huge_sel),
            ],
        )
        .await
        .unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("healthy".to_string(), 7u32);
        map.insert("big".to_string(), 8u32);
        conn.apply_point_map(map).await.unwrap();
        let (data_tx, _data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(16);
        let (ctrl_tx, _ctrl_rx) =
            tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
        let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx);
        let sd = tokio_util::sync::CancellationToken::new();
        let r = tokio::time::timeout(std::time::Duration::from_secs(10), conn.run(sink, sd)).await;
        let err = r
            .expect("run 必须在 bounded time 内返回（JoinSet 回归）")
            .expect_err("task B 致命必须 fail 整个 attempt");
        assert_eq!(err.code, "READ_ITEM_TOO_LARGE");
    }
}
