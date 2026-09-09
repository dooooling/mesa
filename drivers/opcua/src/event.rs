//! OPC UA 事件语义（PR9）：Mesa 映射层（transport/event.rs = wire 语义，本模块 = 映射语义）。
//!
//! 本文件职责：`EventCatalog` 声明、`mesa.events.v1` 计划解析、（Stage ④）
//! SelectClause 标准字段表、Variant 解码、NodeId canonical 化、OPC UA →
//! EventRecord（含 Condition transition 纯函数推断）。运行时编排见
//! `event_runtime.rs`（Stage ⑤）。

use mesa_core_types::{
    ConditionTransition, EventCatalog, EventCondition, EventRecord, EventRecordError,
    EventStreamDescriptor, FieldDescriptor, FieldType, GENERIC_EVENT_BINDING_KIND,
    GenericEventBinding, LocalizedText, SchemaDescriptor, TaskMode, Value,
};
use mesa_driver_sdk::SdkDriverError;
use mesa_opcua_transport::{UaEventSelectClause, UaNodeRef, UaQualifiedNameRef};

// ---------------------------------------------------------------------------
// §8：事件目录（V1 只声明一个 subscribe-only 流 `opcua.events`）
// ---------------------------------------------------------------------------

/// PR9 唯一事件流：OPC UA Events（subscribe only）。
pub const OPCUA_EVENT_STREAM_ID: &str = "opcua.events";

/// 默认 notifier：Server 对象（canonical 基础命名空间形态）。
pub const DEFAULT_EVENT_NOTIFIER: &str = "nsu=http://opcfoundation.org/UA/;i=2253";

/// 默认发布间隔 500ms（>0）。
pub const DEFAULT_PUBLISHING_INTERVAL_MS: u64 = 500;

/// 默认服务端队列 1000。P1-2：上限与本地 callback FIFO 对齐
/// （`EVENT_CALLBACK_QUEUE_CAPACITY` = 1024）：对外宣称的 server queue
/// 不得大于本地 ingress 的确定性吸收能力，否则第 1025 条即 fail-closed，
/// 属于"合法配置必然失败"。默认 1000 保持不动。
pub const DEFAULT_EVENT_QUEUE_SIZE: u32 = 1000;
/// 服务端队列上限（与 transport 本地 FIFO 容量一致）。
pub const MAX_EVENT_QUEUE_SIZE: u32 = mesa_opcua_transport::EVENT_CALLBACK_QUEUE_CAPACITY as u32;

/// 事件范围：全部事件 / 仅 Conditions（OfType 过滤）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventScope {
    All,
    Conditions,
}

impl EventScope {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "all" => Some(EventScope::All),
            "conditions" => Some(EventScope::Conditions),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EventScope::All => "all",
            EventScope::Conditions => "conditions",
        }
    }
}

/// PR9 事件目录：单流 `opcua.events`，subscribe only。认证只走 Connection
/// SecretStore——本 Schema 禁止 Secret 字段（PR8 `EventCatalog::validate`
/// 在入口即拒绝，见 P0-3）。
pub fn opcua_event_catalog() -> EventCatalog {
    let mut scope = FieldDescriptor::new("scope", "Scope", FieldType::Enum)
        .required(false)
        .default_value(serde_json::json!("all"));
    scope.validation.enum_options = Some(vec!["all".into(), "conditions".into()]);
    let mut publishing = FieldDescriptor::new(
        "publishing_interval_ms",
        "Publishing interval ms",
        FieldType::Integer,
    )
    .required(false)
    .default_value(serde_json::json!(DEFAULT_PUBLISHING_INTERVAL_MS));
    publishing.validation.min = Some(1.0);
    let mut queue = FieldDescriptor::new("queue_size", "Queue size", FieldType::Integer)
        .required(false)
        .default_value(serde_json::json!(DEFAULT_EVENT_QUEUE_SIZE));
    queue.validation.min = Some(1.0);
    queue.validation.max = Some(MAX_EVENT_QUEUE_SIZE as f64);
    EventCatalog {
        streams: vec![EventStreamDescriptor {
            id: OPCUA_EVENT_STREAM_ID.into(),
            label: LocalizedText::new("OPC UA Events"),
            modes: vec![TaskMode::Subscribe],
            parameters: SchemaDescriptor {
                fields: vec![
                    FieldDescriptor::new("notifier_node_id", "Notifier NodeId", FieldType::String)
                        .required(false)
                        .default_value(serde_json::json!(DEFAULT_EVENT_NOTIFIER)),
                    scope,
                    publishing,
                    queue,
                ],
            },
            fields: vec![],
        }],
    }
}

// ---------------------------------------------------------------------------
// §9：mesa.events.v1 计划解析（只接受标准 binding，不造私有 kind）
// ---------------------------------------------------------------------------

/// 单个事件任务的冻结计划：解析期全部校验通过后才整体替换旧计划（原子切换）。
/// notifier 为 canonical 身份（配置期只接受 `nsu=`），运行期经快照换算 index。
#[derive(Debug, Clone)]
pub struct OpcUaEventTaskPlan {
    pub id: String,
    pub notifier: mesa_opcua_transport::OpcUaNodeId,
    pub scope: EventScope,
    pub publishing_interval_ms: u64,
    pub queue_size: u32,
}

/// 事件计划快照：revision + 任务表（空表 = 清空订阅）。
#[derive(Debug, Clone, Default)]
pub struct OpcUaEventPlanSnapshot {
    pub revision: u64,
    pub tasks: Vec<OpcUaEventTaskPlan>,
}

fn configuration(code: &str, msg: String) -> SdkDriverError {
    SdkDriverError::configuration(code, msg)
}

