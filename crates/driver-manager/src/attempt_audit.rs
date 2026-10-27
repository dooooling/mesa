//! 运行期审计接口（Issue #102 §10）：把「本次 attempt 要发 StartConnection 了」这件事
//! 交回 Core 持久化。
//!
//! # 为什么是 trait，而不是让 Manager 直接持有 `ConfigStore`
//!
//! Manager 是 Core 侧的编排组件，但它的大量测试用内存版 `PointIdAllocator`
//! （根本没有 DB）。把 store 塞进 Manager 会强迫每个测试造库，也会让
//! 「配置真值只在 Core」这条边界变模糊——Manager 关心的是**连接与 attempt 的生命周期**，
//! 不是审计表的物理形态。所以这里只定义「记录意图 / 记录结果」两个动作，由 Core 注入实现。
//!
//! # 为什么接口是同步的
//!
//! epoch 意图**必须在 `StartConnection` 发出之前落盘**，这是本设计的核心不变量：
//! 只要驱动开始采集，就必然存在一条已持久化的意图记录。用同步签名把这条顺序约束
//! 写进类型里——调用方无法把"先发后记"写成并发交错。实现方只做单行 INSERT（WAL），
//! 与既有控制面审计在 async 路径里同步落盘的做法一致。
//!
//! 未注入 sink 时（合同测试、单测、纯内置端点）全部审计动作退化为 no-op，
//! 不影响任何既有行为。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// epoch 的产生原因：首连 vs 内部重连。
///
/// 区分这两者是 Issue #102 的现场需求：同一个 attempt 里重连多次会产生多个 epoch，
/// 只有"首连"能被父请求直接解释，"重连"必须显式标出原因，否则时间线上会看到
/// 一串无主的 epoch。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochSource {
    /// attempt 的第一次 StartConnection。
    InitialConnection,
    /// 同一个 attempt 内、但**还没有任何一次连接成功过**的重试。
    ///
    /// 与 [`EpochSource::InternalReconnect`] 的区别是审计语义，而不是实现细节：
    /// `internal_reconnect` 意味着"曾经连上过、链路断了又重连"；如果第一次
    /// StartConnection 就失败，后续重试并没有"断线"可言，标成重连会让时间线
    /// 凭空多出一段"已建立过的连接"（评审指出的边界）。
    InitialConnectionRetry,
    /// 同一个 attempt 内部、**曾经成功建立过连接之后**的第 2..n 次 StartConnection。
    InternalReconnect,
}

impl EpochSource {
    /// 稳定字符串（进入审计 `request_json`，不得随意改名）。
    pub fn as_str(self) -> &'static str {
        match self {
            EpochSource::InitialConnection => "initial_connection",
            EpochSource::InitialConnectionRetry => "initial_connection_retry",
            EpochSource::InternalReconnect => "internal_reconnect",
        }
    }
}

/// 一次 epoch 的意图（发送 `StartConnection` 之前写入）。
#[derive(Debug, Clone)]
pub struct EpochIntent {
    /// 子记录 ID：`<parent_request_id>:epoch:<sequence>`。
    pub request_id: String,
    /// 触发本次 attempt 的父请求 ID（Start/Bootstrap/Restore 那次 HTTP 请求）。
    pub parent_request_id: String,
    pub endpoint_id: String,
    pub source: EpochSource,
    /// attempt 内的连接序号（1 = 首连）。
    pub connection_sequence: u64,
    /// `stream_epoch` 以十进制字符串写入审计，避免超出 SQLite INTEGER 时有精度损失。
    pub stream_epoch: u64,
    pub actor: String,
    /// attempt 代次（与 `AttemptCtx::generation` 同一单调值），用于把记录对回 attempt。
    pub generation: u64,
}

