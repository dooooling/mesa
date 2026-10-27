//! 仅供 test-hooks：显式推进 Core 的退避等待，不改变计划值或子进程时钟。

use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// 每个 Endpoint 独立的退避时钟；最多保留一个等待，没有定时自动放行。
#[derive(Default)]
pub struct ReconnectClock {
    pending: Mutex<Option<(Duration, oneshot::Sender<()>)>>,
}

impl ReconnectClock {
    /// 当前计划的等待；None 也用于确认 Stop 已取消并释放等待。
    pub fn pending(&self) -> Option<Duration> {
        self.pending
            .lock()
            .unwrap()
            .as_ref()
            .map(|(delay, _)| *delay)
    }

    /// 推进一次已经到达的退避；不存在等待或等待已被取消时返回 false。
    pub fn advance(&self) -> bool {
        self.pending
            .lock()
            .unwrap()
            .take()
            .is_some_and(|(_, tx)| tx.send(()).is_ok())
    }

    pub(crate) async fn wait(self: &Arc<Self>, delay: Duration) {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            assert!(pending.is_none(), "同一 Endpoint 不得并行等待两次退避");
            *pending = Some((delay, tx));
        }
        // select 的取消会丢弃本 future，也必须清掉尚未推进的等待。
        struct PendingGuard(Arc<ReconnectClock>);
        impl Drop for PendingGuard {
            fn drop(&mut self) {
                self.0.pending.lock().unwrap().take();
            }
        }
        let _guard = PendingGuard(Arc::clone(self));
        let _ = rx.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_advance_and_cancel_release_the_wait() {
        let clock = Arc::new(ReconnectClock::default());
        let other = Arc::new(ReconnectClock::default());
        let task = tokio::spawn({
            let clock = Arc::clone(&clock);
            async move { clock.wait(Duration::from_secs(30)).await }
        });
        while clock.pending().is_none() {
            tokio::task::yield_now().await;
        }
        assert!(!task.is_finished());
        assert_eq!(clock.pending(), Some(Duration::from_secs(30)));
        assert!(!other.advance(), "另一个 Endpoint 不得推进此等待");
        assert!(clock.advance());
        task.await.unwrap();
        assert!(clock.pending().is_none());
        let task = tokio::spawn({
            let clock = Arc::clone(&clock);
            async move { clock.wait(Duration::from_secs(30)).await }
        });
        while clock.pending().is_none() {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        assert!(clock.pending().is_none());
        assert!(!clock.advance(), "已取消的退避不能被复活");
    }
}
