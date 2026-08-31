use mesa_driver_focas2::{FocasApi, NativeFocasApi, parse_address};
#[tokio::main]
async fn main() {
    let api = NativeFocasApi::new();
    let host = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "192.168.15.165".into());
    println!("connecting to {}:8193 ...", host);
    match api.connect(&host, 8193, 5000).await {
        Ok(()) => println!("connect OK"),
        Err(e) => {
            println!("connect failed: {}", e);
            return;
        }
    }
    let addrs = vec![
        parse_address("status").unwrap(),
        parse_address("axis.abs.1").unwrap(),
        parse_address("spindle.load.1").unwrap(),
    ];
    println!("reading {:?} ...", addrs);
    match api.read_batch(&addrs).await {
        Ok(vals) => {
            for (a, v) in addrs.iter().zip(vals.iter()) {
                println!("{:?} -> {:?}", a, v);
            }
        }
        Err(e) => println!("read failed: {}", e),
    }
    api.disconnect().await;
    println!("done");
}
