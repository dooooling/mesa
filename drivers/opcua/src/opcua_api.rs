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
// Native 实现（async-opcua 0.19 Client，Phase 2 真连）
// ---------------------------------------------------------------------------

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

use opcua_client::{ClientBuilder, IdentityToken, Session};
use opcua_types::{
    ByteString, Guid, NodeId as OpcNodeId, UAString, Variant, DataValue, ReadValueId,
    TimestampsToReturn, StatusCode,
};

fn to_opc_node_id(addr: &OpcUaAddress) -> Result<OpcNodeId, String> {
    match &addr.identifier {
        crate::address::Identifier::Numeric(n) => Ok(OpcNodeId::new(addr.namespace, *n)),
        crate::address::Identifier::String(s) => Ok(OpcNodeId::new(addr.namespace, UAString::from(s.as_str()))),
        crate::address::Identifier::Guid(g) => {
            let guid = g.parse::<Guid>().map_err(|e| format!("GUID 解析失败 {g}: {e:?}"))?;
            Ok(OpcNodeId::new(addr.namespace, guid))
        }
        crate::address::Identifier::Opaque(b64) => {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("Opaque Base64 解码失败 {b64}: {e}"))?;
            Ok(OpcNodeId::new(addr.namespace, ByteString::from(bytes)))
        }
    }
}

fn variant_to_value(v: &Variant) -> Option<Value> {
    match v {
        Variant::Empty => None,
        Variant::Boolean(b) => Some(Value::Bool(*b)),
        Variant::SByte(n) => Some(Value::I32(*n as i32)),
        Variant::Byte(n) => Some(Value::U32(*n as u32)),
        Variant::Int16(n) => Some(Value::I32(*n as i32)),
        Variant::UInt16(n) => Some(Value::U32(*n as u32)),
        Variant::Int32(n) => Some(Value::I32(*n)),
        Variant::UInt32(n) => Some(Value::U32(*n)),
        Variant::Int64(n) => Some(Value::I64(*n)),
        Variant::UInt64(n) => Some(Value::U64(*n)),
        Variant::Float(n) => Some(Value::F32(*n)),
        Variant::Double(n) => Some(Value::F64(*n)),
        Variant::String(s) => {
            let str_val = s.as_ref().to_string();
            Some(Value::String(str_val))
        }
        Variant::ByteString(bs) => {
            if let Some(bytes) = &bs.value {
                Some(Value::Bytes(bytes.clone()))
            } else {
                Some(Value::Bytes(vec![]))
            }
        }
        Variant::Guid(g) => Some(Value::String(g.to_string())),
        Variant::DateTime(dt) => Some(Value::String(format!("{:?}", dt))),
        Variant::LocalizedText(t) => {
            let txt = t.text.as_ref().to_string();
            if txt.is_empty() {
                Some(Value::String(format!("{:?}", t)))
            } else {
                Some(Value::String(txt))
            }
        }
        Variant::Array(arr) => Some(Value::String(format!("{:?}", arr))),
        Variant::StatusCode(sc) => Some(Value::String(format!("{:?}", sc))),
        _ => Some(Value::String(format!("{:?}", v))),
    }
}

struct NativeInner {
    endpoint_url: String,
    session: Option<Arc<Session>>,
    _handle: Option<tokio::task::JoinHandle<opcua_types::StatusCode>>,
}

pub struct NativeOpcUaApi {
    inner: Arc<AsyncMutex<Option<NativeInner>>>,
}

impl NativeOpcUaApi {
    pub fn new() -> Self {
        Self { inner: Arc::new(AsyncMutex::new(None)) }
    }

    async fn ensure_connected(&self, endpoint_url: &str, timeout_ms: u64) -> Result<Arc<Session>, String> {
        {
            let guard = self.inner.lock().await;
            if let Some(inner) = guard.as_ref() {
                if inner.endpoint_url == endpoint_url {
                    if let Some(sess) = &inner.session {
                        return Ok(sess.clone());
                    }
                }
            }
        }
        self.connect_inner(endpoint_url, timeout_ms).await
    }

    async fn connect_inner(&self, endpoint_url: &str, timeout_ms: u64) -> Result<Arc<Session>, String> {
        let timeout = Duration::from_millis(timeout_ms);
        let mut client = ClientBuilder::new()
            .application_name("ForgeLink OPC UA")
            .application_uri("urn:forgelink:opcua")
            .trust_server_certs(true)
            .create_sample_keypair(false)
            .session_retry_limit(1)
            .client()
            .map_err(|e| format!("ClientBuilder 失败: {e:?}"))?;

        use opcua_types::{EndpointDescription, MessageSecurityMode, UserTokenPolicy};
        let endpoint: EndpointDescription = (
            endpoint_url,
            "None",
            MessageSecurityMode::None,
            UserTokenPolicy::anonymous(),
        )
            .into();

        let connect_fut = client.connect_to_matching_endpoint(endpoint, IdentityToken::Anonymous);
        let (session, event_loop) = tokio::time::timeout(timeout, connect_fut)
            .await
            .map_err(|_| format!("连接超时 {timeout_ms}ms {endpoint_url}"))?
            .map_err(|e| format!("连接失败 {endpoint_url}: {e:?}"))?;

        let handle = event_loop.spawn();
        let wait_fut = session.wait_for_connection();
        tokio::time::timeout(timeout, wait_fut)
            .await
            .map_err(|_| format!("等待连接超时 {endpoint_url}"))?;

        let sess_clone = session.clone();
        let mut guard = self.inner.lock().await;
        *guard = Some(NativeInner {
            endpoint_url: endpoint_url.to_string(),
            session: Some(session.clone()),
            _handle: Some(handle),
        });
        Ok(sess_clone)
    }
}

#[async_trait]
impl OpcUaApi for NativeOpcUaApi {
    async fn connect(&self, endpoint_url: &str, timeout_ms: u64) -> Result<(), String> {
        self.ensure_connected(endpoint_url, timeout_ms).await.map(|_| ())
    }

    async fn read_batch(&self, addrs: &[OpcUaAddress]) -> Result<Vec<Value>, String> {
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| "尚未连接，请先 connect".to_string())?
        };
        let mut nodes_to_read = Vec::with_capacity(addrs.len());
        for addr in addrs {
            let nid = to_opc_node_id(addr)?;
            nodes_to_read.push(ReadValueId::from(nid));
        }
        let data_values: Vec<DataValue> = session
            .read(&nodes_to_read, TimestampsToReturn::Both, 0.0)
            .await
            .map_err(|e| format!("read 失败: {e:?}"))?;

        let mut out = Vec::with_capacity(data_values.len());
        for dv in data_values {
            if let Some(status) = dv.status {
                if status != StatusCode::Good {
                    out.push(Value::String(format!("ERR:{:?}", status)));
                    continue;
                }
            }
            if let Some(variant) = dv.value {
                if let Some(val) = variant_to_value(&variant) {
                    out.push(val);
                } else {
                    out.push(Value::String(format!("ERR:BadNoValue {:?}", dv.status)));
                }
            } else {
                out.push(Value::String(format!("ERR:BadNoValue {:?}", dv.status)));
            }
        }
        Ok(out)
    }

    async fn disconnect(&self) -> Result<(), String> {
        let mut guard = self.inner.lock().await;
        *guard = None;
        Ok(())
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
