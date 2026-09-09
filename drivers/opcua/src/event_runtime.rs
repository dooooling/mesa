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
    // canonical notifier → 当前会话 index（与 Data 路径同一快照；未知 URI
    // 即 fail-closed，不带着过期 index 订阅）。
    let notifier = task.notifier.resolve(namespaces).map_err(|e| {
        SdkDriverError::new(
            ErrorKind::Address,
            "UNKNOWN_NAMESPACE",
            format!("event task `{}`: {e}", task.id),
        )
    })?;
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
        notifier: notifier.clone(),
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
    // P0-3/P1：where 部分同样是 filter 定义的一半。all 要求 exactly 0 个
    // element（本实现 all 形态不带 where，多一个都是非预期）；conditions
    // 要求恰一个 OfType element 且 Good。报告的 operand BAD 同样 fail-closed
    //（element 表面 Good 也掩盖不了 operand 错误；未报告即接受）。
    // 否则用户要的过滤语义并未成立，必须拒绝。
    fn is_good(bits: u32) -> bool {
        opcua_types::StatusCode::from(bits).is_good()
    }
    let bad_elements: Vec<u32> = created
        .where_clause_statuses
        .iter()
        .enumerate()
        .filter(|(_, s)| !is_good(**s))
        .map(|(i, _)| i as u32)
        .collect();
    let bad_operands: Vec<(u32, u32)> = created
        .where_operand_statuses
        .iter()
        .enumerate()
        .flat_map(|(ei, ops)| {
            ops.iter().enumerate().filter_map(move |(oi, s)| {
                if is_good(*s) {
                    None
                } else {
                    Some((ei as u32, oi as u32))
                }
            })
        })
        .collect();
    let where_ok = match task.scope {
        EventScope::All => created.where_clause_statuses.is_empty() && bad_operands.is_empty(),
        EventScope::Conditions => {
            created.where_clause_statuses.len() == 1
                && bad_elements.is_empty()
                && bad_operands.is_empty()
        }
    };
    if !where_ok {
        rollback_subscription(transport, sub.id).await;
        return Err(SdkDriverError::new(
            ErrorKind::Connection,
            "OPCUA_EVENT_FILTER_REJECTED",
            format!(
                "task `{}`: where 子句未被服务器接受（scope={:?}，BAD element {bad_elements:?}，BAD operand {bad_operands:?}）",
                task.id, task.scope,
            ),
        ));
    }
    let sub_id = sub.id;
    // sub 本体保留（shutdown 时调 close_producer）；主循环与 drain 直接以
    // 不相交字段借用进 select（receiver ↔ fatal 互不干扰）。
    let mut sub = sub;
    let ctx = EventDecodeContext {
        namespaces: namespaces.as_slice(),
        notifier: &notifier,
    };
    // 主循环：fatal/notify/shutdown 三路。shutdown 只置位、不直接退出；
    // 真正的停止是下面的"关门 → drain"序列（P0-1）。
    enum RunExit {
        Shutdown,
        Failed(SdkDriverError),
    }
    let exit = loop {
        tokio::select! {
            _ = shutdown.cancelled() => break RunExit::Shutdown,
            fatal_changed = sub.fatal.changed() => {
                break RunExit::Failed(map_fatal_changed(&task.id, fatal_changed, *sub.fatal.borrow()));
            }
            first = sub.receiver.recv() => {
                let Some(first) = first else {
                    break RunExit::Failed(SdkDriverError::new(
                        ErrorKind::Connection,
                        "OPCUA_EVENT_SESSION_LOST",
                        format!("task `{}`: 事件通道关闭（会话已死）", task.id),
                    ));
                };
                if let Err(e) = publish_batch(&ctx, task, sink, first, &mut sub.receiver).await {
                    break RunExit::Failed(e);
                }
            }
        }
    };
    match exit {
        RunExit::Failed(e) => {
            // 失败即停：尽力关门清理（失败仅诊断），不 drain、不掩盖原始错误。
            sub.close_producer();
            cleanup_event_subscription(transport, &task.id, sub_id, mi_id).await;
            Err(e)
        }
        RunExit::Shutdown => {
            // P0-2 收尾模型：shutdown 第一件事即关本地门（server cleanup RPC
            // 再慢，期间也不会继续把本地 FIFO 撑爆），再删监控项/订阅，
            // receiver 保持 OPEN，drain 到 sender CLOSED（None）。drain 期
            // fatal 值为 Some 照常 fail；sender 丢失则是正常 teardown
            // （server delete 连带 drop session 侧 callback），此后只 drain
            //（flag 防 changed-Err 空转）。
            sub.close_producer();
            cleanup_event_subscription(transport, &task.id, sub_id, mi_id).await;
            let mut fatal_gone = false;
            loop {
                if fatal_gone {
                    let Some(first) = sub.receiver.recv().await else {
                        break;
                    };
                    publish_batch(&ctx, task, sink, first, &mut sub.receiver).await?;
                    continue;
                }
                tokio::select! {
                    fatal_changed = sub.fatal.changed() => {
                        if fatal_changed.is_ok() && sub.fatal.borrow().is_some() {
                            return Err(map_fatal_changed(
                                &task.id,
                                Ok(()),
                                *sub.fatal.borrow(),
                            ));
                        }
                        if fatal_changed.is_err() {
                            fatal_gone = true;
                        }
                        // Ok + None（防御性）/ Err 后继续 drain。
                    }
                    next = sub.receiver.recv() => {
                        let Some(first) = next else {
                            break;
                        };
                        publish_batch(&ctx, task, sink, first, &mut sub.receiver).await?;
                    }
                }
            }
            Ok(())
        }
    }
}

