use forgelink_driver_opcua::{parse_address, NativeOpcUaApi, OpcUaApi};

#[tokio::main]
async fn main() {
    let url = std::env::args().nth(1).unwrap_or("opc.tcp://uademo.prosysopc.com:53530/OPCUA/SimulationServer".to_string());
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
            println!("read OK {} values", vals.len());
            for (i, v) in vals.iter().enumerate() {
                println!("  {}: {:?}", i, v);
            }
        }
        Err(e) => println!("read failed: {}", e),
    }
    let _ = api.disconnect().await;
}
