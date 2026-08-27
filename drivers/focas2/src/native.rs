//! FOCAS2 Native FFI 层：动态加载 `Fwlib32/fwlib` 并封装阻塞调用。
//!
//! 设计要点（对齐 `fanuc/fwlib.cs` 与 `overview.md:31` 矩阵）：
//! - 运行时按 OS/Arch 选择库文件：`win → Fwlib32.dll / FWLIB64.dll`，`linux x64 → libfwlib32-linux-x64.so`，`linux armv7 → libfwlib32-linux-armv7.so`
//! - 使用 `libloading` 延迟加载，缺库时返回 `EW_NODLL=-15` 可重试错误，而非 panic
//! - 全部 FOCAS 调用为阻塞式，调用方必须在 `spawn_blocking` 中执行（由 `lib.rs` 保证）
//! - 错误码映射见 `FocasLibConstants` 与 `focas_ret` `fwlib.cs:56`，便于上层按 `Connection/Address/Unsupported` 分类

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_short, c_ushort};
use std::path::Path;

use libloading::{Library, Symbol};

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
// 关键结构体（简化，仅覆盖 Phase B 所需字段，按 Pack=4 对齐）
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbSt {
    pub dummy: [c_short; 2],
    pub tctype: c_short,
    pub dtype: c_short,
    pub mctype: c_short,
    pub utime: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbActs {
    pub dummy: [c_short; 2],
    pub data: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Faxis {
    pub absolute: [c_int; 8],
    pub machine: [c_int; 8],
    pub relative: [c_int; 8],
    pub distance: [c_int; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Oaxis {
    pub absolute: c_int,
    pub machine: c_int,
    pub relative: c_int,
    pub distance: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbDy1 {
    pub dummy: c_short,
    pub axis: c_short,
    pub alarm: c_short,
    pub prgnum: c_short,
    pub prgmnum: c_short,
    pub seqnum: c_int,
    pub actf: c_int,
    pub acts: c_int,
    pub pos: Faxis,
}

/// ODBDY2_2：对应 fanuc-driver RdDynamic2 axis=1 length=44（单轴），Pack=4 时正好 44 字节
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbDy2 {
    pub dummy: c_short,
    pub axis: c_short,
    pub alarm: c_int,
    pub prgnum: c_int,
    pub prgmnum: c_int,
    pub seqnum: c_int,
    pub actf: c_int,
    pub acts: c_int,
    pub pos: Oaxis,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OdbAxis {
    pub dummy: c_short,
    pub type_: c_short,
    pub data: [c_int; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Odbm {
    pub datano: c_short,
    pub dummy: c_short,
    pub mcr_val: c_int,
    pub dec_val: c_short,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc0 {
    pub type_a: c_short,
    pub type_d: c_short,
    pub datano_s: c_short,
    pub datano_e: c_short,
    pub cdata: [u8; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc1 {
    pub type_a: c_short,
    pub type_d: c_short,
    pub datano_s: c_short,
    pub datano_e: c_short,
    pub idata: [c_short; 8],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IodbPmc2 {
    pub type_a: c_short,
    pub type_d: c_short,
    pub datano_s: c_short,
    pub datano_e: c_short,
    pub ldata: [c_int; 8],
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
}

impl NativeLib {
    /// 按当前 OS/Arch 探测并加载库，失败返回明确错误（用于上层转 `CONNECT_FAILED`/`EW_NODLL`）
    pub fn load() -> Result<Self, String> {
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
                match unsafe { Library::new(&tp) } {
                    Ok(lib) => {
                        tracing::info!(lib=%tp, "FOCAS Native 库加载成功");
                        return Ok(Self::from_library(lib));
                    }
                    Err(e) => {
                        last_err = format!("{tp}: {e}");
                        continue;
                    }
                }
            }
            // 也尝试系统路径直接加载（如已安装到 PATH/LD_LIBRARY_PATH）
            let name = Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
            if let Ok(lib) = unsafe { Library::new(&name) } {
                tracing::info!(lib=%name, "FOCAS Native 库从系统路径加载成功");
                return Ok(Self::from_library(lib));
            }
            last_err = format!("{}: not found, last: {}", p, last_err);
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
        }
        me
    }

    /// 建立句柄（Ethernet）：对标 `cnc_allclibhndl3(ip, port, timeout, &mut hdl)`
    pub fn allclibhndl3(&self, ip: &str, port: u16, timeout_secs: i32) -> Result<u16, FocasRet> {
        let sym = self.cnc_allclibhndl3.as_ref().ok_or(FocasRet::Nodll)?;
        let c_ip = CString::new(ip).map_err(|_| FocasRet::Param)?;
        let mut hdl: c_ushort = 0;
        let rc = unsafe { sym(c_ip.as_ptr(), port as c_ushort, timeout_secs as c_long, &mut hdl) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(hdl) } else { Err(ret) }
    }

    pub fn freelibhndl(&self, hdl: u16) -> Result<(), FocasRet> {
        if let Some(sym) = self.cnc_freelibhndl.as_ref() {
            let rc = unsafe { sym(hdl as c_ushort) };
            let ret = FocasRet::from_raw(rc);
            if ret.is_ok() { Ok(()) } else { Err(ret) }
        } else {
            Ok(())
        }
    }

    pub fn statinfo(&self, hdl: u16) -> Result<OdbSt, FocasRet> {
        let sym = self.cnc_statinfo.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbSt>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(unsafe { out.assume_init() }) } else { Err(ret) }
    }

    pub fn rddynamic2(&self, hdl: u16) -> Result<OdbDy2, FocasRet> {
        let sym = self.cnc_rddynamic2.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbDy2>::uninit();
        // 对齐 fanuc-driver：axis=1（单路径首轴），length=44（sizeof ODBDY2_2），Pack=4 时 44 字节
        let rc = unsafe { sym(hdl as c_ushort, 1 as c_short, 44 as c_short, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(unsafe { out.assume_init() }) } else { Err(ret) }
    }

    pub fn acts(&self, hdl: u16) -> Result<OdbActs, FocasRet> {
        let sym = self.cnc_acts.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbActs>::uninit();
        let rc = unsafe { sym(hdl as c_ushort, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(unsafe { out.assume_init() }) } else { Err(ret) }
    }

    pub fn absolute(&self, hdl: u16, axis: u8) -> Result<c_int, FocasRet> {
        let sym = self.cnc_absolute.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<OdbAxis>::uninit();
        // cnc_absolute(hdl, axis, length=8, buf) 读取 8 轴，取对应轴位
        let rc = unsafe { sym(hdl as c_ushort, axis as c_short, 8 as c_short, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let v = unsafe { out.assume_init() };
            let idx = (axis as usize).saturating_sub(1);
            if idx < 8 { Ok(v.data[idx]) } else { Err(FocasRet::Param) }
        } else { Err(ret) }
    }

    pub fn rdmacro(&self, hdl: u16, number: u32) -> Result<f64, FocasRet> {
        let sym = self.cnc_rdmacro.as_ref().ok_or(FocasRet::Nodll)?;
        let mut out = std::mem::MaybeUninit::<Odbm>::uninit();
        // cnc_rdmacro(hdl, number, length=1, buf)
        let rc = unsafe { sym(hdl as c_ushort, number as c_short, 1 as c_short, out.as_mut_ptr()) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() {
            let v = unsafe { out.assume_init() };
            // mcr_val + dec_val 组合为浮点：value = mcr_val * 10^-dec_val
            let dec = v.dec_val as i32;
            let raw = v.mcr_val as f64;
            let val = if dec == 0 { raw } else { raw / 10_f64.powi(dec) };
            Ok(val)
        } else { Err(ret) }
    }

    pub fn pmc_bit(&self, hdl: u16, adr_type: c_short, addr: u32, bit: u8) -> Result<bool, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc0 { type_a: adr_type, type_d: 0, datano_s: addr as c_short, datano_e: addr as c_short, cdata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, 0 as c_short, addr as c_short, addr as c_short, 9 as c_short, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(((buf.cdata[0] >> bit) & 1) != 0) } else { Err(ret) }
    }

    pub fn pmc_byte(&self, hdl: u16, adr_type: c_short, addr: u32) -> Result<u8, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc0 { type_a: adr_type, type_d: 0, datano_s: addr as c_short, datano_e: addr as c_short, cdata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, 0 as c_short, addr as c_short, addr as c_short, 9 as c_short, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(buf.cdata[0]) } else { Err(ret) }
    }

    pub fn pmc_word(&self, hdl: u16, adr_type: c_short, addr: u32) -> Result<c_short, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc1 { type_a: adr_type, type_d: 1, datano_s: addr as c_short, datano_e: (addr as c_short).wrapping_add(1), idata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, 1 as c_short, addr as c_short, (addr as c_short).wrapping_add(1), 10 as c_short, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(buf.idata[0]) } else { Err(ret) }
    }

    pub fn pmc_dword(&self, hdl: u16, adr_type: c_short, addr: u32) -> Result<c_int, FocasRet> {
        let sym = self.pmc_rdpmcrng.as_ref().ok_or(FocasRet::Nodll)?;
        let mut buf = IodbPmc2 { type_a: adr_type, type_d: 2, datano_s: addr as c_short, datano_e: (addr as c_short).wrapping_add(3), ldata: [0; 8] };
        let rc = unsafe { sym(hdl as c_ushort, adr_type, 2 as c_short, addr as c_short, (addr as c_short).wrapping_add(3), 12 as c_short, &mut buf as *mut _ as *mut u8) };
        let ret = FocasRet::from_raw(rc);
        if ret.is_ok() { Ok(buf.ldata[0]) } else { Err(ret) }
    }

    pub fn pmc_adr_type(kind: char) -> c_short {
        match kind {
            'G' => 0, 'F' => 1, 'Y' => 2, 'X' => 3, 'A' => 4, 'R' => 5, 'T' => 6, 'K' => 7, 'C' => 8, 'D' => 9, 'M' => 10, 'N' | 'E' => 11, 'Z' => 12, _ => 5,
        }
    }
}

// 保证 Send/Sync（Library 本身不是，但 FOCAS 句柄在单线程 blocking 中使用，跨线程仅共享只读函数指针）
unsafe impl Send for NativeLib {}
unsafe impl Sync for NativeLib {}
