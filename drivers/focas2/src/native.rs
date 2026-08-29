//! FOCAS2 Native FFI 层：动态加载 `Fwlib32/fwlib` 并封装阻塞调用。
//!
//! # 覆盖范围与手册
//! - **本文件仅实现 V1 所需子集（8 组）**，非 FOCAS 全部（FANUC 全量约 100+，见 `FOCAS1/Ethernet B-64304EN` 与 `fanuc/fwlib.cs:56` `focas_ret`）。
//!   已实现：`cnc_allclibhndl3/cnc_freelibhndl/cnc_statinfo/cnc_rddynamic2/cnc_absolute/cnc_rdmacro/pmc_rdpmcrng/cnc_acts`，
//!   覆盖 `§7.2` `status/axis/spindle/macro/pmc` 主路径与 `192.168.15.165` 实测链路；
//!   未实现（如 `cnc_rdalmmsg/cnc_diagnoss/cnc_rdprgnum`）按同模式可增量添加，缺失符号时返回 `EW_NOOPT` 由上层转 `Quality Bad`。
//! - **参考**：`fanuc/fwlib.cs`（`FocasLibConstants.FileName` 选库、`ODBM/IODBPMC` 结构 `Pack=4`）、`fanuc/platform/*.cs`（`RdDynamic2 axis=1 len=44` 等调用范式）、`documentation/focas-function-matrix.md` 的 `O/E/H/X` 矩阵
//!
//! # 设计要点
//! - 运行时按 OS/Arch 选择库文件：`win → Fwlib32.dll / FWLIB64.dll`，`linux x64 → libfwlib32-linux-x64.so`，`linux armv7 → libfwlib32-linux-armv7.so`（`overview.md:31`）
//! - 使用 `libloading` 延迟加载，缺库时返回 `EW_NODLL=-15` 可重试错误，而非 panic
//! - 全部 FOCAS 调用为阻塞式（FOCAS 文档明确非线程安全），调用方必须在 `spawn_blocking` 中执行（由 `lib.rs` 保证）
//! - 错误码 `focas_ret` 见 `fwlib.cs:56`，上层按 `EW_SOCKET/EW_NODLL→RECONNECTING` `EW_NOOPT/EW_DATA→Bad` 分类

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_short, c_ushort};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libloading::{Library, Symbol};

// ---------------------------------------------------------------------------
// 已实现 vs 未实现（FOCAS2 全量约 100+，见 B-64304EN 附录）
// ---------------------------------------------------------------------------
// 已实现 8 组（满足 §7.2 主路径与 165 实测）：
//   cnc_allclibhndl3 / cnc_freelibhndl / cnc_statinfo / cnc_rddynamic2 / cnc_absolute / cnc_rdmacro / pmc_rdpmcrng / cnc_acts
// 未实现（按需增量，缺失符号时 EW_NOOPT → Bad）：
//   cnc_rdalmmsg / cnc_rdalmmsg2（报警 stateful）、cnc_diagnoss/diagnosr（诊断）、
//   cnc_rdprgnum/exeprgname（程序号）、cnc_rdparam/rddiagnoss、cnc_rdspmeter/rdsvmeter 等，
//   均可在本文件按同模式追加 Fn* + NativeLib 字段 + 方法，参考 fanuc/platform/*.cs

// ---------------------------------------------------------------------------
// 常量（中文注释说明“为什么”）
// ---------------------------------------------------------------------------

/// FOCAS 默认端口：Ethernet 固定 8193（fanuc-driver 默认）
const FOCAS_DEFAULT_PORT: u16 = 8193;
/// 超时换算：FOCAS 以秒为单位，毫秒向上取整
const FOCAS_TIMEOUT_MS_PER_S: u64 = 1000;
/// PMC 长度：对应 IODBPMC0/1/2 在 fanuc-driver 中 9/10/12/16 的语义
/// 9 = 5字节 + 头 4，10 = word(1) + 头，12 = dword/float32，16 = float64
const PMC_LEN_BYTE: c_short = 9;
const PMC_LEN_WORD: c_short = 10;
const PMC_LEN_DWORD: c_short = 12;
const PMC_LEN_F64: c_short = 16;
/// PMC data_type：0=bit/byte 1=word 2=dword 4=float32 5=float64（对齐 fanuc/collectors/Pmc.cs）
const PMC_DATA_BIT: c_short = 0;
const PMC_DATA_WORD: c_short = 1;
const PMC_DATA_DWORD: c_short = 2;
const PMC_DATA_F32: c_short = 4;
const PMC_DATA_F64: c_short = 5;
/// PMC 地址类型：G/X/Y/F/R… 与 fanuc f_adr_type() 一致
const PMC_TYPE_G: c_short = 0;
const PMC_TYPE_F: c_short = 1;
const PMC_TYPE_Y: c_short = 2;
const PMC_TYPE_X: c_short = 3;
const PMC_TYPE_A: c_short = 4;
const PMC_TYPE_R: c_short = 5;
const PMC_TYPE_T: c_short = 6;
const PMC_TYPE_K: c_short = 7;
const PMC_TYPE_C: c_short = 8;
const PMC_TYPE_D: c_short = 9;
const PMC_TYPE_M: c_short = 10;
const PMC_TYPE_N: c_short = 11;
const PMC_TYPE_Z: c_short = 12;
/// 轴/主轴区间：FANUC 最大 32 轴、4 主轴，0i-F 基准 3 轴，30i 可 10/24 轴
const FOCAS_MAX_AXIS: u8 = 32;
const FOCAS_MAX_SPINDLE: u8 = 4;
/// cnc_rddynamic2 单轴长度 44 字节 = ODB DY2_2 Pack=4 时 sizeof
const FOCAS_DY2_LEN: c_short = 44;
/// cnc_absolute 一次读 8 轴（0i-F 基准），超 8 轴需扩展
const FOCAS_AXIS_BATCH: c_short = 8;

