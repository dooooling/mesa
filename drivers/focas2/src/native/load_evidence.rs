//! 真实机床 load evidence harness（PR C，test-only guarded raw FFI）。
//!
//! 口径（冻结）：
//! - 本模块 `#[cfg(test)]`，只出 ignored live harness + 无 CNC 纯单测；
//!   不碰 `pre_ffi_gate`、不恢复 production `cnc_rdspmeter/rdsvmeter`、
//!   不改 `SpLoad` production layout、不写 load codec、不改 Wire gate/canary、
//!   coverage 保持 15/17。
//! - 同一进程/同一 OS thread/同一 `NativeLib`/同一 FOCAS handle 顺序执行整场
//!   （stream order = RUN order；3-C2 跨流错位教训）。
//! - raw FFI：`GuardedLoadBuffer`（64B guard + 4096B payload + 64B guard，
//!   全 sentinel）；只借 `SpLoad*` 满足函数指针 ABI，**不把 payload 解释成
//!   `SpLoad`**（PR52 暂停原因）；每次必查 guard + tail + num_out。
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

/// panel 门（L1/L2 必须非零且互异；L0/L0R 不设 tolerance——recovery 由
/// Panel+Native+Wire 三方 review 判定，不由 harness 发明阈值）。
pub fn check_panel_gate(phase: LoadPhase, panel: f64) -> Result<(), String> {
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
/// payload 容量（4096B 上界；`SpLoad` 8B 远小于此，越界写入可被 guard 捕获）。
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
    /// 真实输出区（sentinel 预填；`SpLoad*` 借此址，不解释为 `SpLoad`）。
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

/// 单窗采样记录（`jsonl_line` 输入；10 参打包，防 clippy 超参）。
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

/// JSONL 行（每 RUN 一行；`handle` 仅证同 session 未换 handle）。
pub fn jsonl_line(s: &LoadSample<'_>) -> String {
    let fam = match s.family {
        LoadFamily::Spindle => "SPINDLE",
        LoadFamily::Servo => "SERVO",
    };
    // 手工拼 JSONL（单文件 harness，不引入新依赖；panel f64 用 {:?} 保持精度）。
    format!(
        "{{\"run_id\":{run_id:?},\"family\":{fam:?},\"phase\":{phase:?},\
         \"handle\":{handle},\"panel_value\":{panel_value:?},\
         \"num_in\":{num_in},\"num_out\":{num_out},\"rc\":{rc},\
         \"raw_0_512_hex\":{raw:?},\"pre_guard_ok\":{pre_ok},\
         \"post_guard_ok\":{post_ok},\"tail_after_512_clean\":{tail_ok},\
         \"unix_ms\":{unix_ms}}}",
        run_id = s.run_id,
        phase = s.phase.tag(),
        handle = s.handle,
        panel_value = s.panel_value,
        num_in = s.num_in,
        num_out = s.num_out,
        rc = s.rc,
        raw = s.buf.raw_hex(),
        pre_ok = s.buf.pre_guard.iter().all(|&b| b == LOAD_SENTINEL),
        post_ok = s.buf.post_guard.iter().all(|&b| b == LOAD_SENTINEL),
        tail_ok = s.buf.tail_clean(),
        unix_ms = s.unix_ms,
    )
}

/// JSONL 输出路径（`target/focas-load-evidence/load-<seq>.jsonl`）。
pub fn jsonl_path(seq: &str) -> PathBuf {
    PathBuf::from(format!("target/focas-load-evidence/load-{seq}.jsonl"))
}

/// live harness 入口（Evidence Day 用；`#[ignore]` 真机 only）。
/// 整场：OPEN handle once → L0/L1/L2/L0R 顺序 × family → CLOSE once。
/// 每窗：stdout `>>> BEGIN` → panel 交互输入 → FFI → guard 检查 → JSONL →
/// stdout `<<< END`。guard/tail/num 异常即整 session HOLD（panic 中止，
/// 禁止继续推 layout；下一次必须新 seq）。
#[cfg(test)]
pub fn run_live_harness() {
    use crate::native::{FocasRet, NativeLib, SpLoad};
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
    let lib = NativeLib::load().unwrap_or_else(|e| panic!("FWLIB 加载失败（{host}:{port}）：{e}"));
    let hdl = lib
        .cnc_allclibhndl3(&host, port, timeout_secs)
        .unwrap_or_else(|e| panic!("cnc_allclibhndl3 失败：{} {}", e as i16, e.message()));
    println!(">>> LOAD SESSION seq={seq} handle={hdl} families={families:?} (single handle)");
    let out_path = jsonl_path(&seq);
    std::fs::create_dir_all(out_path.parent().unwrap()).expect("jsonl 目录创建失败");
    // panel L1/L2 互异门需跨窗比较（同 family 内）。
    let mut levels: std::collections::BTreeMap<String, (f64, f64)> =
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
            // panel 交互门（操作员输入当前实际 load；L1/L2 零值即中止整场）。
            println!(
                "[WAIT] RUN={id} 请输入面板 {} 当前值：",
                match family {
                    LoadFamily::Spindle => "SPINDLE LOAD S1",
                    LoadFamily::Servo => "SERVO LOAD（记录轴号/值）",
                }
            );
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .expect("stdin 读取失败");
            let panel: f64 = line.trim().parse().unwrap_or_else(|_| {
                panic!("RUN={id} 面板值非法：{line:?}（本 session 作废，下次用新 seq）")
            });
            check_panel_gate(phase, panel).unwrap_or_else(|e| panic!("RUN={id} {e}"));
            // raw FFI（同 handle；selector=0/num_in=4 仅复现 identity window）。
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
                            buf.payload.as_mut_ptr().cast::<SpLoad>(),
                        )
                    }
                    LoadFamily::Servo => {
                        let sym = lib.cnc_rdsvmeter.as_ref().expect("缺符号 cnc_rdsvmeter");
                        sym(
                            hdl as c_ushort,
                            &mut num as *mut c_short,
                            buf.payload.as_mut_ptr().cast::<SpLoad>(),
                        )
                    }
                }
            };
            let ret_ok = FocasRet::from_raw(rc).is_ok();
            let guards_ok = buf.guards_ok();
            let tail_ok = buf.tail_clean();
            let num_sane = (0..=4).contains(&num);
            println!("<<< END RUN={id} RC={rc} num_out={num} guards={guards_ok} tail={tail_ok}");
            if !guards_ok || !tail_ok || !num_sane {
                panic!(
                    "RUN={id} HOLD：guard/tail/num 异常（guards={guards_ok} tail={tail_ok} num={num}）——不推 layout，本 session 作废"
                );
            }
            let _ = ret_ok;
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
                num_in: 4,
                num_out: num,
                rc,
                buf: &buf,
                unix_ms,
            });
            println!("{line}");
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&out_path)
                .and_then(|mut f| {
                    use std::io::Write;
                    writeln!(f, "{line}")
                })
                .expect("jsonl 追加失败");
            // L1/L2 互异门（同 family；读回 levels 累积）。
            let fam_key = format!("{:?}", family);
            match phase {
                LoadPhase::L1 => {
                    levels.insert(fam_key, (panel, f64::NAN));
                }
                LoadPhase::L2 => {
                    if let Some(e) = levels.get_mut(&fam_key) {
                        e.1 = panel;
                        check_levels_distinct(e.0, e.1)
                            .unwrap_or_else(|er| panic!("RUN={id} {er}"));
                    }
                }
                _ => {}
            }
        }
    }
    let _ = lib.cnc_freelibhndl(hdl);
    println!(
        "<<< LOAD SESSION seq={seq} done (jsonl: {})",
        out_path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(check_levels_distinct(27.0, 31.0).is_ok());
        assert!(check_levels_distinct(27.0, 27.0).is_err());
    }

    /// live harness（Evidence Day 用；ignored 真机 only，CI 默认不跑）。
    #[test]
    #[ignore]
    fn load_evidence_harness() {
        run_live_harness();
    }
}
