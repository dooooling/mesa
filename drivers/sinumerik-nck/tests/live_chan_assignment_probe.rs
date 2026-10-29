//! `maxnumChannels` / `chanAssignment` 真机只读验证脚手架（**一次性验证脚手架，
//! 不是生产探针**）。
//!
//! ## 它是什么、不是什么
//!
//! - **是**：一次性的、有界的、只读的验证脚手架，用来回答一个问题——
//!   N 区 `column=4`（`maxnumChannels`，手册 p1069）与 `column=21`
//!   （`chanAssignment`，手册 p1065）这两个地址在本台 828D 上**能不能读、
//!   读出来是什么、通道不存在时到底返回 0 还是被拒**。
//! - **不是**：生产探针。它**不进入** `identity_spec.rs` 的 `PROBES_828D`，
//!   **不进入** `identity.rs` 的枚举执行路径，**不产生**任何节点/点位，
//!   也**不**改动 `r1.json` / `DIGESTS.lock` / `r1.meta.json`。
//! - 需要 `--ignored` 显式运行（默认 CI 不会连真机）：
//!   ```text
//!   cargo test -p mesa-driver-sinumerik-nck --test live_chan_assignment_probe -- --ignored --nocapture
//!   ```
//!
//! ## 两条观测通道（为什么要两条）
//!
//! 1. **生产链路**（`NckClient::connect` + `encode_var_spec` + `read_vars`）：
//!    与采集路径完全一致，但 P1-4 fail-closed 会在
//!    `transport`/`长度` 与声明不符时**丢弃 data**（CNC 采集红线：错误类型的数据
//!    不得标 GOOD）。因此它只能证明"这条请求在**声明的解码**下不可用"，
//!    看不到设备实际回了多少字节。
//! 2. **原始字节链路**（`S7Session::read_var`，只读、同一台设备、同一条 TCP 栈）：
//!    不丢弃任何字节，用于留存**响应原始字节**并解释第 1 条的失败原因。
//!    它**只用于观测**，不参与生产语义判定；两条链路各自独立建连、用完即关。
//!
//! 两条都是**只读**：不写参数、不发控制命令、不启动采集任务。
//!
//! ## 安全边界（硬约束，改本文件前先读）
//!
//! - **直连机床**：TCP/102 + COTP + S7 Setup，**不经 mesad、不碰任何数据库**。
//! - **有界**：`chanAssignment` 只扫 `1..=min(maxnumChannels, 31)`；最多两轮；
//!   `maxnumChannels` 读不到时**不盲扫 31 个索引**，只做 line=1 的定点观测并
//!   明确标注"无法完整枚举"；module 兜底尝试最多 2 个候选且逐个打印。
//! - **失败即失败**：设备拒绝 / 超时 / 通信失败一律记为**读取失败**，
//!   不得解释为"通道不存在"；不换列号重试、不猜。
//!
//! ## 四类结果严格区分（本脚手架的核心）
//!
//! | 结果 | 判定 |
//! |---|---|
//! | 解出 `0` | `NOT_EXIST`（手册定义：通道不存在） |
//! | 解出 `>0` 且不超过方式组上界 | `ASSIGNED(n)` |
//! | `ITEM_REJECTED` / 传输错误 / 超时 / 本地校验丢弃 | `READ_FAILED`（**不是**"不存在"） |
//! | 解出超出合理范围 / 载荷长度与 `TYPE_UWORD` 不符 | `UNDETERMINED`（结果待核实） |
//!
//! ## 关于 `module` 字节
//!
//! 本文件对 N 区一律先用 `module=26`，这是**沿用既有 `AXIS_NAME` 探针的假设**
//! （`identity_spec.rs` 同属 N 区），**不是手册值**：手册只给 column/line，
//! module 字节的权威来源仍是未解决项。若 26 被拒，脚本会**记录该事实**并
//! 逐个尝试一个备选 module，逐条打印，不做盲扫。