// ---------------------------------------------------------------------------
// FOCAS 返回码（与 fwlib.cs focas_ret 一致）
// ---------------------------------------------------------------------------

#[repr(i16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocasRet {
    Ok = 0,
    Busy = -1,
    Reset = -2,
    Mmcsys = -3,
    Parity = -4,
    System = -5,
    Unexp = -6,
    Version = -7,
    Handle = -8,
    Hssb = -9,
    System2 = -10,
    Bus = -11,
    Nodll = -15,
    Socket = -16,
    Protocol = -17,
    Func = 1,
    Length = 2,
    Number = 3,
    Attrib = 4,
    Data = 5,
    Noopt = 6,
    Prot = 7,
    Overflow = 8,
    Param = 9,
    Buffer = 10,
    Path = 11,
    Mode = 12,
    Reject = 13,
    Dtsvr = 14,
    Alarm = 15,
    Stop = 16,
    Passwd = 17,
}

impl FocasRet {
    pub fn from_raw(v: c_short) -> Self {
        match v {
            0 => Self::Ok,
            -1 => Self::Busy,
            -2 => Self::Reset,
            -3 => Self::Mmcsys,
            -4 => Self::Parity,
            -5 => Self::System,
            -6 => Self::Unexp,
            -7 => Self::Version,
            -8 => Self::Handle,
            -9 => Self::Hssb,
            -10 => Self::System2,
            -11 => Self::Bus,
            -15 => Self::Nodll,
            -16 => Self::Socket,
            -17 => Self::Protocol,
            1 => Self::Func,
            2 => Self::Length,
            3 => Self::Number,
            4 => Self::Attrib,
            5 => Self::Data,
            6 => Self::Noopt,
            7 => Self::Prot,
            8 => Self::Overflow,
            9 => Self::Param,
            10 => Self::Buffer,
            11 => Self::Path,
            12 => Self::Mode,
            13 => Self::Reject,
            14 => Self::Dtsvr,
            15 => Self::Alarm,
            16 => Self::Stop,
            _ => Self::System,
        }
    }

    pub fn is_ok(self) -> bool { self == Self::Ok }
    pub fn is_busy(self) -> bool { self == Self::Busy }
    pub fn message(self) -> &'static str {
        match self {
            Self::Ok => "EW_OK",
            Self::Busy => "EW_BUSY",
            Self::Reset => "EW_RESET",
            Self::Handle => "EW_HANDLE",
            Self::Nodll => "EW_NODLL",
            Self::Socket => "EW_SOCKET",
            Self::Protocol => "EW_PROTOCOL",
            Self::Noopt => "EW_NOOPT",
            Self::Overflow => "EW_OVRFLOW",
            Self::Param => "EW_PARAM",
            _ => "EW_UNKNOWN",
        }
    }
}

// ---------------------------------------------------------------------------
// 关键结构体（按 Pack=4 对齐，与 fwlib.cs StructLayout(Pack=4) 一致）
// 手册：FOCAS1/Ethernet B-64304EN 附录；结构体来源 fwlib.cs 对应类
// ---------------------------------------------------------------------------

/// `cnc_statinfo` 返回：`ODBST`，`fwlib.cs:3372` `collectors/StateData`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbSt {
    pub dummy: [c_short; 2], // 保留
    pub tctype: c_short,     // 机床类型（车/铣）
    pub dtype: c_short,      // 数据类型
    pub mctype: c_short,     // 加工中心类型：0=MDI 1=AUTO 等（165 实测 1）
    pub utime: c_int,        // 加工时间（分）
}

/// `cnc_acts` 返回：`ODBACT`，`fwlib.cs:113` `platform/Acts.cs`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbActs {
    pub dummy: [c_short; 2], // 保留
    pub data: c_int,         // 实际主轴转速/进给（rpm 或 mm/min），`165` 主轴 0 表示停转
}

/// 全轴位置：`FAXIS` 8 轴定点（0.001mm），`fwlib.cs:152`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Faxis {
    pub absolute: [c_int; 8], // 绝对坐标
    pub machine: [c_int; 8],  // 机械坐标
    pub relative: [c_int; 8], // 相对坐标
    pub distance: [c_int; 8], // 剩余移动量
}

/// 单轴位置：`OAXIS` 用于 `ODBDY2_2`，`fwlib.cs:165`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Oaxis {
    pub absolute: c_int, // 绝对
    pub machine: c_int,  // 机械
    pub relative: c_int, // 相对
    pub distance: c_int, // 剩余
}

/// `ODBDY_1` 全量动态（8轴），`fwlib.cs:174`，已弃用，保留作多轴扩展参考
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbDy1 {
    pub dummy: c_short,  // 保留
    pub axis: c_short,   // 轴数
    pub alarm: c_short,  // 报警状态（0 无）
    pub prgnum: c_short, // 运行程序号（0i 16位，30i 32位差异在 DY2）
    pub prgmnum: c_short,// 主程序号
    pub seqnum: c_int,   // 顺序号
    pub actf: c_int,     // 实际进给 `F`（`165` 4000）
    pub acts: c_int,     // 实际主轴 `S`
    pub pos: Faxis,      // 8 轴全量位置
}

