//! 测试专用 writer 进度探针：区分命令排队、执行未返回和 Core 消费停滞。

use std::sync::Mutex;
use std::time::Instant;

/// 采样状态仅供定位阻塞，不参与提交、超时、背压或准入判定。
#[derive(Debug, serde::Serialize)]
pub struct TestWriterSnapshot {
    /// 队列内的命令数，不含已经取走的在途命令。
    pub queued: usize,
    pub started: u64,
    /// 命令执行已返回（成功或失败），不等同于 Core 已消费回复或发布事件。
    pub finished: u64,
    /// 从 writer 取出当前 Commit 起的单调时长，包括测试栅栏与 SQLite 执行。
    pub in_flight_ms: Option<u128>,
}

#[derive(Default)]
pub(crate) struct WriterProbe(Mutex<Progress>);

#[derive(Default)]
struct Progress {
    started: u64,
    finished: u64,
    since: Option<Instant>,
}

impl WriterProbe {
    pub fn start(&self) {
        let mut state = self.0.lock().unwrap();
        state.started += 1;
        state.since = Some(Instant::now());
    }

    pub fn finish(&self) {
        let mut state = self.0.lock().unwrap();
        state.finished += 1;
        state.since = None;
    }

    pub fn snapshot(&self, queued: usize) -> TestWriterSnapshot {
        let state = self.0.lock().unwrap();
        TestWriterSnapshot {
            queued,
            started: state.started,
            finished: state.finished,
            in_flight_ms: state.since.map(|time| time.elapsed().as_millis()),
        }
    }
}
