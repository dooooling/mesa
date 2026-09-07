//! OPC UA 事件运行时（PR9 Stage ⑤）：单任务 worker + 清理顺序。
//!
//! 职责：建订阅 → 建监控项（含 filter 结果校验）→ 接收/解码/发布循环 →
//! 按序清理。Data 语义（Latest-Wins / last-known / placeholder）与此无关；
//! Event worker 只走 FIFO + fail-closed，队列语义绝不混入 Data 路径。

use std::sync::Arc;

use mesa_core_types::ErrorKind;
use mesa_driver_sdk::{EventPublishError, EventSink, SdkDriverError};
use mesa_opcua_transport::{
    OpcUaTransport, UaEventFilterSpec, UaEventMonitoredItemSpec, UaEventNotification,
    UaEventStreamFatal, UaNodeRef, UaSubscriptionSpec, UaTransportError, UaTransportErrorKind,
};
use tokio_util::sync::CancellationToken;

use super::event::{
    EventDecodeContext, EventScope, OpcUaEventTaskPlan, STANDARD_EVENT_FIELD_COUNT,
    decode_event_fields, standard_event_clauses,
};

/// 单批最多聚合的 occurrence 数（与 Data 订阅 64 对齐；顺序由 SDK 序号保证）。
const EVENT_PUBLISH_BATCH_MAX: usize = 64;

fn transport_err(code: &str, e: UaTransportError) -> SdkDriverError {
    match e.kind {
        // 规约由本驱动构造：配置类失败即驱动 bug，归 Internal。
        UaTransportErrorKind::Configuration | UaTransportErrorKind::Protocol => {
            SdkDriverError::new(
                ErrorKind::Internal,
                code,
                format!("transport 配置/协议错误（驱动 bug）: {e}"),
            )
        }
        _ => SdkDriverError::new(ErrorKind::Connection, code, e.to_string()),
    }
}

/// 单个事件任务 worker：建订阅 → 建监控项 → 循环 → 清理。
/// 任何完整性/会话/解码失败即 `Err`（上层 fail 当前 attempt → Manager 重连）；
/// 仅正常 shutdown（含 transport 队列排空）返回 `Ok`。
pub async fn run_event_task(
    transport: Arc<dyn OpcUaTransport>,
    task: OpcUaEventTaskPlan,
    namespaces: Arc<Vec<String>>,
    sink: EventSink,
    shutdown: CancellationToken,
) -> Result<(), SdkDriverError> {
    let task_id = task.id.clone();
    let r = run_event_task_inner(&transport, &task, &namespaces, &sink, &shutdown).await;
    if let Err(e) = &r {
        tracing::error!(task = %task_id, error = %e, "OPC UA 事件任务失败（fail 当前 attempt）");
    }
    r
}