/// 一次 epoch 的结论。
#[derive(Debug, Clone)]
pub enum EpochOutcome {
    /// StartConnection ACK 且配置闭环完成：此时才允许说设备进入 RUNNING。
    Completed { point_count: usize },
    /// StartConnection 失败/无 ACK。
    ///
    /// 持久审计只保留**稳定分类 + 稳定错误码**：原始诊断串可能带路径、地址或
    /// 对端错误文本，属于"只进日志、不进审计"的内容（审计表不该变成第二个日志库）。
    Failed {
        /// 稳定分类（`configuration_failed` / `lost` / `shutdown`）。
        category: String,
        /// 稳定错误码（如 `ConfigurationError/DEFINITION_SCOPE_NOT_COLLECTABLE`）。
        code: Option<String>,
    },
}

/// 一次连接的丢失事实（辅助记录，**不阻止** teardown 与重连）。
///
/// 为什么需要它：重连会换 epoch，若不记"旧 epoch 何时、因何结束"，时间线上会看到
/// 一串"消失又出现"的 epoch，无法区分"驱动主动断开"与"Core 放弃等待"。
#[derive(Debug, Clone)]
pub struct RuntimeLostEvent {
    pub parent_request_id: Option<String>,
    pub endpoint_id: String,
    pub generation: u64,
    /// 上一个已建立的 epoch（0 = 还没有成功建立的 epoch）。
    pub last_epoch: u64,
    /// 稳定原因码（白名单映射，如 `connect_failed` / `reader_eof`）。
    /// 原始原因串只进日志——审计只留可聚合的分类。
    pub reason_code: &'static str,
    pub had_running_session: bool,
}

/// Core 实现的审计写入器。
///
/// 两个方法都返回 `Err(String)`：调用方据此决定是否继续——epoch 意图失败**必须**
/// 阻止 `StartConnection`（见 `run_config_flow`），结果更新失败只告警（采集已经在跑，
/// 不能为了补一条结果记录把连接打断）。
pub trait AttemptAuditSink: Send + Sync {
    fn record_epoch_intent(&self, intent: &EpochIntent) -> Result<(), String>;
    fn record_epoch_outcome(&self, request_id: &str, outcome: &EpochOutcome) -> Result<(), String>;

    /// 连接丢失的辅助记录。默认 no-op：未实现该能力的 sink 不需要关心它，
    /// 且任何失败都不得影响 teardown（安全方向）。
    fn record_runtime_lost(&self, _event: &RuntimeLostEvent) -> Result<(), String> {
        Ok(())
    }
}

/// 一次 attempt 的审计上下文：由触发方（REST Start / Bootstrap / 开机恢复）构造。
///
/// `Default` = 无父请求、无 sink，即"不审计"。这样所有既有调用点（合同测试、
/// 单测、内置演示端点）不需要改动即可继续工作。
#[derive(Clone, Default)]
pub struct AttemptContext {
    parent_request_id: Option<String>,
    actor: String,
    sink: Option<Arc<dyn AttemptAuditSink>>,
}

impl std::fmt::Debug for AttemptContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttemptContext")
            .field("parent_request_id", &self.parent_request_id)
            .field("actor", &self.actor)
            .field("audited", &self.is_audited())
            .finish()
    }
}

impl AttemptContext {
    /// 构造一个**启用审计**的上下文。`sink` 由 Core 提供（通常是 ConfigStore 适配器）。
    pub fn audited(
        parent_request_id: impl Into<String>,
        actor: impl Into<String>,
        sink: Arc<dyn AttemptAuditSink>,
    ) -> Self {
        Self {
            parent_request_id: Some(parent_request_id.into()),
            actor: actor.into(),
            sink: Some(sink),
        }
    }

    /// 是否真的会产生审计记录（父请求与 sink 缺一不可）。
    pub fn is_audited(&self) -> bool {
        self.sink.is_some() && self.parent_request_id.is_some()
    }

    pub fn parent_request_id(&self) -> Option<&str> {
        self.parent_request_id.as_deref()
    }

    pub fn actor(&self) -> &str {
        if self.actor.is_empty() {
            "unknown"
        } else {
            &self.actor
        }
    }

    pub fn sink(&self) -> Option<&Arc<dyn AttemptAuditSink>> {
        self.sink.as_ref()
    }