/// fatal watch 映射（主循环与 shutdown-drain 共用：drain 期间的 fatal
/// 同样是完整性破坏，不得吞成正常 Stop）。
fn map_fatal_changed(
    task_id: &str,
    changed: Result<(), tokio::sync::watch::error::RecvError>,
    current: Option<UaEventStreamFatal>,
) -> SdkDriverError {
    match changed {
        Err(_) => SdkDriverError::new(
            ErrorKind::Connection,
            "OPCUA_EVENT_SESSION_LOST",
            format!("task `{task_id}`: 事件流 sender 丢失（会话已死）"),
        ),
        Ok(()) => match current {
            Some(UaEventStreamFatal::CallbackQueueOverflow) => SdkDriverError::new(
                ErrorKind::Connection,
                "OPCUA_EVENT_CALLBACK_OVERFLOW",
                format!("task `{task_id}`: callback 队列溢出，流完整性已失效"),
            ),
            Some(UaEventStreamFatal::MalformedNotification) => SdkDriverError::new(
                ErrorKind::Connection,
                "OPCUA_EVENT_DECODE_FAILED",
                format!("task `{task_id}`: 收到无字段通知，契约已破坏"),
            ),
            // watch 初始 None；changed 触发必有值，防御性分支。
            None => SdkDriverError::new(
                ErrorKind::Internal,
                "OPCUA_EVENT_SESSION_LOST",
                format!("task `{task_id}`: fatal 信号为空（内部不一致）"),
            ),
        },
    }
}

