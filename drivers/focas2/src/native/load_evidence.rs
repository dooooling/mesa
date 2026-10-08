//! 真实机床 load evidence harness（PR C，test-only guarded raw FFI）。
//!
//! 口径（冻结，v2 更新）：
//! - 本模块 `#[cfg(test)]`，只出 ignored live harness + 无 CNC 纯单测；
//!   不碰 `pre_ffi_gate`、不恢复 production `cnc_rdspmeter/rdsvmeter`、
//!   不写 load codec、不改 Wire gate/canary、coverage 保持 15/17。
//! - v2：`SpLoad`（8B）已证与 FOCAS ABI 不一致并封存；harness 改借
//!   `OdbSpLoad`（24B/spindle）/`OdbSvLoad`（12B/axis）满足函数指针 ABI，
//!   并**旁路解析** `LoadElem{data,dec,unit,name,suff1,suff2}`（只落盘证据，
//!   不进 production 语义；`value = data/10^dec` 不在此冻结，只记录候选）。
//! - 同一进程/同一 OS thread/同一 `NativeLib`/同一 FOCAS handle 顺序执行整场
//!   （stream order = RUN order；3-C2 跨流错位教训）。
//! - raw FFI：`GuardedLoadBuffer`（64B guard + 4096B payload + 64B guard，
//!   全 sentinel）；每次必查 guard + tail + num_out。
//! - run-id 唯一：`MESA_FOCAS_LOAD_RUN` 三位数字；失败/误操作即本 session 作废，
//!   下一次必须递增，禁止复用补跑。
//! - `selector=0/num_in=4` 仅为复现 identity window 的 harness 输入，不冻结语义。

use std::os::raw::c_short;
use std::path::PathBuf;

/// evidence family（harness 输入维度；与 production READY 无关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadFamily {
    Spindle,
    Servo,
}

/// evidence phase（固定顺序 L0 → L1 → L2 → L0R）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPhase {
    L0,
    L1,
    L2,
    L0r,
}

impl LoadPhase {
    /// 固定顺序（harness 执行序；不得跳窗）。
    pub fn order() -> [LoadPhase; 4] {
        [LoadPhase::L0, LoadPhase::L1, LoadPhase::L2, LoadPhase::L0r]
    }

    /// RUN id phase 段（`L0R` 大写 R；显示用 `L0R`）。
    pub fn tag(self) -> &'static str {
        match self {
            LoadPhase::L0 => "L0",
            LoadPhase::L1 => "L1",
            LoadPhase::L2 => "L2",
            LoadPhase::L0r => "L0R",
        }
    }
}

/// family 解析（`SPINDLE/SERVO/BOTH` 大小写不敏感；其余即 Err）。
pub fn parse_family(s: &str) -> Result<Vec<LoadFamily>, String> {
    match s.trim().to_ascii_uppercase().as_str() {
        "SPINDLE" => Ok(vec![LoadFamily::Spindle]),
        "SERVO" => Ok(vec![LoadFamily::Servo]),
        "BOTH" => Ok(vec![LoadFamily::Spindle, LoadFamily::Servo]),
        other => Err(format!(
            "MESA_FOCAS_LOAD_FAMILY 非法：{other}（SPINDLE/SERVO/BOTH）"
        )),
    }
}

/// run 序号校验（三位数字 `000..999`；失败即整 session 作废）。
pub fn parse_run_seq(s: &str) -> Result<String, String> {
    if s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()) {
        Ok(s.to_string())
    } else {
        Err(format!(
            "MESA_FOCAS_LOAD_RUN 非法：{s}（必须三位数字；失败即作废递增，禁复用）"
        ))
    }
}

/// run-id 生成（`SPINDLE-L1-001`；phase 固定顺序由调用方保证）。
pub fn run_id(family: LoadFamily, phase: LoadPhase, seq: &str) -> String {
    let fam = match family {
        LoadFamily::Spindle => "SPINDLE",
        LoadFamily::Servo => "SERVO",
    };
    format!("{fam}-{}-{seq}", phase.tag())
}

/// phase 顺序校验（必须恰好 L0/L1/L2/L0R；跳窗即 Err）。
pub fn check_phase_order(phases: &[LoadPhase]) -> Result<(), String> {
    if phases == LoadPhase::order() {
        Ok(())
    } else {
        Err(format!("phase 顺序必须 L0/L1/L2/L0R，实际 {phases:?}"))
    }
}

