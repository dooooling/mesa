//! P1-A1 退避证据回归：`saw_runtime_evidence` 的端到端 Gate（L1–L5 + Event-only P0）。
//!
//! 断言口径（评审裁决）：**权威判据是本 Endpoint 的计划退避值 `retry_in_secs`**
//! （既有日志字段，`endpoint.rs:369`），attempt 次数/相邻间隔只作辅助观测。
//! 因此本套件：
//! - 不读、也不设置 `MESA_RECONNECT_FAST`（避免进程级 env 串扰）；
//! - 用测试侧 `PointIdSource` 装饰器记录每个 attempt 的起始（`assign()` 时间戳），
//!   真实子进程仍按原路径启动/退出；test-hooks 时钟只推进本 Endpoint 的退避等待；
//! - 用本进程 tracing subscriber 捕获日志并解析 `retry_in_secs`（字符串精确）。
//!
//! 触发器 `fail_before_first_batch_ms` 的语义由 test-driver 保证：
//! `StartConnectionAck` 已发出（Core 已置 RUNNING）之后、`run()` 未发布任何
//! Data/Event 批次就失败 —— 正是“假 RUNNING 且零批次”的缺陷形状。

mod common;
mod event_common;

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mesa_core_types::{
    DriverBinding, EventTask, GENERIC_EVENT_BINDING_KIND, PointDescriptor, TaskMode,
};
use mesa_driver_manager::MesaManager;
use mesa_driver_manager::endpoint::{BuiltinEndpoint, PointIdAllocator, PointIdSource};
use mesa_event_store::{EVENT_HUB_CAPACITY, EventHub, EventServices, EventStore, StoreFaults};
use mesa_test_driver::SIM_EVENT_STREAM_COUNTER;

// ---------------------------------------------------------------------------
// O1：进程内日志捕获（只依赖既有日志字段）
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogBuf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// 某 Endpoint 的计划退避序列（按日志顺序）。
    fn retry_secs(&self, endpoint: &str) -> Vec<u64> {
        let needle = format!("endpoint={endpoint}");
        self.text()
            .lines()
            .filter(|l| l.contains(&needle) && l.contains("retry_in_secs="))
            .filter_map(|l| {
                let rest = l.split("retry_in_secs=").nth(1)?;
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse().ok()
            })
            .collect()
    }

    fn saw(&self, endpoint: &str, needle: &str) -> bool {
        let ep = format!("endpoint={endpoint}");
        self.text()
            .lines()
            .any(|l| l.contains(&ep) && l.contains(needle))
    }

    fn count(&self, endpoint: &str, needle: &str) -> usize {
        let ep = format!("endpoint={endpoint}");
        self.text()
            .lines()
            .filter(|l| l.contains(&ep) && l.contains(needle))
            .count()
    }
}

fn log_buf() -> &'static LogBuf {
    static BUF: OnceLock<LogBuf> = OnceLock::new();
    BUF.get_or_init(|| {
        let buf = LogBuf::default();
        // 注意：本文件不使用 common::init_log()，否则 try_init 竞争会让捕获失效。
        let _ = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .try_init();
        buf
    })
}

// ---------------------------------------------------------------------------
// O2：attempt 起始时间线（测试侧 PointIdSource 装饰器）
// ---------------------------------------------------------------------------

struct RecordingSource {
    inner: PointIdAllocator,
    hits: Arc<Mutex<Vec<Instant>>>,
}

impl PointIdSource for RecordingSource {
    fn assign(
        &self,
        endpoint_id: &str,
        descriptors: &[PointDescriptor],
    ) -> Result<Vec<u32>, String> {
        self.hits.lock().unwrap().push(Instant::now());
        self.inner.assign(endpoint_id, descriptors)
    }
    fn known_map(&self, endpoint_id: &str) -> HashMap<String, u32> {
        self.inner.known_map(endpoint_id)
    }
    fn revision(&self, endpoint_id: &str) -> u64 {
        self.inner.revision(endpoint_id)
    }
}

// ---------------------------------------------------------------------------
// 测试夹具
// ---------------------------------------------------------------------------

struct Rig {
    _staged: common::StagedDriversDir,
    mgr: Arc<MesaManager>,
    hits: Arc<Mutex<Vec<Instant>>>,
    buf: &'static LogBuf,
}

