//! DriverManager 编排入口：发现驱动、启动 Endpoint 运行时、统一停机。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use tokio_util::sync::CancellationToken;

use crate::endpoint::{BuiltinEndpoint, PointIdAllocator, PointIdSource, run_endpoint};
use crate::manifest::{DiscoveredDriver, scan_drivers};
use crate::snapshot::{DriverInfo, Snapshot};

/// Descriptor 缓存键（§4.5）：(driver_id, driver_version)
type DescriptorCacheKey = (String, String);

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct CachedDescriptor {
    descriptor: mesa_core_types::DriverDescriptor,
    fetched_at_ns: i64,
}

/// Descriptor 获取错误（§4.4），映射为 REST 503 + error.code
#[derive(Debug, Clone)]
pub struct DescriptorError {
    pub code: String,
    pub message: String,
}

impl DescriptorError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// I-2 管理面（定义目录 / 权威身份预览）：错误类型 + 每 driver 并发预算
// ---------------------------------------------------------------------------

/// 每 `driver_id` 在飞管理请求上限（P0-1 冻结口径：**4**）。
///
/// 为什么是 4：一次 I-2 调用 = 1 个临时驱动进程 + 1 个 TCP 监听端口，是**真实的
/// 本机资源消耗**（与是否连机床无关）。4 与「管理面并发人手操作数」相当，乘上
/// `MAX_TEMP_ATTEMPTS = 2` 后最坏瞬时进程数 8，远低于任何进程数风险阈值。
/// 超限**立即拒绝、不排队**——排队会让「单次调用最坏上界」失效（DC §6.4-①-⑤）。
pub const DEFINITION_QUERY_BUDGET: u32 = 4;

/// 过滤后条目数硬顶（DC §6.4-①-②）：超过即视为驱动违约。
pub const DEFINITION_QUERY_MAX_ENTRIES: usize = 4096;

/// I-2 管理面错误：**携带结构化 `ErrorKind`**。
///
/// 为什么不用既有的 [`DescriptorError`]（只有 code+message）：DC §3.3 / §6.2 要求
/// HTTP 状态由 Core **按 `ErrorKind`** 决定（不按码），P9 护栏禁止 Core 出现任何
/// NCK 码字面量。因此 Manager 必须把 kind 结构化地交出去，Core 只做一次
/// `ErrorKind → StatusCode` 的纯映射。
#[derive(Debug, Clone)]
pub struct DefinitionQueryError {
    pub kind: mesa_core_types::ErrorKind,
    pub code: String,
    pub message: String,
}

impl DefinitionQueryError {
    fn new(
        kind: mesa_core_types::ErrorKind,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
        }
    }

    /// E1/E2：驱动不在清单、不可启动、spawn/握手失败。
    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(
            mesa_core_types::ErrorKind::Connection,
            "DRIVER_UNAVAILABLE",
            message,
        )
    }

    /// E31：并发预算耗尽（本机资源，与设备在线状态无关 ⇒ Internal/503）。
    fn busy(driver_id: &str, in_flight: u32) -> Self {
        Self::new(
            mesa_core_types::ErrorKind::Internal,
            "DEFINITION_QUERY_BUSY",
            format!(
                "driver `{driver_id}` has {in_flight} in-flight definition queries (budget {DEFINITION_QUERY_BUDGET})"
            ),
        )
    }

    /// E3：**Manager 管理 RPC 的内部执行预算耗尽**（不是机床通信超时）。
    ///
    /// kind 取 `Internal` 而不是 `Timeout`：设计 §6.2 的通用表把 `Timeout` 映射为
    /// **504**，而 §6.2.1 与本阶段验收（I38 / T2）要求 **503**；Core 又只能按 kind
    /// 映射（不得按码特判）。**已裁决**：保留 503/`Internal`，并把它明确定位为
    /// 「Manager 自己的临时进程执行预算耗尽」（与 `DEFINITION_QUERY_BUSY` 同族），
    /// 而不是「机床/设备通信超时」——后者会把运维引向查线查机床。
    ///
    /// 该三元组（`503` + `InternalError` + `DEFINITION_QUERY_TIMEOUT`）有回归断言
    /// 固定：`manager.rs` 的单测钉 kind+code，`core-api` 的单测钉 kind→status，
    /// 合同测试 `definitions_rpc_timeout_budget_is_5s` 钉端到端组合。
    fn timeout(driver_id: &str) -> Self {
        Self::new(
            mesa_core_types::ErrorKind::Internal,
            "DEFINITION_QUERY_TIMEOUT",
            format!(
                "management rpc budget exhausted: driver `{driver_id}` definition query exceeded {}ms \
                 (temporary-process budget, not a device timeout)",
                crate::session::DEFINITIONS_RPC_TIMEOUT.as_millis()
            ),
        )
    }

    /// E4：载荷超 256 KiB 或条目数超硬顶（驱动违约）。
    fn too_large(message: impl Into<String>) -> Self {
        Self::new(
            mesa_core_types::ErrorKind::Internal,
            "DEFINITION_QUERY_TOO_LARGE",
            message,
        )
    }

    /// E4-b（**契约勘误**）：驱动返回的响应**格式非法或结构不符合管理 RPC 契约**。
    ///
    /// 为什么不复用 `DEFINITION_QUERY_TOO_LARGE`：该码的设计条件是「体积或条目数
    /// 超限」，把「JSON 语法错/缺 `definitions` 数组」永久伪装成体积超限，会让
    /// 运维在驱动实现出错时去查目录规模——排障方向完全错误。
    /// 本码只用于**格式/结构**违约；体积与条目门禁（E4）保持独立、互不替代。
    fn response_invalid(message: impl Into<String>) -> Self {
        Self::new(
            mesa_core_types::ErrorKind::Internal,
            "DEFINITION_RESPONSE_INVALID",
            message,
        )
    }

    /// E5：对端 Minor < 7（旧二进制会静默忽略未知消息）。
    ///
    /// **必须 `Unsupported`（501）**：把「协议不支持」报成 `Connection`/`Device`
    /// 会被运维读成「去查机床/查线」，而实际要做的是「升级驱动或换 Core」。
    fn unsupported(driver_id: &str, negotiated: u32) -> Self {
        Self::new(
            mesa_core_types::ErrorKind::Unsupported,
            "DEFINITIONS_UNSUPPORTED",
            format!(
                "driver `{driver_id}` negotiated minor {negotiated} < {} (definition RPC unsupported)",
                mesa_driver_protocol::DEFINITIONS_RPC_MIN_MINOR
            ),
        )
    }

    /// 会话层错误 → 结构化错误（**零字符串回猜**，按 `SessionError` 变体路由）。
    fn from_session(driver_id: &str, e: crate::session::SessionError) -> Self {
        use crate::session::SessionError as E;
        match e {
            E::Timeout => Self::timeout(driver_id),
            E::ManagementJsonTooLarge { bytes, limit } => Self::too_large(format!(
                "definition payload {bytes} bytes exceeds {limit} bytes"
            )),
            // Driver 语义错误：kind/code/message 原样过线（码表归属见 DC §6.2.1）
            E::Driver {
                kind,
                code,
                message,
            } => Self::new(kind_from_wire(&kind), code, message),
            other => Self::unavailable(format!("driver `{driver_id}`: {other}")),
        }
    }
}

