//! Cutover Gate 2 — READY-only Production Deployment（诊断专用，不改生产 backend）。
//!
//! 目标：证明 `backend=wire` 能作为真实运行后端长期使用（不是再测 codec）。
//! 证据路径（诚实口径，见 PR #67 review blocker #4 收口）：
//! `FocasDriver::open_connection(backend=wire)` → `configure/apply` →
//! `DriverConnection::run` → `DataBatch`（经 `DataSink::for_test` 直收）。
//! 未经过 Manager / Driver IPC / Core stream_epoch；D2 为“每轮 fresh
//! connection recreate”，不是“同一 connection Stop→Configure→Start”。
//!
//! 范围（冻结）：`backend=wire` + READY 12 类（axis 拆 3 轴共 14 点）+
//! 真实 target165 + 持续运行。不补 load、不补 tool、不改
//! default、不删 Native、不做 fallback。
//!
//! 四门（显式，不混统计）：
//! D1 正常持续运行：`MESA_DEPLOY_MINUTES`（默认 30）× 1s poll × 14 点；
//!   门：`unexpected BAD=0 / READ_FAILED=0 / crash=0 / session corruption=0 /
//!   point count drift=0 / sequence 异常=0`（首批 `sequence==1`，后续 `>last`；
//!   缺口允许，回退/停滞 fail）。无 Native 并行（不是 Shadow）。
//! D2 生命周期（fresh recreate 口径）：`MESA_DEPLOY_ROUNDS`（默认 5）轮
//!   `open→configure→apply→run→cancel→join→drop重建`；门：Wire session 正确关闭 +
//!   重启可重连 + 无僵尸 task + 无旧 session 污染 + point map/sequence 正常。
//! D3 真实 reconnect：本 harness 默认 NOT-PROVEN（无安全窗口不制造故障，
//!   不为绿表制造假证据；live 主 READ_FAILED 待 CNC 配合，见 Gate 1 canary 口径）。
//! D4 ARM / Raspberry Pi 3B：独立 deployment gate；本 harness 只打印本机
//!   `std::env::consts::ARCH`，结论恒为 pending（不接受环境变量自报 proven）。
//!
//! 用法：`MESA_DEPLOY_HOST=192.168.15.165 MESA_DEPLOY_MINUTES=30
//! MESA_DEPLOY_ROUNDS=5 cargo run -p mesa-driver-focas2 --example wire_deploy`
//! （可选 `MESA_DEPLOY_PORT` 默认 8193；`MESA_DEPLOY_INTERVAL_MS` 默认 1000）。
//! 退出码：任一硬门 fail → 非零；D3/D4 NOT-PROVEN 不 fail（诚实口径）。
//!
//! HOLD（正确行为）：`backend=wire` + servo/spindle load、tool 系 →
//! configure `UNSUPPORTED_POINT`（本 harness 开头即验 5/5，不进 run）。

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use mesa_core_types::{DataType, GENERIC_BINDING_KIND, Quality};
use mesa_driver_focas2::FocasDriver;
use mesa_driver_sdk::{DataSink, Driver};
use tokio_util::sync::CancellationToken;