fn poll_tasks() -> Vec<mesa_core_types::AcquisitionTask> {
    vec![common::poll_task(
        "d",
        50,
        serde_json::json!([{"resource_id":"counter","parameters":{},"outputs":[{"output":"value","point_key":"k.counter"}]}]),
    )]
}

fn event_tasks() -> Vec<EventTask> {
    let binding = mesa_core_types::GenericEventBinding {
        stream_id: SIM_EVENT_STREAM_COUNTER.into(),
        parameters: serde_json::json!({}),
    };
    vec![EventTask {
        id: "cnt".into(),
        mode: TaskMode::Poll,
        interval_ms: Some(50),
        binding: DriverBinding {
            kind: GENERIC_EVENT_BINDING_KIND.into(),
            config: serde_json::to_value(&binding).unwrap(),
        },
    }]
}

impl Rig {
    fn new() -> Self {
        let buf = log_buf();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let source = RecordingSource {
            inner: PointIdAllocator::default(),
            hits: Arc::clone(&hits),
        };
        let staged = common::staged_drivers_with_test_driver();
        let mgr = Arc::new(MesaManager::with_source(staged.path(), Arc::new(source)));
        Self {
            _staged: staged,
            mgr,
            hits,
            buf,
        }
    }

    fn start(&self, endpoint_id: &str, connection_json: &str) {
        self.start_with(endpoint_id, connection_json, poll_tasks(), vec![]);
    }

    fn start_with(
        &self,
        endpoint_id: &str,
        connection_json: &str,
        tasks: Vec<mesa_core_types::AcquisitionTask>,
        event_tasks: Vec<EventTask>,
    ) {
        self.mgr.test_reconnect_clock(endpoint_id);
        self.mgr
            .start_endpoint(BuiltinEndpoint {
                endpoint_id: endpoint_id.into(),
                driver_id: "test-driver".into(),
                connection_json: connection_json.into(),
                tasks,
                event_tasks,
            })
            .expect("start_endpoint 必须成功");
    }

    fn attempts(&self) -> usize {
        self.hits.lock().unwrap().len()
    }

