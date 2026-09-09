//! PR9 Stage ⑥ native E2E：Mesa fixture 服务器 → Native transport →
//! OPC UA Driver → EventSink。全程真实协议栈，无 Fake。
//!
//! 纪律：`probe + 观测` barrier（先观测到 sacrificial E0 才 trigger E1..E6），
//! 禁止 sleep 猜测监控项就绪；所有等待皆有超时上限（挂起即失败，不卡 CI）。

mod support;

use std::time::{Duration, Instant};

use mesa_core_types::{ConditionTransition, DriverBinding, EventTask, TaskMode};
use mesa_driver_opcua::OpcUaDriver;
use mesa_driver_sdk::{DataSink, Driver, EventBatch, SdkDriverError};
use support::event_server::{
    FixtureEventServer, e0_probe, e1_base, e2_raised, e3_updated, e4_acknowledged, e5_cleared,
    e6_confirmed, t, trigger_event,
};
use tokio_util::sync::CancellationToken;

const COLLECT_TIMEOUT: Duration = Duration::from_secs(30);

/// 单字节 EventId → `opcua:BASE64URL_NO_PAD`（单字节恒为 2 字符无填充）。
fn eid_of(byte: u8) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let hi = B64[(byte >> 2) as usize] as char;
    let lo = B64[((byte & 0x03) << 4) as usize] as char;
    format!("opcua:{hi}{lo}")
}

async fn read_namespaces(url: &str) -> Vec<String> {
    use mesa_opcua_transport::{NativeOpcUaTransport, OpcUaConnectOptions, OpcUaTransport};
    let cli = std::env::temp_dir().join(format!(
        "mesa-opcua-ns-probe-{}-{}",
        std::process::id(),
        url.len()
    ));
    std::fs::create_dir_all(&cli).unwrap();
    let t = NativeOpcUaTransport::new(OpcUaConnectOptions {
        endpoint_url: url.into(),
        pki_dir: cli.clone(),
        ..Default::default()
    });
    t.connect().await.expect("ns probe 必须可连");
    let ns = t.read_namespace_array().await.expect("ns 必须可读");
    t.disconnect().await.ok();
    let _ = std::fs::remove_dir_all(&cli);
    ns
}

struct Harness {
    srv: FixtureEventServer,
    namespaces: Vec<String>,
    event_rx: tokio::sync::mpsc::Receiver<EventBatch>,
    shutdown: CancellationToken,
    run_handle: tokio::task::JoinHandle<Result<(), SdkDriverError>>,
}

impl Harness {
    async fn start(scope: &str) -> Self {
        let srv = FixtureEventServer::start().await;
        let namespaces = read_namespaces(&srv.endpoint_url()).await;
        let cfg = format!(
            r#"{{"endpoint_url":"{}","timeout_ms":5000}}"#,
            srv.endpoint_url()
        );
        let mut conn = Driver::open_connection(&OpcUaDriver, "e2e", &cfg)
            .await
            .expect("open 必须 Ok");
        conn.configure(1, vec![])
            .await
            .expect("空 data 配置必须 Ok");
        conn.configure_events(
            1,
            vec![EventTask {
                id: "ev-e2e".into(),
                mode: TaskMode::Subscribe,
                interval_ms: None,
                binding: DriverBinding {
                    kind: mesa_core_types::GENERIC_EVENT_BINDING_KIND.into(),
                    config: serde_json::json!({
                        "stream_id": "opcua.events",
                        "parameters": {
                            "notifier_node_id": "nsu=http://opcfoundation.org/UA/;i=2253",
                            "scope": scope,
                            "publishing_interval_ms": 500,
                            "queue_size": 1000,
                        },
                    }),
                },
            }],
        )
        .await
        .expect("事件配置必须 Ok");
        // 刻意不 apply_point_map（Event-only 硬门，§18）。
        let (ctrl_tx, _ctrl_rx) =
            tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (data_tx, _data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(8);
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(64);
        let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx).for_connection(7, 1);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        Self {
            srv,
            namespaces,
            event_rx,
            shutdown,
            run_handle,
        }
    }

