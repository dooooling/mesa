//! Cutover Gate 1 production-path canary（诊断专用，只读，不改生产 backend）。
//!
//! 目标：穿过完整生产链路验证 Gate 1，而不是只验证 `FocasApi` 层：
//! `FocasDriver::open_connection(backend=wire)` → `configure()` →
//! `apply_point_map()` → `run()` → `PointValue/DataBatch`
//! （含 `coerce_value` / `value_fits_data_type` / `Quality` /
//! `quality_code` / `source_label` / `point_id`）。
//!
//! 范围（冻结）：12 READY configure+run PASS；5 HOLD configure 即
//! `UNSUPPORTED_POINT`（不等 run 后 BAD）；alarm `StringArray` 不再错杀；
//! point-local → 仅该点 BAD；read 期整批 Err 语义由 G3 单测锁定，live 主
//! run() 的 READ_FAILED 需 CNC 配合，本轮记 NOT-PROVEN。
//!
//! backend 纪律：canary 只开 `backend=wire` 连接；绝不改默认、不碰
//! Native/FWLIB 路径（C2 隔离纪律延续）。
//!
//! 用法：`MESA_CANARY_HOST=192.168.15.165 cargo run -p mesa-driver-focas2
//! --example wire_canary`（可选 `MESA_CANARY_PORT` 默认 8193）。
//! 退出码：任一 Gate fail → 非零；全过 → 0。
//!
//! Gate 6 诚实口径：chain-head（connect 成功 + 首批 GOOD）由主 run() 完成；
//! 整批 Err 语义由 G3 单测 `production_error_semantics_locked` 锁定；
//! 此处 live 补实例级自杀（影子 `WireFocasApi` 同 endpoint connect + 首读
//! GOOD 后经 canary 窄口只断自身 session 再读，锁 read 期 fatal 形态）+
//! 黑洞连接快速失败 + 无 fallback；live 主 run() 的 READ_FAILED 需 CNC 侧
//! 配合复位，本轮记 NOT-PROVEN（不虚构、不 fail 整轮）。

use std::collections::HashMap;
use std::time::Duration;

use mesa_core_types::{DataType, GENERIC_BINDING_KIND, Quality, Value};
use mesa_driver_focas2::FocasDriver;
use mesa_driver_sdk::{DataSink, Driver};
use tokio_util::sync::CancellationToken;

/// READY 12 类的
/// (resource_id, parameters, output, point_key, 期望 DataType, 期望 source_label)。
/// expected label 独立冻结（手写字符串；不得走生产 resolver 自证——resolver 与
/// `source_label()` 若一起漂移会同错同绿；见 PR #66 review）。
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
            "canary.status",
            DataType::U32,
            "machine.status",
        ),
        (
            "machine",
            serde_json::json!({}),
            "feed",
            "canary.feed",
            DataType::U32,
            "machine.feed",
        ),
        (
            "machine",
            serde_json::json!({}),
            "spindle_speed",
            "canary.spindle_speed",
            DataType::I32,
            "spindle.active.speed",
        ),
        (
            "axis",
            serde_json::json!({"axis": 1}),
            "absolute",
            "canary.axis1",
            DataType::I32,
            "axis[1].absolute",
        ),
        (
            "axis",
            serde_json::json!({"axis": 2}),
            "absolute",
            "canary.axis2",
            DataType::I32,
            "axis[2].absolute",
        ),
        (
            "axis",
            serde_json::json!({"axis": 3}),
            "absolute",
            "canary.axis3",
            DataType::I32,
            "axis[3].absolute",
        ),
        (
            "spindle",
            serde_json::json!({"spindle": 1}),
            "gear",
            "canary.gear1",
            DataType::I32,
            "spindle[1].gear",
        ),
        (
            "spindle",
            serde_json::json!({"spindle": 1}),
            "maxrpm",
            "canary.maxrpm1",
            DataType::I32,
            "spindle[1].maxrpm",
        ),
        (
            "macro",
            serde_json::json!({"number": 501}),
            "value",
            "canary.macro501",
            DataType::F64,
            "macro[501]",
        ),
        (
            "pmc",
            serde_json::json!({"kind": "R", "addr": 100}),
            "value",
            "canary.pmcR100",
            DataType::I32,
            "pmc.R100",
        ),
        (
            "param",
            serde_json::json!({"number": 6711}),
            "value",
            "canary.param6711",
            DataType::I32,
            "param[6711]",
        ),
        (
            "diagnosis",
            serde_json::json!({"number": 301, "axis": 3}),
            "value",
            "canary.diag301a3",
            DataType::F64,
            "diagnosis[301]@axis3",
        ),
        (
            "opmsg",
            serde_json::json!({}),
            "value",
            "canary.opmsg",
            DataType::String,
            "opmsg.value",
        ),
        (
            "alarm",
            serde_json::json!({}),
            "value",
            "canary.alarm",
            DataType::StringArray,
            "alarm.value",
        ),
    ]
}