fn param_u64(
    task_id: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, SdkDriverError> {
    let v = match params.get(key) {
        None => return Ok(default),
        Some(v) => v,
    };
    let n = v.as_u64().ok_or_else(|| {
        configuration(
            "INVALID_EVENT_PARAMETER",
            format!("event task `{task_id}`: `{key}` 需为正整数"),
        )
    })?;
    if n < min || n > max {
        return Err(configuration(
            "INVALID_EVENT_PARAMETER",
            format!("event task `{task_id}`: `{key}` 需在 {min}..={max}"),
        ));
    }
    Ok(n)
}

/// 解析 `mesa.events.v1` 事件任务表为冻结计划。任一非法即整个 revision
/// 失败且旧计划保持不变（调用方在全部成功后才赋值，§35 原子切换）。
pub fn parse_event_tasks(
    tasks: &[mesa_core_types::EventTask],
) -> Result<Vec<OpcUaEventTaskPlan>, SdkDriverError> {
    use mesa_core_types::EventTaskError;
    let mut out = Vec::with_capacity(tasks.len());
    for task in tasks {
        task.validate().map_err(|e| match e {
            EventTaskError::EmptyTaskId => {
                configuration("INVALID_EVENT_TASK", "event task id 不能为空".into())
            }
            EventTaskError::PollRequiresInterval { task } => configuration(
                "INVALID_EVENT_TASK",
                format!("event task `{task}`: Poll 模式必须提供正整数 interval_ms"),
            ),
        })?;
        // 只接受 PR8 冻结标准 binding；私有 kind 一律拒绝。
        if task.binding.kind != GENERIC_EVENT_BINDING_KIND {
            return Err(configuration(
                "UNSUPPORTED_EVENT_BINDING",
                format!(
                    "event task `{}`: binding kind `{}` unsupported, expected `{GENERIC_EVENT_BINDING_KIND}`",
                    task.id, task.binding.kind
                ),
            ));
        }
        let binding = GenericEventBinding::from_json(&task.binding.config).map_err(|e| {
            configuration(
                "INVALID_EVENT_BINDING_CONFIG",
                format!(
                    "event task `{}`: invalid generic event binding: {e}",
                    task.id
                ),
            )
        })?;
        if binding.stream_id != OPCUA_EVENT_STREAM_ID {
            return Err(configuration(
                "UNKNOWN_EVENT_STREAM",
                format!(
                    "event task `{}`: unknown stream `{}`",
                    task.id, binding.stream_id
                ),
            ));
        }
        if task.mode != TaskMode::Subscribe {
            return Err(configuration(
                "UNSUPPORTED_EVENT_MODE",
                format!(
                    "event task `{}`: mode {:?} unsupported, expected Subscribe",
                    task.id, task.mode
                ),
            ));
        }
        // from_json 已把显式 null 归一为 {}；数组/标量一律拒绝（防静默吞参）。
        let params = binding.parameters.as_object().ok_or_else(|| {
            configuration(
                "INVALID_EVENT_PARAMETER",
                format!("event task `{}`: `parameters` 需为对象", task.id),
            )
        })?;
        for key in params.keys() {
            match key.as_str() {
                "notifier_node_id" | "scope" | "publishing_interval_ms" | "queue_size" => {}
                other => {
                    return Err(configuration(
                        "INVALID_EVENT_PARAMETER",
                        format!("event task `{}`: 未知参数 `{other}`", task.id),
                    ));
                }
            }
        }
        let notifier_str = params
            .get("notifier_node_id")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_EVENT_NOTIFIER);
        let notifier = mesa_opcua_transport::parse_canonical(notifier_str).map_err(|e| {
            configuration(
                "INVALID_EVENT_NOTIFIER",
                format!(
                    "event task `{}`: notifier_node_id 非法: {e}（须为 canonical nsu= 形态）",
                    task.id
                ),
            )
        })?;
        let scope = params
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let scope = EventScope::parse(scope).ok_or_else(|| {
            configuration(
                "INVALID_EVENT_PARAMETER",
                format!("event task `{}`: `scope` 需为 all|conditions", task.id),
            )
        })?;
        let publishing_interval_ms = param_u64(
            &task.id,
            params,
            "publishing_interval_ms",
            DEFAULT_PUBLISHING_INTERVAL_MS,
            1,
            u64::MAX,
        )?;
        let queue_size = param_u64(
            &task.id,
            params,
            "queue_size",
            u64::from(DEFAULT_EVENT_QUEUE_SIZE),
            1,
            u64::from(MAX_EVENT_QUEUE_SIZE),
        )? as u32;
        out.push(OpcUaEventTaskPlan {
            id: task.id.clone(),
            notifier,
            scope,
            publishing_interval_ms,
            queue_size,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// §7：标准字段表（唯一真相来源——不要在多处手写索引）
// ---------------------------------------------------------------------------

/// 标准事件字段（位置即契约：callback fields[i] ↔ select clauses[i] ↔ 本表[i]）。
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardEventField {
    EventId = 0,
    EventType,
    SourceNode,
    SourceName,
    Time,
    ReceiveTime,
    Message,
    Severity,
    ConditionId,
    ConditionName,
    BranchId,
    Retain,
    Enabled,
    Active,
    ActiveTransitionTime,
    Acknowledged,
    AckedTransitionTime,
    Confirmed,
    ConfirmedTransitionTime,
}

/// 标准字段数（19）：callback 数组长度必须严格等于此数。
pub const STANDARD_EVENT_FIELD_COUNT: usize = 19;

pub struct StandardEventClause {
    pub name: &'static str,
    pub type_id: u32,
    pub path: &'static [&'static str],
    /// 属性：ConditionId 取 NodeId（空路径，见表下注释），其余取 Value(13)。
    pub attribute_id: u32,
}

/// 标准字段表：类型一律用 `ObjectTypeId`（禁魔法数字）。
pub const STANDARD_EVENT_FIELDS: [StandardEventClause; STANDARD_EVENT_FIELD_COUNT] = [
    StandardEventClause {
        name: "EventId",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["EventId"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "EventType",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["EventType"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "SourceNode",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["SourceNode"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "SourceName",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["SourceName"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Time",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["Time"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "ReceiveTime",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["ReceiveTime"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Message",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["Message"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Severity",
        type_id: opcua_types::ObjectTypeId::BaseEventType as u32,
        path: &["Severity"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "ConditionId",
        // Part 9 Table 10 字面形：ConditionId 不是 ConditionType 下显式建模的
        // 组件，而是 Condition instance 自身的 NodeId，故 type=ConditionType、
        // 空路径、属性 NodeId。这是 production wire contract，fixture 绿不绿
        // 都不得改这里（server 侧不合规由 fixture 补偿，见 support/event_server）。
        type_id: opcua_types::ObjectTypeId::ConditionType as u32,
        path: &[],
        attribute_id: opcua_types::AttributeId::NodeId as u32,
    },
    StandardEventClause {
        name: "ConditionName",
        type_id: opcua_types::ObjectTypeId::ConditionType as u32,
        path: &["ConditionName"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "BranchId",
        type_id: opcua_types::ObjectTypeId::ConditionType as u32,
        path: &["BranchId"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Retain",
        type_id: opcua_types::ObjectTypeId::ConditionType as u32,
        path: &["Retain"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Enabled",
        type_id: opcua_types::ObjectTypeId::ConditionType as u32,
        path: &["EnabledState", "Id"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Active",
        type_id: opcua_types::ObjectTypeId::AlarmConditionType as u32,
        path: &["ActiveState", "Id"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "ActiveTransitionTime",
        type_id: opcua_types::ObjectTypeId::AlarmConditionType as u32,
        path: &["ActiveState", "TransitionTime"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Acknowledged",
        type_id: opcua_types::ObjectTypeId::AcknowledgeableConditionType as u32,
        path: &["AckedState", "Id"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "AckedTransitionTime",
        type_id: opcua_types::ObjectTypeId::AcknowledgeableConditionType as u32,
        path: &["AckedState", "TransitionTime"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "Confirmed",
        type_id: opcua_types::ObjectTypeId::AcknowledgeableConditionType as u32,
        path: &["ConfirmedState", "Id"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
    StandardEventClause {
        name: "ConfirmedTransitionTime",
        type_id: opcua_types::ObjectTypeId::AcknowledgeableConditionType as u32,
        path: &["ConfirmedState", "TransitionTime"],
        attribute_id: opcua_types::AttributeId::Value as u32,
    },
];

/// 标准字段表 → transport 过滤器规约（顺序即 fields 顺序，由 §4 builder 发线）。
pub fn standard_event_clauses() -> Vec<UaEventSelectClause> {
    STANDARD_EVENT_FIELDS
        .iter()
        .map(|f| UaEventSelectClause {
            type_definition_id: UaNodeRef::numeric(0, f.type_id),
            browse_path: f
                .path
                .iter()
                .map(|n| UaQualifiedNameRef {
                    namespace: 0,
                    name: (*n).into(),
                })
                .collect(),
            attribute_id: f.attribute_id,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// §10–§16：OPC UA → EventRecord（纯函数：same fields → same record）
// ---------------------------------------------------------------------------

/// 解码上下文：命名空间快照（启动时读一次 NamespaceArray）+ 配置的 notifier
///（source 回退链末端）。禁止在此放内存 previous-state（§16）。
pub struct EventDecodeContext<'a> {
    pub namespaces: &'a [String],
    pub notifier: &'a UaNodeRef,
}

/// 解码产物：记录 + 服务端队列溢出标志（§6：overflow 事件先映射 publish，
/// COMMIT 后调用方再 fail 当前 task）。
#[derive(Debug, Clone)]
pub struct DecodedEvent {
    pub record: EventRecord,
    pub server_queue_overflow: bool,
}

fn runtime_err(code: &str, msg: String) -> SdkDriverError {
    // 运行期错误（非配置错误）：上层 fail 当前 attempt → Manager 重连开新 epoch。
    SdkDriverError::new(mesa_core_types::ErrorKind::Internal, code, msg)
}

/// NodeId → URI-canonical 身份（§13）：`nsu=<uri>;<i|s|g|b>=<值>`。
/// namespace index 不是可持久化身份，一律经 NamespaceArray 换 URI；
/// index 越界 → fail closed，绝不退回可能漂移的 `ns=N`。
pub fn canonical_node_id(
    namespaces: &[String],
    namespace: u16,
    identifier: &mesa_opcua_transport::UaIdentifier,
) -> Result<String, SdkDriverError> {
    use mesa_opcua_transport::UaIdentifier;
    let uri = namespaces.get(usize::from(namespace)).ok_or_else(|| {
        runtime_err(
            "OPCUA_EVENT_NAMESPACE_INVALID",
            format!("NodeId namespace index {namespace} 超出 NamespaceArray"),
        )
    })?;
    let id = match identifier {
        UaIdentifier::Numeric(n) => format!("i={n}"),
        UaIdentifier::String(s) => format!("s={s}"),
        UaIdentifier::Guid(g) => format!("g={g}"),
        UaIdentifier::Opaque(b64) => format!("b={b64}"),
    };
    Ok(format!("nsu={uri};{id}"))
}

fn canonical_opc_node_id(
    namespaces: &[String],
    nid: &opcua_types::NodeId,
) -> Result<String, SdkDriverError> {
    use opcua_types::node_id::Identifier as OpcId;
    let identifier = match &nid.identifier {
        OpcId::Numeric(n) => mesa_opcua_transport::UaIdentifier::Numeric(*n),
        OpcId::String(s) => mesa_opcua_transport::UaIdentifier::String(s.as_ref().to_string()),
        OpcId::Guid(g) => mesa_opcua_transport::UaIdentifier::Guid(g.to_string()),
        OpcId::ByteString(bs) => {
            use base64::Engine as _;
            let b64 = bs
                .value
                .as_ref()
                .map(|b| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b))
                .unwrap_or_default();
            mesa_opcua_transport::UaIdentifier::Opaque(b64)
        }
    };
    canonical_node_id(namespaces, nid.namespace, &identifier)
}

fn canonical_notifier(
    namespaces: &[String],
    notifier: &UaNodeRef,
) -> Result<String, SdkDriverError> {
    canonical_node_id(namespaces, notifier.namespace, &notifier.identifier)
}

/// 标准 null NodeId（ns=0;i=0）：ConditionId/BranchId 为此即"无"。
fn is_null_opc_node_id(nid: &opcua_types::NodeId) -> bool {
    use opcua_types::node_id::Identifier as OpcId;
    nid.namespace == 0 && matches!(nid.identifier, OpcId::Numeric(0))
}

fn field(
    fields: &[opcua_types::Variant],
    f: StandardEventField,
) -> Result<&opcua_types::Variant, SdkDriverError> {
    fields.get(f as usize).ok_or_else(|| {
        runtime_err(
            "OPCUA_EVENT_DECODE_FAILED",
            format!(
                "事件字段数 {} 与标准表 {} 对不上",
                fields.len(),
                STANDARD_EVENT_FIELD_COUNT
            ),
        )
    })
}

fn mismatch(name: &str) -> SdkDriverError {
    runtime_err(
        "OPCUA_EVENT_FIELD_TYPE_MISMATCH",
        format!("事件字段 `{name}` 类型非预期"),
    )
}

fn opt_bool(v: &opcua_types::Variant, name: &str) -> Result<Option<bool>, SdkDriverError> {
    use opcua_types::Variant as V;
    match v {
        V::Empty => Ok(None),
        V::Boolean(b) => Ok(Some(*b)),
        _ => Err(mismatch(name)),
    }
}

fn opt_datetime_ticks(v: &opcua_types::Variant, name: &str) -> Result<Option<i64>, SdkDriverError> {
    use opcua_types::Variant as V;
    match v {
        V::Empty => Ok(None),
        V::DateTime(dt) => Ok(Some(dt.ticks())),
        _ => Err(mismatch(name)),
    }
}

fn opt_node_id<'a>(
    v: &'a opcua_types::Variant,
    name: &str,
) -> Result<Option<&'a opcua_types::NodeId>, SdkDriverError> {
    use opcua_types::Variant as V;
    match v {
        V::Empty => Ok(None),
        V::NodeId(nid) => {
            if is_null_opc_node_id(nid) {
                Ok(None)
            } else {
                Ok(Some(nid))
            }
        }
        _ => Err(mismatch(name)),
    }
}

fn opt_string(v: &opcua_types::Variant, name: &str) -> Result<Option<String>, SdkDriverError> {
    use opcua_types::Variant as V;
    match v {
        V::Empty => Ok(None),
        V::String(s) => {
            let t = s.as_ref().to_string();
            Ok(if t.is_empty() { None } else { Some(t) })
        }
        _ => Err(mismatch(name)),
    }
}

/// 解码单个原生事件（位置数组 → EventRecord）。纯函数：相同输入必得相同输出，
/// 不读、不写任何内存 previous-state（§16：reconnect/replay 安全）。
pub fn decode_event_fields(
    ctx: &EventDecodeContext<'_>,
    fields: &[opcua_types::Variant],
) -> Result<DecodedEvent, SdkDriverError> {
    use StandardEventField as F;
    use opcua_types::Variant as V;
    if fields.len() != STANDARD_EVENT_FIELD_COUNT {
        return Err(runtime_err(
            "OPCUA_EVENT_DECODE_FAILED",
            format!(
                "事件字段数 {} 与标准表 {} 对不上",
                fields.len(),
                STANDARD_EVENT_FIELD_COUNT
            ),
        ));
    }
    // §10 EventId：原生 ByteString → `opcua:BASE64URL_NO_PAD`；缺失/空即 fail。
    let event_id = match field(fields, F::EventId)? {
        V::ByteString(bs) => {
            let bytes = bs.value.as_ref().ok_or_else(|| {
                runtime_err(
                    "OPCUA_EVENT_MISSING_EVENT_ID",
                    "EventId 为空（无原生 identity，不可去重）".into(),
                )
            })?;
            if bytes.is_empty() {
                return Err(runtime_err(
                    "OPCUA_EVENT_MISSING_EVENT_ID",
                    "EventId 为空（无原生 identity，不可去重）".into(),
                ));
            }
            use base64::Engine as _;
            format!(
                "opcua:{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
            )
        }
        _ => {
            return Err(runtime_err(
                "OPCUA_EVENT_MISSING_EVENT_ID",
                "EventId 非 ByteString（无原生 identity，不可去重）".into(),
            ));
        }
    };
    // EventType canonical 身份（attributes + overflow 判定共用）。
    let event_type_nid = match field(fields, F::EventType)? {
        V::NodeId(nid) => nid.as_ref().clone(),
        _ => return Err(mismatch("EventType")),
    };
    let event_type = canonical_opc_node_id(ctx.namespaces, &event_type_nid)?;
    // EventQueueOverflowEventType（ns=0;i=3035，经 ObjectTypeId 引用，禁字面量）。
    let overflow_type_id = opcua_types::ObjectTypeId::EventQueueOverflowEventType as u32;
    let server_queue_overflow = event_type_nid.namespace == 0
        && matches!(
            event_type_nid.identifier,
            opcua_types::node_id::Identifier::Numeric(n) if n == overflow_type_id
        );
    // §11 时间：Time → occurred_at（缺失即 None，禁伪造）；ReceiveTime → attributes。
    // TimestampNs 为 i64：原样直通，无回绕风险。
    let time_ticks = opt_datetime_ticks(field(fields, F::Time)?, "Time")?;
    let occurred_at_ns = time_ticks.map(crate::ticks_to_unix_ns);
    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert("opcua.event_type".into(), Value::String(event_type.clone()));
    if let Some(rt) = opt_datetime_ticks(field(fields, F::ReceiveTime)?, "ReceiveTime")? {
        attributes.insert(
            "opcua.receive_time_ns".into(),
            Value::I64(crate::ticks_to_unix_ns(rt)),
        );
    }
    // §12 Severity：原值直通；缺失=0；错类型/越界即失败（禁 clamp）。
    let severity = match field(fields, F::Severity)? {
        V::Empty => 0u16,
        V::UInt16(n) => {
            if *n > 1000 {
                return Err(runtime_err(
                    "OPCUA_EVENT_DECODE_FAILED",
                    format!("Severity {n} 超出 0..=1000（禁 clamp）"),
                ));
            }
            *n
        }
        _ => return Err(mismatch("Severity")),
    };
    // §12 Message：LocalizedText.text → message，locale → message_locale。
    let (message, message_locale) = match field(fields, F::Message)? {
        V::Empty => (None, None),
        V::LocalizedText(lt) => {
            let text = lt.text.as_ref().to_string();
            let locale = lt.locale.as_ref().to_string();
            (
                if text.is_empty() { None } else { Some(text) },
                if locale.is_empty() {
                    None
                } else {
                    Some(locale)
                },
            )
        }
        _ => return Err(mismatch("Message")),
    };
    // Source 回退链：SourceName → SourceNode canonical → notifier canonical。
    let source_name = opt_string(field(fields, F::SourceName)?, "SourceName")?;
    let source_node = opt_node_id(field(fields, F::SourceNode)?, "SourceNode")?;
    let source = match (source_name, source_node) {
        (Some(n), _) => n,
        (None, Some(nid)) => canonical_opc_node_id(ctx.namespaces, nid)?,
        (None, None) => canonical_notifier(ctx.namespaces, ctx.notifier)?,
    };
    // §14–§15 Condition 判定：ConditionId 非空 NodeId 即 Condition。
    let condition_id = opt_node_id(field(fields, F::ConditionId)?, "ConditionId")?;
    let (category, kind, condition) = match condition_id {
        None => ("event", "opcua.event", None),
        Some(cid) => {
            let cid_str = canonical_opc_node_id(ctx.namespaces, cid)?;
            let retain = opt_bool(field(fields, F::Retain)?, "Retain")?;
            let active = opt_bool(field(fields, F::Active)?, "Active")?;
            let acknowledged = opt_bool(field(fields, F::Acknowledged)?, "Acknowledged")?;
            let confirmed = opt_bool(field(fields, F::Confirmed)?, "Confirmed")?;
            let active_tt = opt_datetime_ticks(
                field(fields, F::ActiveTransitionTime)?,
                "ActiveTransitionTime",
            )?;
            let acked_tt = opt_datetime_ticks(
                field(fields, F::AckedTransitionTime)?,
                "AckedTransitionTime",
            )?;
            let confirmed_tt = opt_datetime_ticks(
                field(fields, F::ConfirmedTransitionTime)?,
                "ConfirmedTransitionTime",
            )?;
            // §16 transition 纯函数：TransitionTime == Event.Time 即本次 transition
            // 的原生证据；多命中按 Active → Acked → Confirmed；否则 Updated（绝不猜）。
            let transition = match (time_ticks, active_tt, acked_tt, confirmed_tt) {
                (Some(t), Some(a), _, _) if a == t => match active {
                    // 方向未知（Active 缺失）不猜：落 Updated。
                    Some(true) => ConditionTransition::Raised,
                    Some(false) => ConditionTransition::Cleared,
                    None => ConditionTransition::Updated,
                },
                (Some(t), _, Some(a), _) if a == t && acknowledged == Some(true) => {
                    ConditionTransition::Acknowledged
                }
                (Some(t), _, _, Some(c)) if c == t && confirmed == Some(true) => {
                    ConditionTransition::Confirmed
                }
                _ => ConditionTransition::Updated,
            };
            if let Some(n) = opt_string(field(fields, F::ConditionName)?, "ConditionName")? {
                attributes.insert("opcua.condition_name".into(), Value::String(n));
            }
            if let Some(bid) = opt_node_id(field(fields, F::BranchId)?, "BranchId")? {
                attributes.insert(
                    "opcua.branch_id".into(),
                    Value::String(canonical_opc_node_id(ctx.namespaces, bid)?),
                );
            }
            if let Some(e) = opt_bool(field(fields, F::Enabled)?, "Enabled")? {
                attributes.insert("opcua.enabled".into(), Value::Bool(e));
            }
            for (key, tt) in [
                ("opcua.active_transition_time_ns", active_tt),
                ("opcua.acked_transition_time_ns", acked_tt),
                ("opcua.confirmed_transition_time_ns", confirmed_tt),
            ] {
                if let Some(t) = tt {
                    attributes.insert(key.into(), Value::I64(crate::ticks_to_unix_ns(t)));
                }
            }
            (
                "condition",
                "opcua.condition",
                Some(EventCondition {
                    condition_id: cid_str,
                    transition,
                    active,
                    acknowledged,
                    confirmed,
                    retain,
                }),
            )
        }
    };
    // §6 服务端队列溢出：先映射为 system 事件 publish，调用方 COMMIT 后再 fail task。
    let (category, kind) = if server_queue_overflow {
        ("system", "opcua.event-queue-overflow")
    } else {
        (category, kind)
    };
    let record = EventRecord {
        event_id,
        category: category.into(),
        kind: kind.into(),
        source,
        severity,
        code: None,
        message,
        message_locale,
        occurred_at_ns,
        condition,
        correlation_id: None,
        attributes,
    };
    // 大小/结构前置校验：坏记录在 publish 当场拒绝，不占用队列（SDK 会再验，
    // 此处先验以便错误码归因到 OPCUA_EVENT_*）。
    record.validate().map_err(|e| match e {
        EventRecordError::Invalid(m) => {
            runtime_err("OPCUA_EVENT_RECORD_INVALID", format!("record 非法: {m}"))
        }
        EventRecordError::TooLarge(m) => {
            runtime_err("OPCUA_EVENT_RECORD_TOO_LARGE", format!("record 超限: {m}"))
        }
    })?;
    Ok(DecodedEvent {
        record,
        server_queue_overflow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::{DriverBinding, EventTask};

    fn generic_task(id: &str, stream: &str, params: serde_json::Value) -> EventTask {
        EventTask {
            id: id.into(),
            mode: TaskMode::Subscribe,
            interval_ms: None,
            binding: DriverBinding {
                kind: GENERIC_EVENT_BINDING_KIND.into(),
                config: serde_json::json!({"stream_id": stream, "parameters": params}),
            },
        }
    }

    fn default_params() -> serde_json::Value {
        serde_json::json!({})
    }

    #[test]
    fn catalog_advertises_single_subscribe_stream() {
        let catalog = opcua_event_catalog();
        catalog
            .validate()
            .expect("目录必须合法（含无 Secret 检查）");
        assert_eq!(catalog.streams.len(), 1);
        let s = &catalog.streams[0];
        assert_eq!(s.id, OPCUA_EVENT_STREAM_ID);
        assert_eq!(s.modes, vec![TaskMode::Subscribe]);
        let keys: Vec<_> = s.parameters.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "notifier_node_id",
                "scope",
                "publishing_interval_ms",
                "queue_size"
            ]
        );
    }

    #[test]
    fn empty_params_take_documented_defaults() {
        let plans =
            parse_event_tasks(&[generic_task("t", OPCUA_EVENT_STREAM_ID, default_params())])
                .unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].notifier.to_string(), DEFAULT_EVENT_NOTIFIER);
        assert_eq!(plans[0].scope, EventScope::All);
        assert_eq!(
            plans[0].publishing_interval_ms,
            DEFAULT_PUBLISHING_INTERVAL_MS
        );
        assert_eq!(plans[0].queue_size, DEFAULT_EVENT_QUEUE_SIZE);
    }

    #[test]
    fn private_binding_and_unknown_stream_rejected() {
        let mut t = generic_task("t", OPCUA_EVENT_STREAM_ID, default_params());
        t.binding.kind = "opcua.events.v1".into();
        let err = parse_event_tasks(std::slice::from_ref(&t)).unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED_EVENT_BINDING");
        let t = generic_task("t", "opcua.unknown", default_params());
        let err = parse_event_tasks(std::slice::from_ref(&t)).unwrap_err();
        assert_eq!(err.code, "UNKNOWN_EVENT_STREAM");
    }

    #[test]
    fn poll_mode_rejected() {
        let mut t = generic_task("t", OPCUA_EVENT_STREAM_ID, default_params());
        t.mode = TaskMode::Poll;
        t.interval_ms = Some(1000);
        let err = parse_event_tasks(std::slice::from_ref(&t)).unwrap_err();
        assert_eq!(err.code, "UNSUPPORTED_EVENT_MODE");
    }

    #[test]
    fn unknown_params_and_bad_ranges_rejected() {
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"discard_oldest": true}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_PARAMETER"
        );
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"queue_size": 0}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_PARAMETER"
        );
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"queue_size": 10001}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_PARAMETER"
        );
        // P1-2：上限与本地 FIFO 对齐（1024）；1025 非法，1024 合法。
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"queue_size": 1025}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_PARAMETER"
        );
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"queue_size": 1024}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t)).unwrap()[0].queue_size,
            1024
        );
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"publishing_interval_ms": 0}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_PARAMETER"
        );
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"scope": "alarms"}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_PARAMETER"
        );
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"notifier_node_id": "bad"}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_NOTIFIER"
        );
        // P1-1：GUID/Base64 必须真解析（配置期拒绝，不拖到 Start）。
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"notifier_node_id": "nsu=http://example.com/M/;b=@@@NOT-BASE64@@@"}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_NOTIFIER"
        );
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"notifier_node_id": "nsu=http://example.com/M/;g=zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz"}),
        );
        assert_eq!(
            parse_event_tasks(std::slice::from_ref(&t))
                .unwrap_err()
                .code,
            "INVALID_EVENT_NOTIFIER"
        );
        // legacy ns= 一律拒绝（即使是稳定的 ns=0，也必须走 canonical）。
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"notifier_node_id": "ns=0;i=2253"}),
        );
        let err = parse_event_tasks(std::slice::from_ref(&t)).unwrap_err();
        assert_eq!(err.code, "INVALID_EVENT_NOTIFIER");
        assert!(
            err.message.contains("nsu="),
            "须指引 canonical，实际: {}",
            err.message
        );
        // canonical 合法值通过。
        let t = generic_task(
            "t",
            OPCUA_EVENT_STREAM_ID,
            serde_json::json!({"notifier_node_id": "nsu=http://opcfoundation.org/UA/;i=2253"}),
        );
        let plans = parse_event_tasks(std::slice::from_ref(&t)).unwrap();
        assert_eq!(
            plans[0].notifier.canonical_key(),
            "nsu=http://opcfoundation.org/UA/;i=2253"
        );
    }

    #[test]
    fn empty_snapshot_clears() {
        assert!(parse_event_tasks(&[]).unwrap().is_empty());
    }

    // ---- §7 golden：位置契约（错一位即 Severity 变 Message 级事故） ----

    #[test]
    fn standard_field_table_positional_contract() {
        use StandardEventField as F;
        assert_eq!(STANDARD_EVENT_FIELD_COUNT, 19);
        let names = [
            "EventId",
            "EventType",
            "SourceNode",
            "SourceName",
            "Time",
            "ReceiveTime",
            "Message",
            "Severity",
            "ConditionId",
            "ConditionName",
            "BranchId",
            "Retain",
            "Enabled",
            "Active",
            "ActiveTransitionTime",
            "Acknowledged",
            "AckedTransitionTime",
            "Confirmed",
            "ConfirmedTransitionTime",
        ];
        for (f, n) in STANDARD_EVENT_FIELDS.iter().zip(names) {
            assert_eq!(f.name, n, "字段表顺序即线序");
        }
        // 枚举判别值即索引（解码器按 `f as usize` 取数）。
        assert_eq!(F::EventId as usize, 0);
        assert_eq!(F::Severity as usize, 7);
        assert_eq!(F::ConditionId as usize, 8);
        assert_eq!(F::Active as usize, 13);
        assert_eq!(F::Acknowledged as usize, 15);
        assert_eq!(F::ConfirmedTransitionTime as usize, 18);
        // 类型分布：8 Base + 5 Condition（含 ConditionId，Table 10 字面形）
        // + 2 Alarm + 4 Ackable（ObjectTypeId，非字面量）。
        let (base, cond, alarm, ack) = (2041u32, 2782u32, 2915u32, 2881u32);
        assert_eq!(opcua_types::ObjectTypeId::BaseEventType as u32, base);
        assert_eq!(opcua_types::ObjectTypeId::ConditionType as u32, cond);
        assert_eq!(opcua_types::ObjectTypeId::AlarmConditionType as u32, alarm);
        assert_eq!(
            opcua_types::ObjectTypeId::AcknowledgeableConditionType as u32,
            ack
        );
        for (i, f) in STANDARD_EVENT_FIELDS.iter().enumerate() {
            let want = match i {
                0..=7 => base,
                8..=12 => cond,
                13..=14 => alarm,
                _ => ack,
            };
            assert_eq!(f.type_id, want, "字段 {} 类型错位", f.name);
        }
        // 属性：18 个 Value(13) + ConditionId 一个 NodeId(1，空路径）；
        // 状态类一律两段路径；ConditionId 锁定索引 8 + ConditionType +
        // 空路径 + NodeId（Part 9 Table 10 字面形，wire contract 不容形变）。
        assert_eq!(
            STANDARD_EVENT_FIELDS[8].type_id,
            opcua_types::ObjectTypeId::ConditionType as u32
        );
        assert!(STANDARD_EVENT_FIELDS[8].path.is_empty());
        assert_eq!(
            STANDARD_EVENT_FIELDS[8].attribute_id,
            opcua_types::AttributeId::NodeId as u32
        );
        for (i, f) in STANDARD_EVENT_FIELDS.iter().enumerate() {
            if i == F::ConditionId as usize {
                continue; // 已在上文锁死 Table 10 形，不参与 Value 断言
            }
            assert_eq!(
                f.attribute_id,
                opcua_types::AttributeId::Value as u32,
                "字段 {} 必须是 Value 属性",
                f.name
            );
            assert!(!f.path.is_empty(), "字段 {} 必须显式路径", f.name);
        }
        assert_eq!(STANDARD_EVENT_FIELDS[12].path, &["EnabledState", "Id"]);
        assert_eq!(
            STANDARD_EVENT_FIELDS[14].path,
            &["ActiveState", "TransitionTime"]
        );
        let clauses = standard_event_clauses();
        assert_eq!(clauses.len(), STANDARD_EVENT_FIELD_COUNT);
        for (c, f) in clauses.iter().zip(STANDARD_EVENT_FIELDS.iter()) {
            assert_eq!(c.type_definition_id, UaNodeRef::numeric(0, f.type_id));
            assert_eq!(c.attribute_id, f.attribute_id);
            let got: Vec<_> = c.browse_path.iter().map(|q| q.name.as_str()).collect();
            assert_eq!(got, f.path);
        }
    }

    // ---- §10–§16 映射单测（§23 事件集的纯函数版） ----

    fn test_namespaces() -> Vec<String> {
        vec![
            "http://opcfoundation.org/UA/".into(),
            "http://example.com/Other/".into(),
            "http://example.com/MyModel/".into(),
        ]
    }

    fn test_ctx(ns: &[String]) -> EventDecodeContext<'_> {
        static NOTIFIER: std::sync::OnceLock<UaNodeRef> = std::sync::OnceLock::new();
        let notifier = NOTIFIER.get_or_init(|| UaNodeRef::numeric(0, 2253));
        EventDecodeContext {
            namespaces: ns,
            notifier,
        }
    }

    /// Unix ns → OPC UA ticks（测试构造用，与生产换算互逆）。
    fn ns_to_ticks(ns: i64) -> i64 {
        ns / 100 + 11644473600 * 10_000_000
    }

    fn dt(ns: i64) -> opcua_types::Variant {
        opcua_types::Variant::DateTime(Box::new(opcua_types::DateTime::from(ns_to_ticks(ns))))
    }

    fn nid(ns: u16, id: u32) -> opcua_types::Variant {
        opcua_types::Variant::NodeId(Box::new(opcua_types::NodeId::new(ns, id)))
    }

    fn fields_base() -> Vec<opcua_types::Variant> {
        use opcua_types::{ByteString, LocalizedText, UAString, Variant as V};
        let mut f = vec![
            V::ByteString(ByteString::from(vec![1u8, 2, 3])), // EventId
            nid(0, 2041),                                     // EventType
            nid(0, 2253),                                     // SourceNode
            V::String(UAString::from("MesaFixture")),         // SourceName
            dt(1_700_000_000_000_000_000),                    // Time
            dt(1_700_000_000_500_000_000),                    // ReceiveTime
            V::LocalizedText(Box::new(LocalizedText::new("", "fixture event"))), // Message
            V::UInt16(321),                                   // Severity
        ];
        f.extend(std::iter::repeat_n(
            V::Empty,
            STANDARD_EVENT_FIELD_COUNT - f.len(),
        ));
        f
    }

    // 测试脚手架：8 参数仅为单测行文方便，允许。
    #[allow(clippy::too_many_arguments)]
    fn cond_fields(
        event_id: &[u8],
        time_ns: i64,
        active: BoolField,
        active_tt: Option<i64>,
        acked: BoolField,
        acked_tt: Option<i64>,
        confirmed: BoolField,
        confirmed_tt: Option<i64>,
    ) -> Vec<opcua_types::Variant> {
        use opcua_types::{ByteString, UAString, Variant as V};
        let mut f = fields_base();
        f[0] = V::ByteString(ByteString::from(event_id.to_vec()));
        f[4] = dt(time_ns);
        f[8] = V::NodeId(Box::new(opcua_types::NodeId::new(
            2,
            UAString::from("Alarm1"),
        )));
        // 注意索引即契约（12=Enabled，13=Active，14=ActiveTT，15=Acked，
        // 16=AckedTT，17=Confirmed，18=ConfirmedTT）。
        f[13] = active.0;
        f[14] = active_tt.map(dt).unwrap_or(V::Empty);
        f[15] = acked.0;
        f[16] = acked_tt.map(dt).unwrap_or(V::Empty);
        f[17] = confirmed.0;
        f[18] = confirmed_tt.map(dt).unwrap_or(V::Empty);
        f
    }

    struct BoolField(opcua_types::Variant);
    fn b(v: bool) -> BoolField {
        BoolField(opcua_types::Variant::Boolean(v))
    }
    fn nb() -> BoolField {
        BoolField(opcua_types::Variant::Empty)
    }

    #[test]
    fn decode_base_event_identity_time_severity_message() {
        let ns = test_namespaces();
        let d = decode_event_fields(&test_ctx(&ns), &fields_base()).unwrap();
        assert!(!d.server_queue_overflow);
        let r = d.record;
        // EventId 原生：bytes [1,2,3] → base64url-nopad "AQID"。
        assert_eq!(r.event_id, "opcua:AQID");
        assert_eq!(r.category, "event");
        assert_eq!(r.kind, "opcua.event");
        assert!(r.condition.is_none());
        assert_eq!(r.message.as_deref(), Some("fixture event"));
        assert!(r.message_locale.is_none());
        assert_eq!(r.severity, 321);
        assert_eq!(r.occurred_at_ns, Some(1_700_000_000_000_000_000));
        assert_eq!(r.source, "MesaFixture");
        assert_eq!(
            r.attributes.get("opcua.event_type"),
            Some(&Value::String(
                "nsu=http://opcfoundation.org/UA/;i=2041".into()
            ))
        );
        assert_eq!(
            r.attributes.get("opcua.receive_time_ns"),
            Some(&Value::I64(1_700_000_000_500_000_000))
        );
        r.validate().unwrap();
        // 无 transport/config 元数据（同一 EventId 跨订阅 payload 一致的前提）。
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("client_handle"));
        assert!(!json.contains("subscription"));
        assert!(!json.contains("notifier"));
    }

    #[test]
    fn decode_condition_lifecycle_transitions() {
        use mesa_core_types::ConditionTransition as T;
        let ns = test_namespaces();
        let t2 = 1_700_000_010_000_000_000i64;
        let t3 = 1_700_000_020_000_000_000i64;
        let t4 = 1_700_000_030_000_000_000i64;
        let t5 = 1_700_000_040_000_000_000i64;
        // Raised：Active=true 且 ActiveTransitionTime == Time。
        let d = decode_event_fields(
            &test_ctx(&ns),
            &cond_fields(&[2], t2, b(true), Some(t2), nb(), None, nb(), None),
        )
        .unwrap();
        let c = d.record.condition.clone().unwrap();
        assert_eq!(c.transition, T::Raised);
        assert_eq!(c.active, Some(true));
        assert_eq!(c.condition_id, "nsu=http://example.com/MyModel/;s=Alarm1");
        assert_eq!(d.record.category, "condition");
        assert_eq!(d.record.kind, "opcua.condition");
        // Updated：ActiveTransitionTime 停留在 T2，Time 已到 T3。
        let d = decode_event_fields(
            &test_ctx(&ns),
            &cond_fields(&[3], t3, b(true), Some(t2), nb(), None, nb(), None),
        )
        .unwrap();
        assert_eq!(d.record.condition.clone().unwrap().transition, T::Updated);
        // Acknowledged。
        let d = decode_event_fields(
            &test_ctx(&ns),
            &cond_fields(&[4], t4, b(true), Some(t2), b(true), Some(t4), nb(), None),
        )
        .unwrap();
        assert_eq!(
            d.record.condition.clone().unwrap().transition,
            T::Acknowledged
        );
        assert_eq!(d.record.condition.clone().unwrap().acknowledged, Some(true));
        // Cleared：Active=false 且 ActiveTransitionTime == Time。
        let d = decode_event_fields(
            &test_ctx(&ns),
            &cond_fields(&[5], t5, b(false), Some(t5), nb(), None, nb(), None),
        )
        .unwrap();
        assert_eq!(d.record.condition.clone().unwrap().transition, T::Cleared);
    }

    #[test]
    fn decode_deterministic_same_fields_same_record() {
        let ns = test_namespaces();
        let f = cond_fields(
            &[9],
            1_700_000_000_000_000_000,
            b(true),
            None,
            nb(),
            None,
            nb(),
            None,
        );
        let a = decode_event_fields(&test_ctx(&ns), &f).unwrap().record;
        let b = decode_event_fields(&test_ctx(&ns), &f).unwrap().record;
        assert_eq!(a, b);
    }

    #[test]
    fn decode_rejects_missing_event_id_and_bad_severity() {
        let ns = test_namespaces();
        let mut f = fields_base();
        f[0] = opcua_types::Variant::Empty;
        let err = decode_event_fields(&test_ctx(&ns), &f).unwrap_err();
        assert_eq!(err.code, "OPCUA_EVENT_MISSING_EVENT_ID");
        let mut f = fields_base();
        f[7] = opcua_types::Variant::UInt16(1001);
        let err = decode_event_fields(&test_ctx(&ns), &f).unwrap_err();
        assert_eq!(err.code, "OPCUA_EVENT_DECODE_FAILED");
        let mut f = fields_base();
        f[7] = opcua_types::Variant::String(opcua_types::UAString::from("high"));
        let err = decode_event_fields(&test_ctx(&ns), &f).unwrap_err();
        assert_eq!(err.code, "OPCUA_EVENT_FIELD_TYPE_MISMATCH");
    }

    #[test]
    fn decode_rejects_bad_namespace_and_short_array() {
        let ns = test_namespaces();
        // namespace 越界 fail closed（禁退回 ns=N）。
        let mut f = fields_base();
        f[2] = nid(9, 1);
        // SourceName 非空时走不到 SourceNode——先清空 SourceName 再断言。
        f[3] = opcua_types::Variant::String(opcua_types::UAString::from(""));
        let err = decode_event_fields(&test_ctx(&ns), &f).unwrap_err();
        assert_eq!(err.code, "OPCUA_EVENT_NAMESPACE_INVALID");
        // 字段数对不上即违约。
        let short = fields_base()[..18].to_vec();
        let err = decode_event_fields(&test_ctx(&ns), &short).unwrap_err();
        assert_eq!(err.code, "OPCUA_EVENT_DECODE_FAILED");
    }
}