    /// probe barrier：循环 trigger E0 直到观测到它；返回时 probe 循环已停。
    /// 正确性由"观测到 E0"保证，100ms 只是 liveness 轮询间隔。
    async fn probe_barrier(&mut self) {
        let handle = self.srv.handle.clone();
        let cancel = CancellationToken::new();
        let cc = cancel.clone();
        let probe_task = tokio::spawn(async move {
            let probe = e0_probe();
            loop {
                tokio::select! {
                    _ = cc.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        trigger_event(&handle, &probe);
                    }
                }
            }
        });
        let want = eid_of(0xE0);
        let deadline = Instant::now() + COLLECT_TIMEOUT;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            assert!(!remain.is_zero(), "30s 内未观测到 probe E0");
            tokio::select! {
                batch = tokio::time::timeout(remain, self.event_rx.recv()) => {
                    let batch = batch.expect("probe 必须到达").expect("通道不得关闭");
                    if batch.events.iter().any(|e| e.event_id == want) {
                        break;
                    }
                }
                done = &mut self.run_handle => {
                    panic!("run 在 probe 阶段提前结束: {done:?}");
                }
            }
        }
        cancel.cancel();
        probe_task.await.expect("probe 任务不 panic");
    }

    /// 收集指定 event_id 集合（按首次出现顺序），30s 上限防挂。
    async fn collect_ids(&mut self, want: &[String]) -> Vec<mesa_core_types::EventRecord> {
        let mut got: Vec<mesa_core_types::EventRecord> = vec![];
        let deadline = Instant::now() + COLLECT_TIMEOUT;
        while got.len() < want.len() {
            let remain = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remain.is_zero(),
                "30s 内未收齐 {:?}，已收 {:?}",
                want,
                ids_of(&got)
            );
            let batch = tokio::time::timeout(remain, self.event_rx.recv())
                .await
                .expect("事件必须到达")
                .expect("通道不得关闭");
            for e in batch.events {
                if want.contains(&e.event_id) && !got.iter().any(|g| g.event_id == e.event_id) {
                    got.push(e);
                }
            }
        }
        // 按 want 顺序排列（到达顺序即触发顺序，断言用）。
        let mut ordered = vec![];
        for id in want {
            ordered.push(got.iter().find(|g| &g.event_id == id).unwrap().clone());
        }
        ordered
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), self.run_handle)
            .await
            .expect("run 必须退出")
            .expect("run 不 panic")
            .expect("正常 Stop 必须 Ok");
        self.srv.stop().await;
    }
}

fn ids_of(records: &[mesa_core_types::EventRecord]) -> Vec<String> {
    records.iter().map(|r| r.event_id.clone()).collect()
}

fn condition_id_of(ns: &[String]) -> String {
    format!("nsu={};s=Alarm1", ns[1])
}