/// wire 的 `kind` 字符串 → `ErrorKind`（Driver 经 `error_detail` 传的是字符串）。
///
/// 未知取值一律回落 `Internal`（fail-safe）：宁可报 503 也不把未知错误
/// 说成「设备离线」或「参数错误」——那会把排障引向错误方向。
fn kind_from_wire(kind: &str) -> mesa_core_types::ErrorKind {
    use mesa_core_types::ErrorKind as K;
    match kind {
        "ConfigurationError" => K::Configuration,
        "ConnectionError" => K::Connection,
        "Timeout" => K::Timeout,
        "AddressError" => K::Address,
        "ProtocolError" => K::Protocol,
        "DecodeError" => K::Decode,
        "DeviceError" => K::Device,
        "Unsupported" => K::Unsupported,
        _ => K::Internal,
    }
}

/// per-`driver_id` 在飞计数（P0-1 的唯一载体，冻结在 Manager）。
#[derive(Default)]
struct DefinitionQueryBudget {
    in_flight: Mutex<HashMap<String, u32>>,
}

impl DefinitionQueryBudget {
    /// 尝试取号：超预算**立即**失败（不排队、不等待）。
    fn try_acquire(
        self: &Arc<Self>,
        driver_id: &str,
    ) -> Result<DefinitionQueryPermit, DefinitionQueryError> {
        let mut map = self.in_flight.lock().unwrap();
        let n = map.entry(driver_id.to_string()).or_insert(0);
        if *n >= DEFINITION_QUERY_BUDGET {
            return Err(DefinitionQueryError::busy(driver_id, *n));
        }
        *n += 1;
        Ok(DefinitionQueryPermit {
            budget: Arc::clone(self),
            driver_id: driver_id.to_string(),
        })
    }

    fn count(&self, driver_id: &str) -> u32 {
        self.in_flight
            .lock()
            .unwrap()
            .get(driver_id)
            .copied()
            .unwrap_or(0)
    }
}

/// 并发许可（RAII）：**成功 / 失败 / 超时 / 取消**四条路径都由 `Drop` 归还。
///
/// 为什么必须是 RAII 而不是「函数末尾手动减一」：客户端中途断开时，handler 的
/// future 会被**直接 drop**，任何写在 future 尾部的释放语句都不会执行 ——
/// 那正是 P0-1 要求「不得只释放计数许可却留下仍在运行的临时驱动进程」的
/// 反面场景。许可随 future 一起 drop，计数必然回落。
struct DefinitionQueryPermit {
    budget: Arc<DefinitionQueryBudget>,
    driver_id: String,
}

impl Drop for DefinitionQueryPermit {
    fn drop(&mut self) {
        let mut map = self.budget.in_flight.lock().unwrap();
        if let Some(n) = map.get_mut(&self.driver_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                // 归零即删键：避免 driver_id 维度的无界增长
                map.remove(&self.driver_id);
            }
        }
    }
}

/// I-2 两个 RPC 的语义分类（同一套编排只差「调哪个方法」）。
#[derive(Clone, Copy)]
enum DefinitionQueryKind {
    Catalog,
    Preview,
}

/// 目录载荷的两道**独立**门（互不替代）：
/// - **格式/结构**（E4-b）：必须是合法 JSON 对象且含 `definitions` 数组，
///   否则 `DEFINITION_RESPONSE_INVALID`（Internal/503）；
/// - **规模**（E4）：`definitions` 条目数 ≤ 4096，否则 `DEFINITION_QUERY_TOO_LARGE`。
fn check_definition_payload(json: &str) -> Result<(), DefinitionQueryError> {
    let v = parse_json_object(json, "definition catalog")?;
    let Some(defs) = v.get("definitions").and_then(|d| d.as_array()) else {
        return Err(DefinitionQueryError::response_invalid(
            "definition catalog must be a json object containing a `definitions` array",
        ));
    };
    if defs.len() > DEFINITION_QUERY_MAX_ENTRIES {
        return Err(DefinitionQueryError::too_large(format!(
            "definition catalog has {} entries, exceeding {DEFINITION_QUERY_MAX_ENTRIES}",
            defs.len()
        )));
    }
    Ok(())
}

