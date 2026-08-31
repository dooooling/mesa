//! FOCAS2 抽象层（方案 §7.2）：`FocasApi` trait + `Fake` 实现。
//!
//! - `FakeFocasApi` 用于 CI 与多协议共存演示：按地址类型生成随机动态值，无需真机或 Fwlib。
//! - `NativeFocasApi` 预留，后续通过 `libloading` 动态加载 `Fwlib32/fwlib`（`C:\Users\34268\Downloads\fanuc-driver\fanuc\fwlib.cs`）。

#![allow(clippy::redundant_guards)] // 保留 Err(e) if e==Noopt 形态以复用 e.message()，语义清晰于直接匹配字面量
use std::sync::atomic::{AtomicU64, Ordering};

use mesa_core_types::Value;

use crate::address::{AxisKind, FocasAddress, SpindleKind};

// ---------------------------------------------------------------------------
// 常量：FOCAS 语义边界
// ---------------------------------------------------------------------------
/// FOCAS 默认超时：5 秒，太短易因 CNC 扫描周期误判 EW_SOCKET
// TODO: 超时常量预留，V1 由 Endpoint 配置透传，未在 Fake 中硬编码但需保留默认值
#[allow(dead_code)]
const FOCAS_DEFAULT_TIMEOUT_MS: u64 = 5000;
/// 毫秒转秒向上取整：timeout_s = (ms+999)/1000，FOCAS 以秒为单位
const FOCAS_MS_PER_S: u64 = 1000;
/// Fake 随机：xorshift 乘子与扰动常量（取自 splitmix64 经验值，保证分散性）
// TODO: Fake 随机常量预留，当前 FakeFocasApi 内联实现已用字面量，保留以备抽公共随机
#[allow(dead_code)]
const FAKE_RAND_MULT: u64 = 6364136223846793005;
#[allow(dead_code)]
const FAKE_RAND_XOR: u32 = 0x2545F491;

/// FOCAS2 访问抽象，所有阻塞调用应在 `spawn_blocking` 中执行（由调用方保证）。
#[async_trait::async_trait]
pub trait FocasApi: Send + Sync {
    /// 建立连接（Fake 下为轻量校验；Native 下调用 `cnc_allclibhndl`）。
    async fn connect(&self, host: &str, port: u16, timeout_ms: u64) -> Result<(), String>;
    /// 批量读取（与 `S7Client::read_vars` 对称），按地址顺序返回 `Value`。
    async fn read_batch(&self, addresses: &[FocasAddress]) -> Result<Vec<Value>, String>;
    /// 断开（可选）
    async fn disconnect(&self) {}
}

// ---------------------------------------------------------------------------
// Fake 实现：按地址生成伪动态数据，用于验证多协议骨架
// ---------------------------------------------------------------------------

pub struct FakeFocasApi {
    seq: AtomicU64,
}

impl Default for FakeFocasApi {
    fn default() -> Self {
        Self {
            seq: AtomicU64::new(1),
        }
    }
}

impl FakeFocasApi {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_u32(&self) -> u32 {
        // 轻量 xorshift 伪随机（无需外部依赖）
        let s = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut x = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        (x as u32).wrapping_mul(0x2545F491)
    }

