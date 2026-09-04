//! 经公共 transport 直连真实 OPC UA Server 的冒烟示例（Stage 2 P0-B）。
//!
//! 用法：`cargo run -p mesa-driver-opcua --example test_native_opcua -- opc.tcp://host:4840`

use mesa_driver_opcua::parse_address;
use mesa_opcua_transport::{
    NativeOpcUaTransport, OpcUaConnectOptions, OpcUaTransport, UaBrowseRequest,
    UaMonitoredItemSpec, UaNodeRef, UaSubscriptionSpec,
};

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or("opc.tcp://uademo.prosysopc.com:53530/OPCUA/SimulationServer".to_string());
    println!("connecting to {}", url);
    let pki_dir = std::env::var("MESA_OPCUA_PKI_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("data/certificates/opcua"));
    let options = OpcUaConnectOptions {
        endpoint_url: url.clone(),
        pki_dir,
        ..Default::default()
    };
    let transport = NativeOpcUaTransport::new(options);
    match transport.connect().await {
        Ok(()) => println!("connect OK"),
        Err(e) => {
            println!("connect failed: {}", e);
            return;
        }
    }
    let nodes = vec!["ns=2;i=1", "ns=2;s=Sine", "ns=2;i=1001", "ns=2;s=MyString"];
    let mut refs = vec![];
    for n in nodes {
        match parse_address(n) {
            Ok(a) => refs.push(UaNodeRef {
                namespace: a.namespace,
                identifier: match a.identifier {
                    mesa_driver_opcua::Identifier::Numeric(x) => {
                        mesa_opcua_transport::UaIdentifier::Numeric(x)
                    }
                    mesa_driver_opcua::Identifier::String(s) => {
                        mesa_opcua_transport::UaIdentifier::String(s)
                    }
                    mesa_driver_opcua::Identifier::Guid(g) => {
                        mesa_opcua_transport::UaIdentifier::Guid(g)
                    }
                    mesa_driver_opcua::Identifier::Opaque(b) => {
                        mesa_opcua_transport::UaIdentifier::Opaque(b)
                    }
                },
            }),
            Err(e) => println!("parse {} failed: {:?}", n, e),
        }
    }
    match transport.read(&refs).await {
        Ok(vals) => {
            println!("read OK {} DataValues", vals.len());
            for (i, dv) in vals.iter().enumerate() {
                println!(
                    "  {}: status={:?} ts={:?} value={:?}",
                    i, dv.status, dv.source_timestamp, dv.value
                );
            }
        }
        Err(e) => println!("read failed: {}", e),
    }
    match transport.read_namespace_array().await {
        Ok(ns) => println!("namespace array: {:?}", ns),
        Err(e) => println!("namespace array failed: {}", e),
    }
    let root = UaNodeRef::numeric(0, 85);
    match transport
        .browse(UaBrowseRequest {
            node: root.clone(),
            max_refs: 10,
        })
        .await
    {
        Ok(page) => {
            println!("browse OK {} nodes", page.nodes.len());
            for n in &page.nodes {
                println!("  {} {}", n.browse_name, n.node_id);
            }
        }
        Err(e) => println!("browse failed: {}", e),
    }
    // P0-B4：小 max_refs 强制服务端分页，走 browse_next 接力取全页。
    {
        let mut page = transport
            .browse(UaBrowseRequest {
                node: root.clone(),
                max_refs: 1,
            })
            .await
            .expect("browse(max_refs=1) 必须 Ok");
        let mut total = page.nodes.len();
        let mut hops = 0u32;
        while let Some(token) = page.continuation_point {
            hops += 1;
            page = transport
                .browse_next(token.clone())
                .await
                .expect("browse_next 必须 Ok");
            total += page.nodes.len();
            transport
                .release_continuation(token)
                .await
                .expect("release_continuation 必须 Ok（幂等）");
            if hops > 16 {
                panic!("翻页超过 16 跳，疑似死循环");
            }
        }
        println!("browse paging OK: total={total} nodes, next-hops={hops}");
    }
    // P0-B1/B2/B3：订阅分裂生命周期 + 部分 BAD + live 事件 + 严格清理。
    // ns=0;i=2258 = Server_ServerStatus_CurrentTime（每 tick 变化，必产 live 事件）；
    // ns=9;i=999999 不存在，用于验证逐项 BAD 不整体失败。
    {
        let mut sub = transport
            .create_subscription(UaSubscriptionSpec {
                publishing_interval_ms: 250,
                ..Default::default()
            })
            .await
            .expect("create_subscription 必须 Ok");
        println!(
            "subscription OK id={} requested={}ms revised={}ms lifetime={} keep_alive={}",
            sub.id,
            sub.requested_publishing_interval_ms,
            sub.revised_publishing_interval_ms,
            sub.revised_lifetime_count,
            sub.revised_max_keep_alive_count
        );
        let items = vec![
            UaMonitoredItemSpec {
                node: UaNodeRef::numeric(0, 2258),
                client_handle: 1,
                sampling_interval_ms: 250,
                queue_size: 10,
                discard_oldest: true,
            },
            UaMonitoredItemSpec {
                node: UaNodeRef::numeric(9, 999999),
                client_handle: 2,
                sampling_interval_ms: 250,
                queue_size: 10,
                discard_oldest: true,
            },
        ];
        let created = transport
            .create_monitored_items(sub.id, &items)
            .await
            .expect("create_monitored_items 服务级必须 Ok");
        assert_eq!(created.len(), 2, "逐项结果必须与请求一一对应");
        let mut good_ids = Vec::new();
        for r in &created {
            let st = opcua_types::StatusCode::from(r.status_code);
            println!(
                "  item handle={} mi_id={} status={:?} revised_sampling={}ms",
                r.client_handle, r.monitored_item_id, st, r.revised_sampling_interval_ms
            );
            if st.is_good() {
                good_ids.push(r.monitored_item_id);
            } else {
                assert_eq!(r.client_handle, 2, "唯有不存在的节点应 BAD");
            }
        }
        assert_eq!(good_ids.len(), 1, "仅 CurrentTime 一项建成功");
        // 等 3 个 live 事件（CurrentTime 每 publish 必变）
        let mut live = 0u32;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while live < 3 {
            let remain = deadline.saturating_duration_since(std::time::Instant::now());
            if remain.is_zero() {
                panic!("10s 内未收到 3 个 live 事件，仅收到 {live}");
            }
            match tokio::time::timeout(remain, sub.receiver.recv()).await {
                Ok(Some(ev)) => {
                    assert_eq!(ev.client_handle, 1);
                    // P0-A 语义：status 缺席 == Good（服务端省略 Good 默认值）。
                    // 与 decode_data_value() 的 unwrap_or(Good) 一致。
                    let st = ev
                        .data_value
                        .status
                        .unwrap_or(opcua_types::StatusCode::Good);
                    assert!(!st.is_bad(), "live 事件不应为 Bad，实际 {st:?}");
                    println!("  live ev: status={st:?} value={:?}", ev.data_value.value);
                    live += 1;
                }
                Ok(None) => panic!("receiver 提前关闭"),
                Err(_) => panic!("10s 内未收到 3 个 live 事件，仅收到 {live}"),
            }
        }
        let received = sub
            .stats
            .events_received
            .load(std::sync::atomic::Ordering::Relaxed);
        println!("live events OK: got={live} callback_received={received}");
        transport
            .delete_monitored_items(sub.id, &good_ids)
            .await
            .expect("delete_monitored_items 必须 Ok");
        transport
            .delete_subscription(sub.id)
            .await
            .expect("delete_subscription 必须 Ok");
        // 幂等复删必须 Ok
        transport
            .delete_subscription(sub.id)
            .await
            .expect("重复 delete_subscription 幂等 Ok");
        println!("subscription cleanup OK (incl. idempotent re-delete)");
    }
    let _ = transport.disconnect().await;
    println!("disconnect OK");
    // Dynamic Probe（§8）：open 建连 → connection.probe（复用同一 transport 会话）。
    {
        let drv = mesa_driver_opcua::OpcUaDriver;
        let cfg = format!(r#"{{"endpoint_url":"{url}","timeout_ms":5000}}"#);
        let mut conn = mesa_driver_sdk::Driver::open_connection(&drv, "smoke", &cfg)
            .await
            .expect("open 必须 Ok");
        let rep = mesa_driver_sdk::DriverConnection::probe(&mut *conn)
            .await
            .expect("probe 必须 Ok");
        println!(
            "probe OK reachable={} vendor={:?} model={:?} firmware={:?} caps={:?} warnings={:?}",
            rep.reachable,
            rep.vendor,
            rep.model,
            rep.firmware,
            rep.capabilities,
            rep.warnings
                .iter()
                .map(|w| w.code.clone())
                .collect::<Vec<_>>()
        );
        assert!(rep.reachable);
    }
}
