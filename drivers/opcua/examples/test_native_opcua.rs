use mesa_driver_opcua::{NativeOpcUaApi, OpcUaApi, parse_address};

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or("opc.tcp://uademo.prosysopc.com:53530/OPCUA/SimulationServer".to_string());
    println!("connecting to {}", url);
    let api = NativeOpcUaApi::new();
    match api.connect(&url, 5000).await {
        Ok(()) => println!("connect OK"),
        Err(e) => {
            println!("connect failed: {}", e);
            return;
        }
    }
    let nodes = vec![
        "ns=2;i=1",        // Counter (auto numeric)
        "ns=2;s=Sine",     // Sine string
        "ns=2;i=1001",     // Numeric1001
        "ns=2;s=MyString", // MyString
    ];
    let mut addrs = vec![];
    for n in nodes {
        match parse_address(n) {
            Ok(a) => addrs.push(a),
            Err(e) => println!("parse {} failed: {:?}", n, e),
        }
    }
    match api.read_batch(&addrs).await {
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
    let _ = api.disconnect().await;
}
