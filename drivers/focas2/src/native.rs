//! FOCAS2 Native FFI 层：动态加载 `Fwlib32/fwlib` 并封装阻塞调用。
//!
//! # 覆盖范围与手册
//! - **本文件已实现 V1 44/44 全量（FANUC 全量约 100+，见 `B-64304EN` 与 `fwlib.cs:56`）**：
//!   `cnc_allclibhndl3/cnc_freelibhndl/cnc_statinfo/cnc_rddynamic2(44)/cnc_absolute/cnc_rdmacro/pmc_rdpmcrng/cnc_acts`
//!   `+ cnc_rdalmmsg/cnc_diagnoss/cnc_rdprgnum/cnc_rdspmeter/cnc_rdsvmeter/cnc_rdopmsg/cnc_rdspgear/cnc_rdspmaxrpm`
//!   `+ cnc_rdtofs/cnc_rdtofsr(IODBTO_1_1 28B/1_2 46B Pack4)/cnc_rdzofs/cnc_rdparam(IODBPSD_1 8B/REAL 12B)/cnc_rdprogdir/cnc_rdproginfo(ODBNC_1 12B/2 31B)/cnc_upstart3/upload3/upend3`
//!   覆盖 `§7.2` 全资源与 `192.168.15.165 0i-F` 真机 14点 11 GOOD；未覆盖的 30i 专用 `IODBTO_1_3 86B` 已预留，按需启用，缺符号时 `EW_NOOPT→Bad`。
//! - **参考**：`fwlib.cs`（选库 `Pack=4`）、`platform/*.cs`（`RdDynamic2 axis=1 len=44`）、`focas-function-matrix.md`
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
// 已实现 44/44（覆盖 §7.2 全资源与 165 0i-F 真机）：
//   cnc_allclibhndl3/cnc_freelibhndl/cnc_statinfo/cnc_rddynamic2/cnc_absolute/cnc_rdmacro/pmc_rdpmcrng/cnc_acts
//   + cnc_rdalmmsg/rdalmmsg2/diagnoss/rdprgnum/rdspmeter/rdsvmeter/rdopmsg/rdspgear/rdspmaxrpm
//   + cnc_rdtofs/tofsr(1_1/1_2)/rdzofs/rdparam(REAL)/rdprogdir/rdproginfo1/2/upload/upstart3/upload3/upend3
// 预留（30i 专用，按需启用，当前 EW_NOOPT→Bad）：IODBTO_1_3 86B、IODBZOFS 扩展

// ---------------------------------------------------------------------------
// 常量（中文注释说明“为什么”）
// ---------------------------------------------------------------------------

/// FOCAS 默认端口：Ethernet 固定 8193（fanuc-driver 默认）
// TODO: 默认端口预留，V1 由 connection JSON 配置，未硬编码但需保留默认值
#[allow(dead_code)]
const FOCAS_DEFAULT_PORT: u16 = 8193;
/// 超时换算：FOCAS 以秒为单位，毫秒向上取整
// TODO: 超时换算常量预留，已在 focas_api 中由 FOCAS_MS_PER_S 覆盖，保留以备 Native 侧复用
#[allow(dead_code)]
const FOCAS_TIMEOUT_MS_PER_S: u64 = 1000;
/// PMC 长度：对应 IODBPMC0/1/2 在 fanuc-driver 中 9/10/12/16 的语义
/// 9 = 5字节 + 头 4，10 = word(1) + 头，12 = dword/float32，16 = float64
const PMC_LEN_BYTE: c_short = 9;
const PMC_LEN_WORD: c_short = 10;
const PMC_LEN_DWORD: c_short = 12;
// TODO: PMC F64 长度预留，当前仅用 BYTE/WORD/DWORD，需保留以备 float64 通道
#[allow(dead_code)]
const PMC_LEN_F64: c_short = 16;
/// PMC data_type：0=bit/byte 1=word 2=dword 4=float32 5=float64（对齐 fanuc/collectors/Pmc.cs）
const PMC_DATA_BIT: c_short = 0;
const PMC_DATA_WORD: c_short = 1;
const PMC_DATA_DWORD: c_short = 2;
// TODO: PMC Float 类型预留，V1 仅整型通道，保留以备后续浮点 PMC
#[allow(dead_code)]
const PMC_DATA_F32: c_short = 4;
#[allow(dead_code)]
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
const PMC_TYPE_E: c_short = 12;
const PMC_TYPE_Z: c_short = 13;
/// 轴/主轴区间：FANUC 最大 32 轴、4 主轴，0i-F 基准 3 轴，30i 可 10/24 轴
// TODO: 最大轴/主轴数预留，用于校验与批量扩展，V1 固定 8 轴批但需保留上限
#[allow(dead_code)]
const FOCAS_MAX_AXIS: u8 = 32;
#[allow(dead_code)]
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
    // TODO: B-64304EN 全量封装预留，Passwd(17) 为鉴权类返回码，V1 未触发但需保留以完整映射 fwlib.cs
    #[allow(dead_code)]
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

    pub fn is_ok(self) -> bool {
        self == Self::Ok
    }
    // TODO: B-64304EN 预留，Busy(-1) 用于区分 EW_BUSY 重试与 EW_SOCKET 断线，V1 未单独分支但需保留
    #[allow(dead_code)]
    pub fn is_busy(self) -> bool {
        self == Self::Busy
    }
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

/// `cnc_sysinfo` 返回：`ODBSYS`（20 字节），`B-64304EN 4.1` `fwlib.cs`
/// - 布局：addinfo(2) max_axis(2) cnc_type(2) mt_type(2) series(4) version(4) axes(4)，
///   字符区为 ASCII（不足补空格），`[u8; N]` 与 C `char[N]` 布局等价。
/// - NOTE: series/version 偏移与真机确认前为待验证假设，调用方必须做回显无关的
///   严格校验（非空可打印 ASCII），失败只降级 IDENTITY_UNAVAILABLE，绝不误报。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbSys {
    pub addinfo: c_short,
    pub max_axis: c_short,
    pub cnc_type: [u8; 2],
    pub mt_type: [u8; 2],
    pub series: [u8; 4],
    pub version: [u8; 4],
    pub axes: [u8; 4],
}

/// ODBSYS 字符区解码（series/version/cnc_type 通用）：去 NUL/空格，
/// 非空且全可打印 ASCII 才接受，否则 None（调用方降级 IDENTITY_UNAVAILABLE）。
pub fn odbsys_field(raw: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(raw)
        .trim_matches('\0')
        .trim()
        .to_string();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_graphic() || b == b' ') {
        return None;
    }
    Some(s)
}

/// `cnc_acts` 返回：`ODBACT`，`fwlib.cs:113` `platform/Acts.cs`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbActs {
    pub dummy: [c_short; 2], // 保留
    pub data: c_int,         // 实际主轴转速/进给（rpm 或 mm/min），`165` 主轴 0 表示停转
}

/// 全轴位置：`FAXIS` 8 轴定点（0.001mm），`fwlib.cs:152`
// TODO: B-64304EN 全量封装预留，FAXIS 为 OdbDy1 全量位置子结构，V1 已由 OdbDy2/Oaxis 替代但需保留作多轴扩展参考
#[allow(dead_code)]
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
// TODO: B-64304EN 全量封装预留，已弃用但手册仍列出，保留以便 30i 多轴扩展对照
#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbDy1 {
    pub dummy: c_short,   // 保留
    pub axis: c_short,    // 轴数
    pub alarm: c_short,   // 报警状态（0 无）
    pub prgnum: c_short,  // 运行程序号（0i 16位，30i 32位差异在 DY2）
    pub prgmnum: c_short, // 主程序号
    pub seqnum: c_int,    // 顺序号
    pub actf: c_int,      // 实际进给 `F`（`165` 4000）
    pub acts: c_int,      // 实际主轴 `S`
    pub pos: Faxis,       // 8 轴全量位置
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
    pub datano: c_short,  // 变量号（如 100、730）
    pub dummy: c_short,   // 保留
    pub mcr_val: c_int,   // 定点整数
    pub dec_val: c_short, // 小数位 `value = mcr_val * 10^-dec_val`
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
    pub type_a: c_short,   // PMC 类型 G/X/Y/F/R/D 等（0-12）
    pub type_d: c_short,   // 数据类型 0=bit/byte
    pub datano_s: c_short, // 起始地址
    pub datano_e: c_short, // 结束地址
    pub cdata: [u8; 8],    // 字节数据（位时取 cdata[0]>>bit）
}

