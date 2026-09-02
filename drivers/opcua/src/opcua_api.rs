//! OPC UA 访问抽象（对应 FOCAS 的 `focas_api.rs`）。
//!
//! V1 只读，为便于 CI 与真机对比，采用 `trait OpcUaApi + Fake/Native` 分层：
//! - `FakeOpcUaApi`：纯内存、确定性随机，不依赖任何 Server，覆盖 Poll/Subscribe 语义与故障注入
//! - `NativeOpcUaApi`：基于 `async-opcua` 直连真实 Server

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use mesa_core_types::Value;

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

/// 订阅数据变更事件（由 DataChangeCallback 转发）
#[derive(Debug)]
pub struct DataChangeEvent {
    /// 客户端句柄（对应创建时的 client_handle，Fake 下为索引+1，Native 下为真实 handle）
    pub client_handle: u32,
    pub data_value: DataValue,
}

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
    /// 订阅（Subscribe 通路）：创建 Subscription + MonitoredItems，返回 subscription_id 与数据变更通道
    /// KeepAlive 自然不产生事件，无需上层额外过滤
    async fn subscribe(
        &self,
        addrs: &[OpcUaAddress],
        publishing_interval_ms: u64,
        sampling_interval_ms: u64,
        queue_size: u32,
        discard_oldest: bool,
    ) -> Result<(u32, tokio::sync::mpsc::Receiver<DataChangeEvent>), String> {
        let _ = (
            addrs,
            publishing_interval_ms,
            sampling_interval_ms,
            queue_size,
            discard_oldest,
        );
        Err("NOT_IMPLEMENTED: subscribe 未实现".into())
    }
    async fn unsubscribe(&self, subscription_id: u32) -> Result<(), String> {
        let _ = subscription_id;
        Ok(())
    }
    /// 浏览节点（§7.3 Browse）：返回引用描述
    async fn browse(&self, node: &OpcUaAddress) -> Result<Vec<String>, String> {
        let _ = node;
        Err("NOT_IMPLEMENTED: browse 未实现".into())
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
        Self {
            seed: AtomicU64::new(0x1234_5678_9ABC_DEF0),
        }
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
                if n % 2 == 0 {
                    Value::I32((r % 10000) as i32)
                } else {
                    Value::U32((r % 10000) as u32)
                }
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
            return Err(format!(
                "endpoint_url `{endpoint_url}` 非法，需 opc.tcp://host:port"
            ));
        }
        Ok(())
    }

    async fn read_batch(&self, addrs: &[OpcUaAddress]) -> Result<Vec<Value>, String> {
        let mut out = Vec::with_capacity(addrs.len());
        for addr in addrs {
            // 模拟单点不支持：特定字符串触发 Bad（用于测试 Bad 隔离）
            if let crate::address::Identifier::String(s) = &addr.identifier
                && (s.contains("bad") || s.contains("Bad"))
            {
                out.push(Value::String("ERR:BadNodeIdUnknown".into()));
                continue;
            }
            out.push(self.fake_value_for(addr));
        }
        Ok(out)
    }

    async fn subscribe(
        &self,
        addrs: &[OpcUaAddress],
        publishing_interval_ms: u64,
        _sampling_interval_ms: u64,
        _queue_size: u32,
        _discard_oldest: bool,
    ) -> Result<(u32, tokio::sync::mpsc::Receiver<DataChangeEvent>), String> {
        use opcua_types::{DataValue, DateTime, StatusCode, UAString, Variant};
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let addrs: Vec<OpcUaAddress> = addrs.to_vec();
        let seed_base = self.seed.load(Ordering::Relaxed);
        let tick = std::time::Duration::from_millis(publishing_interval_ms.max(10));
        // Fake 用自增 subscription_id
        let sub_id = (self.next_rand() % 10000) as u32 + 1;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tick);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut counter: u64 = 0;
            let mut local_seed = seed_base;
            loop {
                ticker.tick().await;
                counter += 1;
                // 每 7 次模拟一次 KeepAlive（不发送任何 DataChange）
                if counter.is_multiple_of(7) {
                    continue;
                }
                for (idx, addr) in addrs.iter().enumerate() {
                    let client_handle = (idx as u32) + 1;
                    // Bad 节点产生 Bad 状态
                    let is_bad = matches!(&addr.identifier, crate::address::Identifier::String(s) if s.contains("bad") || s.contains("Bad"));
                    let dv = if is_bad {
                        DataValue {
                            value: None,
                            status: Some(StatusCode::BadNodeIdUnknown),
                            source_timestamp: Some(DateTime::now()),
                            source_picoseconds: None,
                            server_timestamp: Some(DateTime::now()),
                            server_picoseconds: None,
                        }
                    } else {
                        local_seed = local_seed.wrapping_mul(FAKE_RAND_MULT).wrapping_add(1);
                        let r = local_seed;
                        let variant = match &addr.identifier {
                            crate::address::Identifier::Numeric(n) => {
                                if n % 2 == 0 {
                                    Variant::Int32((r % 10000) as i32)
                                } else {
                                    Variant::UInt32((r % 10000) as u32)
                                }
                            }
                            crate::address::Identifier::String(s) => {
                                let lower = s.to_ascii_lowercase();
                                if lower.contains("speed")
                                    || lower.contains("sine")
                                    || lower.contains("temp")
                                {
                                    Variant::Double((r % 10000) as f64 / 10.0)
                                } else if lower.contains("counter") || lower.contains("count") {
                                    Variant::UInt32((r % 1000) as u32)
                                } else {
                                    Variant::String(UAString::from(format!("fake:{s}:{}", r % 100)))
                                }
                            }
                            crate::address::Identifier::Guid(_) => {
                                Variant::String(UAString::from(format!("guid:{}", r % 1000)))
                            }
                            crate::address::Identifier::Opaque(_) => {
                                Variant::String(UAString::from(format!("opaque:{}", r % 1000)))
                            }
                        };
                        DataValue {
                            value: Some(variant),
                            status: Some(StatusCode::Good),
                            source_timestamp: Some(DateTime::now()),
                            source_picoseconds: None,
                            server_timestamp: Some(DateTime::now()),
                            server_picoseconds: None,
                        }
                    };
                    let ev = DataChangeEvent {
                        client_handle,
                        data_value: dv,
                    };
                    if tx.try_send(ev).is_err() {
                        return;
                    }
                }
            }
        });
        Ok((sub_id, rx))
    }

    async fn unsubscribe(&self, _subscription_id: u32) -> Result<(), String> {
        Ok(())
    }
    async fn browse(&self, node: &OpcUaAddress) -> Result<Vec<String>, String> {
        // Fake 浏览：基于 node 生成 2-3 个子节点名
        let base = match &node.identifier {
            crate::address::Identifier::String(s) => s.clone(),
            crate::address::Identifier::Numeric(n) => format!("i={n}"),
            _ => "node".into(),
        };
        Ok(vec![format!("{base}.Child1"), format!("{base}.Child2")])
    }
}

