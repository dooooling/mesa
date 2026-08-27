//! IPC p95/p99 单调时钟预检（同机 Instant 差，非 UTC 相减 §10）
//! 1 point/batch burst1 interval1ms 采样 10k 批次算分位数

use std::time::{Duration, Instant};

#[tokio::test]
async fn ipc_latency_p95_p99() {
    // 同机 DataSink→Session 段内测量：此处用 tokio mpsc 模拟有界 256 Latest-Wins
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Instant>(2048);
    let send_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        for _ in 0..2000 {
            ticker.tick().await;
            // 用 send().await 避免 try_send 丢包导致接收端永久等待
            if tx.send(Instant::now()).await.is_err() { break; }
        }
    });
    let mut latencies: Vec<Duration> = Vec::with_capacity(2000);
    while let Some(sent) = rx.recv().await {
        latencies.push(sent.elapsed());
        if latencies.len() >= 2000 { break; }
    }
    let _ = send_task.await;
    latencies.sort();
    let p95 = latencies[ (latencies.len() as f64 * 0.95) as usize ];
    let p99 = latencies[ (latencies.len() as f64 * 0.99) as usize ];
    println!("ipc p95={:?} p99={:?} samples={}", p95, p99, latencies.len());
    // 同机回环阈值（§22 p95≤20ms p99≤50ms，含队列合并时应更低）
    assert!(p95 <= Duration::from_millis(20), "p95 {p95:?} >20ms");
    assert!(p99 <= Duration::from_millis(50), "p99 {p99:?} >50ms");
}
