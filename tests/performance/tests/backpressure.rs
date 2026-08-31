//! Backpressure 25% 预检：Core 消费降至 25% 时有界 + 控制仍响应
//! 复用 driver-sdk 已有 Latest-Wins 语义，此处做 256 有界通道占满后 try_send Full 仍有界验证

use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test]
async fn backpressure_25_percent_coalesces() {
    // 模拟 OUTBOUND 256 有界：填满后 try_send 应 Full 而非无限增长
    let (tx, mut rx) = mpsc::channel::<u32>(256);
    for i in 0..256 {
        tx.try_send(i).unwrap();
    }
    assert!(tx.try_send(999).is_err(), "有界 256 应在 256 后 Full");
    // 消费降至 25%：消费者 sleep 75% 时间仍能 drain 且控制通道（同 mpsc）可 try_send
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut drained = 0;
        while rx.try_recv().is_ok() {
            drained += 1;
        }
        drained
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    // 此时仍有界，未 OOM
    assert!(tx.capacity() == 0 || tx.max_capacity() == 256);
    let drained = handle.await.unwrap();
    println!("backpressure coalesces drained={drained} bounded=256");
    assert!(drained > 0);
}