// ---------------------------------------------------------------------------
// Native 实现（async-opcua 0.19 Client 直连）
// ---------------------------------------------------------------------------

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

use opcua_client::{ClientBuilder, IdentityToken, Session};
use opcua_types::{
    ByteString, DataValue, Guid, NodeId as OpcNodeId, ReadValueId, StatusCode, TimestampsToReturn,
    UAString, Variant,
};

fn to_opc_node_id(addr: &OpcUaAddress) -> Result<OpcNodeId, String> {
    match &addr.identifier {
        crate::address::Identifier::Numeric(n) => Ok(OpcNodeId::new(addr.namespace, *n)),
        crate::address::Identifier::String(s) => {
            Ok(OpcNodeId::new(addr.namespace, UAString::from(s.as_str())))
        }
        crate::address::Identifier::Guid(g) => {
            let guid = g
                .parse::<Guid>()
                .map_err(|e| format!("GUID 解析失败 {g}: {e:?}"))?;
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

pub(crate) fn variant_to_value(v: &Variant) -> Option<Value> {
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
        Variant::DateTime(dt) => {
            // OPC UA DateTime ticks 1601-01-01 转 Unix ns，精确保留 SourceTimestamp (§7.3)
            // 1 tick = 100ns，Unix epoch 1970-01-01 与 1601-01-01 差 11644473600s
            let ticks = dt.ticks();
            const TICKS_PER_SEC: i64 = 10_000_000;
            const UNIX_TICKS_OFFSET: i64 = 11644473600 * TICKS_PER_SEC;
            let unix_ticks = ticks - UNIX_TICKS_OFFSET;
            let unix_ns = unix_ticks * 100;
            Some(Value::DateTime(unix_ns))
        }
        Variant::LocalizedText(t) => {
            let txt = t.text.as_ref().to_string();
            if txt.is_empty() {
                Some(Value::String(format!("{:?}", t)))
            } else {
                Some(Value::String(txt))
            }
        }
        Variant::Array(arr) => {
            // 保留 Typed Array (§9.2)，按元素类型转对应 Array Value
            let vals: Vec<Value> = arr.values.iter().filter_map(variant_to_value).collect();
            if vals.is_empty() {
                return Some(Value::String(format!("{:?}", arr)));
            }
            // 推断首元素类型
            match &vals[0] {
                Value::Bool(_) => Some(Value::BoolArray(
                    vals.into_iter()
                        .filter_map(|v| {
                            if let Value::Bool(b) = v {
                                Some(b)
                            } else {
                                None
                            }
                        })
                        .collect(),
                )),
                Value::I32(_) => Some(Value::I32Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::I32(i) = v { Some(i) } else { None })
                        .collect(),
                )),
                Value::U32(_) => Some(Value::U32Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::U32(u) = v { Some(u) } else { None })
                        .collect(),
                )),
                Value::I64(_) => Some(Value::I64Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::I64(i) = v { Some(i) } else { None })
                        .collect(),
                )),
                Value::U64(_) => Some(Value::U64Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::U64(u) = v { Some(u) } else { None })
                        .collect(),
                )),
                Value::F32(_) => Some(Value::F32Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::F32(f) = v { Some(f) } else { None })
                        .collect(),
                )),
                Value::F64(_) => Some(Value::F64Array(
                    vals.into_iter()
                        .filter_map(|v| if let Value::F64(f) = v { Some(f) } else { None })
                        .collect(),
                )),
                Value::String(_) => Some(Value::StringArray(
                    vals.into_iter()
                        .filter_map(|v| {
                            if let Value::String(s) = v {
                                Some(s)
                            } else {
                                None
                            }
                        })
                        .collect(),
                )),
                Value::DateTime(_) => Some(Value::DateTimeArray(
                    vals.into_iter()
                        .filter_map(|v| {
                            if let Value::DateTime(t) = v {
                                Some(t)
                            } else {
                                None
                            }
                        })
                        .collect(),
                )),
                _ => Some(Value::String(format!("{:?}", arr))),
            }
        }
        Variant::StatusCode(sc) => Some(Value::String(format!("{:?}", sc))),
        _ => Some(Value::String(format!("{:?}", v))),
    }
}
pub(crate) fn status_to_quality(sc: opcua_types::StatusCode) -> mesa_core_types::Quality {
    use mesa_core_types::Quality;
    if sc.is_good() {
        Quality::Good
    } else if sc.is_uncertain() {
        Quality::Uncertain
    } else {
        Quality::Bad
    }
}