/// 预览结果的**格式**门（E4-b）：只要求是合法 JSON 对象。
///
/// 刻意不校验任何键（`point_key` / `scope` 等属驱动语义，Core/Manager 不得解释），
/// 只挡「驱动返回一坨非 JSON 却以 200 透出」这种契约违约。
fn check_preview_payload(json: &str) -> Result<(), DefinitionQueryError> {
    parse_json_object(json, "identity preview result").map(|_| ())
}

/// 解析为 JSON 对象；语法错或顶层不是对象都归 `DEFINITION_RESPONSE_INVALID`。
fn parse_json_object(json: &str, what: &str) -> Result<serde_json::Value, DefinitionQueryError> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        DefinitionQueryError::response_invalid(format!("{what} is not valid json: {e}"))
    })?;
    if !v.is_object() {
        return Err(DefinitionQueryError::response_invalid(format!(
            "{what} must be a json object"
        )));
    }
    Ok(v)
}

/// §4.2 Descriptor version gate（纯函数，可单测）：
/// 先检查 report.contract_major（旧 V1 直接 DESCRIPTOR_CONTRACT_UNSUPPORTED，
/// 不 parse），再 parse descriptor_json，再校验 descriptor 内 major 与 report
/// 一致，最后 validate()。
pub(crate) fn gate_descriptor_report(
    major: u32,
    minor: u32,
    json: &str,
) -> Result<mesa_core_types::DriverDescriptor, DescriptorError> {
    use mesa_core_types::{DESCRIPTOR_CONTRACT_MAJOR, DESCRIPTOR_CONTRACT_MINOR};
    if major != DESCRIPTOR_CONTRACT_MAJOR {
        return Err(DescriptorError::new(
            "DESCRIPTOR_CONTRACT_UNSUPPORTED",
            format!(
                "driver reports contract {major}.{minor}, core supports {DESCRIPTOR_CONTRACT_MAJOR}.{DESCRIPTOR_CONTRACT_MINOR}"
            ),
        ));
    }
    if json.len() > 256 * 1024 {
        return Err(DescriptorError::new(
            "DRIVER_DESCRIPTOR_TOO_LARGE",
            format!("descriptor {} bytes exceeds 256KiB", json.len()),
        ));
    }
    let desc: mesa_core_types::DriverDescriptor = serde_json::from_str(json).map_err(|e| {
        DescriptorError::new(
            "DRIVER_DESCRIPTOR_INVALID_JSON",
            format!("invalid json: {e}"),
        )
    })?;
    if desc.contract_major != major {
        return Err(DescriptorError::new(
            "DESCRIPTOR_CONTRACT_UNSUPPORTED",
            format!(
                "contract major mismatch: report {major}, descriptor {}/{}",
                desc.contract_major, desc.contract_minor
            ),
        ));
    }
    desc.validate()
        .map_err(|e| DescriptorError::new("DRIVER_DESCRIPTOR_VALIDATION_FAILED", e))?;
    Ok(desc)
}

struct RunningEntry {
    cancel: CancellationToken,
    /// P0-3：任务返回 teardown 结论（`None` = 干净，`Some` = drain 异常描述），
    /// `stop_endpoint` 转为显式错误，禁止 detach-success。
    handle: tokio::task::JoinHandle<Option<String>>,
}

pub struct MesaManager {
    drivers: RwLock<Vec<DiscoveredDriver>>,
    snapshot: Arc<Snapshot>,
    source: Arc<dyn PointIdSource>,
    running: Mutex<HashMap<String, RunningEntry>>,
    shutdown: CancellationToken,
    descriptor_cache: RwLock<HashMap<DescriptorCacheKey, CachedDescriptor>>,
    /// 活跃会话注册表：endpoint_id -> Session（用于 Control 面可靠转发，§22）
    active_sessions: std::sync::Arc<
        RwLock<HashMap<String, std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>>>,
    >,
    /// 事件面服务（PR7 v1.1 §20）：`None` = 纯 Data-only 模式（EventStore
    /// 不可用时），endpoint 不起 ingress，数据面完全不受影响（§9 隔离）。
    event_services: RwLock<Option<Arc<mesa_event_store::EventServices>>>,
    /// I-2 管理面并发预算（P0-1）：per-`driver_id` 在飞计数，唯一载体是 Manager。
    /// 为什么放在这里而不是 `driver-protocol`：协议层只负责版本与消息契约，
    /// 持有运行期计数会让「协议 crate」变成有状态组件（DC §8.1.2-M11 明令禁止）。
    definition_query_budget: Arc<DefinitionQueryBudget>,
}

impl MesaManager {
    /// 扫描驱动目录并填充快照中的驱动清单（使用内存版 ID 源，兼容 M0/Contract Test）。
    pub fn discover(drivers_dir: &Path) -> Self {
        Self::with_source(drivers_dir, Arc::new(PointIdAllocator::default()))
    }

    /// 使用指定 [`PointIdSource`] 创建 Manager（Mesad 传入持久版）。
    pub fn with_source(drivers_dir: &Path, source: Arc<dyn PointIdSource>) -> Self {
        let drivers = scan_drivers(drivers_dir);
        let snapshot = Arc::new(Snapshot::new());
        snapshot.set_drivers(Self::driver_infos(&drivers));
        Self {
            drivers: RwLock::new(drivers),
            snapshot,
            source,
            running: Mutex::new(HashMap::new()),
            shutdown: CancellationToken::new(),
            descriptor_cache: RwLock::new(HashMap::new()),
            active_sessions: std::sync::Arc::new(RwLock::new(HashMap::new())),
            event_services: RwLock::new(None),
            definition_query_budget: Arc::new(DefinitionQueryBudget::default()),
        }
    }