/// HOLD 5 类（configure 即拒，不进 run）。
fn hold_points() -> Vec<(&'static str, serde_json::Value, &'static str)> {
    vec![
        ("servo", serde_json::json!({"axis": 1}), "load"),
        ("spindle", serde_json::json!({"spindle": 1}), "load"),
        ("tool", serde_json::json!({"number": 1}), "offset"),
        ("tool", serde_json::json!({"number": 1}), "length"),
        ("tool", serde_json::json!({"number": 1}), "zofs"),
    ]
}

fn variant_of(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "Bool",
        Value::I32(_) => "I32",
        Value::U32(_) => "U32",
        Value::I64(_) => "I64",
        Value::U64(_) => "U64",
        Value::F32(_) => "F32",
        Value::F64(_) => "F64",
        Value::String(_) => "String",
        Value::Bytes(_) => "Bytes",
        Value::DateTime(_) => "DateTime",
        Value::BoolArray(_) => "BoolArray",
        Value::I32Array(_) => "I32Array",
        Value::U32Array(_) => "U32Array",
        Value::I64Array(_) => "I64Array",
        Value::U64Array(_) => "U64Array",
        Value::F32Array(_) => "F32Array",
        Value::F64Array(_) => "F64Array",
        Value::StringArray(_) => "StringArray",
        Value::DateTimeArray(_) => "DateTimeArray",
    }
}

fn expected_variant(dt: DataType) -> &'static str {
    match dt {
        DataType::Bool => "Bool",
        DataType::U32 => "U32",
        DataType::I32 => "I32",
        DataType::I64 => "I64",
        DataType::U64 => "U64",
        DataType::F32 => "F32",
        DataType::F64 => "F64",
        DataType::String => "String",
        DataType::Bytes => "Bytes",
        DataType::DateTime => "DateTime",
        DataType::BoolArray => "BoolArray",
        DataType::I32Array => "I32Array",
        DataType::U32Array => "U32Array",
        DataType::I64Array => "I64Array",
        DataType::U64Array => "U64Array",
        DataType::F32Array => "F32Array",
        DataType::F64Array => "F64Array",
        DataType::StringArray => "StringArray",
        DataType::DateTimeArray => "DateTimeArray",
    }
}

