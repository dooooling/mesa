//! 长稳态测试的有界诊断：每秒记录推进位置，失败后无需依赖内存中的最后快照。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mesa_driver_manager::MesaManager;
use mesa_event_store::EventServices;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub struct SoakTrace {
    pub path: PathBuf,
    stopping: Arc<AtomicBool>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
}

impl SoakTrace {
    /// 每次测试独立记录，避免前后两轮的时间线或计数混在一起。
    pub async fn start(
        mgr: Arc<MesaManager>,
        services: Arc<EventServices>,
        endpoint: String,
    ) -> Self {
        let directory = super::common::repo_root().join("target/validation/event-soak");
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let path = directory.join(format!(
            "{}-{}-{}.jsonl",
            endpoint,
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        let mut file = tokio::fs::File::create(&path).await.unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let stage = Arc::clone(&stopping);
        let cancel = CancellationToken::new();
        let done = cancel.clone();
        let task = tokio::spawn(async move {
            let start = Instant::now();
            let mut last_progress = start;
            let mut last_persisted = 0;
            let mut timer = tokio::time::interval(Duration::from_secs(1));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // 最大档为 150s + 有界停机预算；硬上限防止诊断本身无限写入。
            for _ in 0..256 {
                tokio::select! {
                    biased;
                    _ = done.cancelled() => break,
                    _ = timer.tick() => {}
                }
                let d = &services.diagnostics;
                let persisted = d.ingress_persisted_events_total.load(Ordering::Relaxed);
                if persisted != last_persisted {
                    last_progress = Instant::now();
                    last_persisted = persisted;
                }
                let session = mgr.registry().read().unwrap().get(&endpoint).cloned();
                // 不与 Stop 争用会话锁，更不能让诊断等待同一条卡住的 RPC。
                let session = if let Some(handle) = session {
                    match handle.try_lock() {
                        Ok(s) => serde_json::json!({
                            "wire_batches": s.test_event_batches_received(),
                            "queue_depth_last_send_sample": s.event_queue_depth_last_sample(),
                            "queue_high_water": s.event_queue_high_water(),
                            "overflow_drops": s.event_overflow_drops(),
                            "stream_failed": s.event_stream_failed(),
                            "decode_errors": s.event_decode_errors()
                        }),
                        Err(_) => serde_json::json!({"locked": true}),
                    }
                } else {
                    serde_json::Value::Null
                };
                let sample = serde_json::json!({
                    "utc_ns": mesa_core_types::now_unix_ns(),
                    "elapsed_ms": start.elapsed().as_millis(),
                    "stage": if stage.load(Ordering::Relaxed) { "stopping" } else { "observing" },
                    "attempt_phase": mgr.attempt_phase(&endpoint).map(|p| p.as_str()),
                    "session": session,
                    "ingress_batches": d.ingress_batches_total.load(Ordering::Relaxed),
                    "persisted_events": persisted,
                    "progress_age_ms": last_progress.elapsed().as_millis(),
                    "ingress_commit_max_ns": d.ingress_commit_latency_max_ns.load(Ordering::Relaxed),
                    "writer_commit_max_ns": services.store.writer_commit_latency_max_ns(),
                    "writer": services.store.test_writer_snapshot()
                });
                let mut line = serde_json::to_vec(&sample)?;
                line.push(b'\n');
                // 文件 I/O 走 Tokio blocking pool，不同步阻塞被观测的单线程 runtime。
                file.write_all(&line).await?;
                file.flush().await?;
            }
            Ok(())
        });
        eprintln!("event-soak trace: {}", path.display());
        Self {
            path,
            stopping,
            cancel,
            task: Some(task),
        }
    }

    /// 标出 Stop 起点，区分运行期消费停滞与停止后的排空。
    pub fn stop_started(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }

    /// 有界结束诊断任务；写文件失败单独报告，不覆盖被测 Stop 的返回值。
    pub async fn finish(mut self) {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return;
        };
        match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(other) => eprintln!("event-soak trace write failed: {other:?}"),
            Err(_) => {
                eprintln!("event-soak trace write did not complete in 2s");
                task.abort();
                let _ = task.await;
            }
        }
    }
}

impl Drop for SoakTrace {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
