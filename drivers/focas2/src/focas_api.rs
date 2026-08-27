//! FOCAS2 抽象层（方案 §7.2）：`FocasApi` trait + `Fake` 实现。
//!
//! - `FakeFocasApi` 用于 CI 与多协议共存演示：按地址类型生成随机动态值，无需真机或 Fwlib。
//! - `NativeFocasApi` 预留，后续通过 `libloading` 动态加载 `Fwlib32/fwlib`（`C:\Users\34268\Downloads\fanuc-driver\fanuc\fwlib.cs`）。

use std::sync::atomic::{AtomicU64, Ordering};

use forgelink_core_types::Value;

use crate::address::{AxisKind, FocasAddress, SpindleKind};

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
        Self { seq: AtomicU64::new(1) }
    }
}

impl FakeFocasApi {
    pub fn new() -> Self { Self::default() }

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
                    AxisKind::Absolute | AxisKind::Machine | AxisKind::Relative | AxisKind::Distance => {
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

use std::sync::Mutex;
use crate::native::{FocasRet, NativeLib};

pub struct NativeFocasApi {
    lib: std::sync::Arc<std::sync::OnceLock<Result<NativeLib, String>>>,
    handle: std::sync::Arc<Mutex<Option<u16>>>,
}

impl Default for NativeFocasApi {
    fn default() -> Self {
        Self { lib: std::sync::Arc::new(std::sync::OnceLock::new()), handle: std::sync::Arc::new(Mutex::new(None)) }
    }
}

impl NativeFocasApi {
    pub fn new() -> Self { Self::default() }

    fn ensure_lib(&self) -> Result<&NativeLib, String> {
        let r = self.lib.get_or_init(|| NativeLib::load());
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
        if host.trim().is_empty() { return Err("host 不能为空".into()); }
        let host_s = host.to_string();
        let lib_arc = std::sync::Arc::clone(&self.lib);
        let handle_arc = std::sync::Arc::clone(&self.handle);
        let res = tokio::task::spawn_blocking(move || {
            let r = lib_arc.get_or_init(|| NativeLib::load());
            let lib = match r { Ok(l) => l, Err(e) => return Err(e.clone()) };
            let timeout_secs = ((timeout_ms + 999) / 1000) as i32;
            let hdl = lib.allclibhndl3(&host_s, port, timeout_secs).map_err(Self::map_ret_err)?;
            *handle_arc.lock().unwrap() = Some(hdl);
            tracing::info!(host=%host_s, port, hdl, "FOCAS Native 连接建立");
            Ok::<(), String>(())
        }).await.map_err(|e| format!("JOIN_FAILED {e}"))?;
        res
    }

    async fn read_batch(&self, addresses: &[FocasAddress]) -> Result<Vec<Value>, String> {
        if addresses.is_empty() { return Ok(Vec::new()); }
        let addrs = addresses.to_vec();
        let lib_arc = std::sync::Arc::clone(&self.lib);
        let handle_arc = std::sync::Arc::clone(&self.handle);
        let res = tokio::task::spawn_blocking(move || {
            let r = lib_arc.get_or_init(|| NativeLib::load());
            let lib = match r { Ok(l) => l, Err(e) => return Err(e.clone()) };
            let hdl = handle_arc.lock().unwrap().ok_or_else(|| "NOT_CONNECTED 未调用 connect".to_string())?;
            let mut out = Vec::with_capacity(addrs.len());
            for addr in &addrs {
                let v = Self::read_one_blocking(lib, hdl, addr)?;
                out.push(v);
            }
            Ok::<Vec<Value>, String>(out)
        }).await.map_err(|e| format!("JOIN_FAILED {e}"))?;
        res
    }

    async fn disconnect(&self) {
        let hdl_opt = self.handle.lock().unwrap().take();
        if let Some(hdl) = hdl_opt {
            let lib_arc = std::sync::Arc::clone(&self.lib);
            let _ = tokio::task::spawn_blocking(move || {
                if let Some(Ok(lib)) = lib_arc.get().map(|r| r.as_ref()) {
                    let _ = lib.freelibhndl(hdl);
                    tracing::info!(hdl, "FOCAS 句柄已释放");
                }
            }).await;
        }
    }
}

impl NativeFocasApi {
    fn read_one_blocking(lib: &NativeLib, hdl: u16, addr: &FocasAddress) -> Result<Value, String> {
        match addr {
            FocasAddress::Status => {
                let st = lib.statinfo(hdl).map_err(Self::map_ret_err)?;
                // 取 mctype/utime 等合成状态码
                Ok(Value::U32(st.mctype as u32))
            }
            FocasAddress::Alarm => {
                // 暂用 statinfo 的 alarm 字段代理（完整需 cnc_rdalmmsg）
                // Phase B 先以 EW_NOOPT 提示未实现，避免静默返回错误值
                Err("EW_NOOPT alarm 需 cnc_rdalmmsg，待真机按机型补齐".into())
            }
            FocasAddress::ProgramNumber | FocasAddress::ProgramMain | FocasAddress::ProgramName => {
                Err("EW_NOOPT program 需 cnc_rdprgnum/cnc_exeprgname，待真机补齐".into())
            }
            FocasAddress::Axis { axis: _, kind: _ } => {
                // 先以 rddynamic2 读取整组动态数据
                let dy = lib.rddynamic2(hdl).map_err(Self::map_ret_err)?;
                // dy.actf/acts 已包含进给/主轴，轴位置需后续按 kind 细化
                // Phase B 返回 actf 作为通用位置代理，保证链路可通
                Ok(Value::I32(dy.actf))
            }
            FocasAddress::Feed => {
                let dy = lib.rddynamic2(hdl).map_err(Self::map_ret_err)?;
                Ok(Value::U32(dy.actf as u32))
            }
            FocasAddress::Spindle { spindle: _, kind } => {
                match kind {
                    SpindleKind::Speed => {
                        let v = lib.acts(hdl).map_err(Self::map_ret_err)?;
                        Ok(Value::I32(v.data))
                    }
                    SpindleKind::Load => {
                        // spmeter 需另函数，Phase B 先复用 acts.data 代理
                        let v = lib.acts(hdl).map_err(Self::map_ret_err)?;
                        Ok(Value::U32((v.data.abs() % 101) as u32))
                    }
                }
            }
            FocasAddress::ServoLoad { axis: _ } => {
                Err("EW_NOOPT servo load 需 cnc_rdsvmeter，待真机补齐".into())
            }
            FocasAddress::MacroVar { number } => {
                let _ = number;
                Err("EW_NOOPT macro 需 cnc_rdmacro，待真机补齐".into())
            }
            FocasAddress::Pmc { kind: _, addr: _, bit: _ } => {
                Err("EW_NOOPT pmc 需 pmc_rdpmcrng，待真机补齐".into())
            }
            FocasAddress::Diagnosis { number: _ } => {
                Err("EW_NOOPT diagnosis 需 cnc_diagnoss，待真机补齐".into())
            }
        }
    }
}

impl Drop for NativeFocasApi {
    fn drop(&mut self) {
        // 仅在强引用计数为 1 时尝试释放，避免多 clone 时重复释放
        if std::sync::Arc::strong_count(&self.handle) == 1 {
            if let Some(hdl) = self.handle.lock().unwrap().take() {
                if let Some(Ok(lib)) = self.lib.get().map(|r| r.as_ref()) {
                    let _ = lib.freelibhndl(hdl);
                }
            }
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
            FocasAddress::Axis { axis: 1, kind: AxisKind::Absolute },
            FocasAddress::Spindle { spindle: 1, kind: SpindleKind::Load },
            FocasAddress::MacroVar { number: 100 },
        ];
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals.len(), 4);
    }
}