#[tokio::main]
async fn main() {
    let host = std::env::var("MESA_CANARY_HOST").unwrap_or_else(|_| "192.168.15.165".into());
    let port: u16 = std::env::var("MESA_CANARY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8193);
    let mut failures: Vec<String> = Vec::new();
    let driver = FocasDriver;

    // backend=wire 打开生产连接（与生产同源 JSON）。
    let wire_cfg = serde_json::json!({
        "host": host, "port": port, "timeout_ms": 5000, "backend": "wire",
    })
    .to_string();
    let conn = match driver.open_connection("canary", &wire_cfg).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[CANARY-FAIL] backend=wire open_connection: {e}");
            std::process::exit(1);
        }
    };
    println!("[CANARY] backend=wire open_connection OK");

    // Gate 1：12 READY（文档口径 12 类；axis 拆 3 轴共 14 点）configure PASS。
    let ready = ready_points();
    let task = serde_json::json!({
        "id": "canary",
        "schedule": {"mode": "poll", "interval_ms": 500},
        "binding": {
            "kind": GENERIC_BINDING_KIND,
            "config": {"selections": ready.iter().map(|(r, p, o, k, _, _)| {
                serde_json::json!({
                    "resource_id": r, "parameters": p,
                    "outputs": [{"output": o, "point_key": k}],
                })
            }).collect::<Vec<_>>()}
        }
    });
    let task: mesa_core_types::AcquisitionTask = serde_json::from_value(task).unwrap();
    let descs = match conn.configure(1, vec![task]).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[CANARY-FAIL] READY configure: {e}");
            std::process::exit(1);
        }
    };
    println!("[CANARY] READY configure PASS descs={}", descs.len());
    // descriptor 类型 + source_label 精确比对（独立冻结字符串；不得走生产
    // resolver 自证——同错同绿，见 PR #66 review）。
    for ((_, _, _, key, want_dt, want_label), desc) in ready.iter().zip(descs.iter()) {
        if &desc.data_type != want_dt {
            failures.push(format!(
                "descriptor {key}: want {want_dt:?} got {:?}",
                desc.data_type
            ));
        }
        match &desc.source_label {
            Some(got) if got == want_label => {}
            other => failures.push(format!(
                "descriptor {key}: source_label 应 {want_label:?}，实际 {other:?}"
            )),
        }
        println!(
            "[CANARY] desc {key} dt={:?} label={:?}",
            desc.data_type, desc.source_label
        );
    }

    // Gate 2：5 HOLD configure 即 UNSUPPORTED_POINT（不进 run）。
    let mut hold_rejected = 0;
    for (r, p, o) in hold_points() {
        let t: mesa_core_types::AcquisitionTask = serde_json::from_value(serde_json::json!({
            "id": format!("hold-{r}-{o}"),
            "schedule": {"mode": "poll", "interval_ms": 500},
            "binding": {
                "kind": GENERIC_BINDING_KIND,
                "config": {"selections": [{
                    "resource_id": r, "parameters": p,
                    "outputs": [{"output": o, "point_key": format!("hold.{r}.{o}")}],
                }]}
            }
        }))
        .unwrap();
        match conn.configure(2, vec![t]).await {
            Ok(_) => failures.push(format!("HOLD {r}/{o}: configure 应拒绝但 PASS")),
            Err(e) if e.code == "UNSUPPORTED_POINT" => {
                hold_rejected += 1;
                println!("[CANARY] HOLD {r}/{o} configure 拒绝 OK ({})", e.code);
            }
            Err(e) => failures.push(format!(
                "HOLD {r}/{o}: 错误码应 UNSUPPORTED_POINT，实际 {}",
                e.code
            )),
        }
    }

    // Gate 1 run：apply map → run → 收首批 DataBatch（完整生产链路）。
    let mut map = HashMap::new();
    for (i, d) in descs.iter().enumerate() {
        map.insert(d.point_key.clone(), 1000 + i as u32);
    }
    if let Err(e) = conn.apply_point_map(map.clone()).await {
        eprintln!("[CANARY-FAIL] apply_point_map: {e}");
        std::process::exit(1);
    }
    let (data_tx, mut data_rx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(16);
    let (ctrl_tx, _ctrl_rx) = tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(8);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(8);
    let sink = DataSink::for_test(ctrl_tx, data_tx, event_tx);
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let run_handle = tokio::spawn(async move { conn.run(sink, sd).await });
    let batch = match tokio::time::timeout(Duration::from_secs(30), data_rx.recv()).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            eprintln!("[CANARY-FAIL] run 首批：通道关闭");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("[CANARY-FAIL] run 首批 30s 超时");
            std::process::exit(1);
        }
    };
    println!(
        "[CANARY] run 首批到达 values={} seq={}",
        batch.values.len(),
        batch.sequence
    );
    // Blocker #4 真修：false-green 硬门（READY 正常窗口必须全 GOOD）。
    // - batch 必须 14 点完整（`values.len == 14`；丢点/多点即 fail）。
    // - 期望 point_id 全齐（map 14 个全在批内；缺 id 即 fail）。
    // - GOOD 必须 14，BAD 必须 0（READY 出现 BAD 即 fail，不再“计数待确认”）。
    if batch.values.len() != 14 {
        failures.push(format!("首批必须 14 点完整，实际 {}", batch.values.len()));
    }
    {
        let got_ids: std::collections::BTreeSet<u32> =
            batch.values.iter().map(|pv| pv.point_id).collect();
        let want_ids: std::collections::BTreeSet<u32> = map.values().copied().collect();
        if got_ids != want_ids {
            failures.push(format!(
                "point_id 必须全齐 want={want_ids:?} got={got_ids:?}"
            ));
        }
    }
    // Gate 3/4：Value variant + Quality::Good + point_id/map；alarm 不再错杀。
    let mut n_good = 0;
    let mut n_bad = 0;
    for pv in &batch.values {
        let key = map
            .iter()
            .find(|(_, v)| **v == pv.point_id)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| format!("<unknown:{}>", pv.point_id));
        let want_dt = ready
            .iter()
            .find(|(_, _, _, k, _, _)| k == &key)
            .map(|(_, _, _, _, dt, _)| *dt);
        match want_dt {
            None => failures.push(format!("未知 point {key} id={}", pv.point_id)),
            Some(dt) => {
                let want_v = expected_variant(dt);
                let got_v = variant_of(&pv.value);
                if pv.quality == Quality::Good {
                    n_good += 1;
                    // 宽容对（U32/I32、F32/F64）按生产 coerce 语义放行。
                    let compat = got_v == want_v
                        || (want_v == "I32" && got_v == "U32")
                        || (want_v == "U32" && got_v == "I32")
                        || (want_v == "F64" && got_v == "F32")
                        || (want_v == "F32" && got_v == "F64");
                    if !compat {
                        failures.push(format!(
                            "{key}: GOOD 但 variant 不符 want={want_v} got={got_v} val={:?}",
                            pv.value
                        ));
                    }
                    println!(
                        "[CANARY] GOOD {key} {got_v}={:?} code={:?}",
                        pv.value, pv.quality_code
                    );
                } else {
                    // Blocker #4：READY 正常窗口 BAD 即 fail（不再计数待确认）。
                    n_bad += 1;
                    failures.push(format!(
                        "{key} BAD（READY 正常窗口不允许）：want={want_v} got={got_v} val={:?} code={:?}",
                        pv.value, pv.quality_code
                    ));
                }
            }
        }
    }
    // 主 run() 保持运行至 Gate 6 影子对照完成后再停（Gate 6 观察“主 run 是否被
    // CNC 复位”需要主 run 存活；此处先不停）。
    // NOTE：主 run() 为 500ms 周期无限循环；Gate 6 结束后统一 cancel。

    // Gate 6：read-time fatal 证据链（诚实口径，不虚构 PASS）。
    // 实例级自杀：影子 `WireFocasApi` 同 endpoint connect + 首读 GOOD 后，
    // 经 canary 窄口只断自身 session 再读，锁 read 期 fatal 形态
    // （不碰被测 run、不碰 CNC/对端）。
    {
        use mesa_driver_focas2::wire_pub::WireFocasApi;
        use mesa_driver_focas2::{FocasApi, parse_address};
        use std::time::Duration as StdDuration;
        let shadow = WireFocasApi::new(StdDuration::from_secs(5));
        match shadow.connect(&host, port, 5000).await {
            Ok(()) => println!("[CANARY] readtime-fatal 影子 session connect OK"),
            Err(e) => {
                failures.push(format!("readtime-fatal 影子 connect 失败：{e}"));
            }
        }
        let probe_addr = parse_address("status").unwrap();
        match shadow.read_batch(std::slice::from_ref(&probe_addr)).await {
            Ok(v) => println!("[CANARY] readtime-fatal 影子首读 GOOD {v:?}"),
            Err(e) => failures.push(format!("readtime-fatal 影子首读失败：{e}")),
        }
        // 只断影子实例自有 session（`FocasApi::disconnect` 公共语义，已有；
        // 被测 run()/CNC 不动；不新增生产窄口，见 PR #66 review blocker #2）。
        shadow.disconnect().await;
        match shadow.read_batch(std::slice::from_ref(&probe_addr)).await {
            Ok(v) => {
                println!(
                    "[CANARY] readtime-fatal 影子断后读自恢复 GOOD {v:?}（NOT-PROVEN：client 具重建语义）"
                );
            }
            Err(e) => {
                println!(
                    "[CANARY] readtime-fatal 影子断后读 fatal OK：{e}（read 期 fatal 形态已锁；run() 整批 Err→READ_FAILED 由 G3 单测锁语义）"
                );
            }
        }
        println!(
            "[CANARY] readtime-fatal 主 run() 未主动杀死（CNC 侧复位需配合；live 主 READ_FAILED 记 NOT-PROVEN 待 CNC 配合窗口）"
        );
    }
    // 黑洞连接：同 READY 配置指向本地拒绝端口——connect 期必错；此处只锁
    // “无 fallback”（全程 backend=wire，无 Native 实例），不等价代换
    // read 期 READ_FAILED（后者由 G3 单测锁整批 Err 语义 + 主 live 待 CNC 窗口）。
    {
        let black_cfg = serde_json::json!({
            "host": "127.0.0.1", "port": 9, "timeout_ms": 1000, "backend": "wire",
        })
        .to_string();
        let black_conn = driver
            .open_connection("canary-blackhole", &black_cfg)
            .await
            .expect("黑洞 open 应成功（失败在 run/connect）");
        let bt: mesa_core_types::AcquisitionTask = serde_json::from_value(serde_json::json!({
            "id": "blackhole",
            "schedule": {"mode": "poll", "interval_ms": 200},
            "binding": {"kind": GENERIC_BINDING_KIND, "config": {"selections": [{
                "resource_id": "machine", "parameters": {},
                "outputs": [{"output": "status", "point_key": "black.status"}],
            }]}},
        }))
        .unwrap();
        black_conn
            .configure(1, vec![bt])
            .await
            .expect("黑洞 configure 应 PASS");
        let mut bmap = HashMap::new();
        bmap.insert("black.status".to_string(), 9999u32);
        black_conn.apply_point_map(bmap).await.unwrap();
        let (btx, _brx) = tokio::sync::mpsc::channel::<mesa_core_types::DataBatch>(4);
        let (bctx, _bcrx) = tokio::sync::mpsc::channel::<mesa_driver_protocol::pb::Envelope>(4);
        let (betx, _berx) = tokio::sync::mpsc::channel::<mesa_driver_sdk::EventBatch>(4);
        let bsink = DataSink::for_test(bctx, btx, betx);
        let bsd = CancellationToken::new();
        match tokio::time::timeout(Duration::from_secs(20), black_conn.run(bsink, bsd)).await {
            Ok(Err(e)) if e.code == "CONNECT_FAILED" => {
                println!(
                    "[CANARY] blackhole connect 期 → CONNECT_FAILED OK（无 fallback；read 期 READ_FAILED 由 G3 单测锁整批 Err 语义，live 主 READ_FAILED 待 CNC 配合窗口 NOT-PROVEN）"
                );
            }
            Ok(Err(e)) if e.code == "READ_FAILED" => {
                println!("[CANARY] blackhole → READ_FAILED OK（无 fallback）");
            }
            Ok(Err(e)) => {
                failures.push(format!("blackhole 应 CONNECT/READ_FAILED，实际 {}", e.code))
            }
            Ok(Ok(())) => failures.push("blackhole run 不应 Ok 退出".into()),
            Err(_) => failures.push("blackhole run 20s 未返回（应快速失败）".into()),
        }
    }

    // 主 run() 收尾。
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(15), run_handle).await;

    println!("--- CANARY GATE TABLE ---");
    println!("READY configure   14/14 (12 类，axis×3)");
    println!("HOLD reject       {hold_rejected}/5");
    println!("GOOD              {n_good}/14");
    println!("BAD               {n_bad} (must be 0)");
    println!("failures          {}", failures.len());
    for f in &failures {
        eprintln!("[CANARY-FAIL] {f}");
    }
    if hold_rejected != 5 || !failures.is_empty() {
        eprintln!("[CANARY] FAIL");
        std::process::exit(2);
    }
    println!("[CANARY] PASS");
}