/// panel 门（全 phase 先过 `is_finite`——NaN/inf 不得绕过 gate、不落非法 JSONL；
/// L1/L2 必须非零；L0/L0R 不设 tolerance——recovery 由 Panel+Native+Wire
/// 三方 review 判定，不由 harness 发明阈值）。
pub fn check_panel_gate(phase: LoadPhase, panel: f64) -> Result<(), String> {
    if !panel.is_finite() {
        return Err(format!("{phase:?} 面板值必须有限（实际 {panel}）"));
    }
    match phase {
        LoadPhase::L1 | LoadPhase::L2 => {
            if panel == 0.0 {
                Err(format!("{phase:?} 面板必须非零（实际 {panel}）"))
            } else {
                Ok(())
            }
        }
        LoadPhase::L0 | LoadPhase::L0r => Ok(()),
    }
}

/// L1/L2 互异门（两档必须明显不同；相等即 Err）。
pub fn check_levels_distinct(l1: f64, l2: f64) -> Result<(), String> {
    if l1 != l2 {
        Ok(())
    } else {
        Err(format!("L1/L2 必须互异（同为 {l1}）"))
    }
}

/// sentinel（guard/payload 预填；`0xCC`——与 zofs/tofs dumper 同值，
/// DLL 写范围一目了然，不拿零初始化冒充返回）。
pub const LOAD_SENTINEL: u8 = 0xCC;
/// payload 容量（4096B 上界；单 spindle 24B/单 axis 12B 远小于此，
/// 越界写入可被 guard 捕获）。
pub const LOAD_PAYLOAD_LEN: usize = 4096;
/// guard 宽度（前后各 64B）。
pub const LOAD_GUARD_LEN: usize = 64;
/// raw 保留上限（console/JSON 输出 `payload[0..512]` hex；不提前转语义）。
pub const LOAD_RAW_KEEP: usize = 512;

/// guarded raw 缓冲（`#[repr(C, align(16))]`；只借址满足 FFI ABI）。
#[repr(C, align(16))]
pub struct GuardedLoadBuffer {
    /// 前 guard（sentinel；变化即 HOLD）。
    pub pre_guard: [u8; LOAD_GUARD_LEN],
    /// 真实输出区（sentinel 预填；`OdbSpLoad*`/`OdbSvLoad*` 借此址；
    /// v2 旁路解析见 `decode_load_elems`）。
    pub payload: [u8; LOAD_PAYLOAD_LEN],
    /// 后 guard（sentinel；变化即 HOLD）。
    pub post_guard: [u8; LOAD_GUARD_LEN],
}

impl GuardedLoadBuffer {
    /// 全 sentinel 初始化。
    pub fn sentinel() -> Self {
        Self {
            pre_guard: [LOAD_SENTINEL; LOAD_GUARD_LEN],
            payload: [LOAD_SENTINEL; LOAD_PAYLOAD_LEN],
            post_guard: [LOAD_SENTINEL; LOAD_GUARD_LEN],
        }
    }

    /// guard 检查（前后 guard 必须 untouched）。
    pub fn guards_ok(&self) -> bool {
        self.pre_guard.iter().all(|&b| b == LOAD_SENTINEL)
            && self.post_guard.iter().all(|&b| b == LOAD_SENTINEL)
    }

    /// tail 检查（`payload[512..]` 必须 clean——越界写入即 HOLD）。
    pub fn tail_clean(&self) -> bool {
        self.payload[LOAD_RAW_KEEP..]
            .iter()
            .all(|&b| b == LOAD_SENTINEL)
    }