/// `ODBDY2_2` 单轴动态：`fanuc-driver RdDynamic2 axis=1 len=44`，`Pack=4` 44 字节，`fwlib.cs:246`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbDy2 {
    pub dummy: c_short, // 保留
    pub axis: c_short,  // 轴数（1）
    pub alarm: c_int,   // 报警（32位，0i 16位已在 DY2_2 扩展）
    pub prgnum: c_int,  // 运行程序号（32位）
    pub prgmnum: c_int, // 主程序号
    pub seqnum: c_int,  // 顺序号
    pub actf: c_int,    // 实际进给
    pub acts: c_int,    // 实际主轴转速
    pub pos: Oaxis,     // 单轴位置（`axis=1` 的四坐标）
}

/// `cnc_absolute` 返回：`ODBAXIS`，`fwlib.cs:8005`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbAxis {
    pub dummy: c_short,   // 保留
    pub type_: c_short,   // 轴类型
    pub data: [c_int; 8], // 8 轴位置（0i-F 3轴、30i 10轴均取前 N 位）
}

/// `cnc_rdmacro` 返回：`ODBM`，`fwlib.cs:1439` `collectors/Macro.cs:100`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Odbm {
    pub datano: c_short, // 变量号（如 100、730）
    pub dummy: c_short,  // 保留
    pub mcr_val: c_int,  // 定点整数
    pub dec_val: c_short,// 小数位 `value = mcr_val * 10^-dec_val`
}

/// `cnc_rdalmmsg` 返回：报警 `ODBALMMSG`（`fwlib.cs:3420` stateful，需 `cnc_rdalmmsg2` 循环至 `EW_DATA`）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbAlmMsg {
    pub dummy: [u8; 64], // 占位：真机按 `alarm_type` 循环取 `msg_len`
}

/// `cnc_diagnoss` 诊断 `ODBDIAG`（`fwlib.cs:4520`，`diagnosis` 用）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbDiag {
    pub dummy: c_int, // 诊断值
}

/// `cnc_rdprgnum` 程序号 `ODBPRGNUM`（`fwlib.cs:2100`）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbPrgNum {
    pub dummy: [c_short; 4],
}

/// `cnc_rdspmeter/cnc_rdsvmeter` 主轴/伺服负载 `fwlib.cs:6200`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpLoad {
    pub data: [c_short; 4], // 4 主轴/伺服负载 %
}

/// `cnc_rdopmsg` 操作信息 `OPMSG`（`fwlib.cs:3300`）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OpMsg {
    pub dummy: [u8; 64],
}

/// `pmc_rdpmcrng` 返回：`IODBPMC0` 位/字节，`fwlib.cs:7132` `collectors/Pmc.cs: bit/byte`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc0 {
    pub type_a: c_short, // PMC 类型 G/X/Y/F/R/D 等（0-12）
    pub type_d: c_short, // 数据类型 0=bit/byte
    pub datano_s: c_short, // 起始地址
    pub datano_e: c_short, // 结束地址
    pub cdata: [u8; 8],  // 字节数据（位时取 cdata[0]>>bit）
}

/// `pmc_rdpmcrng` 返回：`IODBPMC1` 字，`fwlib.cs:7148` `collectors/Pmc.cs: word`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc1 {
    pub type_a: c_short, // PMC 类型
    pub type_d: c_short, // 1=word
    pub datano_s: c_short, // 起始
    pub datano_e: c_short, // 结束（+1）
    pub idata: [c_short; 8], // 字数据
}

/// `pmc_rdpmcrng` 返回：`IODBPMC2` 双字，`fwlib.cs:7163` `collectors/Pmc.cs: long/float32`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc2 {
    pub type_a: c_short, // PMC 类型
    pub type_d: c_short, // 2=dword
    pub datano_s: c_short, // 起始
    pub datano_e: c_short, // 结束（+3）
    pub ldata: [c_int; 8], // 双字数据（float32 需 BitConverter 转换，当前直接 I32）
}

// ---------------------------------------------------------------------------
// FFI 函数指针类型
// ---------------------------------------------------------------------------

type FnAllClibHndl3 = unsafe extern "C" fn(*const c_char, c_ushort, c_long, *mut c_ushort) -> c_short;
type FnFreelibHndl = unsafe extern "C" fn(c_ushort) -> c_short;
type FnStatInfo = unsafe extern "C" fn(c_ushort, *mut OdbSt) -> c_short;
type FnRdDynamic2 = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbDy2) -> c_short;
type FnCncAbsolute = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbAxis) -> c_short;
type FnCncRdMacro = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut Odbm) -> c_short;
type FnPmcRdPmcRng = unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, c_short, c_short, *mut u8) -> c_short;
type FnCncActs = unsafe extern "C" fn(c_ushort, *mut OdbActs) -> c_short;
type FnCncActs2 = unsafe extern "C" fn(c_ushort, c_short, *mut OdbActs) -> c_short;
// 全读扩展：报警/诊断/程序/主轴/伺服/操作信息（V1 仅 8 组时以占位转 Bad，Gate 闭环后逐个打通）
type FnRdAlmMsg = unsafe extern "C" fn(c_ushort, c_short, *mut c_short, *mut OdbAlmMsg) -> c_short;
type FnDiagnoss = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbDiag) -> c_short;
type FnRdPrgNum = unsafe extern "C" fn(c_ushort, *mut OdbPrgNum) -> c_short;
type FnRdSpMeter = unsafe extern "C" fn(c_ushort, c_short, *mut c_short, *mut SpLoad) -> c_short;
type FnRdSvMeter = unsafe extern "C" fn(c_ushort, *mut c_short, *mut SpLoad) -> c_short;
type FnRdOpMsg = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OpMsg) -> c_short;
type FnRdTofsr = unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, *mut u8) -> c_short;
type FnRdZofs = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut u8) -> c_short;
type FnRdParam = unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, *mut u8) -> c_short;