#[tokio::test]
async fn native_e2e_base_and_condition_lifecycle() {
    let mut h = Harness::start("all").await;
    h.probe_barrier().await;
    // barrier 之后显式 trigger E1..E6（顺序即 occurrence 顺序）。
    h.srv.trigger(&e1_base());
    h.srv.trigger(&e2_raised());
    h.srv.trigger(&e3_updated());
    h.srv.trigger(&e4_acknowledged());
    h.srv.trigger(&e5_cleared());
    h.srv.trigger(&e6_confirmed());
    let want = [0xE1u8, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6]
        .iter()
        .map(|b| eid_of(*b))
        .collect::<Vec<_>>();
    let got = h.collect_ids(&want).await;
    assert_eq!(ids_of(&got), want, "到达顺序必须等于触发顺序");

    // E1：普通事件全字段。
    let e1 = &got[0];
    assert_eq!(e1.category, "event");
    assert_eq!(e1.kind, "opcua.event");
    assert!(e1.condition.is_none());
    assert_eq!(e1.message.as_deref(), Some("fixture event"));
    assert_eq!(e1.severity, 321);
    assert_eq!(e1.occurred_at_ns, Some(t(1)));
    assert_eq!(e1.source, "MesaFixture");

    // E2..E6：Condition 身份一致，transition 精确。
    let cid = condition_id_of(&h.namespaces);
    let transitions = [
        ConditionTransition::Raised,
        ConditionTransition::Updated,
        ConditionTransition::Acknowledged,
        ConditionTransition::Cleared,
        ConditionTransition::Confirmed,
    ];
    for (rec, want_t) in got[1..].iter().zip(transitions) {
        let c = rec.condition.as_ref().expect("必须有 condition");
        assert_eq!(c.condition_id, cid);
        assert_eq!(c.transition, want_t, "event {}", rec.event_id);
        assert_eq!(rec.category, "condition");
        assert_eq!(rec.kind, "opcua.condition");
    }
    assert_eq!(got[1].occurred_at_ns, Some(t(2)));
    assert_eq!(got[1].condition.as_ref().unwrap().active, Some(true));
    assert_eq!(got[2].occurred_at_ns, Some(t(3)));
    assert_eq!(got[3].condition.as_ref().unwrap().acknowledged, Some(true));
    assert_eq!(got[5].condition.as_ref().unwrap().confirmed, Some(true));
    // E5：source 回退链（SourceName 空 + SourceNode null → notifier canonical）。
    let notifier_canonical = format!("nsu={};i=2253", h.namespaces[0]);
    assert_eq!(got[4].source, notifier_canonical);
    for r in &got {
        r.validate().expect("record 必须合法");
    }
    h.stop().await;
}

#[tokio::test]
async fn native_e2e_scope_conditions_filters_base_events() {
    let mut h = Harness::start("conditions").await;
    h.probe_barrier().await;
    // E1（BaseEventType）必须被 OfType(ConditionType) 滤掉，E2 通过。
    h.srv.trigger(&e1_base());
    h.srv.trigger(&e2_raised());
    let want = vec![eid_of(0xE2)];
    let got = h.collect_ids(&want).await;
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0].condition.as_ref().unwrap().transition,
        ConditionTransition::Raised
    );
    // E1 缺席证明：一个发布周期 + 裕量内无新批次（否定断言唯一可用手段）。
    let extra = tokio::time::timeout(Duration::from_secs(3), h.event_rx.recv()).await;
    assert!(
        extra.is_err(),
        "E1 必须被 scope 过滤（3s 内无新事件），实际 {extra:?}"
    );
    h.stop().await;
}

/// P0-3：真断线 Gate。E1 收到后杀服务器（TCP 断 + 端口释放）→ client 重试
/// 耗尽 → event-loop 结束 → transport 守望关 producer → worker 必须以
/// `OPCUA_EVENT_SESSION_LOST` fail 当前 attempt（有限时间内，不得永久 RUNNING）。
#[tokio::test]
async fn native_e2e_session_loss_fails_attempt() {
    let mut h = Harness::start("all").await;
    h.probe_barrier().await;
    h.srv.trigger(&e1_base());
    let got = h.collect_ids(&[eid_of(0xE1)]).await;
    assert_eq!(got.len(), 1);
    // 杀服务器（abort：TCP 断 + 端口释放，模拟掉电）。注意：刻意不 cancel
    // shutdown——worker 必须走 session-loss 失败路径，而非正常 Stop 路径。
    h.srv.kill().await;
    let res = tokio::time::timeout(Duration::from_secs(90), &mut h.run_handle)
        .await
        .expect("90s 内 run 必须结束（不得永久 RUNNING）")
        .expect("run 任务不 panic");
    let err = res.expect_err("会话死亡必须 fail 当前 attempt");
    assert_eq!(err.code, "OPCUA_EVENT_SESSION_LOST");
    h.shutdown.cancel();
}