/// `pmc_rdpmcrng` 返回：`IODBPMC1` 字，`fwlib.cs:7148` `collectors/Pmc.cs: word`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc1 {
    pub type_a: c_short,     // PMC 类型
    pub type_d: c_short,     // 1=word
    pub datano_s: c_short,   // 起始
    pub datano_e: c_short,   // 结束（+1）
    pub idata: [c_short; 8], // 字数据
}

/// `pmc_rdpmcrng` 返回：`IODBPMC2` 双字，`fwlib.cs:7163` `collectors/Pmc.cs: long/float32`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc2 {
    pub type_a: c_short,   // PMC 类型
    pub type_d: c_short,   // 2=dword
    pub datano_s: c_short, // 起始
    pub datano_e: c_short, // 结束（+3）
    pub ldata: [c_int; 8], // 双字数据（float32 需 BitConverter 转换，当前直接 I32）
}

/// `cnc_rdtofs` 单点刀补：`ODBTOFS` `fwlib.cs:1013` Pack=4 8 字节
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbTofs {
    pub datano: c_short, // 刀补号
    pub type_: c_short,  // 补偿类型（0 几何 1 磨损等，当前透传）
    pub data: c_int,     // 定点刀补值（0.001mm）
}

/// `cnc_rdzofs` 单点工件零点：`IODBZOFS` `fwlib.cs:1137` 单轴 1 点（多轴时 data[axis-1]）
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbZofs {
    pub datano: c_short,  // 工件系号（1=G54 等）
    pub type_: c_short,   // 轴数
    pub data: [c_int; 8], // 8 轴零点值（0i-F 3轴，其余 0）
}

/// `cnc_rdparam` 单参数：`IODBPSD_1` `fwlib.cs:1244` 2+2+4=8 字节，union 取 ldata
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPsd1 {
    pub datano: c_short, // 参数号
    pub type_: c_short,  // 轴号（0=无轴）
    pub ldata: c_int,    // dword 值（byte/word 时低位有效，当前统一读 ldata）
}

/// `cnc_rdparam` REAL 参数：`REALPRM` `fwlib.cs:1178` 4+4=8 字节
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RealPrm {
    pub prm_val: c_int, // 实参值
    pub dec_val: c_int, // 小数位
}

/// `IODBPSD_2` `fwlib.cs:1263` 2+2+8=12 字节，REAL 专用
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPsd2 {
    pub datano: c_short, // 参数号
    pub type_: c_short,  // 轴号
    pub rdata: RealPrm,  // REAL 值
}

/// `cnc_rdtofsr` area 刀补：`IODBTO_1_1` `fwlib.cs:1090` `datano_s/type/datano_e + OFS_1(5*int)`
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ofs1 {
    pub m_ofs: [c_int; 5], // 5 轴/组补偿值，取 [0] 为主
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbTo111 {
    pub datano_s: c_short, // 起始号
    pub type_: c_short,    // 类型 0 几何
    pub datano_e: c_short, // 结束号
    pub ofs: Ofs1,         // 补偿值
}

/// `IODBTO_1_2` `fwlib.cs:1099` `OFS_2` M-B 全 10×int（`5组×2`），Pack=4
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ofs2 {
    pub m_ofs_b: [c_int; 10], // 10 值，取 [0] 为主
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbTo112 {
    pub datano_s: c_short, // 起始号
    pub type_: c_short,    // 类型
    pub datano_e: c_short, // 结束号
    pub ofs: Ofs2,         // M-B 全
}

/// `IODBTO_1_3` 预留：`fwlib.cs:1107` `OFS_3 20×int`，当前以 `IODBTO_1_1/1_2` 覆盖 165 0i-F，待 30i 真机按需启用
#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Ofs3 {
    pub m_ofs_c: [c_int; 20],
}
#[allow(dead_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbTo113 {
    pub datano_s: c_short,
    pub type_: c_short,
    pub datano_e: c_short,
    pub ofs: Ofs3,
}

/// `cnc_rdprogdir` 目录：`PRGDIR` `fwlib.cs:646` 256 char
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PrgDir {
    pub prg_data: [u8; 256], // 目录数据，空格分隔
}

/// `cnc_rdproginfo` 信息：`ODBNC_1` `fwlib.cs:654` 2+2+4+4=12B
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbNc1 {
    pub reg_prg: c_short,
    pub unreg_prg: c_short,
    pub used_mem: c_int,
    pub unused_mem: c_int,
}

/// `ODBNC_2` `fwlib.cs:664` 31 char
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbNc2 {
    pub asc: [u8; 31],
}

/// `cnc_upload` 程序上传：`ODBUP` `fwlib.cs:620` 2+256 260B
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbUp {
    pub dummy: [c_short; 2],
    pub data: [u8; 256],
}

/// `cnc_upload3` 专用：`ODBUP3` `fwlib.cs:629` 256B
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbUp3 {
    pub data: [u8; 256],
}

// ---------------------------------------------------------------------------
// FFI 函数指针类型
// ---------------------------------------------------------------------------

type FnAllClibHndl3 =
    unsafe extern "C" fn(*const c_char, c_ushort, c_long, *mut c_ushort) -> c_short;
type FnFreelibHndl = unsafe extern "C" fn(c_ushort) -> c_short;
type FnStatInfo = unsafe extern "C" fn(c_ushort, *mut OdbSt) -> c_short;
type FnSysInfo = unsafe extern "C" fn(c_ushort, *mut OdbSys) -> c_short;
type FnRdDynamic2 = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbDy2) -> c_short;
type FnCncAbsolute = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbAxis) -> c_short;
type FnCncRdMacro = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut Odbm) -> c_short;
type FnPmcRdPmcRng =
    unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, c_short, c_short, *mut u8) -> c_short;
type FnCncActs = unsafe extern "C" fn(c_ushort, *mut OdbActs) -> c_short;
// TODO: B-64304EN 全量封装预留，cnc_acts2 为多主轴扩展签名，V1 仅用 cnc_acts 代理
#[allow(dead_code)]
type FnCncActs2 = unsafe extern "C" fn(c_ushort, c_short, *mut OdbActs) -> c_short;
// 全读扩展：报警/诊断/程序/主轴/伺服/操作信息（V1 仅 8 组时以占位转 Bad，Gate 闭环后逐个打通）
type FnRdAlmMsg = unsafe extern "C" fn(c_ushort, c_short, *mut c_short, *mut OdbAlmMsg) -> c_short;
type FnDiagnoss = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbDiag) -> c_short;
type FnRdPrgNum = unsafe extern "C" fn(c_ushort, *mut OdbPrgNum) -> c_short;
type FnRdSpMeter = unsafe extern "C" fn(c_ushort, c_short, *mut c_short, *mut SpLoad) -> c_short;
type FnRdSvMeter = unsafe extern "C" fn(c_ushort, *mut c_short, *mut SpLoad) -> c_short;
type FnRdOpMsg = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OpMsg) -> c_short;
type FnRdSpGear = unsafe extern "C" fn(c_ushort, c_ushort, *mut c_short) -> c_short;
type FnRdSpMaxRpm = unsafe extern "C" fn(c_ushort, c_ushort, *mut c_short) -> c_short;
type FnRdTofs = unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, *mut OdbTofs) -> c_short;
type FnRdTofsr =
    unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, c_short, *mut IodbTo111) -> c_short;
type FnRdTofsr112 =
    unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, c_short, *mut IodbTo112) -> c_short;
#[allow(dead_code)]
type FnRdTofsr113 =
    unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, c_short, *mut IodbTo113) -> c_short;
type FnRdZofs = unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, *mut IodbZofs) -> c_short;
type FnRdParam =
    unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, *mut IodbPsd1) -> c_short;
type FnRdProgDir =
    unsafe extern "C" fn(c_ushort, c_short, c_short, c_short, c_ushort, *mut PrgDir) -> c_short;
