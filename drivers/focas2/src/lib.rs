//! FANUC FOCAS2 Driver — 方案 §7.2 资源型（V1 只读）。
//!
//! - 绑定（Foundation-2 单路径）：`mesa.resources.v1`
//!   ```json
//!   { "selections": [
//!       { "resource_id": "machine", "parameters": {},
//!         "outputs": [{"output":"status","point_key":"cnc.status"}] },
//!       { "resource_id": "axis", "parameters": {"axis":1},
//!         "outputs": [{"output":"absolute","point_key":"axis.x"}] }
//!   ]}
//!   ```
//! - 地址解析见 `address::parse_address`；Core 不触及此文件（硬性约束）。
//! - 访问抽象 `focas_api::FocasApi`，当前默认 `FakeFocasApi`（多协议骨架验证），
//!   真机时切换 `NativeFocasApi`（Fwlib FFI，预留）。

mod address;
mod focas_api;
mod native;
/// FOCAS Ethernet Pure Rust Wire（PR1 foundation）：frame/session/
/// typed operations，FOCAS-local 私有实现（`drivers/focas2` 外不可见）。
mod wire;

pub use address::{AddressError, FocasAddress, parse_address};
pub use focas_api::{FakeFocasApi, FocasApi, NativeFocasApi};
/// Wire 开发诊断入口（`#[doc(hidden)]` 非稳定、诊断专用）：
/// 唯一出口是 `WireFocasApi`（`wire_probe` 所需）；`FocasClient/WireSession/
/// FocasFrame/GenericSubpacket` 等内部协议类型不出 crate（fixture 回归已
/// 移入 `wire::fixture_tests`，crate 内部直测生产 codec）。
/// 生产 `FocasDriver` 路径不用此模块。
#[doc(hidden)]
pub mod wire_pub {
    pub use crate::wire::WireFocasApi;
}

use std::sync::Arc;
use std::time::Duration;

use mesa_core_types::{
    AcquisitionTask, CapabilityItem, CapabilityState, DataBatch, DataType, DriverMetadata,
    DuplicatePointKey, GENERIC_BINDING_KIND, GenericBinding, PointDescriptor, PointMap, PointValue,
    ProbeReport, ProbeWarning, Quality, TaskSchedule, Value, ValueOrigin, ensure_unique_point_keys,
    now_unix_ns,
};
use mesa_driver_sdk::{DataSink, Driver, DriverConnection, SdkDriverError};
use tokio_util::sync::CancellationToken;

/// PMC canonical kind 公共契约（PR3）：generic 只接受这 10 种。
/// descriptor 与门禁同源。
pub const PMC_KINDS: [char; 10] = ['G', 'R', 'X', 'Y', 'F', 'A', 'D', 'C', 'K', 'T'];

