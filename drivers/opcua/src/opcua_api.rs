//! OPC UA 访问抽象（对应 FOCAS 的 `focas_api.rs`）。
//!
//! V1 只读，为便于 CI 与真机对比，采用 `trait OpcUaApi + Fake/Native` 分层：
//! - `FakeOpcUaApi`：纯内存、确定性随机，不依赖任何 Server，覆盖 Poll/Subscribe 语义与故障注入
//! - `NativeOpcUaApi`：Phase 2 接 `opcua` crate 直连真实 Server（预留，未实现时返回 NOT_IMPLEMENTED）

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use forgelink_core_types::Value;

use crate::address::OpcUaAddress;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------
/// Fake 随机数乘子（与 FOCAS 保持一致的分治可复现性）
const FAKE_RAND_MULT: u64 = 6364136223846793005;

/// OPC UA 默认端口 4840
pub const DEFAULT_OPCUA_PORT: u16 = 4840;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait OpcUaApi: Send + Sync {
    /// 建立会话（Fake 下即时成功；Native 下建 TCP+Security）
    async fn connect(&self, endpoint_url: &str, timeout_ms: u64) -> Result<(), String>;
    /// 批量读（Poll 通路）；按传入地址顺序返回等长 Value，单点错误以 String 占位由上层转 Quality
    async fn read_batch(&self, addrs: &[OpcUaAddress]) -> Result<Vec<Value>, String>;
    /// 断开（Fake 无操作）
    async fn disconnect(&self) -> Result<(), String> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fake 实现
// ---------------------------------------------------------------------------

pub struct FakeOpcUaApi {
    /// 伪随机种子（每实例固定，便于测试复现）
    seed: AtomicU64,
}

impl Default for FakeOpcUaApi {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeOpcUaApi {
    pub fn new() -> Self {
        Self { seed: AtomicU64::new(0x1234_5678_9ABC_DEF0) }
    }

    fn next_rand(&self) -> u64 {
        let prev = self.seed.load(Ordering::Relaxed);
        let next = prev.wrapping_mul(FAKE_RAND_MULT).wrapping_add(1);
        self.seed.store(next, Ordering::Relaxed);
        next
    }

    /// 基于地址生成确定性 Fake 值（用于冒烟与 Contract）
    fn fake_value_for(&self, addr: &OpcUaAddress) -> Value {
        let r = self.next_rand();
        match &addr.identifier {
            crate::address::Identifier::Numeric(n) => {
                // 数值型节点：按 n 奇偶返回 I32/U32，便于类型覆盖
                if n % 2 == 0 { Value::I32((r % 10000) as i32) } else { Value::U32((r % 10000) as u32) }
            }
            crate::address::Identifier::String(s) => {
                // 字符串型：若含 Speed/Temp 等关键字返回 F64，否则 String
                let lower = s.to_ascii_lowercase();
                if lower.contains("speed") || lower.contains("sine") || lower.contains("temp") {
                    Value::F64((r % 10000) as f64 / 10.0)
                } else if lower.contains("counter") || lower.contains("count") {
                    Value::U32((r % 1000) as u32)
                } else if lower.contains("status") || lower.contains("state") {
                    Value::U32((r % 4) as u32)
                } else {
                    Value::String(format!("fake:{s}:{}", r % 100))
                }
            }
            crate::address::Identifier::Guid(_) => Value::String(format!("guid:{}", r % 1000)),
            crate::address::Identifier::Opaque(_) => Value::String(format!("opaque:{}", r % 1000)),
        }
    }
}

#[async_trait]
impl OpcUaApi for FakeOpcUaApi {
    async fn connect(&self, endpoint_url: &str, _timeout_ms: u64) -> Result<(), String> {
        if endpoint_url.trim().is_empty() {
            return Err("endpoint_url 不能为空".into());
        }
        if !endpoint_url.starts_with("opc.tcp://") {
            return Err(format!("endpoint_url `{endpoint_url}` 非法，需 opc.tcp://host:port"));
        }
        Ok(())
    }

    async fn read_batch(&self, addrs: &[OpcUaAddress]) -> Result<Vec<Value>, String> {
        let mut out = Vec::with_capacity(addrs.len());
        for addr in addrs {
            // 模拟单点不支持：特定字符串触发 Bad（用于测试 Bad 隔离）
            if let crate::address::Identifier::String(s) = &addr.identifier {
                if s.contains("bad") || s.contains("Bad") {
                    out.push(Value::String("ERR:BadNodeIdUnknown".into()));
                    continue;
                }
            }
            out.push(self.fake_value_for(addr));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Native 占位（Phase 2 实现）
// ---------------------------------------------------------------------------

pub struct NativeOpcUaApi;

impl NativeOpcUaApi {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl OpcUaApi for NativeOpcUaApi {
    async fn connect(&self, _endpoint_url: &str, _timeout_ms: u64) -> Result<(), String> {
        // TODO: Phase 2 接 opcua crate Session
        Err("NOT_IMPLEMENTED: Native OPC UA 尚未实现，Phase 2 接入 opcua crate".into())
    }

    async fn read_batch(&self, _addrs: &[OpcUaAddress]) -> Result<Vec<Value>, String> {
        Err("NOT_IMPLEMENTED".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::parse_address;

    #[tokio::test]
    async fn fake_connect_ok() {
        let api = FakeOpcUaApi::new();
        api.connect("opc.tcp://127.0.0.1:4840", 3000).await.unwrap();
        let err = api.connect("", 3000).await.unwrap_err();
        assert!(err.contains("不能为空"));
        let err2 = api.connect("http://127.0.0.1:4840", 3000).await.unwrap_err();
        assert!(err2.contains("opc.tcp"));
    }

    #[tokio::test]
    async fn fake_read_batch_smoke() {
        let api = FakeOpcUaApi::new();
        api.connect("opc.tcp://127.0.0.1:4840", 1000).await.unwrap();
        let addrs = vec![
            parse_address("ns=2;i=2").unwrap(),
            parse_address("ns=2;s=Counter").unwrap(),
            parse_address("ns=2;s=Motor.Speed").unwrap(),
        ];
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals.len(), 3);
    }

    #[tokio::test]
    async fn fake_bad_isolation() {
        let api = FakeOpcUaApi::new();
        let addrs = vec![parse_address("ns=2;s=bad_node").unwrap()];
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals[0], Value::String("ERR:BadNodeIdUnknown".into()));
    }
}