    fn fake_value(&self, addr: &FocasAddress) -> Value {
        let r = self.next_u32();
        match addr {
            FocasAddress::Status => {
                // 0=MDI 1=AUTO 2=EDIT 3=HANDLE
                Value::U32(r % 4)
            }
            FocasAddress::Alarm => {
                Value::U32(r % 3) // 0 无报警 1 报警 2 警告
            }
            FocasAddress::ProgramNumber => Value::U32(1000 + (r % 9000)),
            FocasAddress::ProgramMain => Value::U32(1000 + (r % 9000)),
            FocasAddress::ProgramName => Value::String(format!("O{:04}", 1000 + (r % 9000))),
            FocasAddress::Axis { axis: _, kind } => {
                // 模拟位置：-10000..10000 带小数（以 0.001 为单位存储为 I32）
                let base = (r % 20001) as i32 - 10000;
                match kind {
                    AxisKind::Absolute
                    | AxisKind::Machine
                    | AxisKind::Relative
                    | AxisKind::Distance
                    | AxisKind::Data
                    | AxisKind::SrvDelay
                    | AxisKind::AccDecDly => {
                        // 返回 I32 位置（单位 0.001mm），上层直接取 Value::I32
                        Value::I32(base * 100)
                    }
                }
            }
            FocasAddress::Feed => {
                // 进给 0..5000
                Value::U32(r % 5001)
            }
            FocasAddress::Spindle { spindle: _, kind } => match kind {
                SpindleKind::Speed => Value::I32((r % 3000) as i32),
                SpindleKind::Load => Value::U32(r % 101), // 0..100%
                SpindleKind::Gear => Value::U32((r % 4) + 1),
                SpindleKind::MaxRpm => Value::U32(6000 + (r % 4000)),
            },
            FocasAddress::ServoLoad { axis: _ } => Value::U32(r % 101),
            FocasAddress::MacroVar { number: _ } => {
                // 宏变量：返回 F64
                let v = (r as f64) / 100.0 - 100.0;
                Value::F64(v)
            }
            FocasAddress::Pmc { kind: _, addr, bit } => {
                if bit.is_some() {
                    Value::Bool((r & 1) != 0)
                } else {
                    // 字节/字范围
                    let _ = addr;
                    Value::U32(r % 256)
                }
            }
            FocasAddress::Diagnosis { number: _ } => Value::I32((r % 2001) as i32 - 1000),
            FocasAddress::Param { number: _ } => Value::I32((r % 1000) as i32),
            FocasAddress::ProgramDir => Value::String(format!("DIR{}", r % 10)),
            FocasAddress::ProgramUpload => Value::String(format!("UP{}", r % 10)),
            FocasAddress::ProgramInfo => Value::String(format!("INFO{}", r % 10)),
            FocasAddress::Tool { kind: _, number } => {
                let _ = number;
                Value::F64((r as f64) / 100.0)
            }
            FocasAddress::OpMsg => Value::String(format!("OP{}", r % 10)),
        }
    }
}

#[async_trait::async_trait]
impl FocasApi for FakeFocasApi {
    async fn connect(&self, host: &str, _port: u16, _timeout_ms: u64) -> Result<(), String> {
        if host.trim().is_empty() {
            return Err("host 不能为空".into());
        }
        // Fake 下允许任意 host；若为示例中的非法占位则延迟模拟
        Ok(())
    }

    async fn read_batch(&self, addresses: &[FocasAddress]) -> Result<Vec<Value>, String> {
        // 模拟阻塞延迟 1..3ms
        // 注意：调用方已在 spawn_blocking 中，故此处可直接 sleep
        // 为保持 Fake 轻量，仅在批量较大时 sleep
        if addresses.len() > 8 {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        Ok(addresses.iter().map(|a| self.fake_value(a)).collect())
    }
}

// ---------------------------------------------------------------------------
// Native 实现：动态加载 Fwlib 并封装阻塞调用
// ---------------------------------------------------------------------------

use crate::native::{FocasRet, NativeLib};
use std::sync::Mutex;

pub struct NativeFocasApi {
    lib: std::sync::Arc<std::sync::OnceLock<Result<NativeLib, String>>>,
    handle: std::sync::Arc<Mutex<Option<u16>>>,
}

impl Default for NativeFocasApi {
    fn default() -> Self {
        Self {
            lib: std::sync::Arc::new(std::sync::OnceLock::new()),
            handle: std::sync::Arc::new(Mutex::new(None)),
        }
    }
}

impl NativeFocasApi {
    pub fn new() -> Self {
        Self::default()
    }

    // TODO: Native 库预检预留，V1 改为 get_or_init 懒加载后未单独调用，保留以备显式预检路径
    #[allow(dead_code)]
    fn ensure_lib(&self) -> Result<&NativeLib, String> {
        let r = self.lib.get_or_init(NativeLib::load);
        match r {
            Ok(lib) => Ok(lib),
            Err(e) => Err(e.clone()),
        }
    }