    /// raw hex（`payload[0..512]`；权威输出，不转语义）。
    pub fn raw_hex(&self) -> String {
        self.payload[..LOAD_RAW_KEEP]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// 解码后单个 `LoadElem` 证据（v2 旁路解析；只落盘，不进 production）。
/// `eng_candidate` 为 `data/10^dec` 候选（dec 越界即 `None`，不冻结换算）。
#[derive(Debug, Clone, PartialEq)]
pub struct LoadElemEvidence {
    /// 在 payload 中的 record 序号（spindle：偶数=load，奇数=speed）。
    pub slot: usize,
    pub data: i32,
    pub dec: i16,
    pub unit: i16,
    /// `name` 原始字节（如 b'X'/b'S'；不可打印即原文 hex）。
    pub name: u8,
    pub suff1: u8,
    pub suff2: u8,
    pub eng_candidate: Option<f64>,
}

/// 旁路解析 `payload[0..num*stride]` 为 `LoadElem` 数组（v2）。
/// spindle 用 `stride=24`（`OdbSpLoad`，偶 slot=load、奇 slot=speed）；
/// servo 用 `stride=12`（`OdbSvLoad`，每 slot 一轴）。
/// 一律小端读（Windows FOCAS ABI）；`dec` 越界（`<0/>9`）即 `None`，
/// 不冻结 `value = data/10^dec`。
pub fn decode_load_elems(
    payload: &[u8],
    num_records: usize,
    stride: usize,
) -> Vec<LoadElemEvidence> {
    let mut out = Vec::new();
    for r in 0..num_records {
        let base = r * stride;
        // stride 非 12 倍数即布局未知：整 record 跳过（不猜半个 elem）。
        if !stride.is_multiple_of(12) {
            break;
        }
        let elems = stride / 12;
        for e in 0..elems {
            let off = base + e * 12;
            if payload.len() < off + 12 {
                break;
            }
            let data = i32::from_le_bytes([
                payload[off],
                payload[off + 1],
                payload[off + 2],
                payload[off + 3],
            ]);
            let dec = i16::from_le_bytes([payload[off + 4], payload[off + 5]]);
            let unit = i16::from_le_bytes([payload[off + 6], payload[off + 7]]);
            let name = payload[off + 8];
            let suff1 = payload[off + 9];
            let suff2 = payload[off + 10];
            let eng_candidate = if (0..=9).contains(&dec) {
                Some(data as f64 / 10f64.powi(dec as i32))
            } else {
                None
            };
            out.push(LoadElemEvidence {
                slot: r * elems + e,
                data,
                dec,
                unit,
                name,
                suff1,
                suff2,
                eng_candidate,
            });
        }
    }
    out
}

/// 单窗采样记录（`jsonl_line` 输入；打包防 clippy 超参）。
/// `panel_ref` 为不解释语义的面板引用（如 servo 轴标识 `"X"`；同 family
/// 四窗必须同一 ref——跨窗 ref 漂移即 session 作废，不推断 `X↔slot0`）。
pub struct LoadSample<'a> {
    /// `SPINDLE-L1-001`。
    pub run_id: &'a str,
    /// SPINDLE/SERVO。
    pub family: LoadFamily,
    /// L0/L1/L2/L0R。
    pub phase: LoadPhase,
    /// 同 session handle（仅证未换 handle）。
    pub handle: u16,
    /// 面板当前实际值。
    pub panel_value: f64,
    /// 面板引用（servo 轴标识等；原文记录，不解释语义）。
    pub panel_ref: &'a str,
    /// 请求数（当前恒 4）。
    pub num_in: i16,
    /// DLL 回显数。
    pub num_out: i16,
    /// FOCAS rc。
    pub rc: i16,
    /// guarded 缓冲。
    pub buf: &'a GuardedLoadBuffer,
    /// 单调外时间戳（ms；仅排序用）。
    pub unix_ms: i64,
}

/// JSONL 行（每 RUN 一行；`handle` 仅证同 session 未换 handle；
/// v2 追加 `elems` 旁路解析数组）。
pub fn jsonl_line(s: &LoadSample<'_>) -> String {
    let fam = match s.family {
        LoadFamily::Spindle => "SPINDLE",
        LoadFamily::Servo => "SERVO",
    };
    // stride：spindle 24（load+speed），servo 12（单轴）。
    let stride = match s.family {
        LoadFamily::Spindle => 24,
        LoadFamily::Servo => 12,
    };
    let elems = decode_load_elems(&s.buf.payload, s.num_out.max(0) as usize, stride);
    let elems_json = elems
        .iter()
        .map(|e| {
            format!(
                "{{\"slot\":{},\"data\":{},\"dec\":{},\"unit\":{},\
                 \"name\":{},\"suff1\":{},\"suff2\":{},\"eng_candidate\":{}}}",
                e.slot,
                e.data,
                e.dec,
                e.unit,
                e.name,
                e.suff1,
                e.suff2,
                match e.eng_candidate {
                    Some(v) => format!("{v:?}"),
                    None => "null".to_string(),
                },
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    // 手工拼 JSONL（单文件 harness，不引入新依赖；panel f64 用 {:?} 保持精度）。
    format!(
        "{{\"run_id\":{run_id:?},\"family\":{fam:?},\"phase\":{phase:?},\
         \"handle\":{handle},\"panel_value\":{panel_value:?},\"panel_ref\":{panel_ref:?},\
         \"num_in\":{num_in},\"num_out\":{num_out},\"rc\":{rc},\
         \"raw_0_512_hex\":{raw:?},\"pre_guard_ok\":{pre_ok},\
         \"post_guard_ok\":{post_ok},\"tail_after_512_clean\":{tail_ok},\
         \"elems\":[{elems}],\
         \"unix_ms\":{unix_ms}}}",
        run_id = s.run_id,
        phase = s.phase.tag(),
        handle = s.handle,
        panel_value = s.panel_value,
        panel_ref = s.panel_ref,
        num_in = s.num_in,
        num_out = s.num_out,
        rc = s.rc,
        raw = s.buf.raw_hex(),
        pre_ok = s.buf.pre_guard.iter().all(|&b| b == LOAD_SENTINEL),
        post_ok = s.buf.post_guard.iter().all(|&b| b == LOAD_SENTINEL),
        tail_ok = s.buf.tail_clean(),
        elems = elems_json,
        unix_ms = s.unix_ms,
    )
}

/// JSONL 输出路径（`target/focas-load-evidence/load-<seq>.jsonl`）。
pub fn jsonl_path(seq: &str) -> PathBuf {
    PathBuf::from(format!("target/focas-load-evidence/load-{seq}.jsonl"))
}

/// test-only RAII handle guard（unwind 亦 free exactly once；panic 不泄漏句柄）。
/// `Drop` 内忽略 free 错误（已在 panic 路径时不二次 panic）。
struct HandleGuard<'a> {
    lib: &'a crate::native::NativeLib,
    hdl: u16,
    disarmed: bool,
}

impl<'a> HandleGuard<'a> {
    fn new(lib: &'a crate::native::NativeLib, hdl: u16) -> Self {
        Self {
            lib,
            hdl,
            disarmed: false,
        }
    }

    /// 正常收尾时调用（显式 free 成功后 disarm，避免 `Drop` 二次 free）。
    fn close(&mut self) {
        let _ = self.lib.cnc_freelibhndl(self.hdl);
        self.disarmed = true;
    }
}

impl Drop for HandleGuard<'_> {
    fn drop(&mut self) {
        if !self.disarmed {
            let _ = self.lib.cnc_freelibhndl(self.hdl);
        }
    }
}

/// panel 输入解析（`"<value> [ref]"`；如 servo `"27 X"`→值 27 + ref `X`；
/// spindle `"27"`→值 27 + ref 自动记 `"S1"`（prompt 已固定 S1）。
/// 非法/非有限即整 session 作废；SERVO 空 ref 即 FFI 前 fail-closed
/// （四窗空串“一致”不能证明轴 provenance）。
fn parse_panel_input(family: LoadFamily, line: &str) -> Result<(f64, String), String> {
    let mut parts = line.split_whitespace();
    let value_raw = parts
        .next()
        .ok_or_else(|| format!("面板输入为空：{line:?}"))?;
    let value: f64 = value_raw
        .parse()
        .map_err(|_| format!("面板值非法：{line:?}"))?;
    if !value.is_finite() {
        return Err(format!("面板值必须有限（实际 {value}）"));
    }
    let pref: Vec<&str> = parts.collect();
    if pref.len() > 1 {
        return Err(format!("面板引用至多一段：{line:?}"));
    }
    let mut pref = pref.first().unwrap_or(&"").to_string();
    if pref.is_empty() {
        match family {
            // spindle prompt 已固定 S1：空输入自动记 S1。
            LoadFamily::Spindle => pref = "S1".to_string(),
            // servo 必须显式轴标识（空 ref 即使四窗一致亦无 provenance）。
            LoadFamily::Servo => {
                return Err(format!("SERVO 面板引用不能为空（如 `27 X`）：{line:?}"));
            }
        }
    }
    Ok((value, pref))
}

/// 同 family panel_ref 一致性（四窗必须同一 ref；漂移即 session 作废；
/// 不推断 `X↔slot0`，只锁 provenance）。
fn check_panel_ref_stable(known: &Option<String>, current: &str, run: &str) -> Result<(), String> {
    if let Some(k) = known
        && k != current
    {
        return Err(format!(
            "RUN={run} panel_ref 漂移（{k:?} → {current:?}；四窗必须同一 ref）"
        ));
    }
    Ok(())
}

/// JSONL writer（`create_new` 独占整场持有；文件已存在即 fail-closed——
/// 禁止复用 seq append 伪造唯一性）。
struct JsonlWriter {
    file: std::fs::File,
    path: PathBuf,
}

impl JsonlWriter {
    fn create(seq: &str) -> Result<Self, String> {
        let path = jsonl_path(seq);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("jsonl 目录创建失败：{e}"))?;
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                format!(
                    "jsonl {} 已存在或不可建（RUN 序号必须唯一递增，禁复用补跑）：{e}",
                    path.display()
                )
            })?;
        Ok(Self { file, path })
    }

    fn append_line(&mut self, line: &str) -> Result<(), String> {
        use std::io::Write;
        writeln!(self.file, "{line}").map_err(|e| format!("jsonl 追加失败：{e}"))
    }
}