    /// 注入事件面服务（Mesad 在 EventStore 打开后调用一次）。构造器签名不变，
    /// 老调用方（Contract Test 等）不传即 Data-only。
    pub fn set_event_services(&self, services: Arc<mesa_event_store::EventServices>) {
        *self.event_services.write().unwrap() = Some(services);
    }

    fn driver_infos(drivers: &[DiscoveredDriver]) -> Vec<DriverInfo> {
        drivers
            .iter()
            .map(|d| DriverInfo {
                id: d.manifest.id.clone(),
                name: d.manifest.name.clone(),
                version: d.manifest.version.clone(),
                protocol: format!(
                    "{}.{}",
                    d.manifest.protocol_major, d.manifest.protocol_minor
                ),
                launchable: d.launchable(),
                reason: if d.platform_ok {
                    d.executable_path
                        .as_ref()
                        .map(|_| None)
                        .unwrap_or(Some("executable not found".into()))
                } else {
                    d.platform_reason.clone()
                },
            })
            .collect()
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// 重新扫描驱动目录，刷新可用驱动清单并清空 Descriptor 缓存（§4.5）。
    pub fn rescan(&self, drivers_dir: &Path) -> Vec<DriverInfo> {
        let drivers = scan_drivers(drivers_dir);
        let infos = Self::driver_infos(&drivers);
        *self.drivers.write().unwrap() = drivers;
        self.snapshot.set_drivers(infos.clone());
        // 清空全部 Descriptor Cache（§4.5 精确失效 1）
        self.descriptor_cache.write().unwrap().clear();
        infos
    }

    /// 按 driver_id 查找已发现且可启动的驱动。
    pub fn find_driver(&self, driver_id: &str) -> Option<DiscoveredDriver> {
        self.drivers
            .read()
            .unwrap()
            .iter()
            .find(|d| d.manifest.id == driver_id && d.launchable())
            .cloned()
    }

    pub fn is_running(&self, endpoint_id: &str) -> bool {
        let map = self.running.lock().unwrap();
        if let Some(entry) = map.get(endpoint_id) {
            !entry.handle.is_finished()
        } else {
            false
        }
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.running
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, entry)| !entry.handle.is_finished())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// 启动一个 Endpoint。已在运行则返回错误。
    ///
    /// P0-2 fail-closed：事件使能（`event_tasks` 非空）但 EventStore 不可用
    /// （`event_services == None`）时直接拒绝，连 Driver 进程都不启动——
    /// 否则事件会发出却无人持久化，形成"RUNNING 但丢历史"的静默缺口。
    pub fn start_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        if self.is_running(&cfg.endpoint_id) {
            return Err(format!("endpoint `{}` already running", cfg.endpoint_id));
        }
        if !cfg.event_tasks.is_empty() && self.event_services.read().unwrap().is_none() {
            return Err(format!(
                "EVENT_STORE_UNAVAILABLE: endpoint `{}` has event tasks but EventStore is unavailable",
                cfg.endpoint_id
            ));
        }
        let disc = self
            .find_driver(&cfg.driver_id)
            .ok_or_else(|| format!("driver `{}` not found or not launchable", cfg.driver_id))?;

        let shutdown = self.shutdown.child_token();
        let cancel = shutdown.clone();
        let snapshot = Arc::clone(&self.snapshot);
        let source = Arc::clone(&self.source);
        let registry = std::sync::Arc::clone(&self.active_sessions);
        let events = self.event_services.read().unwrap().clone();
        let handle = tokio::spawn(run_endpoint(
            disc,
            cfg.clone(),
            snapshot,
            source,
            shutdown,
            registry,
            events,
        ));

