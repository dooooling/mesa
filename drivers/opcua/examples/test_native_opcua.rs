//! 经公共 transport 直连真实 OPC UA Server 的冒烟示例（Stage 2 P0-B）。
//!
//! 用法：`cargo run -p mesa-driver-opcua --example test_native_opcua -- opc.tcp://host:4840`

use mesa_driver_opcua::parse_address;
use mesa_opcua_transport::{
    NativeOpcUaTransport, OpcUaConnectOptions, OpcUaTransport, UaBrowseRequest, UaNodeRef,
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
        endpoint_url: url,
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
            node: root,
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
    let _ = transport.disconnect().await;
}