async fn run_event_task_inner(
    transport: &Arc<dyn OpcUaTransport>,
    task: &OpcUaEventTaskPlan,
    namespaces: &Arc<Vec<String>>,
    sink: &EventSink,
    shutdown: &CancellationToken,
) -> Result<(), SdkDriverError> {
    // 与 Data 订阅同一会话参数（lifetime ≥ 3×keepalive）；事件走独立订阅，
    // 同一 Session 下 Data/Event 队列物理隔离（§19）。
    let sub = transport
        .create_event_subscription(UaSubscriptionSpec {
            publishing_interval_ms: task.publishing_interval_ms,
            lifetime_count: 30,
            max_keep_alive_count: 10,
            max_notifications_per_publish: 0,
            priority: 0,
            publishing_enabled: true,
        })
        .await
        .map_err(|e| transport_err("OPCUA_EVENT_SUBSCRIBE_FAILED", e))?;
    tracing::info!(
        task = %task.id,
        sub_id = sub.id,
        revised = sub.revised_publishing_interval_ms,
        "OPC UA 事件订阅已建立"
    );
    // of_type：scope=conditions 时仅 ConditionType，否则不过滤
    //（BaseEventType 全收，由解码器分类）。类型 ID 走 ObjectTypeId，禁字面量。
    let of_type = if task.scope == EventScope::Conditions {
        Some(UaNodeRef::numeric(
            0,
            opcua_types::ObjectTypeId::ConditionType as u32,
        ))
    } else {
        None
    };
    let item = UaEventMonitoredItemSpec {
        notifier: task.notifier.clone(),
        client_handle: 1,
        queue_size: task.queue_size,
        filter: UaEventFilterSpec {
            select_clauses: standard_event_clauses(),
            of_type,
        },
    };
    let items = match transport
        .create_event_monitored_items(sub.id, std::slice::from_ref(&item))
        .await
    {
        Ok(items) => items,
        Err(e) => {
            rollback_subscription(transport, sub.id).await;
            return Err(transport_err("OPCUA_EVENT_SUBSCRIBE_FAILED", e));
        }
    };
    // 数量不变式由 transport 保证（1 req → 1 result）；此处只做语义判定。
    let created = match items.into_iter().next() {
        Some(created) => created,
        None => {
            rollback_subscription(transport, sub.id).await;
            return Err(SdkDriverError::new(
                ErrorKind::Internal,
                "OPCUA_EVENT_SUBSCRIBE_FAILED",
                format!("task `{}`: 监控项结果缺席", task.id),
            ));
        }
    };
    if opcua_types::StatusCode::from(created.status_code).is_good() {
        // 位置契约要求全部 clause Good：任一 BAD 都意味着服务端解析出的通知
        // 数组必然形变（有的栈丢弃坏 clause，有的填 null，行为不统一），绝不
        // 在"缺字段"上继续运行。Condition 字段对非 Condition occurrence 返回
        // Empty 值是正常的（§26）——那是值层面的事；定义层 invalid 即失败。
        let bad: Vec<u32> = created
            .select_clause_statuses
            .iter()
            .enumerate()
            .filter(|(_, s)| !opcua_types::StatusCode::from(**s).is_good())
            .map(|(i, _)| i as u32)
            .collect();
        if !bad.is_empty() {
            rollback_subscription(transport, sub.id).await;
            return Err(SdkDriverError::new(
                ErrorKind::Connection,
                "OPCUA_EVENT_FILTER_REJECTED",
                format!(
                    "task `{}`: {} 个事件 clause 被服务器拒绝（索引 {bad:?}）",
                    task.id,
                    bad.len()
                ),
            ));
        }
    } else {
        rollback_subscription(transport, sub.id).await;
        return Err(SdkDriverError::new(
            ErrorKind::Connection,
            "OPCUA_EVENT_SUBSCRIBE_FAILED",
            format!(
                "task `{}`: 事件监控项创建失败 status={:#X}",
                task.id, created.status_code
            ),
        ));
    }
    let mi_id = created.monitored_item_id;
    // 所有权拆分：receiver 先 drop（§21 先停 raw 接收），再删监控项/订阅。
    let sub_id = sub.id;
    let mut rx = sub.receiver;
    let mut fatal = sub.fatal;
    let ctx = EventDecodeContext {
        namespaces: namespaces.as_slice(),
        notifier: &task.notifier,
    };
    // 主循环：fatal/notify/shutdown 三路；shutdown 后排空 transport 队列
    // 再退出（§31：Stop 前已进 callback 的 occurrence 必须 COMMIT）。
    let mut stopping = false;
    let outcome = loop {
        if stopping {
            match rx.try_recv() {
                Ok(first) => {
                    if let Err(e) = publish_batch(&ctx, task, sink, first, &mut rx).await {
                        break Err(e);
                    }
                    continue;
                }
                Err(_) => break Ok(()),
            }
        }
        tokio::select! {
            _ = shutdown.cancelled() => {
                stopping = true;
            }
            fatal_changed = fatal.changed() => {
                break match fatal_changed {
                    Err(_) => Err(SdkDriverError::new(
                        ErrorKind::Connection,
                        "OPCUA_EVENT_SESSION_LOST",
                        format!("task `{}`: 事件流 sender 丢失（会话已死）", task.id),
                    )),
                    Ok(()) => match *fatal.borrow() {
                        Some(UaEventStreamFatal::CallbackQueueOverflow) => {
                            Err(SdkDriverError::new(
                                ErrorKind::Connection,
                                "OPCUA_EVENT_CALLBACK_OVERFLOW",
                                format!("task `{}`: callback 队列溢出，流完整性已失效", task.id),
                            ))
                        }
                        Some(UaEventStreamFatal::MalformedNotification) => {
                            Err(SdkDriverError::new(
                                ErrorKind::Connection,
                                "OPCUA_EVENT_DECODE_FAILED",
                                format!("task `{}`: 收到无字段通知，契约已破坏", task.id),
                            ))
                        }
                        // watch 初始 None；changed 触发必有值，防御性分支。
                        None => Err(SdkDriverError::new(
                            ErrorKind::Internal,
                            "OPCUA_EVENT_SESSION_LOST",
                            format!("task `{}`: fatal 信号为空（内部不一致）", task.id),
                        )),
                    },
                };
            }
            first = rx.recv() => {
                let Some(first) = first else {
                    // 通道关闭且已空：shutdown 竞态下属干净退出（无可转发项），
                    // 否则即会话完整性丢失。
                    if shutdown.is_cancelled() {
                        break Ok(());
                    }
                    break Err(SdkDriverError::new(
                        ErrorKind::Connection,
                        "OPCUA_EVENT_SESSION_LOST",
                        format!("task `{}`: 事件通道关闭（会话已死）", task.id),
                    ));
                };
                if let Err(e) = publish_batch(&ctx, task, sink, first, &mut rx).await {
                    break Err(e);
                }
            }
        }
    };
    // §21 单任务 teardown：停 raw 接收 → 删监控项 → 删订阅（幂等，失败仅诊断）。
    drop(rx);
    if let Err(e) = transport.delete_monitored_items(sub_id, &[mi_id]).await {
        tracing::warn!(task = %task.id, sub_id, error = %e, "事件监控项清理失败（仅诊断）");
    }
    if let Err(e) = transport.delete_subscription(sub_id).await {
        tracing::warn!(task = %task.id, sub_id, error = %e, "事件订阅清理失败（仅诊断）");
    }
    outcome
}