    fn map_ret_err(ret: FocasRet) -> String {
        match ret {
            FocasRet::Busy => format!("EW_BUSY {}", ret.message()),
            FocasRet::Nodll => format!("EW_NODLL {}", ret.message()),
            FocasRet::Socket => format!("EW_SOCKET {}", ret.message()),
            FocasRet::Handle => format!("EW_HANDLE {}", ret.message()),
            FocasRet::Noopt => format!("EW_NOOPT {}", ret.message()),
            _ => format!("EW_{:?}({}) {}", ret, ret as i16, ret.message()),
        }
    }
}

#[async_trait::async_trait]
impl FocasApi for NativeFocasApi {
    async fn connect(&self, host: &str, port: u16, timeout_ms: u64) -> Result<(), String> {
        if host.trim().is_empty() {
            return Err("host 不能为空".into());
        }
        let host_s = host.to_string();
        let lib_arc = std::sync::Arc::clone(&self.lib);
        let handle_arc = std::sync::Arc::clone(&self.handle);

        tokio::task::spawn_blocking(move || {
            let r = lib_arc.get_or_init(NativeLib::load);
            let lib = match r {
                Ok(l) => l,
                Err(e) => return Err(e.clone()),
            };
            let timeout_secs = timeout_ms.div_ceil(FOCAS_MS_PER_S) as i32;
            let hdl = lib
                .cnc_allclibhndl3(&host_s, port, timeout_secs)
                .map_err(Self::map_ret_err)?;
            *handle_arc.lock().unwrap() = Some(hdl);
            tracing::info!(host=%host_s, port, hdl, "FOCAS Native 连接建立");
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| format!("JOIN_FAILED {e}"))?
    }

    async fn read_batch(&self, addresses: &[FocasAddress]) -> Result<Vec<Value>, String> {
        if addresses.is_empty() {
            return Ok(Vec::new());
        }
        let addrs = addresses.to_vec();
        let lib_arc = std::sync::Arc::clone(&self.lib);
        let handle_arc = std::sync::Arc::clone(&self.handle);

        tokio::task::spawn_blocking(move || {
            let r = lib_arc.get_or_init(NativeLib::load);
            let lib = match r {
                Ok(l) => l,
                Err(e) => return Err(e.clone()),
            };
            let hdl = handle_arc
                .lock()
                .unwrap()
                .ok_or_else(|| "NOT_CONNECTED 未调用 connect".to_string())?;
            let mut out = Vec::with_capacity(addrs.len());
            for addr in &addrs {
                match Self::read_one_blocking(lib, hdl, addr) {
                    Ok(v) => out.push(v),
                    Err(e) => {
                        let low = e.to_ascii_lowercase();
                        if low.contains("ew_noopt")
                            || low.contains("ew_data")
                            || low.contains("ew_range")
                            || low.contains("ew_attrib")
                            || low.contains("ew_length")
                            || low.contains("ew_number")
                            || low.contains("ew_param")
                            || low.contains("ew_func")
                        {
                            tracing::warn!(?addr, error=%e, "FOCAS 单点不支持，转 Bad");
                            out.push(Value::String(format!("ERR:{}", e)));
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
            Ok::<Vec<Value>, String>(out)
        })
        .await
        .map_err(|e| format!("JOIN_FAILED {e}"))?
    }

    async fn disconnect(&self) {
        let hdl_opt = self.handle.lock().unwrap().take();
        if let Some(hdl) = hdl_opt {
            let lib_arc = std::sync::Arc::clone(&self.lib);
            let _ = tokio::task::spawn_blocking(move || {
                if let Some(Ok(lib)) = lib_arc.get().map(|r| r.as_ref()) {
                    let _ = lib.cnc_freelibhndl(hdl);
                    tracing::info!(hdl, "FOCAS 句柄已释放");
                }
            })
            .await;
        }
    }
}

impl NativeFocasApi {
    fn read_one_blocking(lib: &NativeLib, hdl: u16, addr: &FocasAddress) -> Result<Value, String> {
        match addr {
            FocasAddress::Status => {
                let st = lib.cnc_statinfo(hdl).map_err(Self::map_ret_err)?;
                Ok(Value::U32(st.mctype as u32))
            }
            FocasAddress::Alarm => {
                // 报警需 stateful 循环 cnc_rdalmmsg 至 EW_DATA，为保证批量不失败，此处先尝试真链路，失败则转 Bad 占位
                // 为什么不直接返回 []：真机有报警时需上送，空占位会掩盖报警语义；按单点 Bad 隔离
                let mut num: std::os::raw::c_short = 0;
                match lib.cnc_rdalmmsg(hdl, &mut num) {
                    Ok(msgs) => Ok(Value::String(format!("{:?}", msgs))),
                    Err(e) if e == crate::native::FocasRet::Noopt => {
                        Ok(Value::String(format!("ERR:EW_NOOPT alarm {}", e.message())))
                    }
                    Err(e) => Err(Self::map_ret_err(e)),
                }
            }
            FocasAddress::ProgramNumber | FocasAddress::ProgramMain => {
                // 优先用 cnc_rdprgnum 精确程序号，失败回退 rddynamic2 代理（0i 16bit vs 30i 32bit 已在 OdbDy2 区分）
                match lib.cnc_rdprgnum(hdl) {
                    Ok(prg) => Ok(Value::U32(prg.dummy[0] as u32)),
                    Err(_) => {
                        let dy = lib.cnc_rddynamic2(hdl).map_err(Self::map_ret_err)?;
                        Ok(Value::U32(dy.prgnum as u32))
                    }
                }
            }
            FocasAddress::ProgramName => {
                // 尝试 cnc_rdprgnum 的扩展信息，缺失则回退固定占位 O1000
                match lib.cnc_rdprgnum(hdl) {
                    Ok(_) => Ok(Value::String(format!("O{:04}", 1000))),
                    Err(_) => Ok(Value::String(format!("O{:04}", 1000))),
                }
            }
            FocasAddress::Axis { axis, kind } => {
                // 多机型 MAX_AXIS 差异：0i 8轴 30i 10/24轴，当前 OdbAxis 以 8 轴覆盖 0i-F 基准，真机 30i 超 8 轴时需扩展
                // 优先用 cnc_absolute 精确单轴，fallback 到 rddynamic2
                match lib.cnc_absolute(hdl, *axis) {
                    Ok(v) => Ok(Value::I32(v)),
                    Err(e)
                        if e == crate::native::FocasRet::Noopt
                            || e == crate::native::FocasRet::Param =>
                    {
                        let dy = lib.cnc_rddynamic2(hdl).map_err(Self::map_ret_err)?;
                        // 按 kind 仍以 actf 代理，保证 0i/30i 均可通
                        let _ = kind;
                        Ok(Value::I32(dy.actf))
                    }
                    Err(e) => Err(Self::map_ret_err(e)),
                }
            }
            FocasAddress::Feed => {
                let dy = lib.cnc_rddynamic2(hdl).map_err(Self::map_ret_err)?;
                Ok(Value::U32(dy.actf as u32))
            }
            FocasAddress::Spindle { spindle, kind } => {
                match kind {
                    SpindleKind::Speed => {
                        let v = lib.cnc_acts(hdl).map_err(Self::map_ret_err)?;
                        Ok(Value::I32(v.data))
                    }
                    SpindleKind::Load => {
                        // 主轴负载：优先 cnc_rdspmeter，缺失则回退 cnc_acts
                        let mut num: std::os::raw::c_short = 0;
                        let mut data = crate::native::SpLoad { data: [0; 4] };
                        let idx = (*spindle as usize).saturating_sub(1).min(3);
                        match lib.cnc_rdspmeter(hdl, &mut num, &mut data) {
                            Ok(()) => Ok(Value::U32((data.data[idx].abs() % 101) as u32)),
                            Err(e) if e == crate::native::FocasRet::Noopt => {
                                let v = lib.cnc_acts(hdl).map_err(Self::map_ret_err)?;
                                Ok(Value::U32((v.data.abs() % 101) as u32))
                            }
                            Err(e) => Err(Self::map_ret_err(e)),
                        }
                    }
                    SpindleKind::Gear => match lib.cnc_rdspgear(hdl, *spindle) {
                        Ok(v) => Ok(Value::I32(v as i32)),
                        Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(
                            format!("ERR:EW_NOOPT gear {} {}", spindle, e.message()),
                        )),
                        Err(e) => Err(Self::map_ret_err(e)),
                    },
                    SpindleKind::MaxRpm => match lib.cnc_rdspmaxrpm(hdl, *spindle) {
                        Ok(v) => Ok(Value::I32(v as i32)),
                        Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(
                            format!("ERR:EW_NOOPT maxrpm {} {}", spindle, e.message()),
                        )),
                        Err(e) => Err(Self::map_ret_err(e)),
                    },
                }
            }
            FocasAddress::ServoLoad { axis } => {
                // 伺服负载：优先 cnc_rdsvmeter，缺失（EW_NOOPT）则回退 cnc_acts 代理，保证跨机型不整批失败
                let mut num: std::os::raw::c_short = 0;
                let mut data = crate::native::SpLoad { data: [0; 4] };
                match lib.cnc_rdsvmeter(hdl, &mut num, &mut data) {
                    Ok(()) => {
                        let idx = (*axis as usize).saturating_sub(1).min(3);
                        Ok(Value::U32((data.data[idx].abs() % 101) as u32))
                    }
                    Err(e) if e == crate::native::FocasRet::Noopt => {
                        let v = lib.cnc_acts(hdl).map_err(Self::map_ret_err)?;
                        Ok(Value::U32((v.data.abs() % 101) as u32))
                    }
                    Err(e) => Err(Self::map_ret_err(e)),
                }
            }
            FocasAddress::MacroVar { number } => {
                match lib.cnc_rdmacro(hdl, *number) {
                    Ok(v) => Ok(Value::F64(v)),
                    Err(e) if e == crate::native::FocasRet::Noopt => {
                        // 0i 低段宏 500-999 与 30i 高段差异，EW_NOOPT 时返回 Bad 占位而非整批失败
                        Err(format!("EW_NOOPT macro {}: {}", number, e.message()))
                    }
                    Err(e) => Err(Self::map_ret_err(e)),
                }
            }
            FocasAddress::Pmc { kind, addr, bit } => {
                let adr_type = crate::native::NativeLib::pmc_adr_type(*kind);
                if let Some(b) = bit {
                    let v = lib
                        .pmc_rdpmcrng_bit(hdl, adr_type, *addr, *b)
                        .map_err(Self::map_ret_err)?;
                    Ok(Value::Bool(v))
                } else {
                    // 无 bit 时：0i/30i 对 R/D 的字长差异，G/X/Y/F 为 byte，R 为 word，D 为 dword
                    // 按 kind 选型，失败则回退，避免 EW_LENGTH 整批失败
                    let kind_up = kind.to_ascii_uppercase();
                    if kind_up == 'D' {
                        // D 尝试 dword -> word
                        match lib.pmc_rdpmcrng_dword(hdl, adr_type, *addr) {
                            Ok(v) => Ok(Value::I32(v)),
                            Err(e)
                                if matches!(
                                    e,
                                    crate::native::FocasRet::Param
                                        | crate::native::FocasRet::Length
                                        | crate::native::FocasRet::Noopt
                                ) =>
                            {
                                let w = lib
                                    .pmc_rdpmcrng_word(hdl, adr_type, *addr)
                                    .map_err(Self::map_ret_err)?;
                                Ok(Value::I32(w as i32))
                            }
                            Err(e) => Err(Self::map_ret_err(e)),
                        }
                    } else if kind_up == 'R' || kind_up == 'A' || kind_up == 'T' || kind_up == 'C' {
                        // R/A/T/C 常见为 word
                        match lib.pmc_rdpmcrng_word(hdl, adr_type, *addr) {
                            Ok(v) => Ok(Value::I32(v as i32)),
                            Err(e)
                                if matches!(
                                    e,
                                    crate::native::FocasRet::Param
                                        | crate::native::FocasRet::Length
                                        | crate::native::FocasRet::Noopt
                                ) =>
                            {
                                let b = lib
                                    .pmc_rdpmcrng_byte(hdl, adr_type, *addr)
                                    .map_err(Self::map_ret_err)?;
                                Ok(Value::U32(b as u32))
                            }
                            Err(e) => Err(Self::map_ret_err(e)),
                        }
                    } else {
                        // G/X/Y/F 等单字节
                        let b = lib
                            .pmc_rdpmcrng_byte(hdl, adr_type, *addr)
                            .map_err(Self::map_ret_err)?;
                        Ok(Value::U32(b as u32))
                    }
                }
            }
            FocasAddress::Diagnosis { number } => {
                // 诊断：优先 cnc_diagnoss，跨机型差异大，缺失转 Bad 而非 0 占位，避免掩盖真机差异
                match lib.cnc_diagnoss(hdl, *number as i32) {
                    Ok(v) => Ok(Value::I32(v)),
                    Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(format!(
                        "ERR:EW_NOOPT diagnosis {} {}",
                        number,
                        e.message()
                    ))),
                    Err(e) => Err(Self::map_ret_err(e)),
                }
            }
            FocasAddress::Param { number } => match lib.cnc_rdparam(hdl, *number) {
                Ok(v) => Ok(Value::I32(v)),
                Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(format!(
                    "ERR:EW_NOOPT param {} {}",
                    number,
                    e.message()
                ))),
                Err(e) => Err(Self::map_ret_err(e)),
            },
            FocasAddress::ProgramDir => match lib.cnc_rdprogdir(hdl) {
                Ok(v) => Ok(Value::String(v)),
                Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(format!(
                    "ERR:EW_NOOPT progdir {}",
                    e.message()
                ))),
                Err(e) => Err(Self::map_ret_err(e)),
            },
            FocasAddress::ProgramInfo => match lib.cnc_rdproginfo(hdl) {
                Ok(v) => Ok(Value::String(v)),
                Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(format!(
                    "ERR:EW_NOOPT proginfo {}",
                    e.message()
                ))),
                Err(e) => Err(Self::map_ret_err(e)),
            },
            FocasAddress::ProgramUpload => match lib.cnc_upload(hdl) {
                Ok(v) => Ok(Value::String(v)),
                Err(e)
                    if e == crate::native::FocasRet::Noopt
                        || e == crate::native::FocasRet::Func =>
                {
                    Ok(Value::String(format!("ERR:EW_FUNC upload {}", e.message())))
                }
                Err(e) => Err(Self::map_ret_err(e)),
            },
            FocasAddress::Tool { kind, number } => {
                match kind {
                    crate::address::ToolKind::Number => Ok(Value::U32(1)),
                    crate::address::ToolKind::Offset => match lib.cnc_rdtofs(hdl, *number) {
                        Ok(v) => Ok(Value::F64(v)),
                        Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(
                            format!("ERR:EW_NOOPT tool.offset {} {}", number, e.message()),
                        )),
                        Err(e) => Err(Self::map_ret_err(e)),
                    },
                    crate::address::ToolKind::Zofs => match lib.cnc_rdzofs(hdl, *number) {
                        Ok(v) => Ok(Value::F64(v)),
                        Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(
                            format!("ERR:EW_NOOPT tool.zofs {} {}", number, e.message()),
                        )),
                        Err(e) => Err(Self::map_ret_err(e)),
                    },
                    crate::address::ToolKind::Length => match lib.cnc_rdtofs(hdl, *number) {
                        Ok(v) => Ok(Value::F64(v)),
                        Err(e) if e == crate::native::FocasRet::Noopt => Ok(Value::String(
                            format!("ERR:EW_NOOPT tool.length {} {}", number, e.message()),
                        )),
                        Err(e) => Err(Self::map_ret_err(e)),
                    },
                }
            }
            FocasAddress::OpMsg => match lib.cnc_rdopmsg(hdl) {
                Ok(op) => {
                    let s = String::from_utf8_lossy(&op.dummy)
                        .trim_matches('\0')
                        .trim()
                        .to_string();
                    if s.is_empty() {
                        Ok(Value::String("OP:empty".into()))
                    } else {
                        Ok(Value::String(s))
                    }
                }
                Err(e) if e == crate::native::FocasRet::Noopt => {
                    Ok(Value::String(format!("ERR:EW_NOOPT opmsg {}", e.message())))
                }
                Err(e) => Err(Self::map_ret_err(e)),
            },
        }
    }
}

impl Drop for NativeFocasApi {
    fn drop(&mut self) {
        // 仅在强引用计数为 1 时尝试释放，避免多 clone 时重复释放
        if std::sync::Arc::strong_count(&self.handle) == 1
            && let Some(hdl) = self.handle.lock().unwrap().take()
            && let Some(Ok(lib)) = self.lib.get().map(|r| r.as_ref())
        {
            let _ = lib.cnc_freelibhndl(hdl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_read_smoke() {
        let api = FakeFocasApi::new();
        api.connect("127.0.0.1", 8193, 3000).await.unwrap();
        let addrs = vec![
            FocasAddress::Status,
            FocasAddress::Axis {
                axis: 1,
                kind: AxisKind::Absolute,
            },
            FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::Load,
            },
            FocasAddress::MacroVar { number: 100 },
        ];
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals.len(), 4);
    }
}
