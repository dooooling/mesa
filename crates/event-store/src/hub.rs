//! EventHub（v1.1 §15）：已提交事件的轻量实时 fan-out。
//!
//! - 可靠真值永远是 `events.db`；Hub 只是"提交后广播"，不是存储；
//! - 发布永不阻塞 ingress（无订阅者时直接丢弃，`send` 错误忽略）；
//! - 慢消费者看到 `Lagged` 后必须回 DB 按 `seq` replay（SSE 层执行，
//!   见 step ⑦），禁止"跳过 N 条继续假装正常"。

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::schema::StoredEvent;

/// Hub 通道容量（性能旋钮，不是正确性旋钮：溢出走 Lagged→replay，
/// 语义依然完整）。
pub const EVENT_HUB_CAPACITY: usize = 256;

pub struct EventHub {
    tx: broadcast::Sender<StoredEvent>,
}

impl EventHub {
    pub fn new(capacity: usize) -> Arc<Self> {
        let (tx, _) = broadcast::channel(capacity);
        Arc::new(Self { tx })
    }

    /// 发布已提交事件。无订阅者时静默丢弃（DB 才是真值，重连可 replay）。
    pub fn publish(&self, ev: &StoredEvent) {
        let _ = self.tx.send(ev.clone());
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StoredEvent> {
        self.tx.subscribe()
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}