/// 聚合当前已就绪的 occurrence（首个 + try_recv 至多 64）→ 逐个解码 →
/// 一次 publish。任一解码失败即整批失败（fail-closed，不跳过坏 occurrence）。
/// 服务端队列溢出事件先 publish 再返回 overflow 错误（§6：COMMIT 后才 fatal）。
async fn publish_batch(
    ctx: &EventDecodeContext<'_>,
    task: &OpcUaEventTaskPlan,
    sink: &EventSink,
    first: UaEventNotification,
    rx: &mut tokio::sync::mpsc::Receiver<UaEventNotification>,
) -> Result<(), SdkDriverError> {
    let mut raws = vec![first];
    while let Ok(n) = rx.try_recv() {
        raws.push(n);
        if raws.len() >= EVENT_PUBLISH_BATCH_MAX {
            break;
        }
    }
    let mut records = Vec::with_capacity(raws.len());
    let mut server_overflow = false;
    for n in &raws {
        if n.fields.len() != STANDARD_EVENT_FIELD_COUNT {
            return Err(SdkDriverError::new(
                ErrorKind::Connection,
                "OPCUA_EVENT_DECODE_FAILED",
                format!(
                    "task `{}`: 通知字段数 {} 与标准表 {} 对不上",
                    task.id,
                    n.fields.len(),
                    STANDARD_EVENT_FIELD_COUNT
                ),
            ));
        }
        let decoded = decode_event_fields(ctx, &n.fields).map_err(|e| {
            SdkDriverError::new(
                ErrorKind::Connection,
                e.code.clone(),
                format!("task `{}`: {e}", task.id),
            )
        })?;
        server_overflow |= decoded.server_queue_overflow;
        records.push(decoded.record);
    }
    match sink.publish(records).await {
        Ok(_) => {}
        // 会话 teardown 中：publish 原子失败（无任何记录入队），静默结束；
        // 已入队部分由 Stop barrier 覆盖。
        Err(EventPublishError::Closed) => return Ok(()),
        Err(e) => {
            return Err(SdkDriverError::new(
                ErrorKind::Connection,
                "OPCUA_EVENT_PUBLISH_FAILED",
                format!("task `{}`: publish 失败: {e:?}", task.id),
            ));
        }
    }
    if server_overflow {
        return Err(SdkDriverError::new(
            ErrorKind::Connection,
            "OPCUA_EVENT_SERVER_QUEUE_OVERFLOW",
            format!(
                "task `{}`: 服务端上报队列溢出（overflow 事件已 COMMIT，流完整性已失效）",
                task.id
            ),
        ));
    }
    Ok(())
}