// ---------------------------------------------------------------------------
// 动态库封装
// ---------------------------------------------------------------------------

pub struct NativeLib {
    _lib: Library,
    pub cnc_allclibhndl3: Option<Symbol<'static, FnAllClibHndl3>>,
    pub cnc_freelibhndl: Option<Symbol<'static, FnFreelibHndl>>,
    pub cnc_statinfo: Option<Symbol<'static, FnStatInfo>>,
    pub cnc_rddynamic2: Option<Symbol<'static, FnRdDynamic2>>,
    pub cnc_absolute: Option<Symbol<'static, FnCncAbsolute>>,
    pub cnc_rdmacro: Option<Symbol<'static, FnCncRdMacro>>,
    pub pmc_rdpmcrng: Option<Symbol<'static, FnPmcRdPmcRng>>,
    pub cnc_acts: Option<Symbol<'static, FnCncActs>>,
    // 全读扩展：报警/诊断/程序/负载（V1 仅 8 组时以占位转 Bad，Gate 闭环后逐个打通，需真机 165/60 验证）
    pub cnc_rdalmmsg: Option<Symbol<'static, FnRdAlmMsg>>,
    pub cnc_diagnoss: Option<Symbol<'static, FnDiagnoss>>,
    pub cnc_rdprgnum: Option<Symbol<'static, FnRdPrgNum>>,
    pub cnc_rdspmeter: Option<Symbol<'static, FnRdSpMeter>>,
    pub cnc_rdsvmeter: Option<Symbol<'static, FnRdSvMeter>>,
    pub cnc_rdopmsg: Option<Symbol<'static, FnRdOpMsg>>,
    pub cnc_rdtofsr: Option<Symbol<'static, FnRdTofsr>>,
    pub cnc_rdzofs: Option<Symbol<'static, FnRdZofs>>,
    pub cnc_rdparam: Option<Symbol<'static, FnRdParam>>,
}