    async fn wait_attempts(&self, endpoint: &str, n: usize, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while self.attempts() < n {
            assert!(
                Instant::now() < deadline,
                "attempt 数不足：want>={n}, got={}",
                self.attempts()
            );
            self.mgr.test_reconnect_clock(endpoint).advance();
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// 等到日志里出现 n 个 `retry_in_secs`（即 n 次失败后的计划值），返回全部序列。
    async fn wait_retries(&self, endpoint_id: &str, n: usize, secs: u64) -> Vec<u64> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let v = self.buf.retry_secs(endpoint_id);
            if v.len() >= n {
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "计划退避值不足：want>={n}, got={v:?}"
            );
            self.mgr.test_reconnect_clock(endpoint_id).advance();
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn latest(&self, endpoint_id: &str) -> Vec<mesa_driver_manager::snapshot::LatestEntry> {
        self.mgr
            .snapshot()
            .latest_all()
            .into_iter()
            .filter(|e| e.endpoint_id == endpoint_id)
            .collect()
    }

    fn state(&self, endpoint_id: &str) -> String {
        self.mgr
            .snapshot()
            .endpoint(endpoint_id)
            .map(|s| s.state)
            .unwrap_or_default()
    }

    fn epoch(&self, endpoint_id: &str) -> u64 {
        self.mgr
            .snapshot()
            .endpoint(endpoint_id)
            .map(|s| s.epoch)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// L1：连续首采前失败 → 1/2/5/10/30/30
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backoff_escalates_when_every_attempt_dies_before_first_batch() {
    const EP: &str = "hd-a1-l1";
    let rig = Rig::new();
    rig.start(EP, r#"{"fail_before_first_batch_ms":100}"#);

    let vals = rig.wait_retries(EP, 6, 120).await;
    assert_eq!(
        vals,
        vec![1, 2, 5, 10, 30, 30],
        "计划退避必须是 1/2/5/10/30/30（旧码恒 1）"
    );
    assert!(
        rig.buf.saw(EP, "connection started"),
        "必须先真实进入 RUNNING 才构成“假 RUNNING”"
    );
    assert!(
        rig.buf.saw(EP, "SIMULATED_NO_FIRST_BATCH"),
        "失败原因必须来自 fail_before_first_batch_ms 注入"
    );
    assert!(rig.latest(EP).is_empty(), "零批次路径不得产生任何采样");
    let _ = rig.mgr.stop_endpoint(EP).await;
}

// ---------------------------------------------------------------------------
// L2：短暂假 RUNNING 不得无限 1s 重连（权威=计划值；次数仅辅助）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brief_false_running_does_not_loop_at_one_second() {
    const EP: &str = "hd-a1-l2";
    let rig = Rig::new();
    rig.start(EP, r#"{"fail_before_first_batch_ms":300}"#);

    let vals = rig.wait_retries(EP, 5, 120).await;
    assert_eq!(
        &vals[..5],
        &[1, 2, 5, 10, 30],
        "假 RUNNING 后每个无证据 attempt 必须递增退避"
    );
    // 辅助观测：只记录，不作门禁（含 spawn/握手/清理耗时，跨平台会漂移）。
    eprintln!(
        "L2 auxiliary: attempts={}, retry_secs={vals:?}",
        rig.attempts()
    );
    assert_eq!(
        rig.buf.count(EP, "connection started"),
        5,
        "每个 attempt 都应先进入 RUNNING（证明是假 RUNNING，不是配置失败）"
    );
    assert!(rig.latest(EP).is_empty(), "假 RUNNING 全程不得有采样");
    let _ = rig.mgr.stop_endpoint(EP).await;
}

// ---------------------------------------------------------------------------
// L3：有真实运行证据后故障 → 仍可快速重连（不许误伤）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recovery_after_real_batches_keeps_fast_reconnect() {
    const EP: &str = "hd-a1-l3";
    let rig = Rig::new();
    let t0 = Instant::now();
    rig.start(EP, r#"{"crash_after_batches":2}"#);

    common::wait_until(30, || !rig.latest(EP).is_empty()).await;
    eprintln!("L3: first data at {:?}", t0.elapsed());
    // 崩溃子进程的 teardown 走完整预算（grace + reader barrier，实测 ≈17s/轮），
    // 故这里给 3 次重连留足墙钟；断言本身只看计划值 = 1。
    let vals = rig.wait_retries(EP, 3, 120).await;
    eprintln!("L3: retries at {:?} -> {vals:?}", t0.elapsed());
    assert_eq!(
        &vals[..3],
        &[1, 1, 1],
        "每次 attempt 都产出批次 ⇒ 每次都必须归零回 1s"
    );
    assert!(rig.epoch(EP) != 0, "重连成功后必须有非零 stream_epoch");
    let _ = rig.mgr.stop_endpoint(EP).await;
}

// ---------------------------------------------------------------------------
// L5：不同 Endpoint 的退避与证据相互隔离
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backoff_and_evidence_are_isolated_per_endpoint() {
    const A: &str = "hd-a1-iso-a";
    const B: &str = "hd-a1-iso-b";
    let rig = Rig::new();
    rig.start(A, r#"{"fail_before_first_batch_ms":100}"#);
    rig.start(B, "{}");

    let vals = rig.wait_retries(A, 3, 60).await;
    assert_eq!(&vals[..3], &[1, 2, 5], "A 必须按阶梯递增");
    assert!(
        rig.buf.retry_secs(B).is_empty(),
        "B 是健康端点，不得出现任何重连计划值"
    );

    common::wait_until(20, || !rig.latest(B).is_empty()).await;
    let t1 = rig.latest(B)[0].timestamp_ns;
    let epoch_b = rig.epoch(B);
    tokio::time::sleep(Duration::from_millis(600)).await;
    let t2 = rig.latest(B)[0].timestamp_ns;
    assert!(t2 >= t1, "B 的数据必须持续推进");
    assert_eq!(rig.epoch(B), epoch_b, "B 的 epoch 不得因 A 的失败而变化");
    assert_eq!(rig.state(B), "RUNNING", "B 必须保持 RUNNING");

    rig.mgr.stop_endpoint(A).await.expect("stop A");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.state(B), "RUNNING", "Stop A 不得影响 B");
    assert_eq!(rig.epoch(B), epoch_b, "Stop A 不得重启 B");
    let _ = rig.mgr.stop_endpoint(B).await;
}

// ---------------------------------------------------------------------------
// Event-only 专项（P0）：EventBatch 被消费后故障 → 计划退避严格 = 1s
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_only_endpoint_resets_backoff_to_one_second_after_event_evidence() {
    const EP: &str = "hd-a1-event-only";
    let rig = Rig::new();

    let db = event_common::tmp_db("a1-event-only");
    let _ = std::fs::remove_file(&db);
    // at=0 ⇒ 先全健康；稍后再布防（sticky）以制造 ingress fatal。
    let faults = Arc::new(StoreFaults::new(0, false));
    let store = Arc::new(EventStore::open_with_faults(&db, Arc::clone(&faults)).unwrap());
    let services = EventServices::new(store.clone(), EventHub::new(EVENT_HUB_CAPACITY));
    rig.mgr.set_event_services(Arc::clone(&services));

    rig.start_with(EP, "{}", vec![], event_tasks());
    // 前置条件（不是结论）：事件确实被 Core 消费并落盘。
    common::wait_until(30, || !event_common::rows_of(&store, EP).is_empty()).await;

    // 布防：此后每个 attempt 的首个事件批都会被 Store 拒绝 ⇒ ingress fatal ⇒ Lost。
    faults.fail_commit_at.store(1, Ordering::Relaxed);
    faults.fail_sticky.store(true, Ordering::Relaxed);

    let mut observed = Vec::new();
    let mut last_batches = services
        .diagnostics
        .ingress_batches_total
        .load(Ordering::Relaxed);
    for i in 0..3 {
        let vals = rig.wait_retries(EP, i + 1, 30).await;
        let planned = *vals.last().expect("至少一个计划值");
        // 逐个 attempt 证明“本 attempt 消费到了 EventBatch”：
        // 每次故障后 ingress 收到的批次数必须严格增长（不是只看重复归零）。
        let now_batches = services
            .diagnostics
            .ingress_batches_total
            .load(Ordering::Relaxed);
        let delta = now_batches - last_batches;
        eprintln!(
            "Event-only attempt#{}: planned_retry_in_secs={planned}, ingested_batches_delta={delta} (total {now_batches})",
            i + 1
        );
        assert!(
            now_batches > last_batches,
            "第 {} 次故障前本 attempt 未消费到任何 EventBatch（batches {} -> {}）",
            i + 1,
            last_batches,
            now_batches
        );
        last_batches = now_batches;
        observed.push(planned);
    }
    assert_eq!(
        observed,
        vec![1, 1, 1],
        "有 EventBatch 证据时每次故障后都必须归零回 1s（只计 DataBatch 会变成 1,2,5）"
    );
    assert!(
        rig.buf.count(EP, "EVENT_STORE_UNAVAILABLE") >= 3,
        "三次故障都必须来自事件入库失败（ingress fatal），而不是配置失败"
    );
    let _ = rig.mgr.stop_endpoint(EP).await;
    let _ = std::fs::remove_file(&db);
}

// ---------------------------------------------------------------------------
// L4：Stop 取消已升级的退避，且不再产生新 attempt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stop_cancels_escalated_backoff_and_stops_retrying() {
    const EP: &str = "hd-a1-l4";
    let rig = Rig::new();
    rig.start(EP, r#"{"fail_before_first_batch_ms":100}"#);

    // 第 4 次 attempt 起点之后，下一次退避为 10s（1,2,5 已消耗）。
    rig.wait_attempts(EP, 4, 40).await;
    let clock = rig.mgr.test_reconnect_clock(EP);
    common::wait_until(20, || clock.pending() == Some(Duration::from_secs(10))).await;
    let before = rig.attempts();

    let t0 = Instant::now();
    let res = tokio::time::timeout(Duration::from_secs(40), rig.mgr.stop_endpoint(EP))
        .await
        .expect("重连退避中 Stop 必须有界返回");
    res.expect("Stop 必须成功");
    assert!(
        t0.elapsed() < Duration::from_secs(10),
        "Stop 必须取消剩余退避（当前档 10s），实际耗时 {:?}",
        t0.elapsed()
    );
    assert_eq!(rig.state(EP), "STOPPED", "Stop 后终态必须是 STOPPED");

    assert!(
        clock.pending().is_none(),
        "Stop 必须销毁等待，而不是留下一次可唤醒的重连"
    );
    assert!(!clock.advance(), "Stop 后不能再推进已取消的退避");
    tokio::task::yield_now().await;
    assert!(!rig.mgr.has_live_attempt(EP));
    assert_eq!(rig.attempts(), before, "Stop 之后不得再产生任何 attempt");
}