use mesa_driver_sinumerik_nck::{
    AXIS_NAME, IdentityProbeSpec, NativeNckAddress, NckClient, NckConnConfig, NckReadItem,
    decode_value, encode_var_spec, resolve_native,
};
use mesa_s7_transport::{S7ReadVarItem, S7Session};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// 常量（全部带出处）
// ---------------------------------------------------------------------------

/// `maxnumChannels` 的列号：手册 p1069 `maxnumChannels (N, Y) $AN_DIM_CHANNELS`，
/// `Column index: 4`、`TYPE_UWORD`。
const MAXNUM_CHANNELS_COLUMN: u16 = 4;
/// `chanAssignment` 的列号：手册 p1065 `chanAssignment (N, Y) MD10010: ...`，
/// `Column index: 21`、`Line index: Channel number`、`Max. line index: maxnumChannels`。
///
/// NOTE：`10010` 是该变量反映的机床数据号，**不是**列号；两者极易混淆。
const CHAN_ASSIGNMENT_COLUMN: u16 = 21;
/// `numBAGs` 的列号：手册 p1071 `numBAGs (N, Y)`，`Column index: 3`、`TYPE_UWORD`。
/// 用于给 `chanAssignment` 的返回值定上界（手册：`n = channel assigned to mode group n`）。
const NUM_BAGS_COLUMN: u16 = 3;
/// `nckType` 的列号：手册 p1072，`Column index: 31`、`TYPE_UWORD`（828D = 16000）。
const NCK_TYPE_COLUMN: u16 = 31;
/// `nckVersion` 的列号：手册 p1073，`Column index: 32`、`TYPE_DOUBLE`
/// （整数部分即发布号，如 3.4 → 34）。
const NCK_VERSION_COLUMN: u16 = 32;
/// `progStatus` 的列号：C 区 `Column index: 13`（既有真机证据已读到，用作
/// 通道存在性的**独立**对照，见 `docs/evidence/nck-live-samples-probe.txt`）。
const PROG_STATUS_COLUMN: u16 = 13;
/// C 区 `progStatus` 既有证据里的 module 字节（0x7f = 127）。
const PROG_STATUS_MODULE: u8 = 127;

/// N 区 module 首选值：**沿用假设**（同 `AXIS_NAME` 探针），非手册值。
const N_AREA_MODULE_ASSUMED: u8 = 26;
/// N 区 module 备选值：仅在 26 被拒时**逐个**尝试并记录，不做盲扫。
const N_AREA_MODULE_FALLBACKS: [u8; 1] = [127];

/// 任务硬上限：扫描索引不超过 31（即使 `maxnumChannels` 更大也不越过）。
const LINE_CAP: u16 = 31;
/// 通道存在性交叉核对的最大索引数（避免为对照而发大量请求）。
const CROSS_CHECK_MAX: u16 = 8;

// ---------------------------------------------------------------------------
// 连接参数（沿用 `live_samples_probe.rs` 的脚手架默认值）
// ---------------------------------------------------------------------------

