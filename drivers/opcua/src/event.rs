//! OPC UA 事件语义（PR9）：Mesa 映射层（transport/event.rs = wire 语义，本模块 = 映射语义）。
//!
//! 本文件职责：`EventCatalog` 声明、`mesa.events.v1` 计划解析、（Stage ④）
//! SelectClause 标准字段表、Variant 解码、NodeId canonical 化、OPC UA →
//! EventRecord（含 Condition transition 纯函数推断）。运行时编排见
//! `event_runtime.rs`（Stage ⑤）。

use mesa_core_types::{
    EventCatalog, EventStreamDescriptor, FieldDescriptor, FieldType, GENERIC_EVENT_BINDING_KIND,
    GenericEventBinding, LocalizedText, SchemaDescriptor, TaskMode,
};
use mesa_driver_sdk::SdkDriverError;
use mesa_opcua_transport::UaNodeRef;

// ---------------------------------------------------------------------------
// §8：事件目录（V1 只声明一个 subscribe-only 流 `opcua.events`）
// ---------------------------------------------------------------------------

/// PR9 唯一事件流：OPC UA Events（subscribe only）。
pub const OPCUA_EVENT_STREAM_ID: &str = "opcua.events";

/// 默认 notifier：Server 对象（ns=0;i=2253）。
pub const DEFAULT_EVENT_NOTIFIER: &str = "ns=0;i=2253";

/// 默认发布间隔 500ms（>0）。
pub const DEFAULT_PUBLISHING_INTERVAL_MS: u64 = 500;

/// 默认服务端队列 1000（1..=10000，`discard_oldest=false` 见 transport）。
pub const DEFAULT_EVENT_QUEUE_SIZE: u32 = 1000;

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
    queue.validation.max = Some(10_000.0);
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
#[derive(Debug, Clone)]
pub struct OpcUaEventTaskPlan {
    pub id: String,
    pub notifier: UaNodeRef,
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
        let notifier = UaNodeRef::parse(notifier_str).map_err(|e| {
            configuration(
                "INVALID_EVENT_NOTIFIER",
                format!("event task `{}`: notifier_node_id 非法: {e}", task.id),
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
            10_000,
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
    }

    #[test]
    fn empty_snapshot_clears() {
        assert!(parse_event_tasks(&[]).unwrap().is_empty());
    }
}