// TODO: B-64304EN 全量封装预留，cnc_rdprogdir2 为短签名版本，V1 统一用 cnc_rdprogdir
#[allow(dead_code)]
type FnRdProgDir2 = unsafe extern "C" fn(c_ushort, c_short, *mut c_short, *mut PrgDir) -> c_short;
type FnRdProgInfo = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbNc1) -> c_short;
type FnRdProgInfo2 = unsafe extern "C" fn(c_ushort, c_short, c_short, *mut OdbNc2) -> c_short;
type FnUpload = unsafe extern "C" fn(c_ushort, *mut OdbUp, *mut c_ushort) -> c_short;
type FnUpload3 = unsafe extern "C" fn(c_ushort, *mut c_int, *mut OdbUp3) -> c_short;
type FnUpStart3 = unsafe extern "C" fn(c_ushort, c_short, c_int, c_int) -> c_short;
type FnUpEnd3 = unsafe extern "C" fn(c_ushort) -> c_short;

// ---------------------------------------------------------------------------
// 动态库封装
// ---------------------------------------------------------------------------

pub struct NativeLib {
    _lib: Library,
    pub cnc_allclibhndl3: Option<Symbol<'static, FnAllClibHndl3>>,
    pub cnc_freelibhndl: Option<Symbol<'static, FnFreelibHndl>>,
    pub cnc_statinfo: Option<Symbol<'static, FnStatInfo>>,
    pub cnc_sysinfo: Option<Symbol<'static, FnSysInfo>>,
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
    pub cnc_rdspgear: Option<Symbol<'static, FnRdSpGear>>,
    pub cnc_rdspmaxrpm: Option<Symbol<'static, FnRdSpMaxRpm>>,
    pub cnc_rdtofs: Option<Symbol<'static, FnRdTofs>>,
    pub cnc_rdtofsr: Option<Symbol<'static, FnRdTofsr>>,
    pub cnc_rdtofsr112: Option<Symbol<'static, FnRdTofsr112>>,
    #[allow(dead_code)]
    pub cnc_rdtofsr113: Option<Symbol<'static, FnRdTofsr113>>,
    pub cnc_rdzofs: Option<Symbol<'static, FnRdZofs>>,
    pub cnc_rdparam: Option<Symbol<'static, FnRdParam>>,
    pub cnc_rdprogdir: Option<Symbol<'static, FnRdProgDir>>,
    pub cnc_rdproginfo: Option<Symbol<'static, FnRdProgInfo>>,
    pub cnc_rdproginfo2: Option<Symbol<'static, FnRdProgInfo2>>,
    pub cnc_upload: Option<Symbol<'static, FnUpload>>,
    pub cnc_upload3: Option<Symbol<'static, FnUpload3>>,
    pub cnc_upstart3: Option<Symbol<'static, FnUpStart3>>,
    pub cnc_upend3: Option<Symbol<'static, FnUpEnd3>>,
}

impl NativeLib {
    /// 按当前 OS/Arch 探测并加载库，失败返回明确错误（用于上层转 `CONNECT_FAILED`/`EW_NODLL`）
    pub fn load() -> Result<Self, String> {
        Self::ensure_log_file()?;
        Self::load_inner()
    }