        self.running
            .lock()
            .unwrap()
            .insert(cfg.endpoint_id, RunningEntry { cancel, handle });
        Ok(())
    }

    /// 兼容旧名称。
    pub fn start_builtin_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        self.start_endpoint(cfg)
    }

    /// 停止指定 Endpoint。
    ///
    /// P0-3：`Ok(was_running)`——`false` = 当时未运行（幂等成功）；
    /// `Err(detail)` = teardown 异常（drain fatal/timeout 精确码在前，
    /// 或任务 panic）。Stop 等待 teardown 真正完成才返回：
    /// 内部各步骤自带 bounded timeout（post 5s + terminate 10s + barrier 5s +
    /// drain 可组合预算 15s，见 `INGRESS_DRAIN_TIMEOUT`，总 ≈ 35s 最坏），
    /// Manager 直接 await，不再用"比总预算还短的外层 timeout 静默 detach"。
    /// 返回后新旧 attempt 不重叠（同一 endpoint 立即 Start 安全）。
    pub async fn stop_endpoint(&self, endpoint_id: &str) -> Result<bool, String> {
        let entry = self.running.lock().unwrap().remove(endpoint_id);
        let Some(entry) = entry else {
            return Ok(false);
        };
        entry.cancel.cancel();
        match entry.handle.await {
            Ok(None) => Ok(true),
            Ok(Some(drain_err)) => Err(drain_err),
            Err(join) => Err(format!("EVENT_DRAIN_FAILED: endpoint task failed: {join}")),
        }
    }

    /// 统一停机（消费 self，兼容旧调用）。
    pub async fn shutdown(self) {
        self.shutdown_all().await;
    }

    /// 统一停机（&self 版，供 Arc 持有者调用）。
    /// P0-3：直接 await 全部任务（teardown 全步骤有界，必结束），
    /// 异常大声记录；进程退出路径无调用方可接收错误，但绝不静默 detach。
    pub async fn shutdown_all(&self) {
        self.shutdown.cancel();
        let handles: Vec<(String, tokio::task::JoinHandle<Option<String>>)> = {
            let mut m = self.running.lock().unwrap();
            m.drain()
                .map(|(id, e)| {
                    e.cancel.cancel();
                    (id, e.handle)
                })
                .collect()
        };
        for (id, h) in handles {
            match h.await {
                Ok(None) => {}
                Ok(Some(e)) => {
                    tracing::error!(endpoint = %id, "shutdown teardown failed: {e}");
                }
                Err(join) => {
                    tracing::error!(endpoint = %id, "shutdown task failed: {join}");
                }
            }
        }
    }

    /// 懒加载获取 Driver Descriptor（§12）：临时 spawn → handshake → GetDescriptor → cache → shutdown
    pub async fn get_descriptor(
        &self,
        driver_id: &str,
    ) -> Result<mesa_core_types::DriverDescriptor, DescriptorError> {
        // 查找可启动驱动
        let disc = self.find_driver(driver_id).ok_or_else(|| {
            DescriptorError::new(
                "DRIVER_UNAVAILABLE",
                format!("driver `{driver_id}` not found or not launchable"),
            )
        })?;

        // 命中缓存
        let key: DescriptorCacheKey = (disc.manifest.id.clone(), disc.manifest.version.clone());
        if let Some(cached) = self.descriptor_cache.read().unwrap().get(&key) {
            return Ok(cached.descriptor.clone());
        }

        // 未命中：临时进程（startup transport 瞬态由原语重建，见 temp_op）。
        let desc = crate::temp_op::temp_driver_attempt(&disc, |mut td| async {
            let session = &mut td.session;
            let gd = session.get_descriptor().await.map_err(|e| match e {
                crate::session::SessionError::Timeout => {
                    DescriptorError::new("DRIVER_DESCRIPTOR_TIMEOUT", "descriptor timeout 5s")
                }
                crate::session::SessionError::Handshake(msg) if msg.contains("too large") => {
                    DescriptorError::new("DRIVER_DESCRIPTOR_TOO_LARGE", msg)
                }
                other => DescriptorError::new("DRIVER_UNAVAILABLE", format!("{other}")),
            });
            let (major, minor, json) = match gd {
                Ok(v) => v,
                Err(e) => return (td, Err(e)),
            };

            // §4.2 version gate（纯函数，先 major 后 parse，见 gate_descriptor_report）
            let desc = match gate_descriptor_report(major, minor, &json) {
                Ok(v) => v,
                Err(e) => return (td, Err(e)),
            };

            // 缓存
            self.descriptor_cache.write().unwrap().insert(
                key.clone(),
                CachedDescriptor {
                    descriptor: desc.clone(),
                    fetched_at_ns: mesa_core_types::now_unix_ns(),
                },
            );

            let _ = minor;
            (td, Ok(desc))
        })
        .await
        .map_err(|e| match e {
            crate::temp_op::TempOpError::Startup(s) => {
                DescriptorError::new("DRIVER_UNAVAILABLE", s.to_string())
            }
            crate::temp_op::TempOpError::Rpc(e) => e,
        })?;
        Ok(desc)
    }

    /// 供 REST 列出全部驱动及其 Descriptor 状态（可选）
    pub fn descriptor_cache_snapshot(&self) -> std::collections::HashMap<String, String> {
        self.descriptor_cache
            .read()
            .unwrap()
            .iter()
            .map(|((id, ver), _)| (format!("{id}@{ver}"), "cached".into()))
            .collect()
    }

    /// Browse（§20）：临时进程 + 分页，用于 OPC UA 等支持浏览的驱动
    pub async fn browse(
        &self,
        driver_id: &str,
        connection_json: &str,
        parent: &str,
        filter: &str,
        cursor: &str,
        limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), DescriptorError> {
        let disc = self.find_driver(driver_id).ok_or_else(|| {
            DescriptorError::new(
                "DRIVER_UNAVAILABLE",
                format!("driver `{driver_id}` not found"),
            )
        })?;
        // 临时进程（startup transport 瞬态由原语重建，见 temp_op）。
        crate::temp_op::temp_driver_attempt(&disc, |mut td| async {
            let session = &mut td.session;
            // 打开临时连接（handle 1）
            let handle = 1;
            let opened = session
                .call(mesa_driver_protocol::pb::envelope::Body::OpenConnection(
                    mesa_driver_protocol::pb::OpenConnection {
                        connection_handle: handle,
                        endpoint_id: format!("browse-{driver_id}"),
                        config_json: connection_json.to_string(),
                    },
                ))
                .await
                .map_err(|e| {
                    DescriptorError::new("DRIVER_UNAVAILABLE", format!("open failed: {e}"))
                });
            let open_res = match opened {
                Ok(v) => v,
                Err(e) => return (td, Err(e)),
            };
            let open_outcome: Result<(), DescriptorError> = match open_res.body {
                Some(mesa_driver_protocol::pb::envelope::Body::OpenConnectionAck(ack)) => {
                    let ok = ack.result.map(|r| r.ok).unwrap_or(false);
                    if !ok {
                        Err(DescriptorError::new("DRIVER_UNAVAILABLE", "open not ok"))
                    } else {
                        Ok(())
                    }
                }
                Some(mesa_driver_protocol::pb::envelope::Body::DriverError(e)) => {
                    let d = e.detail.unwrap_or_default();
                    Err(DescriptorError::new(d.code, d.message))
                }
                _ => Err(DescriptorError::new(
                    "DRIVER_UNAVAILABLE",
                    "open unexpected",
                )),
            };
            if let Err(e) = open_outcome {
                return (td, Err(e));
            }
            let browsed = session
                .browse(handle, parent, filter, cursor, limit)
                .await
                .map_err(|e| DescriptorError::new("BROWSE_FAILED", format!("{e}")));
            let res = match browsed {
                Ok(v) => v,
                Err(e) => return (td, Err(e)),
            };
            (td, Ok(res))
        })
        .await
        .map_err(|e| match e {
            crate::temp_op::TempOpError::Startup(s) => {
                DescriptorError::new("DRIVER_UNAVAILABLE", s.to_string())
            }
            crate::temp_op::TempOpError::Rpc(e) => e,
        })
    }

    /// 定义目录（I-2，DC §4）：临时进程 + 单 RPC，**不发 `OpenConnection`**。
    ///
    /// `query_json` 由 Core 按 §4.1 的查询参数组装后**逐字**透传；Manager 不解析
    /// 其中任何键的语义（`family`/`variable_id`/`revision` 的判定全在 Driver）。
    /// 并发预算：per-`driver_id` **4**，超限**立即** 503 `DEFINITION_QUERY_BUSY`。
    pub async fn definitions(
        &self,
        driver_id: &str,
        query_json: &str,
    ) -> Result<String, DefinitionQueryError> {
        self.definition_query(driver_id, DefinitionQueryKind::Catalog, query_json)
            .await
    }

    /// 权威身份预览（I-2，DC §5）：与 [`MesaManager::definitions`] 同形态
    /// （临时进程、零 Connection 条目、同一并发预算、同一超时/大小门）。
    pub async fn preview_point_key(
        &self,
        driver_id: &str,
        request_json: &str,
    ) -> Result<String, DefinitionQueryError> {
        self.definition_query(driver_id, DefinitionQueryKind::Preview, request_json)
            .await
    }

    /// P0-1 的可观测点：当前 `driver_id` 的在飞管理请求数。
    ///
    /// 刻意**不新增对外 HTTP 接口**（DC §6.4 只允许测试内观测）：测试用它断言
    /// 「许可已回落到 0」，这是 R-2「无许可泄漏」的直接证据。
    pub fn definition_query_in_flight(&self, driver_id: &str) -> u32 {
        self.definition_query_budget.count(driver_id)
    }

    /// 两个管理 RPC 共用的编排（差异只在「调哪个 Session 方法」）。
    ///
    /// NOTE（**对外超时口径**）：本函数**没有**最外层端到端 deadline，最坏耗时是
    /// 「2 × (启动 6s + RPC 5s) = 22s」的**理论组合上界**，不是经实测证明的服务端
    /// 硬上限。已裁决：**不得**把 22s / 25s 写成对外响应时间保证；若将来要承诺
    /// 硬上限，必须在最外层实现并验证端到端 deadline（含取消传播与进程回收），
    /// 而不是靠把某个内部预算调大。
    async fn definition_query(
        &self,
        driver_id: &str,
        kind: DefinitionQueryKind,
        payload: &str,
    ) -> Result<String, DefinitionQueryError> {
        // E1：驱动不存在/不可启动 —— 在取号**之前**判定，避免不存在的驱动占用预算
        let disc = self.find_driver(driver_id).ok_or_else(|| {
            DefinitionQueryError::unavailable(format!(
                "driver `{driver_id}` not found or not launchable"
            ))
        })?;
        // P0-1：取号后立即持有 RAII 许可，直到本 future 结束——**取消**路径
        // （handler future 被 drop）同样由 `Drop` 归还，不留计数泄漏。
        let _permit = self.definition_query_budget.try_acquire(driver_id)?;

        let out = crate::temp_op::temp_driver_attempt(&disc, |td| async {
            // E5：**门控先于发帧**。Minor < 7 的旧二进制会静默忽略未知消息，
            // 发帧只会干等一个 RPC 超时；且其能力面本来就该报 501。
            let negotiated = td.session.negotiated_minor();
            if !mesa_driver_protocol::definitions_query_supported(negotiated) {
                return (
                    td,
                    Err(DefinitionQueryError::unsupported(driver_id, negotiated)),
                );
            }
            let rpc = match kind {
                DefinitionQueryKind::Catalog => td.session.definitions(payload).await,
                DefinitionQueryKind::Preview => td.session.preview_point_key(payload).await,
            };
            (
                td,
                rpc.map_err(|e| DefinitionQueryError::from_session(driver_id, e)),
            )
        })
        .await
        .map_err(|e| match e {
            crate::temp_op::TempOpError::Startup(s) => {
                DefinitionQueryError::unavailable(s.to_string())
            }
            crate::temp_op::TempOpError::Rpc(e) => e,
        })?;

        // E4 的第二道门（Manager 侧）：格式/结构与条目数。字节门在 Session 侧
        //（256 KiB），两处都按 DC §6.4-①-② 复用既有 descriptor 门的形态。
        match kind {
            DefinitionQueryKind::Catalog => check_definition_payload(&out)?,
            DefinitionQueryKind::Preview => check_preview_payload(&out)?,
        }
        Ok(out)
    }

    /// Control Write（§22；Foundation-3 单真值）：经由活跃会话的可靠
    /// Control 队列转发结构化 WriteTarget，永不 Latest-Wins。
    /// 协商 minor < 6 的旧端直接 CONTROL_MODEL_UNSUPPORTED（不 fallback
    /// 旧字符串 target；wire 兼容 ≠ 产品双轨）。
    /// 并发模型（诚实版）：不同 Control 请求经同一 session 的 writer 锁 +
    /// pending 表串行执行（可靠队列语义要求有序，不并发发帧）；与 Data 面
    /// event_loop 不互斥（event_loop 不持 session 锁，只消费事件通道）。
    /// 即 Control↔Control 串行、Control↔Data 并发——串行是设计选择，
    /// 不是 #2 遗留缺陷。
    pub async fn control_write(
        &self,
        endpoint_id: &str,
        target: &mesa_core_types::WriteTarget,
        value: mesa_core_types::Value,
        expected: Option<mesa_core_types::Value>,
        request_id: &str,
    ) -> Result<Option<mesa_core_types::Value>, DescriptorError> {
        let sess_arc = {
            let m = self.active_sessions.read().unwrap();
            m.get(endpoint_id).cloned()
        }
        .ok_or_else(|| {
            DescriptorError::new(
                "ENDPOINT_NOT_RUNNING",
                format!("endpoint `{endpoint_id}` not running"),
            )
        })?;
        // 只读 negotiated_minor（&self 方法）后经 session 内 writer 锁串行
        // 发送（见本函数文档：Control↔Control 串行是可靠队列的设计选择）。
        let minor = sess_arc.lock().await.negotiated_minor();
        if minor < mesa_driver_protocol::CONTROL_MODEL_MIN_MINOR {
            return Err(DescriptorError::new(
                "CONTROL_MODEL_UNSUPPORTED",
                format!(
                    "structured write 需要 IPC 1.6 (negotiated minor < {})",
                    mesa_driver_protocol::CONTROL_MODEL_MIN_MINOR,
                ),
            ));
        }
        // 约定 handle 1 为 Endpoint 主连接（endpoint.rs HANDLE=1），request_id 贯穿审计
        sess_arc
            .lock()
            .await
            .write(1, request_id, target, value, expected)
            .await
            .map_err(|e| match e {
                // #5.2：Driver 精确码原样保留（EXPECTED_MISMATCH /
                // EXPECTED_VALUE_UNSUPPORTED / TARGET_NOT_FOUND 等直透 REST）；
                // 传输层失败（Timeout/Closed/Handshake）才映射 CONTROL_FAILED。
                crate::session::SessionError::Remote {
                    kind: _,
                    code,
                    message,
                } => DescriptorError::new(&code, message),
                other => DescriptorError::new("CONTROL_FAILED", format!("{other}")),
            })
    }

    /// Control Command（§22）：可靠转发，返回 (status, result_json, error)
    pub async fn control_command(
        &self,
        endpoint_id: &str,
        command_id: &str,
        input_json: &str,
        request_id: &str,
    ) -> Result<(String, String, String), DescriptorError> {
        let sess_arc = {
            let m = self.active_sessions.read().unwrap();
            m.get(endpoint_id).cloned()
        }
        .ok_or_else(|| {
            DescriptorError::new(
                "ENDPOINT_NOT_RUNNING",
                format!("endpoint `{endpoint_id}` not running"),
            )
        })?;
        let sess = sess_arc.lock().await;
        sess.command(1, request_id, command_id, input_json)
            .await
            .map_err(|e| match e {
                // #5.2 同 write：传输失败才 CONTROL_FAILED；CommandResponse
                // 自带 status/error 形态（Driver 语义错误走 status=Failed，
                // 不经 SessionError），此处只映射传输层。
                crate::session::SessionError::Remote {
                    kind: _,
                    code,
                    message,
                } => DescriptorError::new(&code, message),
                other => DescriptorError::new("CONTROL_FAILED", format!("{other}")),
            })
    }

    /// 供 Endpoint 运行时注册/注销活跃会话（§22 Control 面）
    pub fn register_session(
        &self,
        endpoint_id: &str,
        sess: crate::session::Session,
    ) -> std::sync::Arc<tokio::sync::Mutex<crate::session::Session>> {
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(sess));
        self.active_sessions
            .write()
            .unwrap()
            .insert(endpoint_id.to_string(), std::sync::Arc::clone(&arc));
        arc
    }

    pub fn unregister_session(&self, endpoint_id: &str) {
        self.active_sessions.write().unwrap().remove(endpoint_id);
    }

    /// 直接通过 registry 操作（供 endpoint.rs 使用，避免 &self 借用冲突）
    pub fn registry(
        &self,
    ) -> std::sync::Arc<
        RwLock<HashMap<String, std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>>>,
    > {
        std::sync::Arc::clone(&self.active_sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小合法 V2 descriptor JSON（§4.2 gate 单测夹具）。
    fn minimal_v2_json() -> String {
        serde_json::json!({
            "contract_major": mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR,
            "contract_minor": mesa_core_types::DESCRIPTOR_CONTRACT_MINOR,
            "identity": {"driver_id": "t", "name": "T", "version": "0"},
            "connection": {"fields": []},
        })
        .to_string()
    }

    /// V1 report 直接拒绝（先 gate 后 parse：JSON 合法也照样拒）。
    #[test]
    fn v1_descriptor_report_rejected_before_parse() {
        let err = gate_descriptor_report(1, 0, &minimal_v2_json()).unwrap_err();
        assert_eq!(err.code, "DESCRIPTOR_CONTRACT_UNSUPPORTED");
    }

    /// report 与 descriptor 内 major 不一致 → 拒绝。
    #[test]
    fn report_descriptor_major_mismatch_rejected() {
        let mut v: serde_json::Value = serde_json::from_str(&minimal_v2_json()).unwrap();
        v["contract_major"] = serde_json::json!(99);
        let err = gate_descriptor_report(
            mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR,
            mesa_core_types::DESCRIPTOR_CONTRACT_MINOR,
            &v.to_string(),
        )
        .unwrap_err();
        assert_eq!(err.code, "DESCRIPTOR_CONTRACT_UNSUPPORTED");
    }

    /// V2 合法 → 通过。
    #[test]
    fn v2_descriptor_report_accepted() {
        let d = gate_descriptor_report(
            mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR,
            mesa_core_types::DESCRIPTOR_CONTRACT_MINOR,
            &minimal_v2_json(),
        )
        .unwrap();
        assert_eq!(d.contract_major, mesa_core_types::DESCRIPTOR_CONTRACT_MAJOR);
    }

    fn empty_mgr() -> MesaManager {
        // 空驱动目录：门在 find_driver 之前触发，无需真实驱动
        MesaManager::discover(std::path::Path::new(
            "testdata/empty-drivers-does-not-exist",
        ))
    }

    fn endpoint_with_events() -> BuiltinEndpoint {
        BuiltinEndpoint {
            endpoint_id: "ep-evt".into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            tasks: vec![],
            event_tasks: vec![mesa_core_types::EventTask {
                id: "al".into(),
                mode: mesa_core_types::TaskMode::Subscribe,
                interval_ms: None,
                binding: mesa_core_types::DriverBinding {
                    kind: "test-driver.events".into(),
                    config: serde_json::json!({"stream": "test.events.alarm-cycle"}),
                },
            }],
        }
    }

    /// P0-2：事件使能 + 无 EventStore → 拒绝启动（精确码），且不建 Driver 进程。
    #[test]
    fn event_enabled_without_store_is_rejected() {
        let mgr = empty_mgr();
        let err = mgr
            .start_endpoint(endpoint_with_events())
            .expect_err("must refuse without EventStore");
        assert!(
            err.contains("EVENT_STORE_UNAVAILABLE"),
            "精确原因码，got {err}"
        );
        assert!(!mgr.is_running("ep-evt"), "拒绝后不得有运行态残留");
    }

    /// Data-only 无 EventStore 照常放行到驱动查找（门只看 event_tasks）。
    #[test]
    fn data_only_without_store_passes_gate() {
        let mgr = empty_mgr();
        let mut cfg = endpoint_with_events();
        cfg.event_tasks.clear();
        // 空驱动目录 → 死在 find_driver，而不是事件门
        let err = mgr.start_endpoint(cfg).expect_err("no driver here");
        assert!(
            err.contains("not found or not launchable"),
            "应走到驱动查找，got {err}"
        );
    }

    /// **契约三元组冻结**（阶段 2 定向补丁）：Manager 侧错误的 kind 与 code 必须
    /// 成对固定。只改 kind（例如把 E3 改回 `Timeout`）会让 Core 的通用映射把 503
    /// 变成 504 —— 这条断言与 `core-api` 的 kind→status 单测、以及合同测试
    /// `definitions_rpc_timeout_budget_is_5s` 的端到端组合共同锁死该三元组。
    #[test]
    fn definition_query_error_contracts_are_pinned() {
        use mesa_core_types::ErrorKind;
        let cases: [(DefinitionQueryError, ErrorKind, &str); 5] = [
            (
                DefinitionQueryError::timeout("d"),
                ErrorKind::Internal,
                "DEFINITION_QUERY_TIMEOUT",
            ),
            (
                DefinitionQueryError::busy("d", 4),
                ErrorKind::Internal,
                "DEFINITION_QUERY_BUSY",
            ),
            (
                DefinitionQueryError::too_large("x"),
                ErrorKind::Internal,
                "DEFINITION_QUERY_TOO_LARGE",
            ),
            (
                DefinitionQueryError::response_invalid("x"),
                ErrorKind::Internal,
                "DEFINITION_RESPONSE_INVALID",
            ),
            (
                DefinitionQueryError::unavailable("x"),
                ErrorKind::Connection,
                "DRIVER_UNAVAILABLE",
            ),
        ];
        for (e, kind, code) in cases {
            assert_eq!(
                (e.kind, e.code.as_str()),
                (kind, code),
                "码与 kind 必须成对固定（改 kind 会让 Core 映射出另一个 HTTP 状态）"
            );
        }

        // 驱动码透传：Revision 不支持必须保持 `Configuration`（⇒ HTTP 400），
        // 不得被改写成 `Unsupported`（501）或按码特判成 404。
        let e = DefinitionQueryError::from_session(
            "d",
            crate::session::SessionError::Driver {
                kind: "ConfigurationError".into(),
                code: "UNSUPPORTED_DEFINITION_REVISION".into(),
                message: "revision 9 unavailable; available=[1]".into(),
            },
        );
        assert_eq!(e.kind, ErrorKind::Configuration);
        assert_eq!(e.code, "UNSUPPORTED_DEFINITION_REVISION");
        assert!(e.message.contains("available"));
    }

    /// E4 与 E4-b **互不替代**：格式错报 `DEFINITION_RESPONSE_INVALID`，
    /// 条目超硬顶才报 `DEFINITION_QUERY_TOO_LARGE`。
    #[test]
    fn definition_payload_gates_are_distinct() {
        // 非法 JSON
        let e = check_definition_payload("not-json").unwrap_err();
        assert_eq!(e.code, "DEFINITION_RESPONSE_INVALID");
        // 合法 JSON 但顶层不是对象
        let e = check_definition_payload("[1,2,3]").unwrap_err();
        assert_eq!(e.code, "DEFINITION_RESPONSE_INVALID");
        // 缺 definitions 数组
        let e = check_definition_payload(r#"{"format_version":1}"#).unwrap_err();
        assert_eq!(e.code, "DEFINITION_RESPONSE_INVALID");
        // 合法结构 + 未超硬顶 ⇒ 通过
        check_definition_payload(r#"{"definitions":[]}"#).unwrap();
        // 超硬顶 ⇒ 体积/数量门（不是格式门）
        let big = format!(
            r#"{{"definitions":{}}}"#,
            serde_json::json!(vec![
                serde_json::json!({});
                DEFINITION_QUERY_MAX_ENTRIES + 1
            ])
        );
        let e = check_definition_payload(&big).unwrap_err();
        assert_eq!(e.code, "DEFINITION_QUERY_TOO_LARGE");
        // 预览结果：非 JSON / 非对象 ⇒ 同一格式门
        let e = check_preview_payload("<html>").unwrap_err();
        assert_eq!(e.code, "DEFINITION_RESPONSE_INVALID");
        let e = check_preview_payload("[]").unwrap_err();
        assert_eq!(e.code, "DEFINITION_RESPONSE_INVALID");
        check_preview_payload(r#"{"point_key":"k"}"#).unwrap();
    }
}