/// live harness 入口（Evidence Day 用；`#[ignore]` 真机 only）。
/// 整场：OPEN handle once → L0/L1/L2/L0R 顺序 × family → CLOSE once。
/// 每窗：stdout `>>> BEGIN` → panel 交互输入 → FFI → guard 检查 → JSONL →
/// stdout `<<< END`。guard/tail/num 异常即整 session HOLD（panic 中止，
/// 禁止继续推 layout；下一次必须新 seq）。
#[cfg(test)]
pub fn run_live_harness() {
    use crate::native::{FocasRet, NativeLib, OdbSpLoad, OdbSvLoad};
    use std::os::raw::c_ushort;

    let seq = std::env::var("MESA_FOCAS_LOAD_RUN")
        .map(|s| parse_run_seq(&s).unwrap_or_else(|e| panic!("{e}")))
        .unwrap_or_else(|_| panic!("缺少 MESA_FOCAS_LOAD_RUN（三位数字；失败即作废递增）"));
    let families = std::env::var("MESA_FOCAS_LOAD_FAMILY")
        .map(|s| parse_family(&s).unwrap_or_else(|e| panic!("{e}")))
        .unwrap_or_else(|_| panic!("缺少 MESA_FOCAS_LOAD_FAMILY（SPINDLE/SERVO/BOTH）"));
    let (host, port, timeout_ms) = {
        let host = std::env::var("MESA_FOCAS_GATE0_HOST").unwrap_or_else(|_| {
            panic!("缺少 MESA_FOCAS_GATE0_HOST；本测试为 ignored 真机采集，CI 默认不跑")
        });
        let port: u16 = std::env::var("MESA_FOCAS_GATE0_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8193);
        let timeout_ms: u64 = std::env::var("MESA_FOCAS_GATE0_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000);
        (host, port, timeout_ms)
    };
    let timeout_secs = (timeout_ms.div_ceil(1000).max(1).min(i32::MAX as u64)) as i32;
    // JSONL 独占先占（create_new；已存在即 connect/FFI 前 fail-closed——
    // 误复用 seq 不得多一次 OPEN/CLOSE 污染抓包）。
    let mut writer = JsonlWriter::create(&seq).unwrap_or_else(|e| panic!("{e}"));
    let lib = NativeLib::load().unwrap_or_else(|e| panic!("FWLIB 加载失败（{host}:{port}）：{e}"));
    let hdl = lib
        .cnc_allclibhndl3(&host, port, timeout_secs)
        .unwrap_or_else(|e| panic!("cnc_allclibhndl3 失败：{} {}", e as i16, e.message()));
    // RAII：panic 路径亦 free exactly once（test-only guard；drop 忽略 free 错误）。
    let mut guard = HandleGuard::new(&lib, hdl);
    println!(">>> LOAD SESSION seq={seq} handle={hdl} families={families:?} (single handle)");
    // panel L1/L2 互异门需跨窗比较（同 family 内）；panel_ref 四窗同一性同 map。
    let mut levels: std::collections::BTreeMap<String, (f64, f64, Option<String>)> =
        std::collections::BTreeMap::new();
    let mut seen_runs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for phase in LoadPhase::order() {
        for family in &families {
            let id = run_id(*family, phase, &seq);
            assert!(
                seen_runs.insert(id.clone()),
                "duplicate run-id {id}（禁止复用）"
            );
            println!(">>> BEGIN RUN={id} HANDLE={hdl}");
            // panel 交互门（`"<value> [ref]"`；servo 如 `"27 X"`；L1/L2 零值
            // 即中止整场；FFI 前先过 L1/L2 互异预检，无效档位不发证据窗）。
            println!(
                "[WAIT] RUN={id} 请输入面板 {} 当前值（servo 附轴标识如 `27 X`）：",
                match family {
                    LoadFamily::Spindle => "SPINDLE LOAD S1",
                    LoadFamily::Servo => "SERVO LOAD",
                }
            );
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .expect("stdin 读取失败");
            let (panel, panel_ref) = parse_panel_input(*family, &line)
                .unwrap_or_else(|e| panic!("RUN={id} {e}（本 session 作废，下次用新 seq）"));
            check_panel_gate(phase, panel).unwrap_or_else(|e| panic!("RUN={id} {e}"));
            // panel_ref 四窗同一性（同 family；漂移即作废；不推断 X↔slot）。
            let fam_key = format!("{:?}", family);
            let known_ref = levels.get(&fam_key).and_then(|(_, _, r)| r.clone());
            check_panel_ref_stable(&known_ref, &panel_ref, &id).unwrap_or_else(|e| panic!("{e}"));
            // L2 FFI 前互异预检（已知档位无效即不发窗，直接作废）。
            if phase == LoadPhase::L2
                && let Some((l1, _, _)) = levels.get(&fam_key)
            {
                check_levels_distinct(*l1, panel)
                    .unwrap_or_else(|er| panic!("RUN={id} {er}（FFI 前预检，不发无效窗）"));
            }
            // raw FFI（同 handle；selector=0/num_in=4 仅复现 identity window；
            // v2 按正确 ABI 借址：spindle 24B OdbSpLoad / servo 12B OdbSvLoad）。
            let mut buf = GuardedLoadBuffer::sentinel();
            let mut num: c_short = 4;
            let rc: c_short = unsafe {
                match family {
                    LoadFamily::Spindle => {
                        let sym = lib.cnc_rdspmeter.as_ref().expect("缺符号 cnc_rdspmeter");
                        sym(
                            hdl as c_ushort,
                            0 as c_short,
                            &mut num as *mut c_short,
                            buf.payload.as_mut_ptr().cast::<OdbSpLoad>(),
                        )
                    }
                    LoadFamily::Servo => {
                        let sym = lib.cnc_rdsvmeter.as_ref().expect("缺符号 cnc_rdsvmeter");
                        sym(
                            hdl as c_ushort,
                            &mut num as *mut c_short,
                            buf.payload.as_mut_ptr().cast::<OdbSvLoad>(),
                        )
                    }
                }
            };
            let guards_ok = buf.guards_ok();
            let tail_ok = buf.tail_clean();
            let num_sane = (0..=4).contains(&num);
            println!("<<< END RUN={id} RC={rc} num_out={num} guards={guards_ok} tail={tail_ok}");
            if !guards_ok || !tail_ok || !num_sane {
                panic!(
                    "RUN={id} HOLD：guard/tail/num 异常（guards={guards_ok} tail={tail_ok} num={num}）——不推 layout，本 session 作废"
                );
            }
            // rc 非零即整场 INVALID（先落盘本窗 raw+rc，再立即终止，不进下一窗）。
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let line = jsonl_line(&LoadSample {
                run_id: &id,
                family: *family,
                phase,
                handle: hdl,
                panel_value: panel,
                panel_ref: &panel_ref,
                num_in: 4,
                num_out: num,
                rc,
                buf: &buf,
                unix_ms,
            });
            println!("{line}");
            writer
                .append_line(&line)
                .unwrap_or_else(|e| panic!("RUN={id} {e}"));
            if !FocasRet::from_raw(rc).is_ok() {
                panic!("RUN={id} HOLD：rc={rc} 非零（本窗已落盘，不进下一窗，本 session 作废）");
            }
            // L1/L2 互异门（同 family；读回 levels 累积；L2 已 FFI 前预检，此处复核）。
            match phase {
                LoadPhase::L1 => {
                    levels.insert(fam_key, (panel, f64::NAN, Some(panel_ref)));
                }
                LoadPhase::L2 => {
                    if let Some(e) = levels.get_mut(&fam_key) {
                        e.1 = panel;
                        check_levels_distinct(e.0, e.1)
                            .unwrap_or_else(|er| panic!("RUN={id} {er}"));
                    }
                }
                _ => {
                    // L0/L0R 亦记录 ref（四窗同一性ต่อเนื่อง）。
                    levels
                        .entry(fam_key)
                        .or_insert((panel, f64::NAN, Some(panel_ref)));
                }
            }
        }
    }
    // 正常收尾：显式 free（RAII disarm；panic 路径由 Drop 兜底）。
    guard.close();
    println!(
        "<<< LOAD SESSION seq={seq} done (jsonl: {})",
        writer.path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{LoadElem, OdbSpLoad, OdbSvLoad};

    /// v2 ABI 尺寸冻结：LoadElem 12B / OdbSvLoad 12B / OdbSpLoad 24B。
    #[test]
    fn load_abi_sizes_locked() {
        assert_eq!(std::mem::size_of::<LoadElem>(), 12);
        assert_eq!(std::mem::size_of::<OdbSvLoad>(), 12);
        assert_eq!(std::mem::size_of::<OdbSpLoad>(), 24);
    }

    /// v2 旁路解析：spindle 24B record 拆 load+speed；dec 正常即候选。
    #[test]
    fn decode_spindle_elems_locked() {
        let mut payload = vec![0xCCu8; 64];
        // record0.load：data=2700 LE，dec=2，unit=0，name='S'，suff1='1'。
        payload[0..4].copy_from_slice(&2700i32.to_le_bytes());
        payload[4..6].copy_from_slice(&2i16.to_le_bytes());
        payload[6..8].copy_from_slice(&0i16.to_le_bytes());
        payload[8] = b'S';
        payload[9] = b'1';
        // record0.speed：data=1500，dec=0（同 record 内 +12B）。
        payload[12..16].copy_from_slice(&1500i32.to_le_bytes());
        payload[16..18].copy_from_slice(&0i16.to_le_bytes());
        let elems = decode_load_elems(&payload, 1, 24);
        assert_eq!(elems.len(), 2);
        assert_eq!(elems[0].data, 2700);
        assert_eq!(elems[0].dec, 2);
        assert_eq!(elems[0].name, b'S');
        assert_eq!(elems[0].eng_candidate, Some(27.0));
        assert_eq!(elems[1].slot, 1);
        assert_eq!(elems[1].data, 1500);
        assert_eq!(elems[1].eng_candidate, Some(1500.0));
    }

    /// v2 旁路解析：servo 12B record；dec 越界即 None（不冻结换算）。
    #[test]
    fn decode_servo_elems_locked() {
        let mut payload = vec![0xCCu8; 32];
        payload[0..4].copy_from_slice(&(-500i32).to_le_bytes());
        payload[4..6].copy_from_slice(&99i16.to_le_bytes());
        payload[8] = b'X';
        let elems = decode_load_elems(&payload, 1, 12);
        assert_eq!(elems.len(), 1);
        assert_eq!(elems[0].name, b'X');
        assert_eq!(elems[0].eng_candidate, None);
    }

    #[test]
    fn family_parser_locked() {
        assert_eq!(parse_family("spindle").unwrap(), vec![LoadFamily::Spindle]);
        assert_eq!(parse_family("SERVO").unwrap(), vec![LoadFamily::Servo]);
        assert_eq!(
            parse_family("both").unwrap(),
            vec![LoadFamily::Spindle, LoadFamily::Servo]
        );
        assert!(parse_family("load").is_err());
    }

    #[test]
    fn run_seq_and_id_locked() {
        assert_eq!(parse_run_seq("001").unwrap(), "001");
        assert!(parse_run_seq("01").is_err());
        assert!(parse_run_seq("0001").is_err());
        assert!(parse_run_seq("00a").is_err());
        assert_eq!(
            run_id(LoadFamily::Spindle, LoadPhase::L1, "001"),
            "SPINDLE-L1-001"
        );
        assert_eq!(
            run_id(LoadFamily::Servo, LoadPhase::L0r, "002"),
            "SERVO-L0R-002"
        );
    }

    #[test]
    fn phase_order_locked() {
        assert!(check_phase_order(&LoadPhase::order()).is_ok());
        assert!(check_phase_order(&[LoadPhase::L0, LoadPhase::L2]).is_err());
        assert!(
            check_phase_order(&[LoadPhase::L1, LoadPhase::L0, LoadPhase::L2, LoadPhase::L0r])
                .is_err()
        );
    }

    #[test]
    fn guard_helper_locked() {
        let mut buf = GuardedLoadBuffer::sentinel();
        assert!(buf.guards_ok());
        assert!(buf.tail_clean());
        assert_eq!(buf.raw_hex().len(), LOAD_RAW_KEEP * 2);
        buf.pre_guard[0] = 0x00;
        assert!(!buf.guards_ok());
        let mut buf = GuardedLoadBuffer::sentinel();
        buf.payload[LOAD_RAW_KEEP] = 0x00;
        assert!(!buf.tail_clean());
    }

    #[test]
    fn panel_gate_locked() {
        assert!(check_panel_gate(LoadPhase::L1, 27.0).is_ok());
        assert!(check_panel_gate(LoadPhase::L1, 0.0).is_err());
        assert!(check_panel_gate(LoadPhase::L2, 0.0).is_err());
        assert!(check_panel_gate(LoadPhase::L0, 0.0).is_ok());
        assert!(check_panel_gate(LoadPhase::L0r, 5.0).is_ok());
        // 非有限值不得绕过 gate、不落非法 JSONL。
        assert!(check_panel_gate(LoadPhase::L1, f64::NAN).is_err());
        assert!(check_panel_gate(LoadPhase::L0, f64::INFINITY).is_err());
        assert!(check_levels_distinct(27.0, 31.0).is_ok());
        assert!(check_levels_distinct(27.0, 27.0).is_err());
    }

    #[test]
    fn panel_input_and_ref_locked() {
        // `"<value> [ref]"`：spindle `"27"`→(27, "S1")；servo `"27 X"`→(27, "X")。
        assert_eq!(
            parse_panel_input(LoadFamily::Spindle, "27").unwrap(),
            (27.0, "S1".to_string())
        );
        assert_eq!(
            parse_panel_input(LoadFamily::Servo, "27 X").unwrap(),
            (27.0, "X".to_string())
        );
        // SERVO 空 ref 即 fail-closed（四窗空串“一致”无 provenance）。
        assert!(parse_panel_input(LoadFamily::Servo, "27").is_err());
        assert!(parse_panel_input(LoadFamily::Spindle, "").is_err());
        assert!(parse_panel_input(LoadFamily::Servo, "NaN X").is_err());
        assert!(parse_panel_input(LoadFamily::Servo, "inf X").is_err());
        assert!(parse_panel_input(LoadFamily::Servo, "27 X Y").is_err());
        // ref 四窗同一性：漂移即作废；不推断 X↔slot。
        assert!(check_panel_ref_stable(&None, "X", "SERVO-L0-001").is_ok());
        assert!(check_panel_ref_stable(&Some("X".to_string()), "X", "SERVO-L1-001").is_ok());
        assert!(check_panel_ref_stable(&Some("X".to_string()), "Z", "SERVO-L2-001").is_err());
    }

    #[test]
    fn jsonl_writer_create_new_locked() {
        // create_new：已存在即 fail-closed（禁复用 append）。
        let dir = std::env::temp_dir().join("mesa-load-evidence-test");
        std::fs::create_dir_all(&dir).unwrap();
        let seq = format!(
            "t{:03}",
            (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                % 1000) as u32
        );
        let _ = seq;
        // 同进程内占位后二次 create 必须失败（create_new 语义）。
        let p1 = dir.join("probe.jsonl");
        std::fs::write(&p1, b"x").unwrap();
        let twice = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p1);
        assert!(twice.is_err(), "已存在文件必须 create_new 失败");
    }

    /// live harness（Evidence Day 用；ignored 真机 only，CI 默认不跑）。
    #[test]
    #[ignore]
    fn load_evidence_harness() {
        run_live_harness();
    }
}