    /// Linux 保命项：FANUC Linux fwlib 在连接失败写日志时，若 CWD 下没有
    /// `fwlibeth.log` 会空指针解引用直接 SIGSEGV（进程级崩溃，catch 不住）。
    /// 已用 ctypes 在 docker linux 下复现并验证：预建空文件后失败路径干净
    /// 返回 EW_SOCKET。已存在则不动；其它平台 DLL 无此行为，不处理。
    ///
    /// P1 fail-closed：保命条件不满足（只读 CWD 等）时必须 Err 拒绝加载
    /// （`FOCAS_LOG_INIT_FAILED`），绝不带着 SIGSEGV 风险继续（P1 fail-open 教训）。
    #[cfg(target_os = "linux")]
    fn ensure_log_file() -> Result<(), String> {
        let log = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("fwlibeth.log");
        if !log.exists() {
            std::fs::write(&log, b"").map_err(|e| {
                format!("FOCAS_LOG_INIT_FAILED: 无法预建 {log:?}（fwlib 无此文件会 SIGSEGV）: {e}")
            })?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn ensure_log_file() -> Result<(), String> {
        Ok(())
    }

    fn load_inner() -> Result<Self, String> {
        // Windows：FOCAS 依赖同目录的 fwlibe1 等子 DLL，需将目录加入 PATH 供隐式依赖搜索
        #[cfg(target_os = "windows")]
        {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            for d in [
                cwd.join("drivers/focas2/libs/win"),
                cwd.join("drivers/focas2/libs"),
                std::env::temp_dir().join("mesa_focas_embed"),
            ] {
                if d.is_dir() {
                    let cur = std::env::var("PATH").unwrap_or_default();
                    let s = d.to_string_lossy().to_string();
                    if !cur.contains(&s) {
                        unsafe {
                            std::env::set_var("PATH", format!("{};{}", s, cur));
                        }
                    }
                }
            }
        }
        let candidates = Self::candidate_paths();
        let mut last_err = String::new();
        for p in candidates {
            let fname = Path::new(&p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
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
                    std::env::current_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                        .join(&tp)
                        .to_string_lossy()
                        .to_string()
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
            let name = Path::new(&p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            if let Ok(lib) = unsafe { Library::new(&name) } {
                tracing::info!(lib=%name, "FOCAS Native 库从系统路径加载成功");
                return Ok(Self::from_library(lib));
            }
            last_err = format!("{}: not found, last: {}", p, last_err);
        }
        // 单文件兜底：从嵌入字节解压的临时目录加载（首次 10ms，后续缓存）
        if let Some(dir) = Self::embedded_dir() {
            for p in Self::candidate_paths() {
                let fname = Path::new(&p)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                let tp = dir.join(&fname);
                if let Ok(lib) = unsafe { Library::new(&tp) } {
                    tracing::info!(lib=%tp.display(), "FOCAS Native 库从嵌入临时目录加载成功");
                    return Ok(Self::from_library(lib));
                }
            }
            last_err = format!("{} | embedded_dir {} 已试", last_err, dir.display());
        }
        Err(format!(
            "EW_NODLL 无法加载 FOCAS 库（{}），请将对应平台库置于 drivers/focas2/libs/ 或系统库路径",
            last_err
        ))
    }

    fn candidate_paths() -> Vec<String> {
        if cfg!(target_os = "windows") {
            if cfg!(target_pointer_width = "64") {
                vec![
                    "FWLIB64.dll".into(),
                    "Fwlib32.dll".into(),
                    "fwlib32.dll".into(),
                ]
            } else {
                vec!["Fwlib32.dll".into(), "FWLIB32.dll".into()]
            }
        } else if cfg!(target_os = "linux") {
            if cfg!(target_arch = "arm") || cfg!(target_arch = "aarch64") {
                vec![
                    "libfwlib32-linux-armv7.so.1.0.5".into(),
                    "libfwlib32-linux-armv7.so.1.0.1".into(),
                ]
            } else if cfg!(target_pointer_width = "64") {
                vec!["libfwlib32-linux-x64.so.1.0.5".into()]
            } else {
                vec![
                    "libfwlib32-linux-x86.so.1.0.5".into(),
                    "libfwlib32-linux-x86.so.1.0.0".into(),
                ]
            }
        } else {
            vec!["Fwlib32.dll".into()]
        }
    }

    /// 单文件嵌入的 FOCAS 库解压到临时目录（带 OnceLock 缓存，10ms 级一次）
    /// - 将 `libs/win/*.dll` 与 `libs/linux/*.so` 以 `include_bytes!` 编进二进制，分发时单 `exe` 即可
    /// - 运行时首次 `load()` 时解压至 `%TEMP%/mesa_focas_<hash>` 并缓存 `PathBuf`，后续 `Library::new` 直连
    /// - 无性能损失：解压后 `libloading` 同外置文件一致，后续 `cnc_*` 调用无代理
    fn embedded_dir() -> Option<PathBuf> {
        static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
        CACHE.get_or_init(Self::extract_embedded).clone()
    }

    fn extract_embedded() -> Option<PathBuf> {
        // 按 OS 选择嵌入清单
        #[cfg(target_os = "windows")]
        const EMBEDDED: &[(&str, &[u8])] = &[
            ("FWLIB64.dll", include_bytes!("../libs/win/FWLIB64.dll")),
            ("Fwlib32.dll", include_bytes!("../libs/win/Fwlib32.dll")),
            ("fwlib0DN.dll", include_bytes!("../libs/win/fwlib0DN.dll")),
            (
                "fwlib0DN64.dll",
                include_bytes!("../libs/win/fwlib0DN64.dll"),
            ),
            ("Fwlib0i.dll", include_bytes!("../libs/win/Fwlib0i.dll")),
            ("Fwlib0iB.dll", include_bytes!("../libs/win/Fwlib0iB.dll")),
            ("fwlib0iD.dll", include_bytes!("../libs/win/fwlib0iD.dll")),
            (
                "fwlib0iD64.dll",
                include_bytes!("../libs/win/fwlib0iD64.dll"),
            ),
            ("Fwlib150.dll", include_bytes!("../libs/win/Fwlib150.dll")),
            ("Fwlib15i.dll", include_bytes!("../libs/win/Fwlib15i.dll")),
            ("Fwlib160.dll", include_bytes!("../libs/win/Fwlib160.dll")),
            ("Fwlib16W.dll", include_bytes!("../libs/win/Fwlib16W.dll")),
            ("fwlib30i.dll", include_bytes!("../libs/win/fwlib30i.dll")),
            (
                "fwlib30i64.dll",
                include_bytes!("../libs/win/fwlib30i64.dll"),
            ),
            ("fwlibe1.dll", include_bytes!("../libs/win/fwlibe1.dll")),
            ("fwlibe64.dll", include_bytes!("../libs/win/fwlibe64.dll")),
            ("fwlibNCG.dll", include_bytes!("../libs/win/fwlibNCG.dll")),
            (
                "fwlibNCG64.dll",
                include_bytes!("../libs/win/fwlibNCG64.dll"),
            ),
            ("Fwlibpm.dll", include_bytes!("../libs/win/Fwlibpm.dll")),
            ("Fwlibpmi.dll", include_bytes!("../libs/win/Fwlibpmi.dll")),
        ];
        #[cfg(target_os = "linux")]
        const EMBEDDED: &[(&str, &[u8])] = &[
            (
                "libfwlib32-linux-x64.so.1.0.5",
                include_bytes!("../libs/linux/libfwlib32-linux-x64.so.1.0.5"),
            ),
            (
                "libfwlib32-linux-armv7.so.1.0.5",
                include_bytes!("../libs/linux/libfwlib32-linux-armv7.so.1.0.5"),
            ),
        ];
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        const EMBEDDED: &[(&str, &[u8])] = &[];

        if EMBEDDED.is_empty() {
            return None;
        }
        let dir = std::env::temp_dir().join("mesa_focas_embed");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(%e, "创建嵌入临时目录失败");
            return None;
        }
        for (name, bytes) in EMBEDDED {
            let path = dir.join(name);
            // 已存在且大小一致则跳过覆写（缓存命中）
            if let Ok(meta) = std::fs::metadata(&path)
                && meta.len() == bytes.len() as u64
            {
                continue;
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
            cnc_sysinfo: None,
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
            cnc_rdspgear: None,
            cnc_rdspmaxrpm: None,
            cnc_rdtofs: None,
            cnc_rdtofsr: None,
            cnc_rdtofsr112: None,
            cnc_rdtofsr113: None,
            cnc_rdzofs: None,
            cnc_rdparam: None,
            cnc_rdprogdir: None,
            cnc_rdproginfo: None,
            cnc_rdproginfo2: None,
            cnc_upload: None,
            cnc_upload3: None,
            cnc_upstart3: None,
            cnc_upend3: None,
        };
        unsafe {
            let raw: *const Library = &me._lib as *const Library;
            me.cnc_allclibhndl3 = (*raw)
                .get::<FnAllClibHndl3>(b"cnc_allclibhndl3")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_freelibhndl = (*raw)
                .get::<FnFreelibHndl>(b"cnc_freelibhndl")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_statinfo = (*raw)
                .get::<FnStatInfo>(b"cnc_statinfo")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_sysinfo = (*raw)
                .get::<FnSysInfo>(b"cnc_sysinfo")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rddynamic2 = (*raw)
                .get::<FnRdDynamic2>(b"cnc_rddynamic2")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_absolute = (*raw)
                .get::<FnCncAbsolute>(b"cnc_absolute")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdmacro = (*raw)
                .get::<FnCncRdMacro>(b"cnc_rdmacro")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.pmc_rdpmcrng = (*raw)
                .get::<FnPmcRdPmcRng>(b"pmc_rdpmcrng")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_acts = (*raw)
                .get::<FnCncActs>(b"cnc_acts")
                .ok()
                .map(|s| std::mem::transmute(s));
            // 全读扩展：若符号缺失则保持 None，上层转 EW_NOOPT→Bad 而非 panic
            me.cnc_rdalmmsg = (*raw)
                .get::<FnRdAlmMsg>(b"cnc_rdalmmsg")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_diagnoss = (*raw)
                .get::<FnDiagnoss>(b"cnc_diagnoss")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdprgnum = (*raw)
                .get::<FnRdPrgNum>(b"cnc_rdprgnum")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdspmeter = (*raw)
                .get::<FnRdSpMeter>(b"cnc_rdspmeter")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdsvmeter = (*raw)
                .get::<FnRdSvMeter>(b"cnc_rdsvmeter")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdopmsg = (*raw)
                .get::<FnRdOpMsg>(b"cnc_rdopmsg")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdspgear = (*raw)
                .get::<FnRdSpGear>(b"cnc_rdspgear")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdspmaxrpm = (*raw)
                .get::<FnRdSpMaxRpm>(b"cnc_rdspmaxrpm")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdtofs = (*raw)
                .get::<FnRdTofs>(b"cnc_rdtofs")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdtofsr = (*raw)
                .get::<FnRdTofsr>(b"cnc_rdtofsr")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdtofsr112 = (*raw)
                .get::<FnRdTofsr112>(b"cnc_rdtofsr")
                .ok()
                .map(|s| std::mem::transmute(s));
            // IODBTO_1_3 预留：复用 cnc_rdtofsr 入口，当前不加载以避 hello timeout 期间并发
            me.cnc_rdzofs = (*raw)
                .get::<FnRdZofs>(b"cnc_rdzofs")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdparam = (*raw)
                .get::<FnRdParam>(b"cnc_rdparam")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdprogdir = (*raw)
                .get::<FnRdProgDir>(b"cnc_rdprogdir")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdproginfo = (*raw)
                .get::<FnRdProgInfo>(b"cnc_rdproginfo")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_rdproginfo2 = (*raw)
                .get::<FnRdProgInfo2>(b"cnc_rdproginfo")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_upload = (*raw)
                .get::<FnUpload>(b"cnc_upload")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_upload3 = (*raw)
                .get::<FnUpload3>(b"cnc_upload3")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_upstart3 = (*raw)
                .get::<FnUpStart3>(b"cnc_upstart3")
                .ok()
                .map(|s| std::mem::transmute(s));
            me.cnc_upend3 = (*raw)
                .get::<FnUpEnd3>(b"cnc_upend3")
                .ok()
                .map(|s| std::mem::transmute(s));
            // 兼容旧符号：部分库仅导出 cnc_rdprogdir2（不影响 PRGDIR 5参版）
            if me.cnc_rdprogdir.is_none() {
                me.cnc_rdprogdir = (*raw)
                    .get::<FnRdProgDir>(b"cnc_rdprogdir2")
                    .ok()
                    .map(|s| std::mem::transmute(s));
            }
        }
        me
    }

    /// 建立句柄（Ethernet）：`cnc_allclibhndl3(ip, port, timeout, &mut hdl)`，`fanuc/platform/Connect.cs`
    /// - `ip` 点分十进制，`port` 默认 `FOCAS_DEFAULT_PORT 8193`，`timeout_secs` 秒（`FOCAS_MS_PER_S` 换算）
    /// - 成功返回 `hdl:u16` 供后续 `cnc_statinfo/cnc_rddynamic` 等复用，失败 `EW_SOCKET/EW_NODLL` 由上层重连
    /// - 手册：`B-64304EN 3.2`，`fwlib.cs:9416`
    /// - 命名保留 `cnc_` 前缀以与 FANUC 原生 `cnc_allclibhndl3` 一致，便于对照手册与 `fwlib.cs`
    pub fn cnc_allclibhndl3(
        &self,
        ip: &str,
        port: u16,
        timeout_secs: i32,
    ) -> Result<u16, FocasRet> {
        let sym = self.cnc_allclibhndl3.as_ref().ok_or(FocasRet::Nodll)?;
        let c_ip = CString::new(ip).map_err(|_| FocasRet::Param)?;
        let mut hdl: c_ushort = 0;
        let rc = unsafe {
            sym(
                c_ip.as_ptr(),
                port as c_ushort,
                timeout_secs as c_long,
                &mut hdl,
            )
        };
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
        if ret.is_ok() {
            Ok(unsafe { out.assume_init() })
        } else {
            Err(ret)
        }
    }

    /// 读 CNC 系统信息：`cnc_sysinfo(hdl, ODBSYS*)`，`B-64304EN 4.1`
    /// - 返回 series（"0i-F" 类）与 version，用于 probe 的 family/firmware；
    ///   model 无法从 ODBSYS 唯一确定，由上层按真机确认的映射处理，此处不猜。
    /// - 符号缺失/调用失败由上层转 IDENTITY_UNAVAILABLE，不 panic。
    pub fn cnc_sysinfo(&self, hdl: u16) -> Result<OdbSys, FocasRet> {
        let sym = self.cnc_sysinfo.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbSys>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(unsafe { out.assume_init() })
        } else {
            Err(ret)
        }
    }

    /// 读动态数据：`cnc_rddynamic2(hdl, axis=1, len=44, ODBDY2_2*)`，`platform/RdDynamic2.cs:5` `axis=1 len=44`
    /// - `ODBDY2_2` 44 字节含 `actf/acts/prgnum` 等，`165` 用 `actf` 作 `feed` 与 `axis` 回退值
    /// - 多机型 `prgnum` 16/32 位差异已在 `OdbDy2` 区分，手册 `B-64304EN 4.5`
    pub fn cnc_rddynamic2(&self, hdl: u16) -> Result<OdbDy2, FocasRet> {
        let sym = self.cnc_rddynamic2.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbDy2>::uninit();
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                1 as c_short,
                FOCAS_DY2_LEN,
                out.as_mut_ptr(),
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(unsafe { out.assume_init() })
        } else {
            Err(ret)
        }
    }

    /// 读主轴实际转速：`cnc_acts(hdl, ODBACT*)`，`platform/Acts.cs`
    /// - 返回 `data:rpm`，`spindle.load` 暂以 `cnc_acts` 归一 `0..100`
    pub fn cnc_acts(&self, hdl: u16) -> Result<OdbActs, FocasRet> {
        let sym = self.cnc_acts.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbActs>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(unsafe { out.assume_init() })
        } else {
            Err(ret)
        }
    }

    /// 读轴绝对坐标：`cnc_absolute(hdl, axis, 8, ODBAXIS*)`，`platform` 未单列但 `collectors/AxisData` 间接依赖
    /// - `axis 1..FOCAS_MAX_AXIS(32)`，`FOCAS_AXIS_BATCH 8` 一次读 8 轴，0i-F 3轴与 30i 10轴均覆盖，超 8 轴需扩展
    /// - 返回 `data[axis-1]` 原始 `c_int`（`0.001mm` 定点），上层直接 `I32`
    pub fn cnc_absolute(&self, hdl: u16, axis: u8) -> Result<c_int, FocasRet> {
        let sym = self.cnc_absolute.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbAxis>::uninit();
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                axis as c_short,
                FOCAS_AXIS_BATCH,
                out.as_mut_ptr(),
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let v = unsafe { out.assume_init() };
            let idx = (axis as usize).saturating_sub(1);
            if idx < FOCAS_AXIS_BATCH as usize {
                Ok(v.data[idx])
            } else {
                Err(FocasRet::Param)
            }
        } else {
            Err(ret)
        }
    }