    /// "真正会写审计"时的 sink：父请求与 sink 都存在才返回。
    ///
    /// 把"是否审计"的三重判断（上下文存在 / 已审计 / sink 就绪）收成一处，
    /// 避免每个调用点各写一遍嵌套 if（也避免漏判导致无父请求的孤儿记录）。
    pub fn active_sink(&self) -> Option<&Arc<dyn AttemptAuditSink>> {
        if self.is_audited() {
            self.sink.as_ref()
        } else {
            None
        }
    }

    /// 子记录 ID：`<parent>:epoch:<sequence>`。无父请求时为 `None`（不审计）。
    pub fn epoch_request_id(&self, sequence: u64) -> Option<String> {
        self.parent_request_id
            .as_ref()
            .map(|p| format!("{p}:epoch:{sequence}"))
    }
}

/// attempt 内的 epoch 序号分配器（attempt 级，跨重连累加）。
///
/// 单独抽出来的原因：序号语义是"这个 attempt 里第几次发 StartConnection"，
/// 它与 `generation`（attempt 代次）必须分开——重连不换代次，但会换 epoch。
///
/// **两段式（peek → commit）**：序号只在"审计意图确实写成功"之后才前进。
/// 否则一次审计写入失败就会白吃掉一个序号：下一个真正成功的连接会变成
/// `:epoch:2` 且被标成 `internal_reconnect`（评审指出，已修正）。
#[derive(Debug, Default)]
pub struct ConnectionSequence(AtomicU64);

impl ConnectionSequence {
    /// 候选序号（1 起），**不推进**计数器。
    pub fn peek(&self) -> u64 {
        self.0.load(Ordering::Acquire) + 1
    }

    /// 提交候选序号。只在候选被实际使用（意图已持久化）后调用。
    ///
    /// attempt 内只有一个任务推进序号，因此 `compare_exchange` 只是防御性校验：
    /// 若发现计数器已被推进（理论上不可能），返回当前真实序号并让调用方按它落库。
    pub fn commit(&self, candidate: u64) -> u64 {
        match self.0.compare_exchange(
            candidate - 1,
            candidate,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => candidate,
            Err(cur) => cur + 1,
        }
    }

    /// 直接取下一个序号（等价 peek + commit），仅用于不关心持久化成败的调用方。
    pub fn next(&self) -> u64 {
        let c = self.peek();
        self.commit(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_is_not_audited() {
        let c = AttemptContext::default();
        assert!(!c.is_audited());
        assert_eq!(c.parent_request_id(), None);
        assert_eq!(c.actor(), "unknown");
        assert_eq!(c.epoch_request_id(3), None);
    }

    #[test]
    fn audited_context_builds_child_request_ids() {
        struct Noop;
        impl AttemptAuditSink for Noop {
            fn record_epoch_intent(&self, _i: &EpochIntent) -> Result<(), String> {
                Ok(())
            }
            fn record_epoch_outcome(&self, _r: &str, _o: &EpochOutcome) -> Result<(), String> {
                Ok(())
            }
        }
        let c = AttemptContext::audited("lc-start-ep1-1", "local-api", Arc::new(Noop));
        assert!(c.is_audited());
        assert_eq!(c.actor(), "local-api");
        assert_eq!(
            c.epoch_request_id(2).as_deref(),
            Some("lc-start-ep1-1:epoch:2")
        );
    }

    #[test]
    fn connection_sequence_is_monotonic_per_attempt() {
        let seq = ConnectionSequence::default();
        assert_eq!(seq.next(), 1);
        assert_eq!(seq.next(), 2);
        assert_eq!(seq.next(), 3);
    }

    /// 评审修正的回归：peek 不消耗序号；只有 commit 才推进。
    /// 这样"审计意图写失败 → 本次不发 StartConnection"不会让下一次成功连接变成 :epoch:2。
    #[test]
    fn peek_does_not_consume_sequence() {
        let seq = ConnectionSequence::default();
        for _ in 0..5 {
            assert_eq!(seq.peek(), 1, "peek 不得推进序号");
        }
        assert_eq!(seq.commit(1), 1);
        assert_eq!(seq.peek(), 2);
        assert_eq!(seq.commit(2), 2);
        assert_eq!(seq.peek(), 3);
    }
}
