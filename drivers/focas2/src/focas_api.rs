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
// Native 占位（TODO）：后续接入 Fwlib FFI
// ---------------------------------------------------------------------------

/// 预留：通过 `libloading` 加载 `Fwlib32.dll` / `libfwlib32-*.so` 并封装 `cnc_*`。
/// 当前返回 `NOT_IMPLEMENTED`，避免在无真机环境误用。
pub struct NativeFocasApi;

#[async_trait::async_trait]
impl FocasApi for NativeFocasApi {
    async fn connect(&self, _host: &str, _port: u16, _timeout_ms: u64) -> Result<(), String> {
        Err("NativeFocasApi 未实现：需接入 Fwlib FFI（见 fanuc/fwlib.cs）".into())
    }
    async fn read_batch(&self, _addresses: &[FocasAddress]) -> Result<Vec<Value>, String> {
        Err("NativeFocasApi 未实现".into())
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
