//! PR9 Stage ⑥ spike：fixture 服务器启动 → transport 建连 → NamespaceArray。
//! 证书/会话细节在此逐个验证，通过后再长成完整事件流测试。

mod support;

use mesa_opcua_transport::{NativeOpcUaTransport, OpcUaConnectOptions, OpcUaTransport};
use support::event_server::FixtureEventServer;

#[tokio::test]
async fn spike_fixture_server_accepts_transport_connect() {
    let mut srv = FixtureEventServer::start().await;
    let client_pki = srv.pki_dir.join("..").join(format!(
        "mesa-opcua-event-fixture-cli-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&client_pki).unwrap();
    let options = OpcUaConnectOptions {
        endpoint_url: srv.endpoint_url(),
        pki_dir: client_pki.clone(),
        ..Default::default()
    };
    let transport = NativeOpcUaTransport::new(options);
    transport
        .connect()
        .await
        .expect("fixture loopback 必须可连");
    let ns = transport
        .read_namespace_array()
        .await
        .expect("NamespaceArray 必须可读");
    assert!(!ns.is_empty(), "NamespaceArray 非空");
    transport.disconnect().await.expect("disconnect 必须 Ok");
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&client_pki);
}

#[tokio::test]
async fn spike_event_subscription_receives_triggered_base_event() {
    use mesa_opcua_transport::{
        UaEventFilterSpec, UaEventMonitoredItemSpec, UaEventSelectClause, UaNodeRef,
        UaQualifiedNameRef, UaSubscriptionSpec,
    };
    use opcua_types::{ByteString, DateTime, LocalizedText, NodeId, ObjectTypeId, Variant};

    let mut srv = FixtureEventServer::start().await;
    let client_pki = srv.pki_dir.join("..").join(format!(
        "mesa-opcua-event-fixture-cli-ev-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&client_pki).unwrap();
    let transport = NativeOpcUaTransport::new(OpcUaConnectOptions {
        endpoint_url: srv.endpoint_url(),
        pki_dir: client_pki.clone(),
        ..Default::default()
    });
    transport.connect().await.expect("必须可连");

    // 建 Event 订阅（2 clause：EventId + Severity，验证位置契约）。
    let mut sub = transport
        .create_event_subscription(UaSubscriptionSpec {
            publishing_interval_ms: 500,
            ..Default::default()
        })
        .await
        .expect("建 Event 订阅必须 Ok");
    let clause = |name: &str| UaEventSelectClause {
        type_definition_id: UaNodeRef::numeric(0, ObjectTypeId::BaseEventType as u32),
        browse_path: vec![UaQualifiedNameRef {
            namespace: 0,
            name: name.into(),
        }],
        attribute_id: 13,
    };
    let created = transport
        .create_event_monitored_items(
            sub.id,
            &[UaEventMonitoredItemSpec {
                notifier: UaNodeRef::numeric(0, 2253),
                client_handle: 1,
                queue_size: 1000,
                filter: UaEventFilterSpec {
                    select_clauses: vec![clause("EventId"), clause("Severity")],
                    of_type: None,
                },
            }],
        )
        .await
        .expect("建 Event 监控项服务级必须 Ok");
    assert_eq!(created.len(), 1);
    assert!(
        opcua_types::StatusCode::from(created[0].status_code).is_good(),
        "监控项必须 Good，实际 {:#X}",
        created[0].status_code
    );
    assert_eq!(created[0].select_clause_statuses.len(), 2);
    assert!(
        created[0]
            .select_clause_statuses
            .iter()
            .all(|s| opcua_types::StatusCode::from(*s).is_good()),
        "clause 结果必须全 Good"
    );

    // fixture_ready barrier：监控项已 Good，此后 trigger。
    let event = opcua_nodes::BaseEventType::new(
        NodeId::new(0, ObjectTypeId::BaseEventType as u32),
        ByteString::from(vec![0xE1u8]),
        LocalizedText::new("", "spike event"),
        DateTime::now(),
    );
    let server_node = NodeId::new(0, 2253u32);
    {
        let mut notifier = srv.handle.subscriptions().event_notifier();
        notifier.notify(&server_node, &event);
        // drop 提交。
    }

    let notif = tokio::time::timeout(std::time::Duration::from_secs(10), sub.receiver.recv())
        .await
        .expect("10s 内必须收到事件")
        .expect("通道不得关闭");
    assert_eq!(notif.client_handle, 1);
    assert_eq!(notif.fields.len(), 2, "字段数必须严格对应 clauses");
    assert_eq!(
        notif.fields[0],
        Variant::ByteString(ByteString::from(vec![0xE1u8]))
    );
    // Severity 缺省 0（未设置）——位置正确即通过，值语义由 decoder 单测覆盖。
    assert!(matches!(notif.fields[1], Variant::UInt16(_)));

    transport
        .delete_monitored_items(sub.id, &[created[0].monitored_item_id])
        .await
        .expect("删监控项必须 Ok");
    transport
        .delete_subscription(sub.id)
        .await
        .expect("删订阅必须 Ok");
    transport.disconnect().await.expect("disconnect 必须 Ok");
    drop(sub);
    srv.stop().await;
    let _ = std::fs::remove_dir_all(&client_pki);
}