/// 单任务 teardown：删监控项 → 删订阅（幂等，失败仅诊断）。
/// 注意：transport 的 delete_subscription 内部先关本地 producer 门
/// （P0-1），之后 driver 再显式 close_producer 幂等加固。
async fn cleanup_event_subscription(
    transport: &Arc<dyn OpcUaTransport>,
    task_id: &str,
    sub_id: u32,
    mi_id: u32,
) {
    if let Err(e) = transport.delete_monitored_items(sub_id, &[mi_id]).await {
        tracing::warn!(task = %task_id, sub_id, error = %e, "事件监控项清理失败（仅诊断）");
    }
    if let Err(e) = transport.delete_subscription(sub_id).await {
        tracing::warn!(task = %task_id, sub_id, error = %e, "事件订阅清理失败（仅诊断）");
    }
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
    // P0-2：按字节边界有序 split。SDK 保证 TooLarge 不消耗 sequence
    // （序号只在成功入队后递增），故二分重试安全：左半永远先于右半，
    // occurrence order 不变。`publish` 按值拿走 vec，TooLarge 不回吐，
    // 故 len>1 的 chunk 先 clone 留底（单次 memcpy，相对 DB/网络可忽略）。
    // 单条仍 TooLarge 即记录级超限（validate 理论上已拦，属防御性分支）。
    let mut pending = std::collections::VecDeque::from([records]);
    while let Some(chunk) = pending.pop_front() {
        if chunk.is_empty() {
            continue;
        }
        if chunk.len() == 1 {
            match sink.publish(chunk).await {
                Ok(_) => {}
                Err(EventPublishError::Closed) => return Ok(()),
                Err(EventPublishError::TooLarge(_)) => {
                    return Err(SdkDriverError::new(
                        ErrorKind::Connection,
                        "OPCUA_EVENT_RECORD_TOO_LARGE",
                        format!("task `{}`: 单条事件记录超过批大小上限", task.id),
                    ));
                }
                Err(e) => {
                    return Err(SdkDriverError::new(
                        ErrorKind::Connection,
                        "OPCUA_EVENT_PUBLISH_FAILED",
                        format!("task `{}`: publish 失败: {e:?}", task.id),
                    ));
                }
            }
            continue;
        }
        let retry = chunk.clone();
        match sink.publish(chunk).await {
            Ok(_) => {}
            // 会话 teardown 中：publish 原子失败（无任何记录入队），静默结束；
            // 已入队部分由 Stop barrier 覆盖。
            Err(EventPublishError::Closed) => return Ok(()),
            Err(EventPublishError::TooLarge(_)) => {
                let mut left = retry;
                let right = left.split_off(left.len() / 2);
                pending.push_front(right);
                pending.push_front(left); // 左半先行，order 不变
            }
            Err(e) => {
                return Err(SdkDriverError::new(
                    ErrorKind::Connection,
                    "OPCUA_EVENT_PUBLISH_FAILED",
                    format!("task `{}`: publish 失败: {e:?}", task.id),
                ));
            }
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
            notifier: mesa_opcua_transport::parse_canonical(
                "nsu=http://opcfoundation.org/UA/;i=2253",
            )
            .expect("测试 notifier 合法"),
            scope: EventScope::All,
            publishing_interval_ms: 500,
            queue_size: 1000,
        }
    }

    /// 19 字段 BaseEvent 通知（与 event.rs 解码单测同构，走真实 decoder）。
    fn base_notification() -> UaEventNotification {
        base_notification_with(vec![7u8, 7, 7], "runtime event")
    }

    /// 参数化通知：EventId 字节 + message（P0-1/P0-2 测试用）。
    fn base_notification_with(id: Vec<u8>, message: &str) -> UaEventNotification {
        use opcua_types::{ByteString, DateTime, LocalizedText, UAString, Variant as V};
        let dt = |ns: i64| {
            V::DateTime(Box::new(DateTime::from(
                ns / 100 + 11644473600 * 10_000_000,
            )))
        };
        let mut fields = vec![
            V::ByteString(ByteString::from(id)), // EventId
            V::NodeId(Box::new(opcua_types::NodeId::new(0, 2041u32))), // EventType
            V::NodeId(Box::new(opcua_types::NodeId::new(0, 2253u32))), // SourceNode
            V::String(UAString::from("MesaFixture")), // SourceName
            dt(1_700_000_000_000_000_000),       // Time
            dt(1_700_000_000_500_000_000),       // ReceiveTime
            V::LocalizedText(Box::new(LocalizedText::new("", message))), // Message
            V::UInt16(321),                      // Severity
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

    /// P0-1：shutdown 立刻到达（worker 可能一条都还没处理）→ 关门 → drain →
    /// 已接受的 N 条必须 N/N 到达，且退出 Ok（Empty 误判即丢尾，本测试必红）。
    #[tokio::test]
    async fn shutdown_drains_all_accepted_notifications() {
        const N: usize = 8;
        let notifs = (0..N as u8)
            .map(|i| base_notification_with(vec![0xA0 + i], "drain me"))
            .collect();
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(test_namespaces())
                .with_event_notifications(notifs),
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
        // 调度无关：无论 worker 先处理还是先看到 cancel，最终必须 N/N + Ok。
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .expect("worker 必须退出")
            .expect("worker 不 panic")
            .expect("drain 后必须 Ok");
        let mut got = vec![];
        while got.len() < N {
            let batch = tokio::time::timeout(std::time::Duration::from_secs(5), erx.recv())
                .await
                .expect("N 条必须全部到达")
                .expect("通道不得关闭");
            got.extend(batch.events);
        }
        assert_eq!(got.len(), N);
        for (i, r) in got.iter().enumerate() {
            assert_eq!(r.message.as_deref(), Some("drain me"));
            let _ = i;
        }
        // EventId 互异且保序（队列顺序即注入顺序）。
        let mut ids: Vec<&str> = got.iter().map(|r| r.event_id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), N);
        // 清理发生（删监控项 → 删订阅 → 关门）。
        assert_eq!(fake.deleted_subscriptions().len(), 1);
    }

    /// P0-2：64 条 × ~4.6 KiB（message 4000B）≈ 300 KiB > 256 KiB 上限 →
    /// 首 publish 必 TooLarge → 二分成 2 批；32 条全部到达、顺序不变、
    /// batch sequence 连续（TooLarge 不消耗序号）。
    #[tokio::test]
    async fn publish_splits_oversized_batches_preserving_order() {
        const N: usize = 64;
        let big = "M".repeat(4000);
        let notifs = (0..N as u8)
            .map(|i| base_notification_with(vec![0xB0 + (i >> 4), i & 0x0F], &big))
            .collect();
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(test_namespaces())
                .with_event_notifications(notifs),
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
        let mut batches = vec![];
        let mut total = 0usize;
        while total < N {
            let batch = tokio::time::timeout(std::time::Duration::from_secs(10), erx.recv())
                .await
                .expect("拆分后的批必须全部到达")
                .expect("通道不得关闭");
            total += batch.events.len();
            batches.push(batch);
        }
        assert_eq!(total, N);
        // 300 KiB 首 publish 必超限：恰好 2 批（32+32），sequence 连续。
        assert_eq!(batches.len(), 2, "必须二分成 2 批，实际 {}", batches.len());
        assert_eq!(batches[0].events.len(), 32);
        assert_eq!(batches[1].events.len(), 32);
        assert_eq!(batches[1].sequence, batches[0].sequence + 1);
        // 跨批顺序不变（左半先于右半）：event_id 解码回字节，与注入序列
        // 精确比对（注意：base64 sextet 序 ≠ ASCII 序，字符串比较无意义）。
        use base64::Engine as _;
        let all: Vec<&mesa_core_types::EventRecord> =
            batches.iter().flat_map(|b| &b.events).collect();
        let all_bytes: Vec<Vec<u8>> = all
            .iter()
            .map(|r| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(r.event_id.strip_prefix("opcua:").unwrap())
                    .unwrap()
            })
            .collect();
        let expect: Vec<Vec<u8>> = (0..N as u8)
            .map(|i| vec![0xB0 + (i >> 4), i & 0x0F])
            .collect();
        assert_eq!(all_bytes, expect);
        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), worker)
            .await
            .expect("worker 必须退出")
            .expect("worker 不 panic")
            .expect("必须 Ok");
    }

    /// P0-3：select 全 Good 但 where OfType 被拒 → FILTER_REJECTED + 回滚删订阅。
    #[tokio::test]
    async fn event_task_where_rejected_fails_and_rolls_back() {
        use opcua_types::StatusCode;
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(test_namespaces())
                .with_event_where_statuses(
                    &UaNodeRef::numeric(0, 2253),
                    vec![StatusCode::BadFilterOperatorUnsupported],
                ),
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
        .expect_err("where 被拒必须失败");
        assert_eq!(err.code, "OPCUA_EVENT_FILTER_REJECTED");
        assert_eq!(fake.created_subscriptions().len(), 1);
        assert_eq!(fake.deleted_subscriptions(), fake.created_subscriptions());
    }

    /// P1：element 表面 Good 但 operand 报 BAD → 同样 FILTER_REJECTED。
    #[tokio::test]
    async fn event_task_where_operand_bad_fails_and_rolls_back() {
        use opcua_types::StatusCode;
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(test_namespaces())
                .with_event_where_statuses(&UaNodeRef::numeric(0, 2253), vec![StatusCode::Good])
                .with_event_where_operand_statuses(
                    &UaNodeRef::numeric(0, 2253),
                    vec![vec![StatusCode::BadFilterLiteralInvalid]],
                ),
        );
        let transport: Arc<dyn OpcUaTransport> = fake.clone();
        let (sink, _erx) = test_sink();
        let mut plan = test_plan();
        plan.scope = EventScope::Conditions;
        let err = run_event_task(
            transport,
            plan,
            Arc::new(test_namespaces()),
            sink,
            CancellationToken::new(),
        )
        .await
        .expect_err("operand 被拒必须失败");
        assert_eq!(err.code, "OPCUA_EVENT_FILTER_REJECTED");
        assert_eq!(fake.deleted_subscriptions(), fake.created_subscriptions());
    }

    /// P1：scope=all 形态下多一个 where element 同样拒绝（exactly 0）。
    #[tokio::test]
    async fn event_task_all_scope_with_where_element_fails() {
        use opcua_types::StatusCode;
        let fake = Arc::new(
            FakeOpcUaTransport::new()
                .with_namespace_array(test_namespaces())
                .with_event_where_statuses(&UaNodeRef::numeric(0, 2253), vec![StatusCode::Good]),
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
        .expect_err("all 形态带 where 必须失败");
        assert_eq!(err.code, "OPCUA_EVENT_FILTER_REJECTED");
        assert_eq!(fake.deleted_subscriptions(), fake.created_subscriptions());
    }
}