/// 建项失败回滚：同步 best-effort 删空订阅（调用方 await，返回 Err 前
/// 清理已完成，确定性可测）。cleanup 自身失败仅诊断，不掩盖原始错误。
async fn rollback_subscription(transport: &Arc<dyn OpcUaTransport>, sub_id: u32) {
    if let Err(e) = transport.delete_subscription(sub_id).await {
        tracing::debug!(sub_id, ?e, "事件订阅创建失败后回滚删订阅失败（仅诊断）");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_opcua_transport::{FakeOpcUaTransport, UaEventNotification};

    fn test_namespaces() -> Vec<String> {
        vec![
            "http://opcfoundation.org/UA/".into(),
            "http://example.com/Other/".into(),
            "http://example.com/MyModel/".into(),
        ]
    }

    fn test_plan() -> OpcUaEventTaskPlan {
        OpcUaEventTaskPlan {
            id: "ev1".into(),
            notifier: UaNodeRef::numeric(0, 2253),
            scope: EventScope::All,
            publishing_interval_ms: 500,
            queue_size: 1000,
        }
    }

    /// 19 字段 BaseEvent 通知（与 event.rs 解码单测同构，走真实 decoder）。
    fn base_notification() -> UaEventNotification {
        use opcua_types::{ByteString, DateTime, LocalizedText, UAString, Variant as V};
        let dt = |ns: i64| {
            V::DateTime(Box::new(DateTime::from(
                ns / 100 + 11644473600 * 10_000_000,
            )))
        };
        let mut fields = vec![
            V::ByteString(ByteString::from(vec![7u8, 7, 7])), // EventId
            V::NodeId(Box::new(opcua_types::NodeId::new(0, 2041u32))), // EventType
            V::NodeId(Box::new(opcua_types::NodeId::new(0, 2253u32))), // SourceNode
            V::String(UAString::from("MesaFixture")),         // SourceName
            dt(1_700_000_000_000_000_000),                    // Time
            dt(1_700_000_000_500_000_000),                    // ReceiveTime
            V::LocalizedText(Box::new(LocalizedText::new("", "runtime event"))), // Message
            V::UInt16(321),                                   // Severity
        ];
        fields.extend(std::iter::repeat_n(V::Empty, 19 - fields.len()));
        UaEventNotification {
            client_handle: 1,
            fields,
        }
    }

    fn test_sink() -> (
        EventSink,
        tokio::sync::mpsc::Receiver<mesa_driver_sdk::EventBatch>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        (EventSink::for_test(tx, 7, 1), rx)
    }

    #[tokio::test]
    async fn event_task_decodes_publishes_and_cleans_up() {
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(test_namespaces())
                .with_event_notifications(vec![base_notification()]),
        );
        let transport: Arc<dyn OpcUaTransport> = fake.clone();
        let (sink, mut erx) = test_sink();
        let shutdown = CancellationToken::new();
        let worker = tokio::spawn(run_event_task(
            transport,
            test_plan(),
            Arc::new(test_namespaces()),
            sink,
            shutdown.clone(),
        ));
        // 首批到达即解码+发布已发生（确定性触发，非 sleep 猜测）。
        let batch = tokio::time::timeout(std::time::Duration::from_secs(5), erx.recv())
            .await
            .expect("首批事件必须到达")
            .expect("通道不得关闭");
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.events[0].event_id, "opcua:BwcH");
        assert_eq!(batch.events[0].message.as_deref(), Some("runtime event"));
        // Stop：排空后退出 Ok，并按序清理（删监控项 → 删订阅）。
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .expect("worker 必须退出")
            .expect("worker 不 panic")
            .expect("正常 Stop 必须 Ok");
        let subs = fake.created_subscriptions();
        assert_eq!(subs.len(), 1);
        assert_eq!(fake.deleted_subscriptions(), subs);
        assert_eq!(fake.deleted_items().len(), 1);
        assert_eq!(fake.deleted_items()[0].0, subs[0]);
    }

    #[tokio::test]
    async fn event_task_item_bad_fails_and_rolls_back() {
        use opcua_types::StatusCode;
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_event_item_status(&UaNodeRef::numeric(0, 2253), StatusCode::BadNodeIdUnknown),
        );
        let transport: Arc<dyn OpcUaTransport> = fake.clone();
        let (sink, _erx) = test_sink();
        let err = run_event_task(
            transport,
            test_plan(),
            Arc::new(test_namespaces()),
            sink,
            CancellationToken::new(),
        )
        .await
        .expect_err("监控项 BAD 必须失败");
        assert_eq!(err.code, "OPCUA_EVENT_SUBSCRIBE_FAILED");
        // 空订阅已回滚删除。
        assert_eq!(fake.created_subscriptions().len(), 1);
        assert_eq!(fake.deleted_subscriptions(), fake.created_subscriptions());
    }

    #[tokio::test]
    async fn event_task_any_clause_rejected_fails() {
        use opcua_types::StatusCode;
        // 注意：故意 BAD 一个非 Base clause（Enabled，索引 12）——位置契约
        // 要求 19 个全 Good，任何一个 BAD 都必须拒绝（形变即违约）。
        let mut clauses = vec![StatusCode::Good; 19];
        clauses[12] = StatusCode::BadNotSupported;
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(test_namespaces())
                .with_event_clause_statuses(&UaNodeRef::numeric(0, 2253), clauses),
        );
        let transport: Arc<dyn OpcUaTransport> = fake.clone();
        let (sink, _erx) = test_sink();
        let err = run_event_task(
            transport,
            test_plan(),
            Arc::new(test_namespaces()),
            sink,
            CancellationToken::new(),
        )
        .await
        .expect_err("核心 clause 被拒必须失败");
        assert_eq!(err.code, "OPCUA_EVENT_FILTER_REJECTED");
        assert_eq!(fake.deleted_subscriptions(), fake.created_subscriptions());
    }

    #[tokio::test]
    async fn event_task_subscription_error_fails() {
        let fake = Arc::new(FakeOpcUaTransport::new().with_event_subscription_error(
            mesa_opcua_transport::UaTransportError::session(
                mesa_opcua_transport::UaOperation::CreateEventSubscription,
                None,
                "session lost（脚本）",
            ),
        ));
        let transport: Arc<dyn OpcUaTransport> = fake.clone();
        let (sink, _erx) = test_sink();
        let err = run_event_task(
            transport,
            test_plan(),
            Arc::new(test_namespaces()),
            sink,
            CancellationToken::new(),
        )
        .await
        .expect_err("订阅失败必须上抛");
        assert_eq!(err.code, "OPCUA_EVENT_SUBSCRIBE_FAILED");
        assert!(fake.created_subscriptions().is_empty());
    }
}