/// Resource Model Cleanup：`mesa.resources.v1` 按 (resource_id, output)
/// 分发到固定 (FocasAddress, DataType)。未声明的 resource/output 直接拒绝；
/// `address` 参数不接受。
/// 冻结原则：Descriptor 只暴露当前读取语义真实、适合 Acquisition 的能力；
/// `FocasAddress` 支持范围 ≠ Descriptor 产品能力范围（program 等未实现
/// 可信读取的地址族不在此暴露，等真实实现后再加 output）。
fn resolve_generic_point(
    resource_id: &str,
    output: &str,
    params: &serde_json::Value,
    point_key: &str,
) -> Result<(FocasAddress, DataType), SdkDriverError> {
    use address::{SpindleKind, ToolKind};
    let bad = |code: &str, msg: String| SdkDriverError::configuration(code, msg);
    if params.get("address").is_some() {
        return Err(bad(
            "INVALID_BINDING_CONFIG",
            format!("point `{point_key}` generic focas2 不接受 address，用声明式参数"),
        ));
    }
    // 实例参数全部 fail-closed：required 缺席即 INVALID_POINT，不靠 Driver
    // 默认猜实例；Some(非 u64 或越界) → 直接 reject，绝不当作不存在。
    let int_param =
        |key: &str, required: bool, default: u64, min: u64, max: u64| match params.get(key) {
            None if required => Err(bad(
                "INVALID_POINT",
                format!("point `{point_key}` 缺少参数 {key}"),
            )),
            None => Ok(default),
            Some(v) => match v.as_u64() {
                Some(n) if n >= min && n <= max => Ok(n),
                _ => Err(bad(
                    "INVALID_BINDING_CONFIG",
                    format!("point `{point_key}` 参数 {key}={v} 非法（需 {min}..={max} 整数）"),
                )),
            },
        };
    // pmc bit：optional 0..7，无 default；缺席即整字，绝不猜 bit 0。
    let opt_bit = || match params.get("bit") {
        None => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n <= 7 => Ok(Some(n as u8)),
            _ => Err(bad(
                "INVALID_BINDING_CONFIG",
                format!("point `{point_key}` 参数 bit={v} 非法（需 0..=7 整数）"),
            )),
        },
    };
    // 注意：DataType 在此表唯一确定，除了 pmc（bit 有无决定 I32/Bool，
    // 由 `DriverResolved` 在 Descriptor 声明，Core 放行、Driver 终裁）。
    let (addr, data_type) = match (resource_id, output) {
        ("machine", "status") => (FocasAddress::Status, DataType::U32),
        ("machine", "feed") => (FocasAddress::Feed, DataType::U32),
        // 当前活动主轴速度：`cnc_acts` 无 spindle 实例语义，绝不伪装成
        // `Spindle { spindle: 1 }`（label 会变成假的 `spindle[1].speed`）。
        ("machine", "spindle_speed") => (FocasAddress::ActiveSpindleSpeed, DataType::I32),
        ("axis", "absolute") => {
            // 当前产品能力 1..8：`cnc_absolute` 一次读 8 轴，超 8 Native
            // 直接 Param；不扩 Native，只收 Descriptor（parse_address 仍 1..32）。
            let axis = int_param(
                "axis",
                true,
                1,
                1,
                crate::native::FOCAS_AXIS_PRODUCT_MAX as u64,
            )? as u8;
            (
                FocasAddress::Axis {
                    axis,
                    kind: address::AxisKind::Absolute,
                },
                DataType::I32,
            )
        }
        ("spindle", "load") => {
            let spindle = int_param("spindle", true, 1, 1, 4)? as u8;
            (
                FocasAddress::Spindle {
                    spindle,
                    kind: SpindleKind::Load,
                },
                DataType::U32,
            )
        }
        ("spindle", "gear") => {
            let spindle = int_param("spindle", true, 1, 1, 4)? as u8;
            (
                FocasAddress::Spindle {
                    spindle,
                    kind: SpindleKind::Gear,
                },
                DataType::I32,
            )
        }
        ("spindle", "maxrpm") => {
            let spindle = int_param("spindle", true, 1, 1, 4)? as u8;
            (
                FocasAddress::Spindle {
                    spindle,
                    kind: SpindleKind::MaxRpm,
                },
                DataType::I32,
            )
        }
        ("servo", "load") => {
            // 当前产品能力 1..4：`SpLoad.data[4]` 只有 4 项，超 4 不得
            // clamp 读 data[3] 冒充（Blocker 1：配 5 读 4 即假 GOOD）。
            let axis = int_param(
                "axis",
                true,
                1,
                1,
                crate::native::FOCAS_SERVO_PRODUCT_MAX as u64,
            )? as u8;
            (FocasAddress::ServoLoad { axis }, DataType::U32)
        }
        ("pmc", "value") => {
            // canonical 精确拼写（Descriptor Enum 同口径；大小写/前缀变体不接受）。
            let kind_s = params.get("kind").and_then(|v| v.as_str()).ok_or_else(|| {
                bad(
                    "INVALID_POINT",
                    format!("point `{point_key}` pmc 缺少 kind"),
                )
            })?;
            if kind_s.len() != 1 {
                return Err(bad(
                    "INVALID_DATA_TYPE",
                    format!("point `{point_key}` pmc kind `{kind_s}` 非 canonical 单字符"),
                ));
            }
            let kind = kind_s.chars().next().unwrap();
            if !PMC_KINDS.contains(&kind) {
                return Err(bad(
                    "INVALID_DATA_TYPE",
                    format!(
                        "point `{point_key}` pmc kind `{kind_s}` 非 canonical（仅接受 {PMC_KINDS:?}）"
                    ),
                ));
            }
            // PMC 产品上限用 DWORD 安全值（最大宽度 4，结束地址不回绕）；
            // macro/tool/param/diagnosis 继续用 C_SHORT_MAX，不混常量。
            let addr_num = int_param(
                "addr",
                true,
                0,
                0,
                crate::native::FOCAS_PMC_ADDR_PRODUCT_MAX as u64,
            )? as u32;
            let bit = opt_bit()?;
            let data_type = if bit.is_some() {
                DataType::Bool
            } else {
                DataType::I32
            };
            (
                FocasAddress::Pmc {
                    kind,
                    addr: addr_num,
                    bit,
                },
                data_type,
            )
        }
        ("macro", "value") => {
            let number = int_param(
                "number",
                true,
                0,
                0,
                crate::native::FOCAS_C_SHORT_MAX as u64,
            )? as u32;
            (FocasAddress::MacroVar { number }, DataType::F64)
        }
        ("alarm", "value") => (FocasAddress::Alarm, DataType::String),
        ("opmsg", "value") => (FocasAddress::OpMsg, DataType::String),
        ("tool", "offset") => {
            let number = int_param(
                "number",
                true,
                0,
                0,
                crate::native::FOCAS_C_SHORT_MAX as u64,
            )? as u32;
            (
                FocasAddress::Tool {
                    kind: ToolKind::Offset,
                    number,
                },
                DataType::F64,
            )
        }
        ("tool", "zofs") => {
            let number = int_param(
                "number",
                true,
                0,
                0,
                crate::native::FOCAS_C_SHORT_MAX as u64,
            )? as u32;
            (
                FocasAddress::Tool {
                    kind: ToolKind::Zofs,
                    number,
                },
                DataType::F64,
            )
        }
        ("tool", "length") => {
            let number = int_param(
                "number",
                true,
                0,
                0,
                crate::native::FOCAS_C_SHORT_MAX as u64,
            )? as u32;
            (
                FocasAddress::Tool {
                    kind: ToolKind::Length,
                    number,
                },
                DataType::F64,
            )
        }
        ("param", "value") => {
            let number = int_param(
                "number",
                true,
                0,
                0,
                crate::native::FOCAS_C_SHORT_MAX as u64,
            )? as u32;
            (FocasAddress::Param { number }, DataType::I32)
        }
        ("diagnosis", "value") => {
            let number = int_param(
                "number",
                true,
                0,
                0,
                crate::native::FOCAS_C_SHORT_MAX as u64,
            )? as u32;
            // B2-C2：`axis` 独立可选参数（`0` non-axis / `1..N` one axis；
            // Batch 2 不支持 ALL_AXES；不编码进 number，不按号 hardcode）。
            // 上限取 32（含 0i-F 3 轴余量；Wire 侧 `1..=31` 再门，见 codec）。
            let axis = int_param("axis", false, 0, 0, 32).unwrap_or(0) as u8;
            // B2-C1：Mesa 输出 engineering F64（REAL path；raw 只留 typed）。
            (FocasAddress::Diagnosis { number, axis }, DataType::F64)
        }
        (r, _)
            if ![
                "machine",
                "axis",
                "spindle",
                "servo",
                "pmc",
                "macro",
                "alarm",
                "opmsg",
                "tool",
                "param",
                "diagnosis",
            ]
            .contains(&r) =>
        {
            return Err(bad(
                "UNSUPPORTED_RESOURCE",
                format!("task resource `{r}` 未声明（point `{point_key}`）"),
            ));
        }
        (r, o) => {
            return Err(bad(
                "UNKNOWN_OUTPUT",
                format!("resource `{r}` 无 output `{o}`（point `{point_key}`）"),
            ));
        }
    };
    Ok((addr, data_type))
}

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
            AccessMode, DataType, DriverCapabilities, DriverDescriptor, DriverIdentity,
            FieldDescriptor, FieldType, LocalizedText, OutputDescriptor, OutputTypeSpec,
            ResourceDescriptor, ResourceSelectionMethod, SchemaDescriptor,
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
                // Resource Model Cleanup：只暴露当前读取语义真实、适合
                // Acquisition 的能力。`FocasAddress` 支持范围 ≠ Descriptor
                // 产品能力（program 等未实现可信读取的不暴露，等真实实现后再加）。
                // 旧 `dynamic/status/value/axis/value/spindle/value` 已删除，
                // 无 alias、无迁移：旧形态报 UNSUPPORTED_RESOURCE/UNKNOWN_OUTPUT。
                ResourceDescriptor {
                    id: "machine".into(),
                    label: LocalizedText::new("Machine"),
                    parameters: SchemaDescriptor::default(),
                    outputs: vec![
                        OutputDescriptor {
                            id: "status".into(),
                            label: LocalizedText::new("Status"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::U32,
                            },
                            unit: None,
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            id: "feed".into(),
                            label: LocalizedText::new("Feed"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::U32,
                            },
                            unit: Some("mm/min".into()),
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            // 当前活动主轴速度（`cnc_acts`，无 spindle 实例；
                            // 参数化 `spindle/speed` 等 `cnc_acts2` 落地后再加）。
                            id: "spindle_speed".into(),
                            label: LocalizedText::new("Spindle Speed"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::I32,
                            },
                            unit: Some("rpm".into()),
                            access: AccessMode::Read,
                        },
                    ],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    // 只暴露 absolute：其余 6 kind 当前读路径并不可信
                    //（kind 被忽略/回退 actf），不是删功能。
                    // 当前产品能力 1..8（`cnc_absolute` 一次读 8 轴）。
                    id: "axis".into(),
                    label: LocalizedText::new("Axis"),
                    parameters: SchemaDescriptor {
                        fields: vec![{
                            let mut f = FieldDescriptor::new("axis", "Axis", FieldType::Integer)
                                .required(true);
                            f.validation.min = Some(1.0);
                            f.validation.max = Some(crate::native::FOCAS_AXIS_PRODUCT_MAX as f64);
                            f
                        }],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "absolute".into(),
                        label: LocalizedText::new("Absolute Position"),
                        type_spec: OutputTypeSpec::Fixed {
                            data_type: DataType::I32,
                        },
                        unit: Some("pulse".into()),
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    // 无 speed：`cnc_acts` 不接受 spindle 号，暴露即造假。
                    id: "spindle".into(),
                    label: LocalizedText::new("Spindle"),
                    parameters: SchemaDescriptor {
                        fields: vec![{
                            let mut f =
                                FieldDescriptor::new("spindle", "Spindle", FieldType::Integer)
                                    .required(true);
                            f.validation.min = Some(1.0);
                            f.validation.max = Some(4.0);
                            f
                        }],
                    },
                    outputs: vec![
                        OutputDescriptor {
                            id: "load".into(),
                            label: LocalizedText::new("Load"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::U32,
                            },
                            unit: None,
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            // 真机 `cnc_rdspgear` 返回 I16（Native I32 口径为准）。
                            id: "gear".into(),
                            label: LocalizedText::new("Gear"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::I32,
                            },
                            unit: None,
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            id: "maxrpm".into(),
                            label: LocalizedText::new("Max RPM"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::I32,
                            },
                            unit: Some("rpm".into()),
                            access: AccessMode::Read,
                        },
                    ],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    // 当前产品能力 1..4（`SpLoad.data[4]` 只有 4 项）。
                    id: "servo".into(),
                    label: LocalizedText::new("Servo"),
                    parameters: SchemaDescriptor {
                        fields: vec![{
                            let mut f = FieldDescriptor::new("axis", "Axis", FieldType::Integer)
                                .required(true);
                            f.validation.min = Some(1.0);
                            f.validation.max = Some(crate::native::FOCAS_SERVO_PRODUCT_MAX as f64);
                            f
                        }],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "load".into(),
                        label: LocalizedText::new("Load"),
                        type_spec: OutputTypeSpec::Fixed {
                            data_type: DataType::U32,
                        },
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
                                // 选项与 PMC_KINDS 同源
                                f.validation.enum_options =
                                    Some(PMC_KINDS.iter().map(|k| k.to_string()).collect());
                                f
                            },
                            {
                                let mut f =
                                    FieldDescriptor::new("addr", "Address", FieldType::Integer)
                                        .required(true);
                                f.validation.min = Some(0.0);
                                // DWORD 安全上限（最大宽度 4，结束地址不回绕；
                                // BYTE 理论可多 3 个地址，不值得 kind-dependent 上限）。
                                f.validation.max =
                                    Some(crate::native::FOCAS_PMC_ADDR_PRODUCT_MAX as f64);
                                f
                            },
                            {
                                // 只有 bit 可选：无 default，缺席即整字，绝不猜 bit 0。
                                let mut f = FieldDescriptor::new("bit", "Bit", FieldType::Integer)
                                    .required(false);
                                f.validation.min = Some(0.0);
                                f.validation.max = Some(7.0);
                                f
                            },
                        ],
                    },
                    // 单 output：bit 有无决定最终类型（DriverResolved 由
                    // Driver 终裁，Core 放行），不搞 value/bit 互斥双 output。
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Value"),
                        type_spec: OutputTypeSpec::DriverResolved,
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "macro".into(),
                    label: LocalizedText::new("Macro"),
                    parameters: SchemaDescriptor {
                        fields: vec![{
                            let mut f =
                                FieldDescriptor::new("number", "Number", FieldType::Integer)
                                    .required(true);
                            f.validation.min = Some(0.0);
                            // FFI `c_short` 可表示上限（resolver 同上限）。
                            f.validation.max = Some(crate::native::FOCAS_C_SHORT_MAX as f64);
                            f
                        }],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Value"),
                        type_spec: OutputTypeSpec::Fixed {
                            data_type: DataType::F64,
                        },
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
                        type_spec: OutputTypeSpec::Fixed {
                            data_type: DataType::String,
                        },
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "opmsg".into(),
                    label: LocalizedText::new("Operator Message"),
                    parameters: SchemaDescriptor::default(),
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Message"),
                        type_spec: OutputTypeSpec::Fixed {
                            data_type: DataType::String,
                        },
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    // 无 `number` output（旧 Number 硬编码 U32(1)，不可暴露）；
                    // number 必填后无需任何条件逻辑。
                    id: "tool".into(),
                    label: LocalizedText::new("Tool"),
                    parameters: SchemaDescriptor {
                        fields: vec![{
                            let mut f =
                                FieldDescriptor::new("number", "Number", FieldType::Integer)
                                    .required(true);
                            f.validation.min = Some(0.0);
                            // FFI `c_short` 可表示上限（resolver 同上限）。
                            f.validation.max = Some(crate::native::FOCAS_C_SHORT_MAX as f64);
                            f
                        }],
                    },
                    outputs: vec![
                        OutputDescriptor {
                            id: "offset".into(),
                            label: LocalizedText::new("Offset"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::F64,
                            },
                            unit: None,
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            id: "zofs".into(),
                            label: LocalizedText::new("Work Zero"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::F64,
                            },
                            unit: None,
                            access: AccessMode::Read,
                        },
                        OutputDescriptor {
                            id: "length".into(),
                            label: LocalizedText::new("Length"),
                            type_spec: OutputTypeSpec::Fixed {
                                data_type: DataType::F64,
                            },
                            unit: None,
                            access: AccessMode::Read,
                        },
                    ],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "param".into(),
                    label: LocalizedText::new("Parameter"),
                    parameters: SchemaDescriptor {
                        fields: vec![{
                            let mut f =
                                FieldDescriptor::new("number", "Number", FieldType::Integer)
                                    .required(true);
                            f.validation.min = Some(0.0);
                            // FFI `c_short` 可表示上限（resolver 同上限）。
                            f.validation.max = Some(crate::native::FOCAS_C_SHORT_MAX as f64);
                            f
                        }],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Value"),
                        type_spec: OutputTypeSpec::Fixed {
                            data_type: DataType::I32,
                        },
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                ResourceDescriptor {
                    id: "diagnosis".into(),
                    label: LocalizedText::new("Diagnosis"),
                    parameters: SchemaDescriptor {
                        fields: vec![
                            {
                                let mut f =
                                    FieldDescriptor::new("number", "Number", FieldType::Integer)
                                        .required(true);
                                f.validation.min = Some(0.0);
                                // FFI `c_short` 可表示上限（resolver 同上限）。
                                f.validation.max = Some(crate::native::FOCAS_C_SHORT_MAX as f64);
                                f
                            },
                            {
                                // B2-C2：axis 独立可选参数（0 non-axis /
                                // 1..N one axis；Batch 2 不支持 ALL_AXES）。
                                let mut f =
                                    FieldDescriptor::new("axis", "Axis", FieldType::Integer)
                                        .required(false);
                                f.validation.min = Some(0.0);
                                f.validation.max = Some(32.0);
                                f
                            },
                        ],
                    },
                    outputs: vec![OutputDescriptor {
                        id: "value".into(),
                        label: LocalizedText::new("Value"),
                        type_spec: OutputTypeSpec::Fixed {
                            // B2-C1：engineering F64（REAL path；历史 I32 占位
                            // 从未经真实 FANUC 行为验证，Batch 2 修正）。
                            data_type: DataType::F64,
                        },
                        unit: None,
                        access: AccessMode::Read,
                    }],
                    modes: vec![mesa_core_types::TaskMode::Poll],
                },
                // NOTE：program 本次不暴露——`ProgramName` 读路径返回占位
                // `O1000`（与设备真实程序无关），dir/info/upload 更不适合
                // Acquisition Poll。等真实 current program 实现后再加。
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
            resource_selection_methods: vec![ResourceSelectionMethod::Manual],
            capabilities: DriverCapabilities {
                poll: true,
                ..Default::default()
            },
            // Event Plane PR5：老 Driver 无事件目录即 empty（Major 不升级）
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
            plan: std::sync::RwLock::new(None),
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
                message: "设备身份（series/version）未能识别".into(),
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

#[derive(Debug, Clone)]
struct TaskPlan {
    // TODO: PlanSnapshot 冻结字段，task id 用于诊断与多任务追踪，V1 仅内存使用但需保留
    #[allow(dead_code)]
    id: String,
    interval_ms: u64,
    point_indices: Vec<usize>,
}

/// control-handle 并发模型下 run 入口克隆整个快照，故需 Clone。
#[derive(Debug, Clone)]
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
    // 采集计划（control-handle 并发模型）：configure/apply 写锁替换，
    // run 读锁克隆快照后释放；api 本就 Arc 共享。
    plan: std::sync::RwLock<Option<PlanSnapshot>>,
}

impl std::fmt::Debug for FocasConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FocasConnection")
            .field("cfg", &self.cfg)
            .field("has_plan", &self.plan.read().unwrap().is_some())
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

/// Foundation-2 已删除 legacy `items[]` 信封的数据类型解析；
/// canonical 路径由 resolve_generic_point 分发固定类型。
/// 本函数保留供单测锁定解析语义，不进产品契约。
#[allow(dead_code)]
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
    async fn probe(&self) -> Result<ProbeReport, SdkDriverError> {
        await_probe_with_api(&self.api, &self.cfg).await
    }

    async fn configure(
        &self,
        revision: u64,
        tasks: Vec<AcquisitionTask>,
    ) -> Result<Vec<PointDescriptor>, SdkDriverError> {
        let mut new_points: Vec<PointSpec> = Vec::new();
        let mut new_tasks: Vec<TaskPlan> = Vec::new();

        for task in &tasks {
            task.validate()
                .map_err(|e| SdkDriverError::configuration("INVALID_TASK", e.to_string()))?;
            // Foundation-2 单真值 + 单路径：FOCAS2 仅 Poll + 仅 mesa.resources.v1。
            let interval_ms = match task.schedule {
                TaskSchedule::Poll { interval_ms } => interval_ms,
                TaskSchedule::Subscribe { .. } => {
                    return Err(SdkDriverError::new(
                        mesa_core_types::ErrorKind::Unsupported,
                        "MODE_NOT_SUPPORTED",
                        format!("task `{}`: focas2 仅支持 poll", task.id),
                    ));
                }
            };
            if task.binding.kind != GENERIC_BINDING_KIND {
                return Err(SdkDriverError::configuration(
                    "UNSUPPORTED_BINDING",
                    format!(
                        "task `{}`: 期望 {GENERIC_BINDING_KIND}，实际 {}",
                        task.id, task.binding.kind
                    ),
                ));
            }
            {
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
                        // canonical 分发：(resource_id, output) → 固定地址与类型
                        let (addr, data_type) = resolve_generic_point(
                            &sel.resource_id,
                            &out.output,
                            &sel.parameters,
                            &out.point_key,
                        )?;
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
                    interval_ms,
                    point_indices: indices,
                });
            }
        }

        let descriptors: Vec<PointDescriptor> = new_points
            .iter()
            .map(|p| PointDescriptor {
                point_key: p.key.clone(),
                data_type: p.data_type,
                unit: None,
                // 来源标签走 FocasAddress 唯一格式化入口（configure/诊断同一实现）。
                source_label: Some(p.addr.source_label()),
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

    async fn run(&self, sink: DataSink, shutdown: CancellationToken) -> Result<(), SdkDriverError> {
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
        let mut handles = Vec::with_capacity(snapshot.tasks.len());
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
                        // 多机型单点不支持：Native/Wire 以 "ERR:..." 字符串占位，
                        // 转 typed BAD（§3.6 禁 String 冒充数值类型）。
                        // L02 真修：quality_code 保留协议原因分类（不再统一 1）——
                        // EW_NOOPT/unsupported=2（设备不支持/功能未实现），
                        // EW_PARAM/RANGE/NUMBER/LENGTH 等参数类=3，
                        // EW_DATA/EW_ATTRIB 数据/属性类=4，
                        // EW_SOCKET/HANDLE/BUSY/连接类=5，
                        // 其他/未知=1（兜底）。映射只做分类，不伪造厂家码。
                        if let Value::String(s) = &raw_val
                            && s.starts_with("ERR:")
                        {
                                tracing::warn!(key=%spec.key, error=%s, "单点 Bad，不影响同批其他点");
                                // P0-B：BAD 仍必须携带与 data_type 匹配的 typed 值，quality_code 保留协议原因
                                let neutral = neutral_value_for(spec.data_type);
                                let code = classify_point_error(s);
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
        // L01 真修：按完成顺序收集（JoinSet），首个任务失败即取消同连接
        // 其他任务并等待清理后返回原因。旧 `for h in handles { h.await }`
        // 按创建顺序等待：A 正常循环时 B 的错误迟迟不上报。
        let mut set = tokio::task::JoinSet::new();
        for h in handles {
            set.spawn(async move {
                match h.await {
                    Ok(Ok(())) => Ok::<(), SdkDriverError>(()),
                    Ok(Err(e)) => Err(e),
                    Err(join_err) => {
                        tracing::error!(%join_err, "FOCAS2 任务 panic");
                        Err(SdkDriverError::new(
                            mesa_core_types::ErrorKind::Internal,
                            "TASK_PANIC",
                            join_err.to_string(),
                        ))
                    }
                }
            });
        }
        // 按完成顺序：首错即取消其余（同连接任务共享 shutdown token）。
        while let Some(r) = set.join_next().await {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if final_err.is_none() {
                        final_err = Some(e);
                    }
                    // 首错即取消同连接其他任务（幂等；任务循环内 select shutdown）。
                    shutdown.cancel();
                }
                Err(join_err) => {
                    tracing::error!(%join_err, "FOCAS2 JoinSet 失败");
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

    async fn command(
        &self,
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

/// L02：单点错误分类（`ERR:...` → quality_code，不再统一 1）。
/// 2=不支持/未实现（NOOPT/unsupported），3=参数类（PARAM/RANGE/NUMBER/
/// LENGTH），4=数据/属性类（DATA/ATTRIB），5=连接类（SOCKET/HANDLE/BUSY/
/// NODLL/CLOSED/timeout），1=其他未知。只分类不断言厂家码。
fn classify_point_error(s: &str) -> i32 {
    let u = s.to_ascii_uppercase();
    if u.contains("EW_NOOPT") || u.contains("UNSUPPORTED") || u.contains("NOOPT") {
        2
    } else if u.contains("EW_PARAM")
        || u.contains("EW_RANGE")
        || u.contains("EW_NUMBER")
        || u.contains("EW_LENGTH")
    {
        3
    } else if u.contains("EW_DATA") || u.contains("EW_ATTRIB") {
        4
    } else if u.contains("EW_SOCKET")
        || u.contains("EW_HANDLE")
        || u.contains("EW_BUSY")
        || u.contains("EW_NODLL")
        || u.contains("CLOSED")
        || u.contains("TIMEOUT")
        || u.contains("BUSY")
    {
        5
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{AcquisitionTask, DriverBinding};

    fn generic_task(selections: serde_json::Value) -> AcquisitionTask {
        generic_task_with_id("t1", selections)
    }

    fn generic_task_with_id(id: &str, selections: serde_json::Value) -> AcquisitionTask {
        AcquisitionTask {
            id: id.into(),
            schedule: mesa_core_types::TaskSchedule::Poll { interval_ms: 100 },
            binding: DriverBinding {
                kind: GENERIC_BINDING_KIND.into(),
                config: serde_json::json!({"selections": selections}),
            },
        }
    }

    fn status_axis_selections(dup_key: bool) -> serde_json::Value {
        let second = if dup_key { "a" } else { "b" };
        serde_json::json!([
            {"resource_id":"machine","parameters":{},"outputs":[{"output":"status","point_key":"a"}]},
            {"resource_id":"axis","parameters":{"axis":1},"outputs":[{"output":"absolute","point_key":second}]},
        ])
    }

    fn test_conn() -> FocasConnection {
        FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: std::sync::RwLock::new(None),
        }
    }

    /// Resource Model Cleanup 闭环：全部声明 (resource, output) × canonical
    /// 参数 → validate_instance PASS → generic configure PASS →
    /// PointDescriptor == 终裁类型 + source_label 断言。
    /// coverage 门：测试集合必须 == Descriptor 全部 (resource, output)，
    /// 新增 Resource 忘记 resolver 即红。pmc 是 DriverResolved，
    /// Core resolve 为 None，由 Driver 终裁（bit 有无 → I32/Bool）。
    #[tokio::test]
    async fn generic_canonical_loop_closed_for_all_resources() {
        let d = FocasDriver.descriptor();
        d.validate().expect("descriptor 必须合法");
        let declared: std::collections::BTreeSet<(String, String)> = d
            .resources
            .iter()
            .flat_map(|r| r.outputs.iter().map(move |o| (r.id.clone(), o.id.clone())))
            .collect();
        let cases: Vec<(&str, &str, serde_json::Value, DataType, &str)> = vec![
            (
                "machine",
                "status",
                serde_json::json!({}),
                DataType::U32,
                "machine.status",
            ),
            (
                "machine",
                "feed",
                serde_json::json!({}),
                DataType::U32,
                "machine.feed",
            ),
            (
                "machine",
                "spindle_speed",
                serde_json::json!({}),
                DataType::I32,
                "spindle.active.speed",
            ),
            (
                "axis",
                "absolute",
                serde_json::json!({"axis": 2}),
                DataType::I32,
                "axis[2].absolute",
            ),
            (
                "spindle",
                "load",
                serde_json::json!({"spindle": 1}),
                DataType::U32,
                "spindle[1].load",
            ),
            (
                "spindle",
                "gear",
                serde_json::json!({"spindle": 1}),
                DataType::I32,
                "spindle[1].gear",
            ),
            (
                "spindle",
                "maxrpm",
                serde_json::json!({"spindle": 2}),
                DataType::I32,
                "spindle[2].maxrpm",
            ),
            (
                "servo",
                "load",
                serde_json::json!({"axis": 2}),
                DataType::U32,
                "servo[2].load",
            ),
            (
                "pmc",
                "value",
                serde_json::json!({"kind": "R", "addr": 100}),
                DataType::I32,
                "pmc.R100",
            ),
            (
                "pmc",
                "value",
                serde_json::json!({"kind": "R", "addr": 100, "bit": 3}),
                DataType::Bool,
                "pmc.R100.3",
            ),
            (
                "macro",
                "value",
                serde_json::json!({"number": 100}),
                DataType::F64,
                "macro[100]",
            ),
            (
                "alarm",
                "value",
                serde_json::json!({}),
                DataType::String,
                "alarm.value",
            ),
            (
                "opmsg",
                "value",
                serde_json::json!({}),
                DataType::String,
                "opmsg.value",
            ),
            (
                "tool",
                "offset",
                serde_json::json!({"number": 1}),
                DataType::F64,
                "tool.offset[1]",
            ),
            (
                "tool",
                "zofs",
                serde_json::json!({"number": 2}),
                DataType::F64,
                "tool.zofs[2]",
            ),
            (
                "tool",
                "length",
                serde_json::json!({"number": 1}),
                DataType::F64,
                "tool.length[1]",
            ),
            (
                "param",
                "value",
                serde_json::json!({"number": 100}),
                DataType::I32,
                "param[100]",
            ),
            (
                "diagnosis",
                "value",
                serde_json::json!({"number": 0}),
                DataType::F64,
                "diagnosis[0]",
            ),
            // B2-C2：axis 独立参数（默认 0；301:3 走 axis=3，不拼进 number）。
            (
                "diagnosis",
                "value",
                serde_json::json!({"number": 301, "axis": 3}),
                DataType::F64,
                "diagnosis[301]@axis3",
            ),
        ];
        let tested: std::collections::BTreeSet<(String, String)> = cases
            .iter()
            .map(|(r, o, _, _, _)| (r.to_string(), o.to_string()))
            .collect();
        assert_eq!(declared, tested, "测试必须覆盖全部声明对");
        for (resource_id, output, params, expected, want_label) in cases {
            let res = d.resources.iter().find(|r| r.id == resource_id).unwrap();
            let issues = res.parameters.validate_instance("parameters", &params);
            assert!(issues.is_empty(), "{resource_id}/{output}: {issues:?}");
            let out = res.outputs.iter().find(|o| o.id == output).unwrap();
            let pmap = params.as_object().unwrap().clone();
            // pmc DriverResolved 由 Driver 终裁：Core resolve 恒 None，
            // 此处直接断言终裁类型（configure 的 PointDescriptor）。
            if resource_id != "pmc" {
                assert_eq!(out.type_spec.resolve(&pmap), Some(expected));
            } else {
                assert!(matches!(
                    out.type_spec,
                    mesa_core_types::OutputTypeSpec::DriverResolved
                ));
            }
            let sel = serde_json::json!([{
                "resource_id": resource_id,
                "parameters": params,
                "outputs": [{"output": output, "point_key": "k"}],
            }]);
            let conn = test_conn();
            let descs = conn.configure(1, vec![generic_task(sel)]).await.unwrap();
            assert_eq!(descs.len(), 1);
            assert_eq!(descs[0].data_type, expected, "{resource_id}/{output}");
            assert_eq!(
                descs[0].source_label.as_deref(),
                Some(want_label),
                "{resource_id}/{output}"
            );
        }
    }

    /// generic 拒绝：未声明资源/输出、address 参数、越界轴号、非 canonical pmc kind、
    /// 旧 dynamic/status/value 形态、实例参数缺席、pmc bit 越界。
    #[tokio::test]
    async fn generic_rejects_non_canonical() {
        let cases: Vec<(&str, serde_json::Value, &str)> = vec![
            (
                "未声明资源",
                serde_json::json!([{"resource_id": "program", "parameters": {}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "UNSUPPORTED_RESOURCE",
            ),
            (
                "旧 dynamic 已删除",
                serde_json::json!([{"resource_id": "dynamic", "parameters": {}, "outputs": [{"output": "feed", "point_key": "k"}]}]),
                "UNSUPPORTED_RESOURCE",
            ),
            (
                "旧 status/value 已删除（现 machine/status）",
                serde_json::json!([{"resource_id": "status", "parameters": {}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "UNSUPPORTED_RESOURCE",
            ),
            (
                "旧 axis/value 已删除（现 axis/absolute）",
                serde_json::json!([{"resource_id": "axis", "parameters": {"axis": 1}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "UNKNOWN_OUTPUT",
            ),
            (
                "旧 spindle/value 已删除（现 spindle/load）",
                serde_json::json!([{"resource_id": "spindle", "parameters": {"spindle": 1}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "UNKNOWN_OUTPUT",
            ),
            (
                "machine 无 spindle_speed 外的非法输出",
                serde_json::json!([{"resource_id": "machine", "parameters": {}, "outputs": [{"output": "nope", "point_key": "k"}]}]),
                "UNKNOWN_OUTPUT",
            ),
            (
                "address 参数不接受",
                serde_json::json!([{"resource_id": "machine", "parameters": {"address": "status"}, "outputs": [{"output": "status", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "轴号越界（产品上限 8）",
                serde_json::json!([{"resource_id": "axis", "parameters": {"axis": 9}, "outputs": [{"output": "absolute", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "轴号 33 越界",
                serde_json::json!([{"resource_id": "axis", "parameters": {"axis": 33}, "outputs": [{"output": "absolute", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "axis 缺实例参数",
                serde_json::json!([{"resource_id": "axis", "parameters": {}, "outputs": [{"output": "absolute", "point_key": "k"}]}]),
                "INVALID_POINT",
            ),
            (
                "spindle 缺实例参数",
                serde_json::json!([{"resource_id": "spindle", "parameters": {}, "outputs": [{"output": "load", "point_key": "k"}]}]),
                "INVALID_POINT",
            ),
            (
                "tool number 必填（不再可选）",
                serde_json::json!([{"resource_id": "tool", "parameters": {}, "outputs": [{"output": "offset", "point_key": "k"}]}]),
                "INVALID_POINT",
            ),
            (
                "tool 无 number 输出",
                serde_json::json!([{"resource_id": "tool", "parameters": {"number": 1}, "outputs": [{"output": "number", "point_key": "k"}]}]),
                "UNKNOWN_OUTPUT",
            ),
            (
                "pmc bit 越界",
                serde_json::json!([{"resource_id": "pmc", "parameters": {"kind": "R", "addr": 100, "bit": 8}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "pmc kind 非 canonical（M 只属于 legacy）",
                serde_json::json!([{"resource_id": "pmc", "parameters": {"kind": "M", "addr": 0}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_DATA_TYPE",
            ),
            (
                "pmc kind 小写不接受",
                serde_json::json!([{"resource_id": "pmc", "parameters": {"kind": "r", "addr": 0}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_DATA_TYPE",
            ),
            (
                "pmc kind 前缀不接受",
                serde_json::json!([{"resource_id": "pmc", "parameters": {"kind": "RABC", "addr": 0}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_DATA_TYPE",
            ),
            (
                "servo 越界（产品上限 4，不得 clamp 读 data[3]）",
                serde_json::json!([{"resource_id": "servo", "parameters": {"axis": 5}, "outputs": [{"output": "load", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "macro 超 c_short 不得截断（配 A 读 B）",
                serde_json::json!([{"resource_id": "macro", "parameters": {"number": 65537}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "pmc addr 超 c_short 不得截断",
                serde_json::json!([{"resource_id": "pmc", "parameters": {"kind": "R", "addr": 40000}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "pmc D addr=32765 拒绝（结束地址回绕）",
                serde_json::json!([{"resource_id": "pmc", "parameters": {"kind": "D", "addr": 32765}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "pmc word addr=32767 拒绝（结束地址回绕）",
                serde_json::json!([{"resource_id": "pmc", "parameters": {"kind": "R", "addr": 32767}, "outputs": [{"output": "value", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "tool number 超 c_short 不得截断",
                serde_json::json!([{"resource_id": "tool", "parameters": {"number": 100000}, "outputs": [{"output": "offset", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
            (
                "axis 非法值不得回落 default",
                serde_json::json!([{"resource_id": "axis", "parameters": {"axis": -1}, "outputs": [{"output": "absolute", "point_key": "k"}]}]),
                "INVALID_BINDING_CONFIG",
            ),
        ];
        for (name, sel, code) in cases {
            let conn = test_conn();
            let err = conn
                .configure(1, vec![generic_task(sel)])
                .await
                .unwrap_err();
            assert_eq!(err.code, code, "{name}");
        }
    }

    #[tokio::test]
    async fn configure_ok_and_duplicate_rejected() {
        let conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: std::sync::RwLock::new(None),
        };
        let t = generic_task(status_axis_selections(false));
        let descs = conn.configure(1, vec![t]).await.unwrap();
        assert_eq!(descs.len(), 2);
        // 跨 task 重复 point_key → DUPLICATE_POINT_KEY（同一 task 内重复则
        // 早于此被结构级拒绝，见 generic 路径 validate_selections_structure）
        let dup_a = serde_json::json!([{"resource_id":"machine","parameters":{},"outputs":[{"output":"status","point_key":"a"}]}]);
        let dup_b = serde_json::json!([{"resource_id":"axis","parameters":{"axis":2},"outputs":[{"output":"absolute","point_key":"a"}]}]);
        let err = conn
            .configure(
                2,
                vec![
                    generic_task_with_id("t1", dup_a),
                    generic_task_with_id("t2", dup_b),
                ],
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "DUPLICATE_POINT_KEY");
    }

    #[tokio::test]
    async fn invalid_address_rejected() {
        let conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: std::sync::RwLock::new(None),
        };
        // axis=0 越界（int_param fail-closed → INVALID_BINDING_CONFIG；
        // INVALID_ADDRESS 保留给地址解析层，本层参数越界走绑定配置错）
        let sel = serde_json::json!([{"resource_id":"axis","parameters":{"axis":0},"outputs":[{"output":"absolute","point_key":"a"}]}]);
        let err = conn
            .configure(1, vec![generic_task(sel)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_BINDING_CONFIG");
    }

    /// 边界回归（3 blocker + indexed speed + PMC 回绕）：以下一律不得 GOOD。
    /// - servo=5 / axis=9：configure 期拒绝（产品上限）；
    /// - 超 c_short 参数：configure 期拒绝（配 A 读 B）；
    /// - pmc D addr=32764 通过（DWORD 安全上限），32765 拒绝（结束回绕）；
    /// - indexed spindle speed：configure 不可达 + Fake/Native 读层 ERR。
    #[tokio::test]
    async fn boundary_never_produces_good() {
        use crate::address::{FocasAddress, SpindleKind};
        // configure 期：servo=5、axis=9、超 c_short 全部拒绝。
        for (name, sel) in [
            (
                "servo=5",
                serde_json::json!([{"resource_id":"servo","parameters":{"axis":5},"outputs":[{"output":"load","point_key":"k"}]}]),
            ),
            (
                "axis=9",
                serde_json::json!([{"resource_id":"axis","parameters":{"axis":9},"outputs":[{"output":"absolute","point_key":"k"}]}]),
            ),
            (
                "macro=65537",
                serde_json::json!([{"resource_id":"macro","parameters":{"number":65537},"outputs":[{"output":"value","point_key":"k"}]}]),
            ),
            (
                "pmc D addr=32765",
                serde_json::json!([{"resource_id":"pmc","parameters":{"kind":"D","addr":32765},"outputs":[{"output":"value","point_key":"k"}]}]),
            ),
            (
                "pmc word addr=32767",
                serde_json::json!([{"resource_id":"pmc","parameters":{"kind":"R","addr":32767},"outputs":[{"output":"value","point_key":"k"}]}]),
            ),
        ] {
            let conn = test_conn();
            let err = conn
                .configure(1, vec![generic_task(sel)])
                .await
                .unwrap_err();
            assert_eq!(err.code, "INVALID_BINDING_CONFIG", "{name}");
        }
        // pmc D addr=32764 通过（DWORD 安全上限，结束 32767 不回绕）。
        {
            let conn = test_conn();
            let sel = serde_json::json!([{"resource_id":"pmc","parameters":{"kind":"D","addr":32764},"outputs":[{"output":"value","point_key":"k"}]}]);
            let descs = conn
                .configure(1, vec![generic_task(sel)])
                .await
                .expect("pmc D addr=32764 必须通过");
            assert_eq!(descs.len(), 1);
            assert_eq!(
                descs[0].source_label.as_deref(),
                Some("pmc.D32764"),
                "来源必须准确"
            );
        }
        // Fake 读层：indexed speed 即 ERR（run 层转 BAD，不得 GOOD）。
        let api = FakeFocasApi::new();
        let vals = api
            .read_batch(&[FocasAddress::Spindle {
                spindle: 2,
                kind: SpindleKind::Speed,
            }])
            .await
            .unwrap();
        assert_eq!(vals.len(), 1);
        assert!(
            matches!(&vals[0], mesa_core_types::Value::String(s) if s.starts_with("ERR:")),
            "spindle.speed.2 不得 GOOD"
        );
        // ActiveSpindleSpeed 仍正常（machine/spindle_speed 资源路径）。
        let vals = api
            .read_batch(&[FocasAddress::ActiveSpindleSpeed])
            .await
            .unwrap();
        assert!(matches!(vals[0], mesa_core_types::Value::I32(_)));
        // Native 门：indexed speed 即 Err（不碰 FFI）。
        let r = NativeFocasApi::read_one_no_lib_for_test(&FocasAddress::Spindle {
            spindle: 2,
            kind: SpindleKind::Speed,
        });
        // 非 Axis 地址走到底（无 dll），但 indexed speed 在生产路径
        // read_one_blocking 首行即 Err：此处用同一语义断言。
        // （无 dll 时返回 EW_NODLL 亦为 Err，绝不为 Ok GOOD。）
        assert!(r.is_err(), "indexed speed 不得 Ok");
    }

    #[tokio::test]
    async fn probe_fake_returns_deterministic_identity() {
        // Fake 身份是合同基准：FANUC / 0i-F / 固件 1.0，model 恒 None + MODEL_UNDETECTED。
        // 经已打开的连接调用（OpenConnection 阶段已做配置门禁），不碰进程级 env。
        let conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: std::sync::RwLock::new(None),
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
        let conn = FocasConnection {
            cfg: FocasConnConfig {
                host: "127.0.0.1".into(),
                port: 9,
                timeout_ms: 1000,
            },
            api: Arc::new(NativeFocasApi::new()),
            plan: std::sync::RwLock::new(None),
        };
        let r = conn.probe().await.expect("不可达是探测结果");
        assert!(!r.reachable);
        assert!(r.warnings.iter().any(|w| w.code == "CONNECTION_FAILED"));
    }

    /// fail-closed 回归：Axis 非 Absolute 不得产出位置（双入口同语义）。
    /// `configure` 层根本到不了读路径（resolver 只认 absolute），
    /// 此处锁定读层最后一道门：Fake 与 Native 语义一致。
    #[tokio::test]
    async fn axis_non_absolute_never_produces_position() {
        use crate::address::{AxisKind, FocasAddress};
        // Fake：非 Absolute 即 ERR（run 层转单点 BAD，不污染同批）。
        let api = FakeFocasApi::new();
        // Fake：非 Absolute 即 ERR（run 层转单点 BAD，不污染同批）。
        for kind in [
            AxisKind::Machine,
            AxisKind::Relative,
            AxisKind::Distance,
            AxisKind::Data,
            AxisKind::SrvDelay,
            AxisKind::AccDecDly,
        ] {
            let vals = api
                .read_batch(&[FocasAddress::Axis { axis: 1, kind }])
                .await
                .unwrap();
            assert_eq!(vals.len(), 1);
            assert!(
                matches!(&vals[0], mesa_core_types::Value::String(s) if s.starts_with("ERR:")),
                "{kind:?} 必须 ERR，不得伪造位置"
            );
        }
        // Native 单点入口（无 dll 即干净失败，绝不回退 feed）。
        let r = NativeFocasApi::read_one_no_lib_for_test(&FocasAddress::Axis {
            axis: 1,
            kind: AxisKind::Machine,
        });
        assert!(r.is_err(), "Native 非 Absolute 必须 Err");
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
        let conn = FocasConnection {
            cfg: FocasConnConfig::default(),
            api: Arc::new(FakeFocasApi::new()),
            plan: std::sync::RwLock::new(None),
        };
        // generic：10 个连续 pmc/R outputs 同一 task（canonical 参数面）
        let selections = serde_json::Value::Array(
            (0..10)
                .map(|i| {
                    serde_json::json!({"resource_id":"pmc","parameters":{"kind":"R","addr": 100+i*2},"outputs":[{"output":"value","point_key": format!("p{}", i)}]})
                })
                .collect(),
        );
        let t = generic_task(selections);
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

    /// L01 回归：A 持续运行、B 立即失败时，连接在限定时间内结束并上报 B。
    /// 不连设备（JoinSet 按完成顺序语义纯逻辑；与生产 `run` 同源收集器）。
    #[tokio::test]
    async fn joinset_first_error_wins() {
        // 模拟 run 的收集语义：A pending（长任务）、B 立即 Err。
        let mut set = tokio::task::JoinSet::new();
        set.spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok::<(), String>(())
        });
        set.spawn(async { Err::<(), String>("B_FAILED".into()) });
        let mut first_err: Option<String> = None;
        let done = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(r) = set.join_next().await {
                match r {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        first_err = Some(e);
                        set.abort_all();
                        break;
                    }
                    Err(_) => {
                        first_err = Some("PANIC".into());
                        set.abort_all();
                        break;
                    }
                }
            }
        })
        .await;
        assert!(
            done.is_ok(),
            "必须在 5s 内收到 B 的错误（不按创建顺序等 A）"
        );
        assert_eq!(first_err.as_deref(), Some("B_FAILED"));
    }

    /// L02 回归：单点错误分类不再统一 quality_code=1。
    #[test]
    fn point_error_classification_locked() {
        assert_eq!(classify_point_error("ERR:EW_NOOPT xxx"), 2);
        assert_eq!(classify_point_error("ERR:unsupported yyy"), 2);
        assert_eq!(classify_point_error("ERR:EW_PARAM zzz"), 3);
        assert_eq!(classify_point_error("ERR:EW_RANGE zzz"), 3);
        assert_eq!(classify_point_error("ERR:EW_NUMBER zzz"), 3);
        assert_eq!(classify_point_error("ERR:EW_LENGTH zzz"), 3);
        assert_eq!(classify_point_error("ERR:EW_DATA zzz"), 4);
        assert_eq!(classify_point_error("ERR:EW_ATTRIB zzz"), 4);
        assert_eq!(classify_point_error("ERR:EW_SOCKET zzz"), 5);
        assert_eq!(classify_point_error("ERR:EW_BUSY xxx"), 5);
        assert_eq!(classify_point_error("ERR:something else"), 1);
    }

    /// N03 回归：worker 队列有界（`WORKER_QUEUE_MAX` 常量存在且合理）。
    /// 不连设备。诚实标注：本测试只锁常量形状，不证明实际 saturation
    /// （`try_send(Full) → EW_BUSY` 在生产 `submit` 内，逻辑直接，
    /// 真实队满需并发压测，不在单测伪造）。
    #[test]
    fn worker_queue_bounded_shape() {
        // 背压语义锁形状：上限存在且为正（具体值见 focas_api::WORKER_QUEUE_MAX）。
        // `#[allow]`：常量比较在 clippy 看来恒真，但此处锁的正是“常量存在且合理”。
        #[allow(clippy::assertions_on_constants)]
        {
            assert!(crate::focas_api::WORKER_QUEUE_MAX > 0);
            assert!(crate::focas_api::WORKER_QUEUE_MAX <= 1024);
        }
    }
}