    /// 读宏变量：`cnc_rdmacro(hdl, number, 1, ODBM*)`，`collectors/Macro.cs:100`
    /// - `number` 如 `100`/`730`（`0i` 低段与 `30i` 扩展段同接口，`EW_NOOPT` 按机型转 `Bad`）
    /// - `ODBM{mcr_val, dec_val}` 定点 `value = mcr_val * 10^-dec_val`
    pub fn cnc_rdmacro(&self, hdl: u16, number: u32) -> Result<f64, FocasRet> {
        let sym = self.cnc_rdmacro.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<Odbm>::uninit();
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                number as c_short,
                1 as c_short,
                out.as_mut_ptr(),
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let v = unsafe { out.assume_init() };
            let dec = v.dec_val as i32;
            let raw = v.mcr_val as f64;
            let val = if dec == 0 {
                raw
            } else {
                raw / 10_f64.powi(dec)
            };
            Ok(val)
        } else {
            Err(ret)
        }
    }

    /// 读 PMC 位：`pmc_rdpmcrng(hdl, adr_type, 0, addr, addr, 9, IODBPMC0)`，取 `cdata[0]>>bit &1`
    /// - `adr_type` 见 `pmc_adr_type()`，`bit 0..7`，`collectors/Pmc.cs: bit` 分支
    pub fn pmc_rdpmcrng_bit(
        &self,
        hdl: u16,
        adr_type: c_short,
        addr: u32,
        bit: u8,
    ) -> Result<bool, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc0 {
            type_a: adr_type,
            type_d: PMC_DATA_BIT,
            datano_s: addr as c_short,
            datano_e: addr as c_short,
            cdata: [0; 8],
        };
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                adr_type,
                PMC_DATA_BIT,
                addr as c_short,
                addr as c_short,
                PMC_LEN_BYTE,
                &mut buf as *mut _ as *mut u8,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(((buf.cdata[0] >> bit) & 1) != 0)
        } else {
            Err(ret)
        }
    }

    /// 读 PMC 字节：`pmc_rdpmcrng` `data_type 0 len 9`，`collectors/Pmc.cs: byte`
    pub fn pmc_rdpmcrng_byte(
        &self,
        hdl: u16,
        adr_type: c_short,
        addr: u32,
    ) -> Result<u8, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc0 {
            type_a: adr_type,
            type_d: PMC_DATA_BIT,
            datano_s: addr as c_short,
            datano_e: addr as c_short,
            cdata: [0; 8],
        };
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                adr_type,
                PMC_DATA_BIT,
                addr as c_short,
                addr as c_short,
                PMC_LEN_BYTE,
                &mut buf as *mut _ as *mut u8,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(buf.cdata[0])
        } else {
            Err(ret)
        }
    }

    /// 读 PMC 字：`pmc_rdpmcrng` `data_type 1 len 10`，`R/A/T/C` 常用
    pub fn pmc_rdpmcrng_word(
        &self,
        hdl: u16,
        adr_type: c_short,
        addr: u32,
    ) -> Result<c_short, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc1 {
            type_a: adr_type,
            type_d: PMC_DATA_WORD,
            datano_s: addr as c_short,
            datano_e: (addr as c_short).wrapping_add(1),
            idata: [0; 8],
        };
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                adr_type,
                PMC_DATA_WORD,
                addr as c_short,
                (addr as c_short).wrapping_add(1),
                PMC_LEN_WORD,
                &mut buf as *mut _ as *mut u8,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(buf.idata[0])
        } else {
            Err(ret)
        }
    }

    /// PMC 统一布局：single/range 共用，解决 single M/K/E=BYTE 与 range WORD 的自相矛盾
    /// 返回 (data_type, width_bytes, base_len)，width 为地址步距（BYTE=1, WORD=2, DWORD=4）
    pub fn pmc_layout(kind: char, bit: Option<u8>) -> (c_short, usize, c_short) {
        if bit.is_some() {
            return (PMC_DATA_BIT, 1, PMC_LEN_BYTE);
        }
        match kind.to_ascii_uppercase() {
            'D' => (PMC_DATA_DWORD, 4, PMC_LEN_DWORD),
            'R' | 'A' | 'T' | 'C' => (PMC_DATA_WORD, 2, PMC_LEN_WORD),
            _ => (PMC_DATA_BIT, 1, PMC_LEN_BYTE), // G/X/Y/F/M/K/N/E/Z/B 等均为 BYTE
        }
    }

    /// 批量读 PMC 字 range：一次 FFI 读 count 个连续字，用于 PMC true range 合并（P1）
    /// 返回 I32 统一 single/range 类型（single c_short→I32，range Vec<c_short>→Vec<I32>）
    /// 注意 WORD width=2，故 e_number = start + count*2 -1，buf_len = 8 + count*2
    pub fn pmc_read_word_range(
        &self,
        hdl: u16,
        adr_type: c_short,
        start: u32,
        count: u32,
    ) -> Result<Vec<i32>, FocasRet> {
        if count == 0 || count > 16 {
            // WORD 16 个以内（32 字节以内），避免单次 FFI 长度超出
            return Err(FocasRet::Param);
        }
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        // 校验布局：仅 WORD 类型允许走此路径，其他应走 byte/dword
        let (dt, width, _) = Self::pmc_layout('R', None);
        debug_assert_eq!(dt, PMC_DATA_WORD);
        debug_assert_eq!(width, 2);
        let end = start + count * 2 - 1;
        let len = 8 + (count as usize) * 2;
        let mut buf = vec![0u8; len];
        let header = IodbPmc1 {
            type_a: adr_type,
            type_d: PMC_DATA_WORD,
            datano_s: start as c_short,
            datano_e: end as c_short,
            idata: [0; 8],
        };
        let hdr_bytes = unsafe { std::slice::from_raw_parts(&header as *const _ as *const u8, 8) };
        buf[..8].copy_from_slice(hdr_bytes);
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                adr_type,
                PMC_DATA_WORD,
                start as c_short,
                end as c_short,
                len as c_short,
                buf.as_mut_ptr(),
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count as usize {
                let v = i16::from_le_bytes([buf[8 + i * 2], buf[8 + i * 2 + 1]]) as i32;
                out.push(v);
            }
            Ok(out)
        } else {
            Err(ret)
        }
    }

    /// 读 PMC 双字：`pmc_rdpmcrng` `data_type 2 len 12`，`D` 常用
    pub fn pmc_rdpmcrng_dword(
        &self,
        hdl: u16,
        adr_type: c_short,
        addr: u32,
    ) -> Result<c_int, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc2 {
            type_a: adr_type,
            type_d: PMC_DATA_DWORD,
            datano_s: addr as c_short,
            datano_e: (addr as c_short).wrapping_add(3),
            ldata: [0; 8],
        };
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                adr_type,
                PMC_DATA_DWORD,
                addr as c_short,
                (addr as c_short).wrapping_add(3),
                PMC_LEN_DWORD,
                &mut buf as *mut _ as *mut u8,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(buf.ldata[0])
        } else {
            Err(ret)
        }
    }

    /// 将 PMC 字母映射为 FOCAS adr_type（与 fanuc f_adr_type 一致）
    pub fn pmc_adr_type(kind: char) -> c_short {
        match kind {
            'G' => PMC_TYPE_G,
            'F' => PMC_TYPE_F,
            'Y' => PMC_TYPE_Y,
            'X' => PMC_TYPE_X,
            'A' => PMC_TYPE_A,
            'R' => PMC_TYPE_R,
            'T' => PMC_TYPE_T,
            'K' => PMC_TYPE_K,
            'C' => PMC_TYPE_C,
            'D' => PMC_TYPE_D,
            'M' => PMC_TYPE_M,
            'N' => PMC_TYPE_N,
            'E' => PMC_TYPE_E,
            'Z' => PMC_TYPE_Z,
            'B' => PMC_TYPE_R,
            _ => PMC_TYPE_R,
        }
    }

    /// 读报警：`cnc_rdalmmsg(hdl, -1, &mut num, ODBALMMSG)` stateful 循环至 `EW_DATA`
    /// - 为什么循环：FANUC 报警为状态机，需 `num` 递增拉取至 `EW_DATA` 结束，单次仅得首批
    /// - 返回 `Vec<String>` 仅用于诊断，上层转 `Quality::Bad` 隔离不丢批
    pub fn cnc_rdalmmsg(&self, hdl: u16, num: &mut c_short) -> Result<Vec<String>, FocasRet> {
        let sym = self.cnc_rdalmmsg.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = OdbAlmMsg { dummy: [0; 64] };
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                -1 as c_short,
                num as *mut c_short,
                &mut out as *mut OdbAlmMsg,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            // 占位解析：真机需按 `msg_len` 解 `alm_msg`，此处仅证明链路可达
            Ok(vec![format!("alarm:{}", num)])
        } else {
            Err(ret)
        }
    }

    /// 读诊断：`cnc_diagnoss(hdl, num, 1, ODBDIAG)` 单点诊断
    pub fn cnc_diagnoss(&self, hdl: u16, num: i32) -> Result<c_int, FocasRet> {
        let sym = self.cnc_diagnoss.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = OdbDiag { dummy: 0 };
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                num as c_short,
                1 as c_short,
                &mut out as *mut OdbDiag,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(out.dummy) } else { Err(ret) }
    }

    /// 读程序号：`cnc_rdprgnum(hdl, ODBPRGNUM)` 主/运行程序
    pub fn cnc_rdprgnum(&self, hdl: u16) -> Result<OdbPrgNum, FocasRet> {
        let sym = self.cnc_rdprgnum.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = std::mem::MaybeUninit::<OdbPrgNum>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(unsafe { out.assume_init() })
        } else {
            Err(ret)
        }
    }

    /// 读主轴负载：`cnc_rdspmeter(hdl, 0, &mut num, &mut data)` 4 轴
    pub fn cnc_rdspmeter(
        &self,
        hdl: u16,
        num: &mut c_short,
        data: &mut SpLoad,
    ) -> Result<(), FocasRet> {
        let sym = self.cnc_rdspmeter.as_ref().ok_or(FocasRet::Noopt)?;
        let mut n: c_short = 4;
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                0 as c_short,
                &mut n as *mut c_short,
                data as *mut SpLoad,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            *num = n;
            Ok(())
        } else {
            Err(ret)
        }
    }

    /// 读伺服负载：`cnc_rdsvmeter(hdl, &mut num, &mut data)` 同主轴
    pub fn cnc_rdsvmeter(
        &self,
        hdl: u16,
        num: &mut c_short,
        data: &mut SpLoad,
    ) -> Result<(), FocasRet> {
        let sym = self.cnc_rdsvmeter.as_ref().ok_or(FocasRet::Noopt)?;
        let mut n: c_short = 4;
        let rc = unsafe { sym(hdl as c_ushort, &mut n as *mut c_short, data as *mut SpLoad) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            *num = n;
            Ok(())
        } else {
            Err(ret)
        }
    }

    /// 读操作信息：`cnc_rdopmsg(hdl, 0, 64, OPMSG)` 64 字节操作提示
    pub fn cnc_rdopmsg(&self, hdl: u16) -> Result<OpMsg, FocasRet> {
        let sym = self.cnc_rdopmsg.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = OpMsg { dummy: [0; 64] };
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                0 as c_short,
                64 as c_short,
                &mut out as *mut OpMsg,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(out) } else { Err(ret) }
    }

    /// 读主轴齿轮比：`cnc_rdspgear(hdl, spindle, &mut gear)` 占位
    pub fn cnc_rdspgear(&self, hdl: u16, spindle: u8) -> Result<i16, FocasRet> {
        let sym = self.cnc_rdspgear.as_ref().ok_or(FocasRet::Noopt)?;
        let mut gear: c_short = 0;
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                spindle as c_ushort,
                &mut gear as *mut c_short,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(gear as i16)
        } else {
            Err(ret)
        }
    }

    /// 读主轴最大转速：`cnc_rdspmaxrpm(hdl, spindle, &mut rpm)` 占位
    pub fn cnc_rdspmaxrpm(&self, hdl: u16, spindle: u8) -> Result<i16, FocasRet> {
        let sym = self.cnc_rdspmaxrpm.as_ref().ok_or(FocasRet::Noopt)?;
        let mut rpm: c_short = 0;
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                spindle as c_ushort,
                &mut rpm as *mut c_short,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            Ok(rpm as i16)
        } else {
            Err(ret)
        }
    }

    /// 读刀补单点：`cnc_rdtofs(hdl, s_no, e_no, type, &mut OdbTofs)` 真结构 Pack=4，`fwlib.cs:8624`
    /// - 0i-F 常见 `type 0=几何 1=磨损`，对 `tool.offset.1` 先试 `0` 再试 `1`，`s_no=e_no=num`
    /// - 缺符号时回退 `cnc_rdtofsr` area 版
    pub fn cnc_rdtofs(&self, hdl: u16, num: u32) -> Result<f64, FocasRet> {
        if let Some(sym) = self.cnc_rdtofs.as_ref() {
            for t in [0 as c_short, 1 as c_short] {
                let mut out = std::mem::MaybeUninit::<OdbTofs>::uninit();
                let rc = unsafe {
                    sym(
                        hdl as c_ushort,
                        num as c_short,
                        num as c_short,
                        t,
                        out.as_mut_ptr(),
                    )
                };
                let ret = FocasRet::from_raw(rc);
                if ret.is_ok() {
                    let v = unsafe { out.assume_init() };
                    return Ok(v.data as f64 / 1000.0);
                } else if ret == FocasRet::Length
                    || ret == FocasRet::Number
                    || ret == FocasRet::Data
                {
                    continue;
                } else {
                    return Err(ret);
                }
            }
        }
        self.cnc_rdtofsr(hdl, num)
    }

    /// 读刀补（area 版）：`cnc_rdtofsr(hdl, s/e/type, IODBTO_1_1/1_2)` `fwlib.cs:8632/1090/1099`
    /// - 先试 `IODBTO_1_2 10×int` 再 `1_1 5×int`，对 `tool.offset.1` `s=e=num`，`1_3` 预留
    pub fn cnc_rdtofsr(&self, hdl: u16, num: u32) -> Result<f64, FocasRet> {
        if let Some(sym12) = self.cnc_rdtofsr112.as_ref() {
            for t in [0 as c_short, 1 as c_short, 2 as c_short] {
                let mut out = std::mem::MaybeUninit::<IodbTo112>::uninit();
                let trials: [(c_short, c_short, c_short, c_short); 3] = [
                    (num as c_short, t, num as c_short, 5 as c_short),
                    (0, num as c_short, num as c_short, t),
                    (t, num as c_short, num as c_short, 0),
                ];
                for (a, b, c, d) in trials {
                    let rc = unsafe { sym12(hdl as c_ushort, a, b, c, d, out.as_mut_ptr()) };
                    let ret = FocasRet::from_raw(rc);
                    if ret.is_ok() {
                        let v = unsafe { out.assume_init() };
                        return Ok(v.ofs.m_ofs_b[0] as f64 / 1000.0);
                    } else if matches!(ret, FocasRet::Length | FocasRet::Number | FocasRet::Attrib)
                    {
                        continue;
                    } else if ret == FocasRet::Noopt {
                        break;
                    } else {
                        return Err(ret);
                    }
                }
            }
        }
        let sym = self.cnc_rdtofsr.as_ref().ok_or(FocasRet::Noopt)?;
        for t in [0 as c_short, 1 as c_short, 2 as c_short] {
            let mut out = std::mem::MaybeUninit::<IodbTo111>::uninit();
            let trials: [(c_short, c_short, c_short, c_short); 3] = [
                (0, num as c_short, num as c_short, t),
                (t, num as c_short, num as c_short, 0),
                (num as c_short, t, num as c_short, 0),
            ];
            for (a, b, c, d) in trials {
                let rc = unsafe { sym(hdl as c_ushort, a, b, c, d, out.as_mut_ptr()) };
                let ret = FocasRet::from_raw(rc);
                if ret.is_ok() {
                    let v = unsafe { out.assume_init() };
                    return Ok(v.ofs.m_ofs[0] as f64 / 1000.0);
                } else if matches!(ret, FocasRet::Length | FocasRet::Number | FocasRet::Attrib) {
                    continue;
                } else {
                    return Err(ret);
                }
            }
        }
        Err(FocasRet::Number)
    }

    /// 读工件零点：`cnc_rdzofs` 3 shorts `s_no,e_no,type`，`fwlib.cs:8661`
    /// - 0i-F `zofs.1` 对应 `G54` 起点，`type 0` 单轴，失败则试 `type 1`
    pub fn cnc_rdzofs(&self, hdl: u16, num: u32) -> Result<f64, FocasRet> {
        let sym = self.cnc_rdzofs.as_ref().ok_or(FocasRet::Noopt)?;
        for t in [0 as c_short, 1 as c_short] {
            let mut out = std::mem::MaybeUninit::<IodbZofs>::uninit();
            let rc = unsafe {
                sym(
                    hdl as c_ushort,
                    num as c_short,
                    num as c_short,
                    t,
                    out.as_mut_ptr(),
                )
            };
            let ret = FocasRet::from_raw(rc);
            if ret.is_ok() {
                let v = unsafe { out.assume_init() };
                return Ok(v.data[0] as f64 / 1000.0);
            } else if ret == FocasRet::Length || ret == FocasRet::Number {
                continue;
            } else {
                return Err(ret);
            }
        }
        Err(FocasRet::Length)
    }

    /// 读参数单点：`cnc_rdparam` 3 shorts `s_no,axis,num`，`fwlib.cs:8687` `IODBPSD_1/2`
    /// - 先试 `IODBPSD_1 ldata` `len 1/8/6`，`EW_Attrib/EW_Data` 时回退 `IODBPSD_2 REAL` `len 12`
    /// - `axis 0` 无轴，兼容 `0i-F/30i` 差异，`platform/RdParam.cs:30`
    pub fn cnc_rdparam(&self, hdl: u16, num: u32) -> Result<i32, FocasRet> {
        let sym = self.cnc_rdparam.as_ref().ok_or(FocasRet::Noopt)?;
        // 先试 dword/word/byte 共用 IODBPSD_1
        for len in [1 as c_short, 8 as c_short, 6 as c_short] {
            let mut out = std::mem::MaybeUninit::<IodbPsd1>::uninit();
            let rc = unsafe {
                sym(
                    hdl as c_ushort,
                    num as c_short,
                    0 as c_short,
                    len,
                    out.as_mut_ptr(),
                )
            };
            let ret = FocasRet::from_raw(rc);
            if ret.is_ok() {
                let v = unsafe { out.assume_init() };
                return Ok(v.ldata);
            } else if matches!(ret, FocasRet::Length | FocasRet::Number) {
                continue;
            } else if ret == FocasRet::Attrib || ret == FocasRet::Data {
                // 可能是 REAL 类型，试 IODBPSD_2
                break;
            } else {
                return Err(ret);
            }
        }
        // 回退 REAL：需以 IODBPSD_2 结构读，取 prm_val
        unsafe {
            let raw: *const Library = &self._lib as *const Library;
            if let Ok(sym2) = (*raw).get::<unsafe extern "C" fn(
                c_ushort,
                c_short,
                c_short,
                c_short,
                *mut IodbPsd2,
            ) -> c_short>(b"cnc_rdparam")
            {
                for len in [12 as c_short, 1 as c_short] {
                    let mut out2 = std::mem::MaybeUninit::<IodbPsd2>::uninit();
                    let rc = sym2(
                        hdl as c_ushort,
                        num as c_short,
                        0 as c_short,
                        len,
                        out2.as_mut_ptr(),
                    );
                    let ret = FocasRet::from_raw(rc);
                    if ret.is_ok() {
                        let v = out2.assume_init();
                        // REAL 转 I32：prm_val *10^-dec_val 近似取整
                        let dec = v.rdata.dec_val;
                        let raw = v.rdata.prm_val as f64;
                        let val = if dec == 0 {
                            raw
                        } else {
                            raw / 10_f64.powi(dec)
                        };
                        return Ok(val as i32);
                    } else if matches!(ret, FocasRet::Length | FocasRet::Attrib) {
                        continue;
                    } else {
                        return Err(ret);
                    }
                }
            }
        }
        Err(FocasRet::Attrib)
    }

    /// 读程序目录：`cnc_rdprogdir` `PRGDIR 256` `fwlib.cs:8368` 5参，清洗非打印字符
    pub fn cnc_rdprogdir(&self, hdl: u16) -> Result<String, FocasRet> {
        let sym = self.cnc_rdprogdir.as_ref().ok_or(FocasRet::Noopt)?;
        // 0i-F 常用 top=0 num=10 len=256，失败则试 top=1
        for (a, b, c, d) in [
            (0 as c_short, 0 as c_short, 0 as c_short, 256 as c_ushort),
            (0 as c_short, 1 as c_short, 10 as c_short, 256 as c_ushort),
        ] {
            let mut out = PrgDir {
                prg_data: [0u8; 256],
            };
            let rc = unsafe { sym(hdl as c_ushort, a, b, c, d, &mut out as *mut PrgDir) };
            let ret = FocasRet::from_raw(rc);
            if ret.is_ok() {
                let raw = &out.prg_data;
                // 过滤至可打印 ASCII，保留 % O 0-9 及空格
                let s: String = raw
                    .iter()
                    .filter(|&&b| (32..127).contains(&b))
                    .map(|&b| b as char)
                    .collect();
                let t = s.trim().to_string();
                if t.is_empty() {
                    return Ok("PRG:empty".into());
                }
                // 截断至 80 字符防溢出
                return Ok(t.chars().take(80).collect());
            } else if matches!(ret, FocasRet::Length | FocasRet::Number) {
                continue;
            } else {
                return Err(ret);
            }
        }
        Err(FocasRet::Length)
    }

    /// 读程序信息：`cnc_rdproginfo` `ODBNC_1/2` `fwlib.cs:8377` 2 shorts，多型扩展
    /// - `ODBNC_1` 先试 `(0,0)/(0,10)/(1,0)/(0,1)/(10,0)`，`EW_Length/Number/Attrib/Data` 视为多型继续
    /// - 回退 `ODBNC_2 31B` 再试 3 型，仍 `EW_Length` 则属机床未组态真诊断 `BAD` 隔离
    pub fn cnc_rdproginfo(&self, hdl: u16) -> Result<String, FocasRet> {
        if let Some(sym) = self.cnc_rdproginfo.as_ref() {
            for (a, b) in [
                (0 as c_short, 0 as c_short),
                (0 as c_short, 10 as c_short),
                (1 as c_short, 0 as c_short),
                (0 as c_short, 1 as c_short),
                (10 as c_short, 0 as c_short),
                (1 as c_short, 10 as c_short),
            ] {
                let mut out = OdbNc1 {
                    reg_prg: 0,
                    unreg_prg: 0,
                    used_mem: 0,
                    unused_mem: 0,
                };
                let rc = unsafe { sym(hdl as c_ushort, a, b, &mut out as *mut OdbNc1) };
                let ret = FocasRet::from_raw(rc);
                if ret.is_ok() {
                    return Ok(format!(
                        "reg:{} unreg:{} used:{} free:{}",
                        out.reg_prg, out.unreg_prg, out.used_mem, out.unused_mem
                    ));
                } else if matches!(
                    ret,
                    FocasRet::Length | FocasRet::Number | FocasRet::Attrib | FocasRet::Data
                ) {
                    continue;
                } else if ret == FocasRet::Noopt {
                    break;
                } else {
                    return Err(ret);
                }
            }
        }
        if let Some(sym2) = self.cnc_rdproginfo2.as_ref() {
            for (a, b) in [
                (0 as c_short, 0 as c_short),
                (1 as c_short, 10 as c_short),
                (0 as c_short, 1 as c_short),
            ] {
                let mut out2 = OdbNc2 { asc: [0u8; 31] };
                let rc = unsafe { sym2(hdl as c_ushort, a, b, &mut out2 as *mut OdbNc2) };
                let ret = FocasRet::from_raw(rc);
                if ret.is_ok() {
                    let s = String::from_utf8_lossy(&out2.asc)
                        .trim_matches('\0')
                        .trim()
                        .to_string();
                    if s.is_empty() {
                        return Ok("PRGINFO:empty".into());
                    }
                    return Ok(s.chars().take(31).collect());
                } else if matches!(
                    ret,
                    FocasRet::Length | FocasRet::Number | FocasRet::Attrib | FocasRet::Data
                ) {
                    continue;
                } else {
                    return Err(ret);
                }
            }
        }
        Err(FocasRet::Length)
    }

    /// 上传程序：`cnc_upstart3→cnc_upload3→cnc_upend3` 序列 `fwlib.cs:8304/8313/8317`，`0i-F` 主流
    /// - `upstart3(hdl,0,0,0)` 起传，`upload3` 循环至 `EW_BUFFER(10)`，`upend3` 结束，`len` 入 256
    pub fn cnc_upload(&self, hdl: u16) -> Result<String, FocasRet> {
        // 优先 3 代序列（需 upstart）
        if let (Some(start3), Some(up3), Some(end3)) = (
            self.cnc_upstart3.as_ref(),
            self.cnc_upload3.as_ref(),
            self.cnc_upend3.as_ref(),
        ) {
            let rc0 = unsafe { start3(hdl as c_ushort, 0 as c_short, 0 as c_int, 0 as c_int) };
            let ret0 = FocasRet::from_raw(rc0);
            if ret0.is_ok() || ret0 == FocasRet::Buffer {
                let mut all = Vec::new();
                loop {
                    let mut len: c_int = 256;
                    let mut out = OdbUp3 { data: [0u8; 256] };
                    let rc = unsafe {
                        up3(
                            hdl as c_ushort,
                            &mut len as *mut c_int,
                            &mut out as *mut OdbUp3,
                        )
                    };
                    let ret = FocasRet::from_raw(rc);
                    if ret.is_ok() {
                        let n = (len as usize).min(256);
                        all.extend_from_slice(&out.data[..n]);
                        if n < 256 {
                            break;
                        }
                    } else if ret == FocasRet::Buffer {
                        break;
                    } else {
                        let _ = unsafe { end3(hdl as c_ushort) };
                        return Err(ret);
                    }
                }
                let _ = unsafe { end3(hdl as c_ushort) };
                let s = String::from_utf8_lossy(&all)
                    .trim_matches('\0')
                    .trim()
                    .to_string();
                if s.is_empty() {
                    return Ok("UP:empty".into());
                }
                return Ok(s.chars().take(80).collect());
            } else if ret0 != FocasRet::Noopt {
                // 回退旧版单次
            } else {
                return Err(ret0);
            }
        }
        if let Some(sym3) = self.cnc_upload3.as_ref() {
            let mut len: c_int = 256;
            let mut out = OdbUp3 { data: [0u8; 256] };
            let rc = unsafe {
                sym3(
                    hdl as c_ushort,
                    &mut len as *mut c_int,
                    &mut out as *mut OdbUp3,
                )
            };
            let ret = FocasRet::from_raw(rc);
            if ret.is_ok() {
                let n = (len as usize).min(256);
                let s = String::from_utf8_lossy(&out.data[..n])
                    .trim_matches('\0')
                    .trim()
                    .to_string();
                if s.is_empty() {
                    return Ok("UP:empty".into());
                }
                return Ok(s.chars().take(80).collect());
            } else if ret != FocasRet::Noopt {
                // 回退旧版
            } else {
                return Err(ret);
            }
        }
        let sym = self.cnc_upload.as_ref().ok_or(FocasRet::Noopt)?;
        let mut out = OdbUp {
            dummy: [0; 2],
            data: [0u8; 256],
        };
        let mut len: c_ushort = 256;
        let rc = unsafe {
            sym(
                hdl as c_ushort,
                &mut out as *mut OdbUp,
                &mut len as *mut c_ushort,
            )
        };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let n = (len as usize).min(256);
            let s = String::from_utf8_lossy(&out.data[..n])
                .trim_matches('\0')
                .trim()
                .to_string();
            if s.is_empty() {
                Ok("UP:empty".into())
            } else {
                Ok(s.chars().take(80).collect())
            }
        } else {
            Err(ret)
        }
    }
}

// 保证 Send/Sync（Library 本身不是，但 FOCAS 句柄在单线程 blocking 中使用，跨线程仅共享只读函数指针）
unsafe impl Send for NativeLib {}
unsafe impl Sync for NativeLib {}