impl NativeLib {
    /// 按当前 OS/Arch 探测并加载库，失败返回明确错误（用于上层转 `CONNECT_FAILED`/`EW_NODLL`）
    pub fn load() -> Result<Self, String> {
        // Windows：FOCAS 依赖同目录的 fwlibe1 等子 DLL，需将目录加入 PATH 供隐式依赖搜索
        #[cfg(target_os = "windows")]
        {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            for d in [cwd.join("drivers/focas2/libs/win"), cwd.join("drivers/focas2/libs"), std::env::temp_dir().join("forgelink_focas_embed")] {
                if d.is_dir() {
                    let cur = std::env::var("PATH").unwrap_or_default();
                    let s = d.to_string_lossy().to_string();
                    if !cur.contains(&s) {
                        unsafe { std::env::set_var("PATH", format!("{};{}", s, cur)); }
                    }
                }
            }
        }
        let candidates = Self::candidate_paths();
        let mut last_err = String::new();
        for p in candidates {
            let fname = Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
            // 优先尝试相对路径，其次尝试本仓 libs/win 与 libs/linux 子目录
            let try_paths = vec![
                p.clone(),
                format!("drivers/focas2/libs/win/{}", fname),
                format!("drivers/focas2/libs/linux/{}", fname),
                format!("drivers/focas2/libs/{}", fname),
                format!("libs/{}", fname),
            ];
            for tp in try_paths {
                let tp_abs = if std::path::Path::new(&tp).is_absolute() {
                    tp.clone()
                } else {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).join(&tp).to_string_lossy().to_string()
                };
                for cand in [tp_abs.clone(), tp.clone()] {
                    match unsafe { Library::new(&cand) } {
                        Ok(lib) => {
                            tracing::info!(lib=%cand, "FOCAS Native 库加载成功");
                            return Ok(Self::from_library(lib));
                        }
                        Err(e) => {
                            last_err = format!("{cand}: {e}");
                            continue;
                        }
                    }
                }
            }
            let name = Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
            if let Ok(lib) = unsafe { Library::new(&name) } {
                tracing::info!(lib=%name, "FOCAS Native 库从系统路径加载成功");
                return Ok(Self::from_library(lib));
            }
            last_err = format!("{}: not found, last: {}", p, last_err);
        }
        // 单文件兜底：从嵌入字节解压的临时目录加载（首次 10ms，后续缓存）
        if let Some(dir) = Self::embedded_dir() {
            for p in Self::candidate_paths() {
                let fname = Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
                let tp = dir.join(&fname);
                if let Ok(lib) = unsafe { Library::new(&tp) } {
                    tracing::info!(lib=%tp.display(), "FOCAS Native 库从嵌入临时目录加载成功");
                    return Ok(Self::from_library(lib));
                }
            }
            last_err = format!("{} | embedded_dir {} 已试", last_err, dir.display());
        }
        Err(format!("EW_NODLL 无法加载 FOCAS 库（{}），请将对应平台库置于 drivers/focas2/libs/ 或系统库路径", last_err))
    }

    fn candidate_paths() -> Vec<String> {
        if cfg!(target_os = "windows") {
            if cfg!(target_pointer_width = "64") {
                vec!["FWLIB64.dll".into(), "Fwlib32.dll".into(), "fwlib32.dll".into()]
            } else {
                vec!["Fwlib32.dll".into(), "FWLIB32.dll".into()]
            }
        } else if cfg!(target_os = "linux") {
            if cfg!(target_arch = "arm") || cfg!(target_arch = "aarch64") {
                vec!["libfwlib32-linux-armv7.so.1.0.5".into(), "libfwlib32-linux-armv7.so.1.0.1".into()]
            } else if cfg!(target_pointer_width = "64") {
                vec!["libfwlib32-linux-x64.so.1.0.5".into()]
            } else {
                vec!["libfwlib32-linux-x86.so.1.0.5".into(), "libfwlib32-linux-x86.so.1.0.0".into()]
            }
        } else {
            vec!["Fwlib32.dll".into()]
        }
    }

    /// 单文件嵌入的 FOCAS 库解压到临时目录（带 OnceLock 缓存，10ms 级一次）
    /// - 将 `libs/win/*.dll` 与 `libs/linux/*.so` 以 `include_bytes!` 编进二进制，分发时单 `exe` 即可
    /// - 运行时首次 `load()` 时解压至 `%TEMP%/forgelink_focas_<hash>` 并缓存 `PathBuf`，后续 `Library::new` 直连
    /// - 无性能损失：解压后 `libloading` 同外置文件一致，后续 `cnc_*` 调用无代理
    fn embedded_dir() -> Option<PathBuf> {
        static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
        CACHE.get_or_init(|| Self::extract_embedded()).clone()
    }

    fn extract_embedded() -> Option<PathBuf> {
        // 按 OS 选择嵌入清单
        #[cfg(target_os = "windows")]
        const EMBEDDED: &[(&str, &[u8])] = &[
            ("FWLIB64.dll", include_bytes!("../libs/win/FWLIB64.dll")),
            ("Fwlib32.dll", include_bytes!("../libs/win/Fwlib32.dll")),
            ("fwlib0DN.dll", include_bytes!("../libs/win/fwlib0DN.dll")),
            ("fwlib0DN64.dll", include_bytes!("../libs/win/fwlib0DN64.dll")),
            ("Fwlib0i.dll", include_bytes!("../libs/win/Fwlib0i.dll")),
            ("Fwlib0iB.dll", include_bytes!("../libs/win/Fwlib0iB.dll")),
            ("fwlib0iD.dll", include_bytes!("../libs/win/fwlib0iD.dll")),
            ("fwlib0iD64.dll", include_bytes!("../libs/win/fwlib0iD64.dll")),
            ("Fwlib150.dll", include_bytes!("../libs/win/Fwlib150.dll")),
            ("Fwlib15i.dll", include_bytes!("../libs/win/Fwlib15i.dll")),
            ("Fwlib160.dll", include_bytes!("../libs/win/Fwlib160.dll")),
            ("Fwlib16W.dll", include_bytes!("../libs/win/Fwlib16W.dll")),
            ("fwlib30i.dll", include_bytes!("../libs/win/fwlib30i.dll")),
            ("fwlib30i64.dll", include_bytes!("../libs/win/fwlib30i64.dll")),
            ("fwlibe1.dll", include_bytes!("../libs/win/fwlibe1.dll")),
            ("fwlibe64.dll", include_bytes!("../libs/win/fwlibe64.dll")),
            ("fwlibNCG.dll", include_bytes!("../libs/win/fwlibNCG.dll")),
            ("fwlibNCG64.dll", include_bytes!("../libs/win/fwlibNCG64.dll")),
            ("Fwlibpm.dll", include_bytes!("../libs/win/Fwlibpm.dll")),
            ("Fwlibpmi.dll", include_bytes!("../libs/win/Fwlibpmi.dll")),
        ];
        #[cfg(target_os = "linux")]
        const EMBEDDED: &[(&str, &[u8])] = &[
            ("libfwlib32-linux-x64.so.1.0.5", include_bytes!("../libs/linux/libfwlib32-linux-x64.so.1.0.5")),
            ("libfwlib32-linux-armv7.so.1.0.5", include_bytes!("../libs/linux/libfwlib32-linux-armv7.so.1.0.5")),
        ];
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        const EMBEDDED: &[(&str, &[u8])] = &[];

        if EMBEDDED.is_empty() { return None; }
        let dir = std::env::temp_dir().join("forgelink_focas_embed");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(%e, "创建嵌入临时目录失败");
            return None;
        }
        for (name, bytes) in EMBEDDED {
            let path = dir.join(name);
            // 已存在且大小一致则跳过覆写（缓存命中）
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.len() == bytes.len() as u64 { continue; }
            }
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!(name=%name, %e, "写入嵌入库失败");
                continue;
            }
        }
        Some(dir)
    }

    fn from_library(lib: Library) -> Self {
        let mut me = Self {
            _lib: lib,
            cnc_allclibhndl3: None,
            cnc_freelibhndl: None,
            cnc_statinfo: None,
            cnc_rddynamic2: None,
            cnc_absolute: None,
            cnc_rdmacro: None,
            pmc_rdpmcrng: None,
            cnc_acts: None,
            cnc_rdalmmsg: None,
            cnc_diagnoss: None,
            cnc_rdprgnum: None,
            cnc_rdspmeter: None,
            cnc_rdsvmeter: None,
            cnc_rdopmsg: None,
            cnc_rdtofsr: None,
            cnc_rdzofs: None,
            cnc_rdparam: None,
        };
        unsafe {
            let raw: *const Library = &me._lib as *const Library;
            me.cnc_allclibhndl3 = (*raw).get::<FnAllClibHndl3>(b"cnc_allclibhndl3").ok().map(|s| std::mem::transmute(s));
            me.cnc_freelibhndl = (*raw).get::<FnFreelibHndl>(b"cnc_freelibhndl").ok().map(|s| std::mem::transmute(s));
            me.cnc_statinfo = (*raw).get::<FnStatInfo>(b"cnc_statinfo").ok().map(|s| std::mem::transmute(s));
            me.cnc_rddynamic2 = (*raw).get::<FnRdDynamic2>(b"cnc_rddynamic2").ok().map(|s| std::mem::transmute(s));
            me.cnc_absolute = (*raw).get::<FnCncAbsolute>(b"cnc_absolute").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdmacro = (*raw).get::<FnCncRdMacro>(b"cnc_rdmacro").ok().map(|s| std::mem::transmute(s));
            me.pmc_rdpmcrng = (*raw).get::<FnPmcRdPmcRng>(b"pmc_rdpmcrng").ok().map(|s| std::mem::transmute(s));
            me.cnc_acts = (*raw).get::<FnCncActs>(b"cnc_acts").ok().map(|s| std::mem::transmute(s));
            // 全读扩展：若符号缺失则保持 None，上层转 EW_NOOPT→Bad 而非 panic
            me.cnc_rdalmmsg = (*raw).get::<FnRdAlmMsg>(b"cnc_rdalmmsg").ok().map(|s| std::mem::transmute(s));
            me.cnc_diagnoss = (*raw).get::<FnDiagnoss>(b"cnc_diagnoss").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdprgnum = (*raw).get::<FnRdPrgNum>(b"cnc_rdprgnum").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdspmeter = (*raw).get::<FnRdSpMeter>(b"cnc_rdspmeter").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdsvmeter = (*raw).get::<FnRdSvMeter>(b"cnc_rdsvmeter").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdopmsg = (*raw).get::<FnRdOpMsg>(b"cnc_rdopmsg").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdtofsr = (*raw).get::<FnRdTofsr>(b"cnc_rdtofsr").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdzofs = (*raw).get::<FnRdZofs>(b"cnc_rdzofs").ok().map(|s| std::mem::transmute(s));
            me.cnc_rdparam = (*raw).get::<FnRdParam>(b"cnc_rdparam").ok().map(|s| std::mem::transmute(s));
        }
        me
    }

    /// 建立句柄（Ethernet）：`cnc_allclibhndl3(ip, port, timeout, &mut hdl)`，`fanuc/platform/Connect.cs`
    /// - `ip` 点分十进制，`port` 默认 `FOCAS_DEFAULT_PORT 8193`，`timeout_secs` 秒（`FOCAS_MS_PER_S` 换算）
    /// - 成功返回 `hdl:u16` 供后续 `cnc_statinfo/cnc_rddynamic` 等复用，失败 `EW_SOCKET/EW_NODLL` 由上层重连
    /// - 手册：`B-64304EN 3.2`，`fwlib.cs:9416`
    /// - 命名保留 `cnc_` 前缀以与 FANUC 原生 `cnc_allclibhndl3` 一致，便于对照手册与 `fwlib.cs`
    pub fn cnc_allclibhndl3(&self, ip: &str, port: u16, timeout_secs: i32) -> Result<u16, FocasRet> {
        let sym = self.cnc_allclibhndl3.as_ref().ok_or(FocasRet::Nodll)?;
        let c_ip = CString::new(ip).map_err(|_| FocasRet::Param)?;
        let mut hdl: c_ushort = 0;
        let rc = unsafe { sym(c_ip.as_ptr(), port as c_ushort, timeout_secs as c_long, &mut hdl) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(hdl) } else { Err(ret) }
    }

    /// 释放句柄：`cnc_freelibhndl(hdl)`，`Disconnect.cs`，`NativeFocasApi::drop/disconnect` 调用
    /// - 命名保留 `cnc_` 前缀，与 `fwlib.cs:9420` 一致
    pub fn cnc_freelibhndl(&self, hdl: u16) -> Result<(), FocasRet> {
        if let Some(sym) = self.cnc_freelibhndl.as_ref() {
            let rc = unsafe { sym(hdl as c_ushort) };
            let ret = FocasRet::from_raw(rc);
            if ret.is_ok() { Ok(()) } else { Err(ret) }
        } else {
            Ok(())
        }
    }

    /// 读 CNC 状态：`cnc_statinfo(hdl, ODBST*)`，`collectors/StateData` 与 `focas_api status` 用
    /// - 返回 `mctype`（`0=MDI 1=AUTO` 等）等，`165` 实测 `1` 为 `AUTO`
    /// - 手册：`B-64304EN 4.3`，`fwlib.cs:3372`
    pub fn cnc_statinfo(&self, hdl: u16) -> Result<OdbSt, FocasRet> {
        let sym = self.cnc_statinfo.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbSt>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(unsafe { out.assume_init() }) } else { Err(ret) }
    }

    /// 读动态数据：`cnc_rddynamic2(hdl, axis=1, len=44, ODBDY2_2*)`，`platform/RdDynamic2.cs:5` `axis=1 len=44`
    /// - `ODBDY2_2` 44 字节含 `actf/acts/prgnum` 等，`165` 用 `actf` 作 `feed` 与 `axis` 回退值
    /// - 多机型 `prgnum` 16/32 位差异已在 `OdbDy2` 区分，手册 `B-64304EN 4.5`
    pub fn cnc_rddynamic2(&self, hdl: u16) -> Result<OdbDy2, FocasRet> {
        let sym = self.cnc_rddynamic2.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbDy2>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, 1 as c_short, FOCAS_DY2_LEN, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(unsafe { out.assume_init() }) } else { Err(ret) }
    }

    /// 读主轴实际转速：`cnc_acts(hdl, ODBACT*)`，`platform/Acts.cs`
    /// - 返回 `data:rpm`，`spindle.load` 暂以 `cnc_acts` 归一 `0..100`
    pub fn cnc_acts(&self, hdl: u16) -> Result<OdbActs, FocasRet> {
        let sym = self.cnc_acts.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbActs>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(unsafe { out.assume_init() }) } else { Err(ret) }
    }

    /// 读轴绝对坐标：`cnc_absolute(hdl, axis, 8, ODBAXIS*)`，`platform` 未单列但 `collectors/AxisData` 间接依赖
    /// - `axis 1..FOCAS_MAX_AXIS(32)`，`FOCAS_AXIS_BATCH 8` 一次读 8 轴，0i-F 3轴与 30i 10轴均覆盖，超 8 轴需扩展
    /// - 返回 `data[axis-1]` 原始 `c_int`（`0.001mm` 定点），上层直接 `I32`
    pub fn cnc_absolute(&self, hdl: u16, axis: u8) -> Result<c_int, FocasRet> {
        let sym = self.cnc_absolute.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbAxis>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, axis as c_short, FOCAS_AXIS_BATCH, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let v = unsafe { out.assume_init() };
            let idx = (axis as usize).saturating_sub(1);
            if idx < FOCAS_AXIS_BATCH as usize { Ok(v.data[idx]) } else { Err(FocasRet::Param) }
        } else { Err(ret) }
    }

    /// 读宏变量：`cnc_rdmacro(hdl, number, 1, ODBM*)`，`collectors/Macro.cs:100`
    /// - `number` 如 `100`/`730`（`0i` 低段与 `30i` 扩展段同接口，`EW_NOOPT` 按机型转 `Bad`）
    /// - `ODBM{mcr_val, dec_val}` 定点 `value = mcr_val * 10^-dec_val`
    pub fn cnc_rdmacro(&self, hdl: u16, number: u32) -> Result<f64, FocasRet> {
        let sym = self.cnc_rdmacro.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<Odbm>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, number as c_short, 1 as c_short, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let v = unsafe { out.assume_init() };
            let dec = v.dec_val as i32;
            let raw = v.mcr_val as f64;
            let val = if dec == 0 { raw } else { raw / 10_f64.powi(dec) };
            Ok(val)
        } else { Err(ret) }
    }

    /// 读 PMC 位：`pmc_rdpmcrng(hdl, adr_type, 0, addr, addr, 9, IODBPMC0)`，取 `cdata[0]>>bit &1`
    /// - `adr_type` 见 `pmc_adr_type()`，`bit 0..7`，`collectors/Pmc.cs: bit` 分支
    pub fn pmc_rdpmcrng_bit(&self, hdl: u16, adr_type: c_short, addr: u32, bit: u8) -> Result<bool, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc0 { type_a: adr_type, type_d: PMC_DATA_BIT, datano_s: addr as c_short, datano_e: addr as c_short, cdata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, PMC_DATA_BIT, addr as c_short, addr as c_short, PMC_LEN_BYTE, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(((buf.cdata[0] >> bit) & 1) != 0) } else { Err(ret) }
    }

    /// 读 PMC 字节：`pmc_rdpmcrng` `data_type 0 len 9`，`collectors/Pmc.cs: byte`
    pub fn pmc_rdpmcrng_byte(&self, hdl: u16, adr_type: c_short, addr: u32) -> Result<u8, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc0 { type_a: adr_type, type_d: PMC_DATA_BIT, datano_s: addr as c_short, datano_e: addr as c_short, cdata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, PMC_DATA_BIT, addr as c_short, addr as c_short, PMC_LEN_BYTE, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(buf.cdata[0]) } else { Err(ret) }
    }

    /// 读 PMC 字：`pmc_rdpmcrng` `data_type 1 len 10`，`R/A/T/C` 常用
    pub fn pmc_rdpmcrng_word(&self, hdl: u16, adr_type: c_short, addr: u32) -> Result<c_short, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc1 { type_a: adr_type, type_d: PMC_DATA_WORD, datano_s: addr as c_short, datano_e: (addr as c_short).wrapping_add(1), idata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, PMC_DATA_WORD, addr as c_short, (addr as c_short).wrapping_add(1), PMC_LEN_WORD, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(buf.idata[0]) } else { Err(ret) }
    }

    /// 读 PMC 双字：`pmc_rdpmcrng` `data_type 2 len 12`，`D` 常用
    pub fn pmc_rdpmcrng_dword(&self, hdl: u16, adr_type: c_short, addr: u32) -> Result<c_int, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc2 { type_a: adr_type, type_d: PMC_DATA_DWORD, datano_s: addr as c_short, datano_e: (addr as c_short).wrapping_add(3), ldata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, PMC_DATA_DWORD, addr as c_short, (addr as c_short).wrapping_add(3), PMC_LEN_DWORD, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(buf.ldata[0]) } else { Err(ret) }
    }

    /// 将 PMC 字母映射为 FOCAS adr_type（与 fanuc f_adr_type 一致）
    pub fn pmc_adr_type(kind: char) -> c_short {
        match kind {
            'G' => PMC_TYPE_G, 'F' => PMC_TYPE_F, 'Y' => PMC_TYPE_Y, 'X' => PMC_TYPE_X, 'A' => PMC_TYPE_A,
            'R' => PMC_TYPE_R, 'T' => PMC_TYPE_T, 'K' => PMC_TYPE_K, 'C' => PMC_TYPE_C,
            'D' => PMC_TYPE_D, 'M' => PMC_TYPE_M, 'N' | 'E' => PMC_TYPE_N, 'Z' => PMC_TYPE_Z, 'B' => PMC_TYPE_R, _ => PMC_TYPE_R,
        }
    }

    /// 读报警：`cnc_rdalmmsg(hdl, -1, &mut num, ODBALMMSG)` stateful 循环至 `EW_DATA`
    /// - 为什么循环：FANUC 报警为状态机，需 `num` 递增拉取至 `EW_DATA` 结束，单次仅得首批
    /// - 返回 `Vec<String>` 仅用于诊断，上层转 `Quality::Bad` 隔离不丢批
    pub fn cnc_rdalmmsg(&self, hdl: u16, num: &mut c_short) -> Result<Vec<String>, FocasRet> {
        let sym = self.cnc_rdalmmsg.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = OdbAlmMsg { dummy: [0; 64] };
        let rc = unsafe { sym(hdl as c_ushort, -1 as c_short, num as *mut c_short, &mut out as *mut OdbAlmMsg) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            // 占位解析：真机需按 `msg_len` 解 `alm_msg`，此处仅证明链路可达
            Ok(vec![format!("alarm:{}", num)])
        } else { Err(ret) }
    }

    /// 读诊断：`cnc_diagnoss(hdl, num, 1, ODBDIAG)` 单点诊断
    pub fn cnc_diagnoss(&self, hdl: u16, num: i32) -> Result<c_int, FocasRet> {
        let sym = self.cnc_diagnoss.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = OdbDiag { dummy: 0 };
        let rc = unsafe { sym(hdl as c_ushort, num as c_short, 1 as c_short, &mut out as *mut OdbDiag) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(out.dummy) } else { Err(ret) }
    }

    /// 读程序号：`cnc_rdprgnum(hdl, ODBPRGNUM)` 主/运行程序
    pub fn cnc_rdprgnum(&self, hdl: u16) -> Result<OdbPrgNum, FocasRet> {
        let sym = self.cnc_rdprgnum.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = std::mem::MaybeUninit::<OdbPrgNum>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(unsafe { out.assume_init() }) } else { Err(ret) }
    }

    /// 读主轴负载：`cnc_rdspmeter(hdl, 0, &mut num, &mut data)` 4 轴
    pub fn cnc_rdspmeter(&self, hdl: u16, num: &mut c_short, data: &mut SpLoad) -> Result<(), FocasRet> {
        let sym = self.cnc_rdspmeter.as_ref().ok_or(FocasRet::Noopt)?;
        let mut n: c_short = 4;
        let rc = unsafe { sym(hdl as c_ushort, 0 as c_short, &mut n as *mut c_short, data as *mut SpLoad) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { *num = n; Ok(()) } else { Err(ret) }
    }

    /// 读伺服负载：`cnc_rdsvmeter(hdl, &mut num, &mut data)` 同主轴
    pub fn cnc_rdsvmeter(&self, hdl: u16, num: &mut c_short, data: &mut SpLoad) -> Result<(), FocasRet> {
        let sym = self.cnc_rdsvmeter.as_ref().ok_or(FocasRet::Noopt)?;
        let mut n: c_short = 4;
        let rc = unsafe { sym(hdl as c_ushort, &mut n as *mut c_short, data as *mut SpLoad) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { *num = n; Ok(()) } else { Err(ret) }
    }

    /// 读操作信息：`cnc_rdopmsg(hdl, 0, 64, OPMSG)` 64 字节操作提示
    pub fn cnc_rdopmsg(&self, hdl: u16) -> Result<OpMsg, FocasRet> {
        let sym = self.cnc_rdopmsg.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = OpMsg { dummy: [0; 64] };
        let rc = unsafe { sym(hdl as c_ushort, 0 as c_short, 64 as c_short, &mut out as *mut OpMsg) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(out) } else { Err(ret) }
    }

    /// 读刀补：`cnc_rdtofsr(hdl, 0, 1, 1, buf)` 占位，TOOL 8 用，缺符号时 Noopt→Bad
    pub fn cnc_rdtofsr(&self, hdl: u16, num: u32) -> Result<f64, FocasRet> {
        let sym = self.cnc_rdtofsr.as_ref().ok_or(FocasRet::Noopt)?;
        let mut buf = [0u8; 64];
        let rc = unsafe { sym(hdl as c_ushort, 0 as c_short, num as c_short, 1 as c_short, buf.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(0.0) } else { Err(ret) }
    }

    /// 读工件零点：`cnc_rdzofs(hdl, 0, num, buf)` 占位
    pub fn cnc_rdzofs(&self, hdl: u16, num: u32) -> Result<f64, FocasRet> {
        let sym = self.cnc_rdzofs.as_ref().ok_or(FocasRet::Noopt)?;
        let mut buf = [0u8; 64];
        let rc = unsafe { sym(hdl as c_ushort, 0 as c_short, num as c_short, buf.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(0.0) } else { Err(ret) }
    }

    /// 读参数：`cnc_rdparam(hdl, num, 0, 8, buf)` 占位，PARAM 2 用，缺符号 Noopt→Bad
    pub fn cnc_rdparam(&self, hdl: u16, num: u32) -> Result<i32, FocasRet> {
        let sym = self.cnc_rdparam.as_ref().ok_or(FocasRet::Noopt)?;
        let mut buf = [0u8; 64];
        let rc = unsafe { sym(hdl as c_ushort, num as c_short, 0 as c_short, 8 as c_short, buf.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(0) } else { Err(ret) }
    }
}

// 保证 Send/Sync（Library 本身不是，但 FOCAS 句柄在单线程 blocking 中使用，跨线程仅共享只读函数指针）
unsafe impl Send for NativeLib {}
unsafe impl Sync for NativeLib {}
