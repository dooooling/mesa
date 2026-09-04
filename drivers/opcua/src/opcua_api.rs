//! OPC UA 访问抽象（对应 FOCAS 的 `focas_api.rs`）。
//!
//! V1 只读，采用 `trait OpcUaApi + Fake/TransportAdapter` 分层：
//! - `FakeOpcUaApi`：纯内存、确定性随机，不依赖任何 Server，覆盖 Poll/Subscribe 语义与故障注入
//! - `TransportApiAdapter`（见 `transport_adapter.rs`）：经公共 `mesa-opcua-transport`
//!   直连真实 Server；Native Session 实现已整体迁移，禁止在此回退直连实现

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use mesa_core_types::Value;
use opcua_types::{DataValue, DateTime, StatusCode, Variant};

use crate::address::OpcUaAddress;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------
/// Fake 随机数乘子（与 FOCAS 保持一致的分治可复现性）
const FAKE_RAND_MULT: u64 = 6364136223846793005;

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
    /// 批量读（Poll 通路）；按传入地址顺序返回等长 DataValue，保留原生 StatusCode/SourceTimestamp（§5.5 P0-A）
    /// 单点错误以 StatusCode!=Good 的 DataValue 表达，上层按 typed BAD + ValueOrigin 处理，禁止 String ERR 占位
    async fn read_batch(&self, addrs: &[OpcUaAddress]) -> Result<Vec<DataValue>, String>;
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
    #[allow(dead_code)]
    fn fake_value_for(&self, addr: &OpcUaAddress) -> Value {
        // 复用 fake_variant_for 保证 Poll/Subscribe 一致性
        variant_to_value(&self.fake_variant_for(addr)).unwrap_or(Value::String("fake:empty".into()))
    }

    /// 生成 Variant（与 subscribe 保持一致，便于 DataValue 统一）
    fn fake_variant_for(&self, addr: &OpcUaAddress) -> Variant {
        use opcua_types::{UAString, Variant};
        let r = self.next_rand();
        match &addr.identifier {
            crate::address::Identifier::Numeric(n) => {
                if n % 2 == 0 {
                    Variant::Int32((r % 10000) as i32)
                } else {
                    Variant::UInt32((r % 10000) as u32)
                }
            }
            crate::address::Identifier::String(s) => {
                let lower = s.to_ascii_lowercase();
                if lower.contains("speed") || lower.contains("sine") || lower.contains("temp") {
                    Variant::Double((r % 10000) as f64 / 10.0)
                } else if lower.contains("counter") || lower.contains("count") {
                    Variant::UInt32((r % 1000) as u32)
                } else if lower.contains("status") || lower.contains("state") {
                    Variant::UInt32((r % 4) as u32)
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

    async fn read_batch(&self, addrs: &[OpcUaAddress]) -> Result<Vec<DataValue>, String> {
        let mut out = Vec::with_capacity(addrs.len());
        for addr in addrs {
            // 模拟单点不支持：特定字符串触发 Bad DataValue（用于测试 Bad 隔离与 typed BAD）
            if let crate::address::Identifier::String(s) = &addr.identifier
                && (s.contains("bad") || s.contains("Bad"))
            {
                out.push(DataValue {
                    value: None,
                    status: Some(StatusCode::BadNodeIdUnknown),
                    source_timestamp: Some(DateTime::now()),
                    source_picoseconds: None,
                    server_timestamp: Some(DateTime::now()),
                    server_picoseconds: None,
                });
                continue;
            }
            let variant = self.fake_variant_for(addr);
            out.push(DataValue {
                value: Some(variant),
                status: Some(StatusCode::Good),
                source_timestamp: Some(DateTime::now()),
                source_picoseconds: None,
                server_timestamp: Some(DateTime::now()),
                server_picoseconds: None,
            });
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

// 注意：Native 会话实现已迁移至公共 `mesa-opcua-transport`（Stage 2 P0-B），
// 本文件仅保留 Fake（测试/Contract）与 Variant→Value 解码（数据语义属 Driver Adapter）。
// transport 只交出原生 DataValue，解码仍在此处，避免协议层污染数据语义。

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
        // Good DataValue 保留 SourceTimestamp 与 Variant
        for dv in &vals {
            assert!(dv.status.unwrap().is_good());
            assert!(dv.source_timestamp.is_some());
            assert!(dv.value.is_some());
        }
    }

    #[tokio::test]
    async fn fake_bad_isolation() {
        let api = FakeOpcUaApi::new();
        let addrs = vec![parse_address("ns=2;s=bad_node").unwrap()];
        let vals = api.read_batch(&addrs).await.unwrap();
        assert_eq!(vals.len(), 1);
        let dv = &vals[0];
        assert_eq!(dv.status.unwrap(), StatusCode::BadNodeIdUnknown);
        assert!(dv.source_timestamp.is_some());
        assert!(dv.value.is_none());
    }
}