fn cfg_from_env() -> NckConnConfig {
    let host = std::env::var("MESA_NCK_HOST").unwrap_or_else(|_| "192.168.15.60".into());
    let port: u16 = std::env::var("MESA_NCK_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(102);
    let local_tsap: u16 = std::env::var("MESA_NCK_LOCAL_TSAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024);
    let remote_tsap: u16 = std::env::var("MESA_NCK_REMOTE_TSAP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3332);
    NckConnConfig::from_json(&json!({
        "host": host, "port": port, "family": "828d",
        "local_tsap": local_tsap, "remote_tsap": remote_tsap,
        "timeout_ms": 2000, "pdu_length": 480, "max_read_items": 1,
    }))
    .expect("连接参数合法")
}

// ---------------------------------------------------------------------------
// 观测记录（请求字节 / 响应字节 / 解码值 / 错误码 / 时间戳）
// ---------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// UTC 时间戳（毫秒精度）。测试进程无 chrono 依赖，这里用 civil_from_days 手算，
/// 避免为一个时间戳引入新依赖。
fn utc_stamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时钟早于 1970");
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant 的 civil_from_days（1970-01-01 起算的天数 → 年月日）。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// 生产链路单次读的可观测事实（`data` 可能被 P1-4 丢弃，故为 Option）。
#[derive(Debug, Clone)]
struct Obs {
    req_hex: String,
    return_code: Option<u8>,
    transport_size: Option<u8>,
    data: Option<Vec<u8>>,
    decoded: String,
    job_error: Option<String>,
}

impl Obs {
    fn ok(&self) -> bool {
        self.job_error.is_none() && self.return_code == Some(0xFF) && self.data.is_some()
    }

    /// 单行留痕（证据文件每一行都来自这里，字段顺序固定便于 grep）。
    fn line(&self, tag: &str) -> String {
        let data_hex = match &self.data {
            Some(b) => hex(b),
            None => "<none>".into(),
        };
        let mut s = format!(
            "[{}] {tag:<34} req={} | ret={} | transport={} | len={} | data={} | decode={}",
            utc_stamp(),
            self.req_hex,
            match self.return_code {
                Some(c) => format!("0x{c:02X}"),
                None => "<none>".into(),
            },
            match self.transport_size {
                Some(t) => format!("0x{t:02X}"),
                None => "<none>".into(),
            },
            match &self.data {
                Some(b) => b.len().to_string(),
                None => "<none>".into(),
            },
            data_hex,
            self.decoded,
        );
        if let Some(e) = &self.job_error {
            s.push_str(&format!(" | job_error={e}"));
        }
        s
    }
}

/// 原始字节链路单次读（**不丢弃任何字节**，仅用于观测）。
#[derive(Debug, Clone)]
struct RawObs {
    req_hex: String,
    return_code: u8,
    transport_size: u8,
    data: Vec<u8>,
}

impl RawObs {
    fn ok(&self) -> bool {
        self.return_code == 0xFF
    }

    fn line(&self, tag: &str) -> String {
        format!(
            "[{}] {tag:<34} req={} | ret=0x{:02X} | transport=0x{:02X} | len={} | data={}",
            utc_stamp(),
            self.req_hex,
            self.return_code,
            self.transport_size,
            self.data.len(),
            hex(&self.data),
        )
    }
}

// ---------------------------------------------------------------------------
// 地址构造
// ---------------------------------------------------------------------------

/// N 区单值地址（`area_no` 恒 1；`line` 由调用方给出）。
fn n_area_addr(column: u16, line: u16, module: u8, decode: &str) -> NativeNckAddress {
    NativeNckAddress::from_parameters(
        json!({
            "area": "N", "area_no": 1, "module": module, "column": column,
            "line": line, "count": 1, "unit_mode": "current",
            "decode": decode, "transport_size": "0x09",
        })
        .as_object()
        .unwrap(),
    )
    .expect("N 区单值地址合法")
}

/// C 区通道单值地址（`area_no` = 通道号，用于通道存在性独立对照）。
fn c_area_channel_addr(channel: u16, column: u16, module: u8, decode: &str) -> NativeNckAddress {
    NativeNckAddress::from_parameters(
        json!({
            "area": "C", "area_no": channel, "module": module, "column": column,
            "line": 1, "count": 1, "unit_mode": "current",
            "decode": decode, "transport_size": "0x09",
        })
        .as_object()
        .unwrap(),
    )
    .expect("C 区通道地址合法")
}

// ---------------------------------------------------------------------------
// 两条观测通道
// ---------------------------------------------------------------------------

/// 生产链路：发一次读请求并把**请求字节**留存。
///
/// 注意：`NckClient::read_vars` 在 P1-4 校验不通过时丢弃 `data`
/// （这是生产红线，本脚手架**不改**它）。因此这里只能得到
/// "在声明解码下是否可用"，设备实际字节由 `raw_read` 补。
async fn observe(client: &mut NckClient, addr: NativeNckAddress) -> Obs {
    let rp = match resolve_native(addr) {
        Ok(rp) => rp,
        Err(e) => {
            return Obs {
                req_hex: "<unresolved>".into(),
                return_code: None,
                transport_size: None,
                data: None,
                decoded: format!("WIRE_ERR {e}"),
                job_error: None,
            };
        }
    };
    let req_hex = hex(&encode_var_spec(&rp.wire));
    let item = NckReadItem {
        wire: rp.wire,
        expected_data_len: rp.expected_data_len,
        expected_transport_size: rp.expected_transport_size,
    };
    read_and_fold(client, item, req_hex, rp.kind).await
}

/// 生产链路（探针常量路径）：`IdentityProbeSpec` 直接给 item。
async fn observe_spec(client: &mut NckClient, spec: IdentityProbeSpec, target: u16) -> Obs {
    let (wire, item) = spec.resolve(target).expect("探针槽位合法");
    read_and_fold(client, item, hex(&encode_var_spec(&wire)), spec.decode).await
}

async fn read_and_fold(
    client: &mut NckClient,
    item: NckReadItem,
    req_hex: String,
    kind: mesa_driver_sinumerik_nck::NckDataKind,
) -> Obs {
    match client.read_vars(&[item]).await {
        Err(e) => Obs {
            req_hex,
            return_code: None,
            transport_size: None,
            data: None,
            decoded: "<no data>".into(),
            job_error: Some(format!("{} {}", e.code, e.message)),
        },
        Ok(results) => match results.first() {
            None => Obs {
                req_hex,
                return_code: None,
                transport_size: None,
                data: None,
                decoded: "<no item result>".into(),
                job_error: Some("NO_RESULT".into()),
            },
            Some(r) => {
                let decoded = match &r.data {
                    None => {
                        "LOCAL_REJECT(P1-4：设备拒绝，或 transport/长度与声明不符，data 被丢弃)"
                            .to_string()
                    }
                    Some(bytes) => match decode_value(bytes, kind) {
                        Ok(v) => format!("OK {v:?}"),
                        Err(e) => format!("DECODE_ERR {e}"),
                    },
                };
                Obs {
                    req_hex,
                    return_code: Some(r.return_code),
                    transport_size: Some(r.transport_size),
                    data: r.data.clone(),
                    decoded,
                    job_error: None,
                }
            }
        },
    }
}

/// 原始字节链路：不丢弃任何字节（**只用于观测**，不做生产语义判定）。
async fn raw_read(session: &mut S7Session, addr: NativeNckAddress) -> Result<RawObs, String> {
    let rp = resolve_native(addr).map_err(|e| format!("WIRE_ERR {e}"))?;
    let req = encode_var_spec(&rp.wire);
    let item = S7ReadVarItem {
        var_spec: req.clone(),
        expected_data_len: rp.expected_data_len,
    };
    let results = session
        .read_var(&[item])
        .await
        .map_err(|e| format!("JOB_ERR {} {}", e.code, e.message))?;
    let r = results
        .first()
        .ok_or_else(|| "NO_RESULT（响应里没有 item）".to_string())?;
    Ok(RawObs {
        req_hex: hex(&req),
        return_code: r.return_code,
        transport_size: r.transport_size,
        data: r.data.clone(),
    })
}

// ---------------------------------------------------------------------------
// 判定（四类严格区分）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// 解出 `>0`：通道存在，分配到该方式组。
    Assigned(u16),
    /// 解出 `0`：手册定义 = 通道不存在。
    NotExist,
    /// 设备拒绝 / 超时 / 通信失败：**读取失败**（不是"不存在"）。
    ReadFailed,
    /// 超出合理范围或载荷无法按 `TYPE_UWORD` 解出：**结果待核实**。
    Undetermined(String),
}

fn verdict_str(v: &Verdict) -> String {
    match v {
        Verdict::Assigned(n) => format!("ASSIGNED({n})"),
        Verdict::NotExist => "NOT_EXIST".into(),
        Verdict::ReadFailed => "READ_FAILED".into(),
        Verdict::Undetermined(r) => format!("UNDETERMINED({r})"),
    }
}

/// `TYPE_UWORD` 载荷 → 数值。
///
/// 手册只给类型不给字节数；本机实测载荷长度见证据文件。2 字节按 LE 直取；
/// 4 字节且高位为 0 时取低 16 位（无歧义）；其余一律**拒绝猜**。
fn uword_from_payload(data: &[u8]) -> Result<u16, String> {
    match data.len() {
        2 => Ok(u16::from_le_bytes([data[0], data[1]])),
        4 => {
            let hi = u16::from_le_bytes([data[2], data[3]]);
            if hi == 0 {
                Ok(u16::from_le_bytes([data[0], data[1]]))
            } else {
                Err(format!(
                    "4 字节载荷高位非零（0x{hi:04X}），按 TYPE_UWORD 取低 16 位有歧义"
                ))
            }
        }
        n => Err(format!("{n} 字节载荷，TYPE_UWORD 无法据此解出")),
    }
}

/// `chanAssignment` 单索引判定。
fn verdict_of(obs: &RawObs, num_bags: Option<u16>) -> Verdict {
    if !obs.ok() {
        return Verdict::ReadFailed;
    }
    let v = match uword_from_payload(&obs.data) {
        Ok(v) => v,
        Err(why) => return Verdict::Undetermined(why),
    };
    if v == 0 {
        return Verdict::NotExist;
    }
    // 手册：n = channel assigned to mode group n，n 上界 numBAGs。
    // numBAGs 未读到时**不猜具体上界**，只用粗筛上界 32 并把来源标注清楚。
    let upper = num_bags.unwrap_or(32);
    if v > upper {
        return Verdict::Undetermined(format!(
            "值 {v} 超过方式组上界 {upper}（numBAGs 来源：{}）",
            if num_bags.is_some() {
                "device-observed"
            } else {
                "未知，粗筛 32"
            }
        ));
    }
    Verdict::Assigned(v)
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "只读真实机床 maxnumChannels/chanAssignment 验证脚手架；需显式 --ignored 运行"]
async fn live_chan_assignment_probe() {
    let cfg = cfg_from_env();
    println!("=== 阶段 0：会话建立（只读，直连机床，不经 mesad）===");
    println!(
        "[{}] 目标 {}:{} family={} local_tsap={} remote_tsap={} 请求 pdu={} max_read_items={} timeout_ms={}",
        utc_stamp(),
        cfg.host,
        cfg.port,
        cfg.family,
        cfg.local_tsap,
        cfg.remote_tsap,
        cfg.requested_pdu_length,
        cfg.max_read_items,
        cfg.timeout_ms
    );

    // =================================================================
    // 阶段 1（生产链路）：设备环境 + AXIS_NAME 基准
    // =================================================================
    let mut client = match NckClient::connect(&cfg).await {
        Ok(c) => c,
        Err(e) => {
            // 连接失败即终止：保留真实错误，不换端口/TSAP 重试。
            println!(
                "[{}] CONNECT_FAILED code={} message={}",
                utc_stamp(),
                e.code,
                e.message
            );
            panic!("连接失败：{} {}", e.code, e.message);
        }
    };
    println!(
        "[{}] 已连接（生产链路 NckClient），协商 pdu={}（只读）",
        utc_stamp(),
        client.negotiated_pdu_length()
    );

    println!("\n=== 阶段 1A：设备型号/软件版本（N 区，module 沿用假设 26）===");
    let nck_type = observe(
        &mut client,
        n_area_addr(NCK_TYPE_COLUMN, 1, N_AREA_MODULE_ASSUMED, "U16_LE"),
    )
    .await;
    println!("{}", nck_type.line("nckType col=31 line=1 m26"));
    let nck_version = observe(
        &mut client,
        n_area_addr(NCK_VERSION_COLUMN, 1, N_AREA_MODULE_ASSUMED, "F64_LE"),
    )
    .await;
    println!("{}", nck_version.line("nckVersion col=32 line=1 m26"));

    println!("\n=== 阶段 1B：AXIS_NAME 基准（N 区 col=10000，生产探针常量）===");
    let mut axis_ok = 0usize;
    let mut axis_names = Vec::new();
    for axis in 1..=8u16 {
        let obs = observe_spec(&mut client, AXIS_NAME, axis).await;
        println!("{}", obs.line(&format!("AXIS_NAME line={axis}")));
        if obs.ok() {
            axis_ok += 1;
            if let Some(bytes) = &obs.data {
                axis_names.push(format!(
                    "{axis}:{}",
                    String::from_utf8_lossy(bytes).trim_end_matches('\0').trim()
                ));
            }
        }
    }
    println!(
        "    → AXIS_NAME 基准 {axis_ok}/8 通过；轴名={}",
        axis_names.join(", ")
    );
    if axis_ok < 8 {
        // 基准读不到就停止：否则后续失败无法区分"地址错"与"链路坏"。
        panic!("AXIS_NAME 基准未全部通过（{axis_ok}/8），按任务要求停止后续步骤");
    }

    println!("\n=== 阶段 1C：maxnumChannels（N col=4 line=1，声明 TYPE_UWORD）===");
    for module in std::iter::once(N_AREA_MODULE_ASSUMED).chain(N_AREA_MODULE_FALLBACKS) {
        let note = if module == N_AREA_MODULE_ASSUMED {
            "沿用假设"
        } else {
            "备选（逐个尝试，非盲扫）"
        };
        let obs = observe(
            &mut client,
            n_area_addr(MAXNUM_CHANNELS_COLUMN, 1, module, "U16_LE"),
        )
        .await;
        println!(
            "{}",
            obs.line(&format!("maxnumChannels m{module} [{note}]"))
        );
    }

    println!("\n=== 阶段 1D：chanAssignment line=1 预检（N col=21）===");
    let pre = observe(
        &mut client,
        n_area_addr(CHAN_ASSIGNMENT_COLUMN, 1, N_AREA_MODULE_ASSUMED, "U16_LE"),
    )
    .await;
    println!("{}", pre.line("chanAssignment line=1 预检"));

    println!("\n=== 阶段 1E：通道存在性对照（C/<n>/S/progStatus col=13 m127）===");
    let mut cross: Vec<(u16, Option<u16>, Option<u8>)> = Vec::new();
    for ch in 1..=CROSS_CHECK_MAX {
        let obs = observe(
            &mut client,
            c_area_channel_addr(ch, PROG_STATUS_COLUMN, PROG_STATUS_MODULE, "U16_LE"),
        )
        .await;
        let val = match &obs.data {
            Some(b) if b.len() == 2 => Some(u16::from_le_bytes([b[0], b[1]])),
            _ => None,
        };
        println!("{}", obs.line(&format!("C/{ch} progStatus")));
        cross.push((ch, val, obs.return_code));
    }
    drop(client);

    // =================================================================
    // 阶段 2（原始字节链路）：完整留存响应字节 + 两轮有界扫描
    // =================================================================
    println!("\n=== 阶段 2：原始字节复核（S7Session::read_var，不丢弃字节）===");
    let mut session = match S7Session::connect(cfg.to_transport()).await {
        Ok(s) => s,
        Err(e) => {
            println!(
                "[{}] RAW_CONNECT_FAILED code={} message={}",
                utc_stamp(),
                e.code,
                e.message
            );
            panic!("原始字节链路连接失败：{} {}", e.code, e.message);
        }
    };
    println!(
        "[{}] 已连接（原始字节链路），协商 pdu={}（只读）",
        utc_stamp(),
        session.negotiated_pdu_length()
    );

    let show_raw = |label: &str, r: Result<RawObs, String>| match r {
        Ok(o) => {
            println!("{}", o.line(label));
            Some(o)
        }
        Err(e) => {
            println!("[{}] {label:<34} {e}", utc_stamp());
            None
        }
    };

    let raw_type = show_raw(
        "nckType col=31 line=1 m26",
        raw_read(
            &mut session,
            n_area_addr(NCK_TYPE_COLUMN, 1, N_AREA_MODULE_ASSUMED, "U16_LE"),
        )
        .await,
    );
    if let Some(o) = &raw_type {
        println!(
            "    → nckType 载荷 {} 字节，uword 解出={:?}（手册 p1072：16000 = 828D）",
            o.data.len(),
            uword_from_payload(&o.data)
        );
    }
    let raw_ver = show_raw(
        "nckVersion col=32 line=1 m26",
        raw_read(
            &mut session,
            n_area_addr(NCK_VERSION_COLUMN, 1, N_AREA_MODULE_ASSUMED, "F64_LE"),
        )
        .await,
    );
    if let Some(o) = &raw_ver {
        match o.data.len() {
            8 => {
                let b = &o.data;
                let v = f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
                println!(
                    "    → nckVersion 载荷 8 字节，f64(LE)={v}（手册 p1073：整数部分即发布号）"
                );
            }
            4 => {
                let b = &o.data;
                let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                println!(
                    "    → nckVersion 载荷 4 字节，f32(LE)={v}（与手册 TYPE_DOUBLE 不符，结果待核实）"
                );
            }
            n => println!("    → nckVersion 载荷 {n} 字节，无法按 TYPE_DOUBLE 解出（结果待核实）"),
        }
    }
    let raw_maxnum = show_raw(
        "maxnumChannels col=4 line=1 m26",
        raw_read(
            &mut session,
            n_area_addr(MAXNUM_CHANNELS_COLUMN, 1, N_AREA_MODULE_ASSUMED, "U16_LE"),
        )
        .await,
    );
    let maxnum: Option<u16> = raw_maxnum
        .as_ref()
        .filter(|o| o.ok())
        .and_then(|o| uword_from_payload(&o.data).ok());
    match maxnum {
        Some(v) => println!(
            "    → maxnumChannels={v}（device-observed）；扫描上界 = min({v}, {LINE_CAP}) = {}",
            v.min(LINE_CAP)
        ),
        None => println!(
            "    → maxnumChannels 未能解出（载荷/返回码见上）：**不盲扫**，本轮只做 line=1 定点观测"
        ),
    }
    if maxnum.map(|m| m > LINE_CAP).unwrap_or(false) {
        println!(
            "    !! maxnumChannels={} > {LINE_CAP}：枚举**不完整**（驱动/任务上限 31），不得声称扫描完成",
            maxnum.unwrap()
        );
    }
    let raw_numbags = show_raw(
        "numBAGs col=3 line=1 m26",
        raw_read(
            &mut session,
            n_area_addr(NUM_BAGS_COLUMN, 1, N_AREA_MODULE_ASSUMED, "U16_LE"),
        )
        .await,
    );
    let num_bags: Option<u16> = raw_numbags
        .as_ref()
        .filter(|o| o.ok())
        .and_then(|o| uword_from_payload(&o.data).ok());
    println!("    → numBAGs={num_bags:?}（读不到则不猜上界，判定标注待核实）");

    // 扫描上界：maxnumChannels 读不到时只做 line=1 定点观测，绝不盲扫 31 个索引。
    let limit = maxnum.map(|m| m.min(LINE_CAP)).unwrap_or(1);
    println!(
        "\n=== 阶段 2A：chanAssignment（N col=21，line=1..={limit}，module={N_AREA_MODULE_ASSUMED}）==="
    );
    let mut rounds: Vec<Vec<(u16, Verdict)>> = Vec::new();
    for round in 1..=2u8 {
        println!("--- 第 {round} 轮 ---");
        let mut results = Vec::new();
        for line in 1..=limit {
            let r = raw_read(
                &mut session,
                n_area_addr(
                    CHAN_ASSIGNMENT_COLUMN,
                    line,
                    N_AREA_MODULE_ASSUMED,
                    "U16_LE",
                ),
            )
            .await;
            match r {
                Ok(o) => {
                    let verdict = verdict_of(&o, num_bags);
                    println!(
                        "{} | verdict={}",
                        o.line(&format!("chanAssignment R{round} line={line}")),
                        verdict_str(&verdict)
                    );
                    results.push((line, verdict));
                }
                Err(e) => {
                    println!(
                        "[{}] chanAssignment R{round} line={line}  {e} | verdict=READ_FAILED",
                        utc_stamp()
                    );
                    results.push((line, Verdict::ReadFailed));
                }
            }
        }
        rounds.push(results);
    }

    println!("\n--- 两轮一致性 ---");
    let (a, b) = (&rounds[0], &rounds[1]);
    let same = a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y);
    for (x, y) in a.iter().zip(b.iter()) {
        println!(
            "    line={:<3} 第1轮={:<26} 第2轮={:<26} {}",
            x.0,
            verdict_str(&x.1),
            verdict_str(&y.1),
            if x == y { "一致" } else { "不一致" }
        );
    }
    println!(
        "    → 两轮{}",
        if same {
            "完全一致"
        } else {
            "存在差异（见上）"
        }
    );

    // =================================================================
    // 阶段 3：配置对照 + 汇总
    // =================================================================
    println!("\n--- chanAssignment（第 1 轮）与 progStatus 可读性对照 ---");
    for (line, verdict) in &rounds[0] {
        if *line > CROSS_CHECK_MAX {
            continue;
        }
        let (prog_val, prog_ret) = cross
            .iter()
            .find(|(c, _, _)| c == line)
            .map(|(_, v, r)| (*v, *r))
            .unwrap_or((None, None));
        let prog_ok = prog_val.is_some();
        let consistent = match verdict {
            Verdict::Assigned(_) => prog_ok,
            Verdict::NotExist => !prog_ok,
            _ => false,
        };
        println!(
            "    line={:<3} chanAssignment={:<24} progStatus={:<18} {}",
            line,
            verdict_str(verdict),
            match (prog_val, prog_ret) {
                (Some(v), _) => format!("可读 U32({v})"),
                (None, Some(r)) => format!("拒绝 ret=0x{r:02X}"),
                (None, None) => "失败".to_string(),
            },
            if consistent {
                "一致"
            } else {
                "不一致/不可判定"
            }
        );
    }

    println!("\n=== 汇总：四类判定分布（第 1 轮）===");
    let mut assigned = Vec::new();
    let mut not_exist = Vec::new();
    let mut failed = Vec::new();
    let mut undetermined = Vec::new();
    for (line, v) in &rounds[0] {
        match v {
            Verdict::Assigned(n) => assigned.push(format!("{line}→BAG{n}")),
            Verdict::NotExist => not_exist.push(line.to_string()),
            Verdict::ReadFailed => failed.push(line.to_string()),
            Verdict::Undetermined(_) => undetermined.push(line.to_string()),
        }
    }
    println!("    ASSIGNED    : {}", join_or_dash(&assigned));
    println!("    NOT_EXIST   : {}", join_or_dash(&not_exist));
    println!("    READ_FAILED : {}", join_or_dash(&failed));
    println!("    UNDETERMINED: {}", join_or_dash(&undetermined));
    if not_exist.is_empty() {
        println!(
            "    !! 本次没有观察到 \"返回 0 = 通道不存在\" 的索引：该分类**未实测**，不得声称五项验证全部完成"
        );
    }
    println!("\n<<< 验证结束（只读：未写参数、未发控制、未启动采集任务）");
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "—".to_string()
    } else {
        items.join(", ")
    }
}