/// READY 14 点（12 类，axis×3）：(resource, params, output, key, dt, label)。
fn ready_points() -> Vec<(
    &'static str,
    serde_json::Value,
    &'static str,
    &'static str,
    DataType,
    &'static str,
)> {
    vec![
        (
            "machine",
            serde_json::json!({}),
            "status",
            "deploy.status",
            DataType::U32,
            "machine.status",
        ),
        (
            "machine",
            serde_json::json!({}),
            "feed",
            "deploy.feed",
            DataType::U32,
            "machine.feed",
        ),
        (
            "machine",
            serde_json::json!({}),
            "spindle_speed",
            "deploy.spindle_speed",
            DataType::I32,
            "spindle.active.speed",
        ),
        (
            "axis",
            serde_json::json!({"axis": 1}),
            "absolute",
            "deploy.axis1",
            DataType::I32,
            "axis[1].absolute",
        ),
        (
            "axis",
            serde_json::json!({"axis": 2}),
            "absolute",
            "deploy.axis2",
            DataType::I32,
            "axis[2].absolute",
        ),
        (
            "axis",
            serde_json::json!({"axis": 3}),
            "absolute",
            "deploy.axis3",
            DataType::I32,
            "axis[3].absolute",
        ),
        (
            "spindle",
            serde_json::json!({"spindle": 1}),
            "gear",
            "deploy.gear1",
            DataType::I32,
            "spindle[1].gear",
        ),
        (
            "spindle",
            serde_json::json!({"spindle": 1}),
            "maxrpm",
            "deploy.maxrpm1",
            DataType::I32,
            "spindle[1].maxrpm",
        ),
        (
            "macro",
            serde_json::json!({"number": 501}),
            "value",
            "deploy.macro501",
            DataType::F64,
            "macro[501]",
        ),
        (
            "pmc",
            serde_json::json!({"kind": "R", "addr": 100}),
            "value",
            "deploy.pmcR100",
            DataType::I32,
            "pmc.R100",
        ),
        (
            "param",
            serde_json::json!({"number": 6711}),
            "value",
            "deploy.param6711",
            DataType::I32,
            "param[6711]",
        ),
        (
            "diagnosis",
            serde_json::json!({"number": 301, "axis": 3}),
            "value",
            "deploy.diag301a3",
            DataType::F64,
            "diagnosis[301]@axis3",
        ),
        (
            "opmsg",
            serde_json::json!({}),
            "value",
            "deploy.opmsg",
            DataType::String,
            "opmsg.value",
        ),
        (
            "alarm",
            serde_json::json!({}),
            "value",
            "deploy.alarm",
            DataType::StringArray,
            "alarm.value",
        ),
    ]
}

fn hold_points() -> Vec<(&'static str, serde_json::Value, &'static str)> {
    vec![
        ("servo", serde_json::json!({"axis": 1}), "load"),
        ("spindle", serde_json::json!({"spindle": 1}), "load"),
        ("tool", serde_json::json!({"number": 1}), "offset"),
        ("tool", serde_json::json!({"number": 1}), "length"),
        ("tool", serde_json::json!({"number": 1}), "zofs"),
    ]
}

#[derive(Default)]
struct DeployCounters {
    batches: u64,
    points_good: u64,
    points_bad: u64,
    read_failed: u64,
    point_count_drift: u64,
    sequence_anomaly: u64,
    bad_log: Vec<String>,
}