struct NativeInner {
    endpoint_url: String,
    session: Option<Arc<Session>>,
    _handle: Option<tokio::task::JoinHandle<opcua_types::StatusCode>>,
}

pub struct NativeOpcUaApi {
    inner: Arc<AsyncMutex<Option<NativeInner>>>,
    pki_dir: std::path::PathBuf,
    security_policy: std::sync::Mutex<String>,
    security_mode: std::sync::Mutex<String>,
    username: std::sync::Mutex<Option<String>>,
    password: std::sync::Mutex<Option<String>>,
    certificate: std::sync::Mutex<Option<String>>,
}

impl Default for NativeOpcUaApi {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeOpcUaApi {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AsyncMutex::new(None)),
            pki_dir: Self::default_pki_dir(),
            security_policy: std::sync::Mutex::new("None".into()),
            security_mode: std::sync::Mutex::new("None".into()),
            username: std::sync::Mutex::new(None),
            password: std::sync::Mutex::new(None),
            certificate: std::sync::Mutex::new(None),
        }
    }

    pub fn new_with_pki_dir(p: impl Into<std::path::PathBuf>) -> Self {
        Self {
            inner: Arc::new(AsyncMutex::new(None)),
            pki_dir: p.into(),
            security_policy: std::sync::Mutex::new("None".into()),
            security_mode: std::sync::Mutex::new("None".into()),
            username: std::sync::Mutex::new(None),
            password: std::sync::Mutex::new(None),
            certificate: std::sync::Mutex::new(None),
        }
    }
    pub fn set_security(&self, policy: String, mode: String) {
        *self.security_policy.lock().unwrap() = policy;
        *self.security_mode.lock().unwrap() = mode;
    }
    pub fn set_credentials(&self, username: Option<String>, password: Option<String>) {
        *self.username.lock().unwrap() = username;
        *self.password.lock().unwrap() = password;
    }
    pub fn set_certificate(&self, cert: Option<String>) {
        *self.certificate.lock().unwrap() = cert;
    }

    fn default_pki_dir() -> std::path::PathBuf {
        std::env::var("MESA_OPCUA_PKI_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("data/certificates/opcua"))
    }

    fn resolve_pki_dir(&self) -> std::path::PathBuf {
        // 若 self.pki_dir 为默认占位且环境变量已设置，优先环境变量（便于 Mesad 透传）
        if let Ok(env) = std::env::var("MESA_OPCUA_PKI_DIR") {
            let env_p = std::path::PathBuf::from(env);
            // 若实例 pki_dir 非默认（显式传入），保持显式；否则用环境变量
            let def = std::path::PathBuf::from("data/certificates/opcua");
            if self.pki_dir == def {
                return env_p;
            }
        }
        self.pki_dir.clone()
    }

    async fn ensure_connected(
        &self,
        endpoint_url: &str,
        timeout_ms: u64,
    ) -> Result<Arc<Session>, String> {
        {
            let guard = self.inner.lock().await;
            if let Some(inner) = guard.as_ref()
                && inner.endpoint_url == endpoint_url
                && let Some(sess) = &inner.session
            {
                return Ok(sess.clone());
            }
        }
        self.connect_inner(endpoint_url, timeout_ms).await
    }

    async fn connect_inner(
        &self,
        endpoint_url: &str,
        timeout_ms: u64,
    ) -> Result<Arc<Session>, String> {
        let timeout = Duration::from_millis(timeout_ms);
        // pki_dir：优先环境变量 MESA_OPCUA_PKI_DIR（与 Core CertStore 同值），否则 data/certificates/opcua；显式 pki_dir 由上层通过 NativeOpcUaApi::new_with_pki_dir 注入
        let pki_dir = self.resolve_pki_dir();
        // 若指定 certificate，则优先使用该证书（路径或 thumbprint 映射），否则沿用 own/own.der 固定 PKI
        let (cert_path, key_path) = {
            let cert_opt = self.certificate.lock().unwrap().clone();
            if let Some(c) = cert_opt.filter(|s| !s.trim().is_empty()) {
                let c = c.trim().to_string();
                if c.ends_with(".der") || c.ends_with(".pem") {
                    let key = if c.ends_with(".der") {
                        c.replace(".der", ".key")
                    } else {
                        c.replace(".pem", ".key")
                    };
                    (c, key)
                } else {
                    // 视为 thumbprint 或相对路径，直接作为 cert 路径，key 同源
                    (c.clone(), format!("{c}.key"))
                }
            } else {
                ("own/own.der".into(), "own/own.key".into())
            }
        };
        let mut client = ClientBuilder::new()
            .application_name("Mesa OPC UA")
            .application_uri("urn:Mesa:opcua")
            .pki_dir(pki_dir)
            .certificate_path(cert_path)
            .private_key_path(key_path)
            .trust_server_certs(false)
            .verify_server_certs(true)
            .create_sample_keypair(false)
            .session_retry_limit(1)
            .client()
            .map_err(|e| format!("ClientBuilder 失败: {e:?}"))?;

        use opcua_types::{EndpointDescription, MessageSecurityMode, UserTokenPolicy};
        let policy = self.security_policy.lock().unwrap().clone();
        let mode_str = self.security_mode.lock().unwrap().clone();
        let mode = match mode_str.as_str() {
            "Sign" => MessageSecurityMode::Sign,
            "SignAndEncrypt" => MessageSecurityMode::SignAndEncrypt,
            _ => MessageSecurityMode::None,
        };
        let username = self.username.lock().unwrap().clone();
        let password = self.password.lock().unwrap().clone();
        // 根据是否提供用户名选择认证方式：有则 UserName，无则 Anonymous
        let (user_policy, identity) = if let (Some(u), Some(p)) = (username, password) {
            let pol = UserTokenPolicy {
                policy_id: opcua_types::UAString::from("username"),
                token_type: opcua_types::UserTokenType::UserName,
                issued_token_type: opcua_types::UAString::null(),
                issuer_endpoint_url: opcua_types::UAString::null(),
                security_policy_uri: opcua_types::UAString::from(policy.as_str()),
            };
            (pol, IdentityToken::UserName(u, opcua_client::Password(p)))
        } else {
            (UserTokenPolicy::anonymous(), IdentityToken::Anonymous)
        };
        let endpoint: EndpointDescription =
            (endpoint_url, policy.as_str(), mode, user_policy).into();

        let connect_fut = client.connect_to_matching_endpoint(endpoint, identity);
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
        self.ensure_connected(endpoint_url, timeout_ms)
            .await
            .map(|_| ())
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
            if let Some(status) = dv.status
                && status != StatusCode::Good
            {
                out.push(Value::String(format!("ERR:{:?}", status)));
                continue;
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

    async fn subscribe(
        &self,
        addrs: &[OpcUaAddress],
        publishing_interval_ms: u64,
        sampling_interval_ms: u64,
        queue_size: u32,
        discard_oldest: bool,
    ) -> Result<(u32, tokio::sync::mpsc::Receiver<DataChangeEvent>), String> {
        use opcua_client::DataChangeCallback;
        use opcua_types::{
            MonitoredItemCreateRequest, MonitoringMode, MonitoringParameters, ReadValueId,
            TimestampsToReturn,
        };
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| "尚未连接，请先 connect".to_string())?
        };
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        // DataChangeCallback 在服务端推送时触发，try_send 到有界通道，背压由 DataSink Latest-Wins 承接
        let tx_cb = tx.clone();
        let callback =
            DataChangeCallback::new(move |dv: DataValue, item: &opcua_client::MonitoredItem| {
                let h = item.client_handle();
                let ev = DataChangeEvent {
                    client_handle: h,
                    data_value: dv,
                };
                let _ = tx_cb.try_send(ev);
            });
        let pub_interval = Duration::from_millis(publishing_interval_ms.max(10));
        // lifetime 约 3*keep_alive，keep_alive 10 次发布
        let sub_id = session
            .create_subscription(pub_interval, 30, 10, 0, 0, true, callback)
            .await
            .map_err(|e| format!("create_subscription 失败: {e:?}"))?;
        // 为每个地址创建受监控项，client_handle 按 1..n 分配，对应 addrs 索引+1
        let mut reqs = Vec::with_capacity(addrs.len());
        for (idx, addr) in addrs.iter().enumerate() {
            let nid = to_opc_node_id(addr)?;
            let params = MonitoringParameters {
                client_handle: (idx as u32) + 1,
                sampling_interval: sampling_interval_ms as f64,
                filter: opcua_types::ExtensionObject::null(),
                queue_size,
                discard_oldest,
            };
            let req = MonitoredItemCreateRequest::new(
                ReadValueId::from(nid),
                MonitoringMode::Reporting,
                params,
            );
            reqs.push(req);
        }
        session
            .create_monitored_items(sub_id, TimestampsToReturn::Both, reqs)
            .await
            .map_err(|e| format!("create_monitored_items 失败: {e:?}"))?;
        Ok((sub_id, rx))
    }

    async fn unsubscribe(&self, subscription_id: u32) -> Result<(), String> {
        let session = {
            let guard = self.inner.lock().await;
            guard.as_ref().and_then(|i| i.session.clone())
        };
        if let Some(sess) = session {
            let _ = sess.delete_subscription(subscription_id).await;
        }
        Ok(())
    }
    async fn browse(&self, node: &OpcUaAddress) -> Result<Vec<String>, String> {
        let session = {
            let guard = self.inner.lock().await;
            guard
                .as_ref()
                .and_then(|i| i.session.clone())
                .ok_or_else(|| "尚未连接".to_string())?
        };
        let nid = to_opc_node_id(node)?;
        use opcua_types::{BrowseDescription, BrowseDirection};
        let bd = BrowseDescription {
            node_id: nid,
            browse_direction: BrowseDirection::Forward,
            reference_type_id: opcua_types::ReferenceTypeId::HierarchicalReferences.into(),
            include_subtypes: true,
            node_class_mask: 0,
            result_mask: 0x3F,
        };
        let results = session
            .browse(&[bd], 0, None)
            .await
            .map_err(|e| format!("browse 失败: {e:?}"))?;
        let mut out = Vec::new();
        for r in results {
            if let Some(refs) = r.references {
                for rf in refs {
                    let name = rf.browse_name.name.as_ref().to_string();
                    let nid_str = format!("{:?}", rf.node_id.node_id);
                    out.push(format!("{} {}", name, nid_str));
                }
            }
        }
        if out.is_empty() {
            out.push("empty".into());
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
        let err2 = api
            .connect("http://127.0.0.1:4840", 3000)
            .await
            .unwrap_err();
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