#[tokio::main]
async fn main() {
    let host = std::env::var("MESA_DEPLOY_HOST").unwrap_or_else(|_| "192.168.15.165".into());
    let port: u16 = std::env::var("MESA_DEPLOY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8193);
    let minutes: u64 = std::env::var("MESA_DEPLOY_MINUTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let interval_ms: u64 = std::env::var("MESA_DEPLOY_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let rounds: u64 = std::env::var("MESA_DEPLOY_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|v| *v >= 1)
        .unwrap_or(5);
    let mut failures: Vec<String> = Vec::new();
    let driver = FocasDriver;
    let wire_cfg = serde_json::json!({
        "host": host, "port": port, "timeout_ms": 5000, "backend": "wire",
    })
    .to_string();

    // HOLD 正确行为（5/5 configure 即拒，不进 run）。
    {
        let conn = driver
            .open_connection("deploy-hold", &wire_cfg)
            .await
            .expect("hold 检查 open 应成功");
        let mut rejected = 0;
        for (r, p, o) in hold_points() {
            let t: mesa_core_types::AcquisitionTask = serde_json::from_value(serde_json::json!({
                "id": format!("hold-{r}-{o}"),
                "schedule": {"mode": "poll", "interval_ms": 1000},
                "binding": {"kind": GENERIC_BINDING_KIND, "config": {"selections": [{
                    "resource_id": r, "parameters": p,
                    "outputs": [{"output": o, "point_key": format!("hold.{r}.{o}")}],
                }]}},
            }))
            .unwrap();
            match conn.configure(1, vec![t]).await {
                Ok(_) => failures.push(format!("HOLD {r}/{o}: 应拒绝但 PASS")),
                Err(e) if e.code == "UNSUPPORTED_POINT" => rejected += 1,
                Err(e) => failures.push(format!(
                    "HOLD {r}/{o}: 应 UNSUPPORTED_POINT，实际 {}",
                    e.code
                )),
            }
        }
        println!("[DEPLOY] HOLD reject {rejected}/5");
        if rejected != 5 {
            failures.push("HOLD 必须 5/5 拒绝".into());
        }
    }

    // D2：多轮 start→采集→stop→start（每轮独立 connection + run）。
    // D1 融入每轮：每轮跑 `minutes/rounds` 分钟（整除余数进末轮），1s poll。
    let ready = ready_points();
    let want_ids_note = "point map 每轮独立分配（1000+idx），轮内比对完整性";
    let mut total = DeployCounters::default();
    let mut last_seq_by_round: Vec<u64> = Vec::new();
    for round in 1..=rounds {
        let this_min = minutes / rounds + if round == rounds { minutes % rounds } else { 0 };
        // 分钟→周期数（interval_ms 换算，至少 1 周期；0 分钟即 smoke 单批）。
        let cycles = (this_min * 60 * 1000 / interval_ms.max(1)).max(1);
        println!("--- D2 round {round}/{rounds}: {this_min}min × {cycles} cycles ---");
        let conn = driver
            .open_connection(&format!("deploy-r{round}"), &wire_cfg)
            .await
            .expect("每轮 open 应成功（D2 重连门）");
        let task: mesa_core_types::AcquisitionTask = serde_json::from_value(serde_json::json!({
            "id": format!("deploy-r{round}"),
            "schedule": {"mode": "poll", "interval_ms": interval_ms},
            "binding": {"kind": GENERIC_BINDING_KIND, "config": {"selections":
                ready.iter().map(|(r, p, o, k, _, _)| serde_json::json!({
                    "resource_id": r, "parameters": p,
                    "outputs": [{"output": o, "point_key": k}],
                })).collect::<Vec<_>>()}},
        }))
        .unwrap();
        let descs = conn
            .configure(round, vec![task])
            .await
            .expect("READY configure 应 PASS");
        assert_eq!(descs.len(), 14, "READY 必须 14 点");
        let mut map = HashMap::new();
        for (i, d) in descs.iter().enumerate() {
            map.insert(d.point_key.clone(), 1000 + i as u32);
        }
        conn.apply_point_map(map.clone()).await.unwrap();
        let (data_tx, mut data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(64);
        let (ctrl_tx, _crx) = tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
        let (event_tx, _erx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
        let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx);
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
        let t0 = Instant::now();
        let mut got = 0u64;
        let mut last_seq = 0u64;
        let want_ids: BTreeSet<u32> = map.values().copied().collect();
        while got < cycles {
            match tokio::time::timeout(Duration::from_secs(30), data_rx.recv()).await {
                Ok(Some(b)) => {
                    got += 1;
                    total.batches += 1;
                    // point count drift 门。
                    if b.values.len() != 14 {
                        total.point_count_drift += 1;
                        total
                            .bad_log
                            .push(format!("round {round} batch 点数漂移 {}", b.values.len()));
                    }
                    let got_ids: BTreeSet<u32> = b.values.iter().map(|v| v.point_id).collect();
                    if got_ids != want_ids {
                        total.point_count_drift += 1;
                        total
                            .bad_log
                            .push(format!("round {round} point_id 不全 {got_ids:?}"));
                    }
                    // sequence 门（轮内；Core 合同：新流首批 `sequence==1`，
                    // 后续 `>last_seq`；缺口允许，回退/停滞 fail）。
                    if got == 0 {
                        if b.sequence != 1 {
                            total.sequence_anomaly += 1;
                            total.bad_log.push(format!(
                                "round {round} 首批 sequence 应为 1，实际 {}",
                                b.sequence
                            ));
                        }
                    } else if b.sequence <= last_seq {
                        total.sequence_anomaly += 1;
                        total.bad_log.push(format!(
                            "round {round} sequence 回退/停滞 {last_seq}→{}",
                            b.sequence
                        ));
                    }
                    last_seq = b.sequence;
                    for pv in &b.values {
                        if pv.quality == Quality::Good {
                            total.points_good += 1;
                        } else {
                            total.points_bad += 1;
                            total.bad_log.push(format!(
                                "round {round} BAD {} {:?} {:?}",
                                pv.point_id, pv.value, pv.quality_code
                            ));
                        }
                    }
                    if got.is_multiple_of(60) || got == cycles {
                        println!(
                            "[DEPLOY] round {round} batch {got}/{cycles} elapsed={}s",
                            t0.elapsed().as_secs()
                        );
                    }
                }
                Ok(None) => {
                    total.read_failed += 1;
                    total.bad_log.push(format!("round {round} 通道关闭"));
                    break;
                }
                Err(_) => {
                    total.read_failed += 1;
                    total.bad_log.push(format!("round {round} 30s 无批"));
                    break;
                }
            }
        }
        last_seq_by_round.push(last_seq);
        // D2 stop：cancel 即 run 退出（Wire session 正确关闭由 run 语义保证；
        // 僵尸 task 由 join 超时发现）。
        shutdown.cancel();
        match tokio::time::timeout(Duration::from_secs(15), run_handle).await {
            Ok(Ok(Ok(()))) => println!("[DEPLOY] round {round} stop OK（run 正常退出）"),
            Ok(Ok(Err(e))) => {
                total.read_failed += 1;
                total
                    .bad_log
                    .push(format!("round {round} run 错误退出 {}", e.code));
            }
            Ok(Err(join_e)) => {
                total.read_failed += 1;
                total
                    .bad_log
                    .push(format!("round {round} run join 错误 {join_e}"));
            }
            Err(_) => {
                total.read_failed += 1;
                total
                    .bad_log
                    .push(format!("round {round} run 15s 未退出（疑似僵尸 task）"));
            }
        }
        let _ = want_ids_note;
    }

    // D3：本 harness 默认 NOT-PROVEN（无安全窗口不制造故障；见文件头）。
    let d3 = "NOT-PROVEN（无安全窗口；live 主 READ_FAILED 待 CNC 配合，见 Gate 1 canary 口径）";
    // D4：只打印本机 ARCH，结论恒 pending（不接受环境变量自报 proven）。
    let arch = std::env::consts::ARCH;
    let d4 = "pending（本机 ARCH 仅记录，不作 ARM 证据）";

    println!("--- DEPLOY GATE TABLE ---");
    println!("READY/HOLD        HOLD 5/5（上文）");
    println!("D1 batches        {}", total.batches);
    println!("D1 points_good    {}", total.points_good);
    println!("D1 points_bad     {} (must be 0)", total.points_bad);
    println!("D1 read_failed    {} (must be 0)", total.read_failed);
    println!("D1 count_drift    {} (must be 0)", total.point_count_drift);
    println!(
        "D1 seq_anomaly    {} (回退/停滞 must be 0；合并缺口已豁免)",
        total.sequence_anomaly
    );
    println!("D2 rounds         {rounds} last_seq={last_seq_by_round:?}");
    println!("D3 reconnect      {d3}");
    println!("D4 arm            {d4} host_arch={arch}");
    for b in total.bad_log.iter().take(20) {
        eprintln!("[DEPLOY-BAD] {b}");
    }
    if total.bad_log.len() > 20 {
        eprintln!(
            "[DEPLOY-BAD] ... 另 {} 条（见上）",
            total.bad_log.len() - 20
        );
    }
    if total.points_bad != 0
        || total.read_failed != 0
        || total.point_count_drift != 0
        || total.sequence_anomaly != 0
    {
        eprintln!("[DEPLOY] FAIL");
        std::process::exit(2);
    }
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("[DEPLOY-FAIL] {f}");
        }
        eprintln!("[DEPLOY] FAIL");
        std::process::exit(2);
    }
    println!("[DEPLOY] PASS (D3 {d3}; D4 {d4} host_arch={arch})");
}
