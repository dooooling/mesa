//! DriverManager 编排入口：发现驱动、启动 Endpoint 运行时、统一停机。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::endpoint::{
    BuiltinEndpoint, PointIdAllocator, PointIdSource, run_endpoint_with_attempt,
};
use crate::manifest::{DiscoveredDriver, scan_drivers};
use crate::session::SessionError;
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
///
/// `kind` 是**可选的通用错误类别**：Driver 在 IPC 上本就声明了 `ErrorKind`
/// （见 `error_detail`），但早期实现只把 `code`/`message` 带出会话层，于是
/// 「驱动声明的类别」在 Manager 边界被丢弃，REST 只能靠错误码白名单猜 HTTP
/// 状态——同一个语义（连接失败 / 探测超时）在不同驱动下会落到不同状态码。
///
/// 为什么用 `Option` 而不是直接给 `ErrorKind`：本类型的既有构造点有 25 处
/// （Descriptor 获取、OpenConnection 失败、会话级失败等），它们拿不到驱动
/// 声明的类别。强制它们编造一个 kind 会让「驱动没说」与「驱动说是 Internal」
/// 变得不可区分，也会无谓改动大量与本次缺陷无关的代码路径。因此
/// [`DescriptorError::new`] 保持 `kind: None`（**既有行为逐字不变**），只有
/// 真正拿到驱动类别的路径才用 [`DescriptorError::with_kind`]。
#[derive(Debug, Clone)]
pub struct DescriptorError {
    pub code: String,
    pub message: String,
    /// `None` = 该错误路径没有驱动声明的类别（旧行为）；`Some` = 驱动声明过，
    /// 调用方（REST）应据此按通用类别映射，而不是按错误码猜。
    pub kind: Option<mesa_core_types::ErrorKind>,
}

impl DescriptorError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            kind: None,
        }
    }

    /// 带通用错误类别的构造：仅用于**驱动确实声明了类别**的路径。
    ///
    /// 语义上要求 `kind` 与 `code` 同源（都来自同一份 `pb::ErrorDetail`），
    /// 不允许在这里"补充"驱动没说的类别。
    fn with_kind(
        code: impl Into<String>,
        kind: mesa_core_types::ErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            kind: Some(kind),
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

/// 驱动侧结构化错误（IPC `ErrorDetail` 的 kind/code/message）→ [`DescriptorError`]。
///
/// 为什么单独抽出来：这是"kind 过 Manager 边界"的**唯一**语义点，抽成纯函数
/// 才能单测锁定。两条分支的取舍是刻意的：
/// - `kind` 非空 ⇒ 驱动确实声明过通用类别，结构化透出（`Some`），REST 按类别映射；
/// - `kind` 为空 ⇒ 驱动 `ok=false` 却没附 detail（对端违约）或旧路径没带类别，
///   此时**不猜**类别（不回落 `Internal`），交回 REST 的旧码白名单，
///   保证既有接口状态码逐字不变。
fn descriptor_error_from_driver(kind: &str, code: String, message: String) -> DescriptorError {
    if kind.is_empty() {
        DescriptorError::new(code, message)
    } else {
        DescriptorError::with_kind(code, kind_from_wire(kind), message)
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

// ---------------------------------------------------------------------------
// G7：代次化 attempt 表（生命周期的唯一真值来源）
// ---------------------------------------------------------------------------

/// attempt 阶段（G7 §6.2）。
///
/// **只有 [`AttemptGuard::commit`] 能写终态 `TeardownDone`**；`FailedUnknown`
/// 只能由 `AttemptGuard::drop` 在"未 commit"时写。任何在别处写终态的改动
/// 都是缺陷（等于让别的路径拥有"释放保护"的权力，P0-A 的同型错误）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AttemptPhase {
    /// 已登记、任务尚未进入 RUNNING（spawn 前就存在，insert-before-spawn）。
    Starting = 0,
    /// 已进入 RUNNING（会话建立、配置完成）。
    Running = 1,
    /// Stop 已受理，teardown 进行中（条目仍在 ⇒ 保护仍在）。
    Stopping = 2,
    /// teardown 已被证明完成（只能由 commit 写入，且与条目移除同一临界区）。
    TeardownDone = 3,
    /// 异常终止（panic / abort / 未 commit 的提前返回）：teardown 未证明，
    /// 条目**保留**，新 Start 一律拒绝（fail-closed）。
    FailedUnknown = 4,
}

impl AttemptPhase {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Running,
            2 => Self::Stopping,
            3 => Self::TeardownDone,
            4 => Self::FailedUnknown,
            _ => Self::Starting,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Running => "RUNNING",
            Self::Stopping => "STOPPING",
            Self::TeardownDone => "TEARDOWN_DONE",
            Self::FailedUnknown => "FAILED_UNKNOWN",
        }
    }

    /// 是否算"有活着的生命周期操作"（用于 `is_running` 兼容投影）。
    fn is_live(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// 一次 attempt 的全部可判定证据（G7 §6.2）。
///
/// 单次 attempt 只有一个结论：`outcome` 是 `OnceLock`，两个并发 Stop 不可能
/// 看到两次不同的失败描述（§5.2 第四行）。
pub struct AttemptCtx {
    /// 单调代次（per endpoint，永不回收；墓碑以它为下界）。
    pub generation: u64,
    /// Issue #102：本 attempt 的审计上下文（父请求 + 来源 + 可选 sink）。
    /// attempt 级而非 endpoint 级：开机恢复、REST Start、Bootstrap 的父请求各不相同，
    /// 而重连必须沿用同一个父请求 ID 才能把多个 epoch 归到同一次启动。
    audit: crate::attempt_audit::AttemptContext,
    /// Issue #102：attempt 内 StartConnection 序号（1 = 首连；重连自增）。
    connection_seq: crate::attempt_audit::ConnectionSequence,
    /// Issue #102：本 attempt 是否**曾经成功建立过连接**（StartConnection ACK 成功）。
    ///
    /// 用来区分 "首次连接的重试" 与 "连上之后的断线重连"：只有真的连上过，
    /// `internal_reconnect` 这个来源才成立（评审指出的语义边界）。
    connection_established: std::sync::atomic::AtomicBool,
    phase: AtomicU8,
    /// 驱动子进程 pid（spawn 后回填；0 = 尚未 spawn）。
    pub pid: AtomicU32,
    /// teardown 末尾：registry 已摘除本 attempt 的 session（E4）。
    pub session_released: AtomicBool,
    /// teardown 末尾：reader 观测到对端 EOF。
    pub remote_eof: AtomicBool,
    /// teardown 结论（`Ok(was_running)` / `Err(精确码前缀 + detail)`）。
    pub outcome: OnceLock<Result<bool, String>>,
    /// **唯一正常完成标记**，只能由 [`AttemptGuard::commit`] 置 true。
    committed: AtomicBool,
    /// 完成通知（合流点：所有 Stop 等待同一个结论）。
    done_tx: tokio::sync::watch::Sender<Option<Result<bool, String>>>,
    /// 测试专用 panic 注入开关（`test-hooks`）。
    #[cfg(feature = "test-hooks")]
    pub test_panic_before_teardown: AtomicBool,
    #[cfg(feature = "test-hooks")]
    pub test_reconnect_clock: OnceLock<Arc<crate::test_clock::ReconnectClock>>,
}

impl AttemptCtx {
    fn new(generation: u64, audit: crate::attempt_audit::AttemptContext) -> Self {
        let (done_tx, _) = tokio::sync::watch::channel(None);
        Self {
            generation,
            audit,
            connection_seq: crate::attempt_audit::ConnectionSequence::default(),
            connection_established: std::sync::atomic::AtomicBool::new(false),
            phase: AtomicU8::new(AttemptPhase::Starting as u8),
            pid: AtomicU32::new(0),
            session_released: AtomicBool::new(false),
            remote_eof: AtomicBool::new(false),
            outcome: OnceLock::new(),
            committed: AtomicBool::new(false),
            done_tx,
            #[cfg(feature = "test-hooks")]
            test_panic_before_teardown: AtomicBool::new(false),
            #[cfg(feature = "test-hooks")]
            test_reconnect_clock: OnceLock::new(),
        }
    }

    /// 本 attempt 的审计上下文（Issue #102）。未注入 sink 时 `is_audited()==false`，
    /// 运行期审计全部退化为 no-op。
    pub fn audit(&self) -> &crate::attempt_audit::AttemptContext {
        &self.audit
    }

    /// 分配 attempt 内的下一个连接序号（1 起，重连自增）。
    pub fn next_connection_sequence(&self) -> u64 {
        self.connection_seq.next()
    }

    /// 候选连接序号（不推进；Issue #102 两段式序号）。
    pub fn peek_connection_sequence(&self) -> u64 {
        self.connection_seq.peek()
    }

    /// Issue #102：为本次连接登记 epoch 意图（**两段式序号**）。
    ///
    /// - `Ok(Some(request_id))`：意图已持久化，序号已提交；
    /// - `Ok(None)`：本 attempt 未启用审计（sink/父请求缺失），全部 no-op；
    /// - `Err(e)`：意图写不进去 ⇒ 调用方必须**放弃本次连接**（不发 StartConnection），
    ///   并且序号**没有被消耗**——下一次成功连接仍然拿到 `:epoch:1` /
    ///   `initial_connection`，不会把"审计抖动"伪装成"内部重连"。
    pub fn try_audit_epoch_intent(
        &self,
        endpoint_id: &str,
        stream_epoch: u64,
    ) -> Result<Option<String>, String> {
        let Some(sink) = self.audit.active_sink() else {
            return Ok(None);
        };
        let candidate = self.connection_seq.peek();
        let Some(request_id) = self.audit.epoch_request_id(candidate) else {
            return Ok(None);
        };
        let intent = crate::attempt_audit::EpochIntent {
            request_id: request_id.clone(),
            parent_request_id: self
                .audit
                .parent_request_id()
                .unwrap_or_default()
                .to_string(),
            endpoint_id: endpoint_id.to_string(),
            // 来源按「序号 + 是否曾经连上过」判定（评审修正）：
            // - 候选 1：本 attempt 首连；
            // - 候选 >1 且从未成功建连：首次连接的重试（没有"断线"可言）；
            // - 候选 >1 且曾成功建连：真正的断线重连。
            source: if candidate == 1 {
                crate::attempt_audit::EpochSource::InitialConnection
            } else if self.has_established_connection() {
                crate::attempt_audit::EpochSource::InternalReconnect
            } else {
                crate::attempt_audit::EpochSource::InitialConnectionRetry
            },
            connection_sequence: candidate,
            stream_epoch,
            actor: self.audit.actor().to_string(),
            generation: self.generation,
        };
        // 先落盘，后提交序号：顺序不可交换，否则失败会白吃一个序号。
        sink.record_epoch_intent(&intent)?;
        let committed = self.connection_seq.commit(candidate);
        debug_assert_eq!(
            committed, candidate,
            "attempt 内的连接序号只能由该 attempt 的任务推进"
        );
        Ok(Some(request_id))
    }

    /// 本 attempt 是否曾经成功建立过连接（StartConnection ACK 成功）。
    pub fn has_established_connection(&self) -> bool {
        self.connection_established.load(Ordering::Acquire)
    }

    /// 标记本 attempt 已成功建立过一次连接。
    ///
    /// 只在 StartConnection 真正 ACK 之后调用：它决定后续重试的来源是
    /// `initial_connection_retry`（还没连上过）还是 `internal_reconnect`（真断线）。
    pub fn mark_connection_established(&self) {
        self.connection_established.store(true, Ordering::Release);
    }

    pub fn phase(&self) -> AttemptPhase {
        AttemptPhase::from_u8(self.phase.load(Ordering::Acquire))
    }

    pub fn set_phase(&self, p: AttemptPhase) {
        self.phase.store(p as u8, Ordering::Release);
    }

    pub fn is_committed(&self) -> bool {
        self.committed.load(Ordering::Acquire)
    }

    fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<Result<bool, String>>> {
        self.done_tx.subscribe()
    }

    /// 读取已发布的结论（未发布则 `None`）。
    ///
    /// 以 `outcome`（OnceLock）为唯一真值：`watch` 只是**通知**通道，
    /// 当发送时恰好没有接收者，`send` 会失败且不更新值——但结论仍然成立。
    pub fn settled(&self) -> Option<Result<bool, String>> {
        self.outcome
            .get()
            .cloned()
            .or_else(|| self.done_tx.borrow().clone())
    }
}

struct AttemptEntry {
    ctx: Arc<AttemptCtx>,
    cancel: CancellationToken,
    // 注意：这里**没有** JoinHandle（G7 §6.6）：条目存在 ⇔ 生命周期操作未证明
    // 结束。任务 handle 被丢弃不会产生孤儿，因为"活着"与"有条目"是同一件事。
}

/// attempt 表：`live` + `last_completed_gen` **同锁**（P0-3 的原子性来源）。
///
/// 隔离态不另设 map：它就是 `live` 里 `phase == FailedUnknown` 的条目
///（避免两处真值再次分叉）。
#[derive(Default)]
struct AttemptTable {
    live: HashMap<String, AttemptEntry>,
    /// 只由 commit 路径（与移除条目同一临界区）与 reaper 复核通过后写入；
    /// 永不回退、永不清理（§6.11）。
    last_completed_gen: HashMap<String, u64>,
}

/// attempt 任务自持的守卫（G7 §6.3.2）。
///
/// 放在**任务内部**才正确：随任务结束而 drop，drop 里只做同步簿记。
/// `Drop` 在 panic 展开时同样执行，因此必须区分"正常完成"（`committed`）
/// 与"异常终止"——判据**不是** `std::thread::panicking()`，而是显式提交标记。
struct AttemptGuard {
    table: Arc<Mutex<AttemptTable>>,
    id: String,
    generation: u64,
    ctx: Arc<AttemptCtx>,
    quarantine_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

impl AttemptGuard {
    /// 唯一正常退出路径：必须在 `run_endpoint_with_attempt` 正常返回、
    /// 且 `ctx.outcome` 已写入之后调用。
    fn commit(&self) {
        debug_assert!(
            self.ctx.outcome.get().is_some(),
            "commit 前必须已有 teardown 结论"
        );
        // 先置终态再置标记：观察者看到 committed=true 时 phase 必然已是终态。
        self.ctx.set_phase(AttemptPhase::TeardownDone);
        self.ctx.committed.store(true, Ordering::Release);
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        let mut t = self.table.lock().unwrap();
        // 防御：条目已不属于本代次（例如被后续 Start 取代）则什么都不做。
        if t.live
            .get(&self.id)
            .is_none_or(|e| e.ctx.generation != self.generation)
        {
            drop(t);
            let _ = self.ctx.done_tx.send(self.ctx.outcome.get().cloned());
            return;
        }
        if self.ctx.committed.load(Ordering::Acquire) {
            // 正常完成：写完成代次 + 移除条目，**同一临界区**（P0-3）。
            t.last_completed_gen
                .insert(self.id.clone(), self.generation);
            t.live.remove(&self.id);
        } else {
            // 异常终止：**保留条目**、转隔离态、禁止新 Start（P0-1）。
            // Drop 里不能 await，也不能做任何"证明收敛"的工作（kill/wait 全是
            // async）——这正是只能"保留 + 交给 reaper"的原因。
            self.ctx.set_phase(AttemptPhase::FailedUnknown);
            let _ = self.ctx.outcome.set(Err(format!(
                "ATTEMPT_FAILED_UNKNOWN: attempt generation {} terminated abnormally (panic/abort); \
                 teardown completion NOT proven",
                self.generation
            )));
            if std::thread::panicking() {
                // 只用于留证据，**不参与判定**。
                tracing::error!(
                    endpoint = %self.id,
                    generation = self.generation,
                    "attempt task panicked; entry quarantined as FAILED_UNKNOWN"
                );
            }
        }
        let _ = self.ctx.done_tx.send(self.ctx.outcome.get().cloned());
        drop(t);
        if !self.ctx.committed.load(Ordering::Acquire) {
            // 异常路径把 id 交给监督清理：try_send 非阻塞，Drop 里绝不 await。
            let _ = self.quarantine_tx.send(self.id.clone());
        }
    }
}

/// Stop 受理结果（G7 §6.4 `begin_stop`）：**同步**返回，拆解已经开始。
///
/// `ctx == None` ⇒ 无 attempt（幂等成功 `was_running=false`）；
/// `Some` ⇒ 合流等待同一次 teardown 结论（Claimed / Join / Quarantined 三者
/// 都映射到同一个 `outcome`，因此两个并发 Stop 必然拿到同一结论）。
pub struct StopHandle {
    ctx: Option<Arc<AttemptCtx>>,
}

impl StopHandle {
    fn not_running() -> Self {
        Self { ctx: None }
    }

    fn join(ctx: Arc<AttemptCtx>) -> Self {
        Self { ctx: Some(ctx) }
    }

    /// 是否真的认领了一次 teardown（false = 当时无 attempt）。
    pub fn was_running(&self) -> bool {
        self.ctx.is_some()
    }

    /// 等待**同一次** teardown 的结论。丢弃本 future（客户端断开）不会取消
    /// 任何后台操作——它只持有一个 `watch::Receiver`（§6.3 第 3 条）。
    ///
    /// 结论真值是 `ctx.outcome`（OnceLock，只可写一次）；`watch` 只用于唤醒，
    /// 因此"发送时没有接收者导致 send 失败"也不会让本函数永久挂住。
    pub async fn await_result(self) -> Result<bool, String> {
        let Some(ctx) = self.ctx else {
            return Ok(false);
        };
        let mut rx = ctx.subscribe();
        loop {
            if let Some(v) = ctx.outcome.get() {
                return v.clone();
            }
            tokio::select! {
                changed = rx.changed() => {
                    if changed.is_err() {
                        // 发送端已销毁：再读一次结论，读不到就明确报"不可证明"。
                        return match ctx.outcome.get() {
                            Some(v) => v.clone(),
                            None => Err(
                                "ATTEMPT_FAILED_UNKNOWN: attempt context dropped before publishing outcome"
                                    .to_string(),
                            ),
                        };
                    }
                }
                // 兜底轮询：覆盖"send 失败（无接收者）但结论已写入"的路径。
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }
}

pub struct MesaManager {
    drivers: RwLock<Vec<DiscoveredDriver>>,
    snapshot: Arc<Snapshot>,
    source: Arc<dyn PointIdSource>,
    /// G7：代次化 attempt 表（生命周期唯一真值；替换旧的 `running` 表）。
    attempts: Arc<Mutex<AttemptTable>>,
    /// 每 endpoint 已发放的最大代次（单调、永不回收；墓碑以下界用它）。
    gens: Mutex<HashMap<String, u64>>,
    /// 异常终止的 attempt 交给监督清理（Drop 里 try_send，绝不 await）。
    quarantine_tx: tokio::sync::mpsc::UnboundedSender<String>,
    /// reaper 启动一次性（首次 `start_endpoint` 时在运行期内 spawn）。
    reaper_started: OnceLock<()>,
    /// 构造时无 runtime 的接收端暂存（首次在 runtime 内使用时启动）。
    pending_reaper: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>,
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
    /// 测试专用：新 attempt 是否在 teardown 第 2 步之前 panic（`test-hooks`）。
    #[cfg(feature = "test-hooks")]
    test_panic_before_teardown: AtomicBool,
    /// 测试专用：挂住 reaper 的复核（`test-hooks`），用于**确定性**观察隔离态。
    #[cfg(feature = "test-hooks")]
    reaper_hold: Arc<AtomicBool>,
    #[cfg(feature = "test-hooks")]
    test_reconnect_clocks: Mutex<HashMap<String, Arc<crate::test_clock::ReconnectClock>>>,
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
        let (quarantine_tx, quarantine_rx) = tokio::sync::mpsc::unbounded_channel();
        let attempts = Arc::new(Mutex::new(AttemptTable::default()));
        let mgr = Self {
            drivers: RwLock::new(drivers),
            snapshot,
            source,
            attempts: Arc::clone(&attempts),
            gens: Mutex::new(HashMap::new()),
            quarantine_tx,
            reaper_started: OnceLock::new(),
            pending_reaper: Mutex::new(None),
            shutdown: CancellationToken::new(),
            descriptor_cache: RwLock::new(HashMap::new()),
            active_sessions: std::sync::Arc::new(RwLock::new(HashMap::new())),
            event_services: RwLock::new(None),
            definition_query_budget: Arc::new(DefinitionQueryBudget::default()),
            #[cfg(feature = "test-hooks")]
            test_panic_before_teardown: AtomicBool::new(false),
            #[cfg(feature = "test-hooks")]
            reaper_hold: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "test-hooks")]
            test_reconnect_clocks: Mutex::new(HashMap::new()),
        };
        // 监督任务：**只**清理"任务已死、条目处于隔离态"的残留（不启停端点、
        // 不写期望态、不改快照）⇒ 不是对账器/看门狗（G7 §10-1 边界）。
        // 仅在运行期内启动；纯同步构造（无 runtime）时延后到首次 start。
        if tokio::runtime::Handle::try_current().is_ok() {
            mgr.spawn_reaper(quarantine_rx);
        } else {
            mgr.pending_reaper.lock().unwrap().replace(quarantine_rx);
        }
        mgr
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

    /// 兼容投影：是否有**活的**生命周期操作（Starting/Running）。
    ///
    /// G7 §1.2 / §10-6：`is_running()==false` 是"停干净"的**必要不充分**条件
    ///（拆解开始即假），只作为 UI/兼容投影保留，对外语义与修复前一致；
    /// **禁止**把它用于完成判据、门控判定或回归断言的核心断言。可信判据是
    /// [`Self::has_live_attempt`] / [`Self::current_gen`] / 代次化快照。
    pub fn is_running(&self, endpoint_id: &str) -> bool {
        self.attempts
            .lock()
            .unwrap()
            .live
            .get(endpoint_id)
            .is_some_and(|e| e.ctx.phase().is_live())
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.attempts
            .lock()
            .unwrap()
            .live
            .iter()
            .filter(|(_, e)| e.ctx.phase().is_live())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// 是否存在**未完成**的 attempt（含 Stopping 与隔离态 FailedUnknown）。
    ///
    /// 这是"Stop 在途保护"的唯一判据：条目存在 ⇔ 该 attempt 尚未被证明完成
    /// teardown。它不依赖任何计数器副本，因此不会随请求取消而提前释放。
    pub fn has_live_attempt(&self, endpoint_id: &str) -> bool {
        self.attempts.lock().unwrap().live.contains_key(endpoint_id)
    }

    /// 当前 attempt 的阶段（诊断用；无条目返回 `None`）。
    pub fn attempt_phase(&self, endpoint_id: &str) -> Option<AttemptPhase> {
        self.attempts
            .lock()
            .unwrap()
            .live
            .get(endpoint_id)
            .map(|e| e.ctx.phase())
    }

    /// 当前代次：有条目取条目代次，无条目取"最后一次已证明完成的代次"。
    ///
    /// 两者取自**同一把表锁** ⇒ 不可能读到"既无条目又无凭证"的组合（P0-3）。
    /// 无凭证（从未有 attempt / 进程刚重启）返回 `None`——判据侧必须把它当作
    /// "无基准 ≠ 已停干净"（fail-closed）。
    pub fn current_gen(&self, endpoint_id: &str) -> Option<u64> {
        let t = self.attempts.lock().unwrap();
        t.live
            .get(endpoint_id)
            .map(|e| e.ctx.generation)
            .or_else(|| t.last_completed_gen.get(endpoint_id).copied())
    }

    /// 该 id **已发放的最大代次**（墓碑下界）。
    ///
    /// 注意是"已发放的最大值"，不是"下一个将要发放的值"：墓碑比较是
    /// `generation <= retired`，差一会让端点重建后的首个状态写入被永久丢弃。
    pub fn gen_floor(&self, endpoint_id: &str) -> u64 {
        self.gens
            .lock()
            .unwrap()
            .get(endpoint_id)
            .copied()
            .unwrap_or(0)
    }

    /// 分配下一个代次（per endpoint，单调、永不回收）。
    fn next_gen(&self, endpoint_id: &str) -> u64 {
        let mut g = self.gens.lock().unwrap();
        let e = g.entry(endpoint_id.to_string()).or_insert(0);
        *e += 1;
        *e
    }

    /// 首次在运行期内使用时启动监督清理任务（构造期无 runtime 的兼容路径）。
    fn ensure_reaper(&self) {
        if self.reaper_started.get().is_some() {
            return;
        }
        // 同步上下文（无 reactor）里 `tokio::spawn` 会 panic；此时保持惰性，
        // 等第一次真正在运行期内 start 时再启动（生产入口都在运行期内）。
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let rx = self.pending_reaper.lock().unwrap().take();
        if let Some(rx) = rx {
            self.spawn_reaper(rx);
        }
    }

    fn spawn_reaper(&self, rx: tokio::sync::mpsc::UnboundedReceiver<String>) {
        if self.reaper_started.set(()).is_err() {
            return;
        }
        let attempts = Arc::clone(&self.attempts);
        let registry = Arc::clone(&self.active_sessions);
        let shutdown = self.shutdown.clone();
        #[cfg(feature = "test-hooks")]
        let hold = Arc::clone(&self.reaper_hold);
        tokio::spawn(attempt_reaper_loop(
            rx,
            attempts,
            registry,
            shutdown,
            #[cfg(feature = "test-hooks")]
            hold,
        ));
    }

    /// 测试专用：注入"teardown 第 2 步之前 panic"（`test-hooks`）。
    ///
    /// 用于验证隔离态 fail-closed：panic 后条目必须保留、新 Start 必须 409、
    /// Stop 必须返回同一条 `ATTEMPT_FAILED_UNKNOWN`，直到 reaper 复核通过。
    #[cfg(feature = "test-hooks")]
    pub fn set_test_panic_before_teardown(&self, on: bool) {
        self.test_panic_before_teardown.store(on, Ordering::Relaxed);
    }

    /// 测试专用：必须在 Start 前安装；每个 Endpoint 独立，不读取或改写全局 env。
    #[cfg(feature = "test-hooks")]
    pub fn test_reconnect_clock(
        &self,
        endpoint_id: &str,
    ) -> Arc<crate::test_clock::ReconnectClock> {
        Arc::clone(
            self.test_reconnect_clocks
                .lock()
                .unwrap()
                .entry(endpoint_id.to_string())
                .or_default(),
        )
    }

    /// 测试专用：挂住 / 放行 reaper 的复核（`test-hooks`）。
    ///
    /// 为什么需要：真实收敛证明可能在微秒级完成（子进程已被 `kill_on_drop`
    /// 终止、会话键可摘除），隔离态窗口短到无法确定性观察。挂住它只是把
    /// **已有的状态**保持住，不改变任何生产语义（生产构建里该字段不存在）。
    #[cfg(feature = "test-hooks")]
    pub fn set_test_hold_reaper(&self, hold: bool) {
        self.reaper_hold.store(hold, Ordering::Relaxed);
    }

    /// 启动一个 Endpoint。已在运行 / 正在停止 / 隔离中一律拒绝。
    ///
    /// P0-2 fail-closed：事件使能（`event_tasks` 非空）但 EventStore 不可用
    /// （`event_services == None`）时直接拒绝，连 Driver 进程都不启动——
    /// 否则事件会发出却无人持久化，形成"RUNNING 但丢历史"的静默缺口。
    ///
    /// G7 §6.6：**insert-before-spawn + Entry::Vacant**。检查存在与插入占位条目
    /// 在同一次加锁内完成，覆盖写在类型层面不可表达；第二个 Start 在检查处即被
    /// 拒绝，**根本没有 spawn 发生**（因此不存在需要终止旧条目的危险动作）。
    pub fn start_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        // 既有调用方（合同测试、内置演示端点）不关心审计：等价于"无父请求、不记录"。
        self.start_endpoint_with_context(cfg, crate::attempt_audit::AttemptContext::default())
    }

    /// 带审计上下文的启动（Issue #102）：REST Start / Bootstrap / 开机恢复都走这条，
    /// 由 Core 传入父请求 ID、来源与 sink，运行期 epoch 子记录才能挂回正确的那次请求。
    ///
    /// 与 [`Self::start_endpoint`] 的唯一差别就是 `AttemptCtx` 里多带一个审计上下文；
    /// 门/attempt 表/insert-before-spawn 的语义完全不变。
    pub fn start_endpoint_with_context(
        &self,
        cfg: BuiltinEndpoint,
        audit: crate::attempt_audit::AttemptContext,
    ) -> Result<(), String> {
        self.ensure_reaper();
        let mut t = self.attempts.lock().unwrap();
        match t.live.entry(cfg.endpoint_id.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => Err(match e.get().ctx.phase() {
                AttemptPhase::FailedUnknown => format!(
                    "ATTEMPT_FAILED_UNKNOWN: endpoint `{}` generation {} awaiting supervisor \
                     verification; teardown completion NOT proven",
                    cfg.endpoint_id,
                    e.get().ctx.generation
                ),
                AttemptPhase::Stopping => format!(
                    "STOP_IN_PROGRESS: endpoint `{}` is stopping (teardown not completed)",
                    cfg.endpoint_id
                ),
                _ => format!("endpoint `{}` already running", cfg.endpoint_id),
            }),
            std::collections::hash_map::Entry::Vacant(v) => {
                if !cfg.event_tasks.is_empty() && self.event_services.read().unwrap().is_none() {
                    return Err(format!(
                        "EVENT_STORE_UNAVAILABLE: endpoint `{}` has event tasks but EventStore is unavailable",
                        cfg.endpoint_id
                    ));
                }
                let disc = self.find_driver(&cfg.driver_id).ok_or_else(|| {
                    format!("driver `{}` not found or not launchable", cfg.driver_id)
                })?;

                let generation = self.next_gen(&cfg.endpoint_id);
                let shutdown = self.shutdown.child_token();
                let cancel = shutdown.clone();
                let snapshot = Arc::clone(&self.snapshot);
                let source = Arc::clone(&self.source);
                let registry = std::sync::Arc::clone(&self.active_sessions);
                let events = self.event_services.read().unwrap().clone();
                let ctx = Arc::new(AttemptCtx::new(generation, audit));
                #[cfg(feature = "test-hooks")]
                if let Some(clock) = self
                    .test_reconnect_clocks
                    .lock()
                    .unwrap()
                    .get(&cfg.endpoint_id)
                    .cloned()
                {
                    let _ = ctx.test_reconnect_clock.set(clock);
                }
                #[cfg(feature = "test-hooks")]
                ctx.test_panic_before_teardown.store(
                    self.test_panic_before_teardown.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                let guard = AttemptGuard {
                    table: Arc::clone(&self.attempts),
                    id: cfg.endpoint_id.clone(),
                    generation,
                    ctx: Arc::clone(&ctx),
                    quarantine_tx: self.quarantine_tx.clone(),
                };
                v.insert(AttemptEntry {
                    ctx: Arc::clone(&ctx),
                    cancel,
                });
                // 锁必须在 spawn 之前释放：任务内部的 commit/Drop 需要同一把锁。
                drop(t);

                let task_ctx = Arc::clone(&ctx);
                tokio::spawn(async move {
                    let g = guard; // 任务自持：panic 时同样 drop
                    let outcome = run_endpoint_with_attempt(
                        disc,
                        cfg,
                        snapshot,
                        source,
                        shutdown,
                        registry,
                        events,
                        crate::session::HeartbeatParams::default(),
                        Some(Arc::clone(&task_ctx)),
                    )
                    .await;
                    // 只有正常返回才走到这里；teardown 结论先写入，再 commit。
                    let _ = task_ctx.outcome.set(outcome.map_or(Ok(true), Err));
                    g.commit();
                });
                Ok(())
            }
        }
    }

    /// 兼容旧名称。
    pub fn start_builtin_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        self.start_endpoint(cfg)
    }

    /// Stop 的**受理**（同步，G7 §6.4 `begin_stop`）。
    ///
    /// 语义（逐条对应 §5.2 六行）：
    /// - 无条目 ⇒ `NotRunning`（幂等成功 `was_running=false`）；
    /// - `Stopping` ⇒ **合流**：不重复 cancel/teardown，等待同一次结论；
    /// - `FailedUnknown`（隔离态）⇒ 不重复 teardown、不清除隔离，直接返回该条目
    ///   已发布的同一次明确失败；
    /// - 其余 ⇒ 置 `Stopping` + cancel（拆解**立即**开始），返回同一个结论通道。
    ///
    /// 全同步 ⇒ "受理"与"启动拆解"不可分割：不存在"代次已 +1、desired 已写假、
    /// 但 teardown 没启动"的中间态。
    pub fn begin_stop(&self, endpoint_id: &str) -> StopHandle {
        let t = self.attempts.lock().unwrap();
        let Some(e) = t.live.get(endpoint_id) else {
            return StopHandle::not_running();
        };
        match e.ctx.phase() {
            // 已证明完成、只差 Drop 移除条目：不 cancel、不改 phase，只等结论。
            _ if e.ctx.is_committed() => StopHandle::join(Arc::clone(&e.ctx)),
            AttemptPhase::FailedUnknown => StopHandle::join(Arc::clone(&e.ctx)),
            AttemptPhase::Stopping => StopHandle::join(Arc::clone(&e.ctx)),
            _ => {
                e.ctx.set_phase(AttemptPhase::Stopping);
                e.cancel.cancel();
                StopHandle::join(Arc::clone(&e.ctx))
            }
        }
    }

    /// 停止指定 Endpoint。
    ///
    /// P0-3：`Ok(was_running)`——`false` = 当时未运行（幂等成功）；
    /// `Err(detail)` = teardown 异常（drain fatal/timeout 精确码在前，
    /// 或任务异常终止的 `ATTEMPT_FAILED_UNKNOWN`）。Stop 等待 teardown 真正
    /// 完成才返回：内部各步骤自带 bounded timeout（post 5s + terminate 10s +
    /// barrier 5s + drain 可组合预算 15s，见 `INGRESS_DRAIN_TIMEOUT`，
    /// 总 ≈ 35s 最坏）。
    ///
    /// G7：签名不变（既有 30+ 调用点零改动）；实现改为 `begin_stop` +
    /// 等待同一个结论，因此并发 Stop 会**合流**到同一次 teardown。
    pub async fn stop_endpoint(&self, endpoint_id: &str) -> Result<bool, String> {
        let t0 = Instant::now();
        let handle = self.begin_stop(endpoint_id);
        let res = handle.await_result().await;
        let elapsed = t0.elapsed();
        // 尾部无界时的诊断（§6.3.1）：超过 teardown 预算大声告警，不加自动强杀。
        if elapsed > Duration::from_secs(35) * 3 {
            tracing::error!(
                endpoint = %endpoint_id,
                elapsed_ms = elapsed.as_millis() as u64,
                "teardown exceeded 3x budget; attempt entry stays (fail-closed, Start will 409)"
            );
        } else if elapsed > Duration::from_secs(35) {
            tracing::warn!(
                endpoint = %endpoint_id,
                elapsed_ms = elapsed.as_millis() as u64,
                "teardown exceeded budget"
            );
        }
        res
    }
    /// 统一停机（消费 self，兼容旧调用）。
    pub async fn shutdown(self) {
        self.shutdown_all().await;
    }

    /// 统一停机（&self 版，供 Arc 持有者调用）。
    /// P0-3：直接 await 全部结论（teardown 全步骤有界，必结束），
    /// 异常大声记录；进程退出路径无调用方可接收错误，但绝不静默 detach。
    pub async fn shutdown_all(&self) {
        self.shutdown.cancel();
        let handles: Vec<(String, StopHandle)> = {
            let t = self.attempts.lock().unwrap();
            t.live
                .iter()
                .map(|(id, e)| {
                    e.cancel.cancel();
                    (id.clone(), StopHandle::join(Arc::clone(&e.ctx)))
                })
                .collect()
        };
        for (id, h) in handles {
            match h.await_result().await {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(endpoint = %id, "shutdown teardown failed: {e}");
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
                // 驱动给出的结构化错误码原样上抛（`UNKNOWN_PARENT` 这类码必须
                // 一路到达 REST，让调用方能区分"父节点写错"与"驱动不可用"）；
                // 只有会话级失败（超时/断开/协议）才归为 `BROWSE_FAILED`。
                //
                // `kind` 必须一起带出：驱动声明的是**通用类别**（配置/连接/超时/
                // 地址/不支持/…），REST 只有拿到它才能把"连不上设备"映射成 503、
                // "探测超时"映射成 504，而不是把它们和"父节点写错"一起压成 400。
                .map_err(|e| match e {
                    SessionError::Driver {
                        kind,
                        code,
                        message,
                    } => descriptor_error_from_driver(&kind, code, message),
                    other => DescriptorError::new("BROWSE_FAILED", format!("{other}")),
                });
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

/// 隔离态复核的 pid 判据：**只有明确 `Dead` 才算收敛**。
///
/// `Alive`（还在跑）与 `Unknown`（查不清：权限不足/查询异常）都不构成"已死"
/// 证据，必须继续隔离——fail-closed 不得被一次查询异常降级成 fail-open。
fn pid_converged(status: crate::process::ProcessStatus) -> bool {
    matches!(status, crate::process::ProcessStatus::Dead)
}

/// 隔离态监督清理（G7 §6.3.3）。
///
/// **边界（必须守住，代码注释即 review 门）**：只清理"任务已死、条目处于
/// `FailedUnknown`"的残留——kill 残留 pid、摘除属于该 id 的 registry 键、
/// 复核通过后移除条目并写完成代次。**不启动、不重启、不写 `desired_running`、
/// 不改快照、不 cancel 任何活着的 attempt** ⇒ 它不是对账器/看门狗（§10-1）。
///
/// 复核条件（有界重试）：① pid 不再存活（必要时先强杀）；② registry 中该
/// `endpoint_id` 的键已不存在（隔离条目不可能是活 attempt，故该键必为残留）。
/// 二者都成立才允许清除；否则保留条目、大声告警并继续重试——永远无法证明
/// 收敛时只能重启 Core（与 §6.3.1 "尾部无界 ⇒ Start 永远 409" 同一 fail-closed 选择）。
async fn attempt_reaper_loop(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    table: Arc<Mutex<AttemptTable>>,
    registry: Arc<
        RwLock<HashMap<String, std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>>>,
    >,
    shutdown: CancellationToken,
    #[cfg(feature = "test-hooks")] hold: Arc<AtomicBool>,
) {
    // 复核重试参数：总窗口 ≈ 30s（60 × 500ms），覆盖 tokio 孤儿队列异步回收僵尸。
    const RETRIES: u32 = 60;
    const RETRY_DELAY: Duration = Duration::from_millis(500);
    loop {
        let id = tokio::select! {
            _ = shutdown.cancelled() => return,
            id = rx.recv() => match id {
                Some(id) => id,
                None => return,
            },
        };
        // 测试闸门：仅在 `test-hooks` 构建里存在；生产路径整段被编译掉。
        // 用轮询而不是 channel：语义就是"保持隔离态直到测试放行"，轮询没有
        // 额外的唤醒时序问题（50ms 粒度对测试无影响）。
        #[cfg(feature = "test-hooks")]
        while hold.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut cleared = false;
        for attempt in 0..RETRIES {
            // 取快照（不持锁做 IO）
            let target = {
                let t = table.lock().unwrap();
                match t.live.get(&id) {
                    Some(e) if e.ctx.phase() == AttemptPhase::FailedUnknown => {
                        Some((e.ctx.generation, e.ctx.pid.load(Ordering::Acquire)))
                    }
                    // 已被其它路径清理 / 不是隔离态：无需动作
                    _ => None,
                }
            };
            let Some((generation, pid)) = target else {
                cleared = true;
                break;
            };
            // 三态复核（P0 回归）：只有**明确 Dead** 才算收敛。Unknown（权限不足/
            // 查询异常）必须继续隔离并重试——旧实现把"打不开"当"已死"，会在
            // Windows 上把仍在运行的残留驱动判成已收敛，放行新 Start 与之并存。
            let pid_status = if pid == 0 {
                crate::process::ProcessStatus::Dead
            } else {
                crate::process::process_status(pid)
            };
            if pid_status == crate::process::ProcessStatus::Alive {
                tracing::warn!(
                    endpoint = %id, generation, pid,
                    "attempt_reaper: residual driver process alive, terminating"
                );
                let _ = crate::process::kill_pid(pid);
            } else if pid_status == crate::process::ProcessStatus::Unknown {
                tracing::warn!(
                    endpoint = %id, generation, pid,
                    "attempt_reaper: 无法证明残留进程已死（进程查询异常），保持隔离并重试"
                );
            }
            let pid_dead = pid_converged(pid_status);
            // 摘除属于本 id 的残留会话键（隔离条目已无活 attempt 持有它）。
            let had_session = registry.write().unwrap().remove(&id).is_some();
            let session_released = !registry.read().unwrap().contains_key(&id);
            if pid_dead && session_released {
                let mut t = table.lock().unwrap();
                // 复核与清除必须在同一临界区重新确认代次与阶段（防止期间被改）。
                let still = t.live.get(&id).is_some_and(|e| {
                    e.ctx.generation == generation && e.ctx.phase() == AttemptPhase::FailedUnknown
                });
                if still {
                    // 异常 attempt 同样需要"终结凭证"，但 outcome **保持 Err**
                    // （失败必须仍是失败，不得改写为成功）。
                    t.last_completed_gen.insert(id.clone(), generation);
                    t.live.remove(&id);
                    cleared = true;
                }
                tracing::error!(
                    endpoint = %id, generation,
                    had_session,
                    "attempt_reaper: quarantined attempt cleared after convergence proof; \
                     original outcome remains FAILED (ATTEMPT_FAILED_UNKNOWN)"
                );
                break;
            }
            if attempt + 1 == RETRIES {
                tracing::error!(
                    endpoint = %id, generation, pid,
                    "attempt_reaper: convergence NOT proven within budget; entry stays \
                     quarantined (Start keeps returning ATTEMPT_FAILED_UNKNOWN until restart)"
                );
            }
            tokio::time::sleep(RETRY_DELAY).await;
        }
        if !cleared {
            tracing::error!(
                endpoint = %id,
                "attempt_reaper: gave up this round; entry remains quarantined (fail-closed)"
            );
        }
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

    /// Browse 错误过 Manager 边界的契约：**驱动声明的通用类别必须结构化透出**。
    ///
    /// 缺陷现场：`SessionError::Driver { code, message, .. }` 的 `..` 把 kind 丢了，
    /// 于是"父节点写错"（配置类）与"连不上设备"（连接类）、"远端超时"（超时类）
    /// 在 REST 侧完全无法区分，一律落到 400。本用例把四条分支钉死：
    /// 驱动声明 ⇒ `Some(类别)`；空 kind ⇒ `None`（不猜）；九种类别全覆盖；
    /// 既有构造点走 `new` ⇒ 恒 `None`（行为零变化）。
    #[test]
    fn browse_error_preserves_driver_declared_kind() {
        use mesa_core_types::ErrorKind as K;

        // ① 驱动声明的类别逐字过线，code/message 不改写（REST 原样进错误信封）。
        let e = descriptor_error_from_driver(
            "ConnectionError",
            "CONNECT_FAILED".into(),
            "connect refused".into(),
        );
        assert_eq!(e.kind, Some(K::Connection));
        assert_eq!(e.code, "CONNECT_FAILED");
        assert_eq!(e.message, "connect refused");

        // ② 九种通用类别都能被识别（wire 字符串 ↔ 变体一一对应）。
        let cases = [
            ("ConfigurationError", K::Configuration),
            ("ConnectionError", K::Connection),
            ("Timeout", K::Timeout),
            ("AddressError", K::Address),
            ("ProtocolError", K::Protocol),
            ("DecodeError", K::Decode),
            ("DeviceError", K::Device),
            ("Unsupported", K::Unsupported),
            ("InternalError", K::Internal),
        ];
        for (wire, want) in cases {
            let e = descriptor_error_from_driver(wire, "C".into(), "m".into());
            assert_eq!(e.kind, Some(want), "wire `{wire}` 必须映射为 {want:?}");
        }

        // ③ 空 kind = 驱动没声明类别（ok=false 却无 detail）⇒ 不猜成 Internal，
        //    回落旧码白名单，保证升级不悄悄改变已发布接口的状态码。
        let e = descriptor_error_from_driver("", "BROWSE_FAILED".into(), "peer 违约".into());
        assert_eq!(e.kind, None, "空 kind 不得被猜成 Internal");

        // ④ 既有构造点（Descriptor 获取 / OpenConnection / 会话级失败等）走 `new`，
        //    kind 恒为 None —— 这是"25 处调用点未被迫改动"的机械证据。
        let legacy = DescriptorError::new("DRIVER_UNAVAILABLE", "x");
        assert_eq!(legacy.kind, None);
        assert_eq!(legacy.code, "DRIVER_UNAVAILABLE");
        assert_eq!(legacy.message, "x");
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

/// G7 attempt 表 / 隔离态 / 完成代次凭证的**算法层**确定性单测
///（无 HTTP、无时序；`test-hooks` 门控，因为需要挂住 reaper 才能观察隔离态）。
#[cfg(all(test, feature = "test-hooks"))]
mod g7_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn empty_manager(tag: &str) -> MesaManager {
        let dir = std::env::temp_dir().join(format!(
            "mesa-g7-unit-{tag}-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        MesaManager::discover(&dir)
    }

    fn cfg(id: &str) -> BuiltinEndpoint {
        BuiltinEndpoint {
            endpoint_id: id.into(),
            driver_id: "test-driver".into(),
            connection_json: "{}".into(),
            tasks: vec![],
            event_tasks: vec![],
        }
    }

    /// 手工登记一个 attempt（等价于 `start_endpoint` 的 insert-before-spawn），
    /// 返回它的守卫。
    fn register(mgr: &MesaManager, id: &str, generation: u64) -> AttemptGuard {
        let ctx = Arc::new(AttemptCtx::new(
            generation,
            crate::attempt_audit::AttemptContext::default(),
        ));
        mgr.attempts.lock().unwrap().live.insert(
            id.to_string(),
            AttemptEntry {
                ctx: Arc::clone(&ctx),
                cancel: CancellationToken::new(),
            },
        );
        AttemptGuard {
            table: Arc::clone(&mgr.attempts),
            id: id.to_string(),
            generation,
            ctx,
            quarantine_tx: mgr.quarantine_tx.clone(),
        }
    }

    /// P0-1 主门：未 commit 就 drop（panic 展开同路径）⇒ 条目**保留**、
    /// phase=FailedUnknown、结论是**同一次**明确失败。
    #[tokio::test]
    async fn guard_without_commit_quarantines_entry() {
        let mgr = empty_manager("quarantine");
        mgr.set_test_hold_reaper(true);
        let guard = register(&mgr, "ep", 3);
        let ctx = Arc::clone(&guard.ctx);
        drop(guard); // 模拟 teardown 中途 panic：Drop 执行但从未 commit
        assert_eq!(ctx.phase(), AttemptPhase::FailedUnknown);
        assert!(mgr.has_live_attempt("ep"), "异常终止不得移除条目");
        assert_eq!(mgr.current_gen("ep"), Some(3), "隔离态代次仍可取");
        assert!(!ctx.is_committed());
        match ctx.settled() {
            Some(Err(e)) => assert!(e.starts_with("ATTEMPT_FAILED_UNKNOWN"), "{e}"),
            other => panic!("必须发布明确失败: {other:?}"),
        }
        let err = mgr.start_endpoint(cfg("ep")).unwrap_err();
        assert!(
            err.starts_with("ATTEMPT_FAILED_UNKNOWN"),
            "隔离态必须拒绝新 Start: {err}"
        );
        // Stop 命中隔离态：不重复 teardown、不清除隔离，返回同一次失败。
        let h = mgr.begin_stop("ep");
        assert!(h.was_running(), "隔离条目仍算'有 attempt'");
        let e = h.await_result().await.unwrap_err();
        assert!(e.starts_with("ATTEMPT_FAILED_UNKNOWN"), "{e}");
        assert!(mgr.has_live_attempt("ep"), "Stop 不得清除隔离");
        mgr.set_test_hold_reaper(false);
    }

    /// 正常完成：`commit()` 之后的 Drop 在**同一临界区**写完成代次并移除条目
    ///（P0-3：`current_gen` 仍可回退到该凭证，正常停止的判据才有基准）。
    #[tokio::test]
    async fn committed_guard_records_completed_generation() {
        let mgr = empty_manager("commit");
        let guard = register(&mgr, "ep", 5);
        let ctx = Arc::clone(&guard.ctx);
        let _ = ctx.outcome.set(Ok(true));
        guard.commit();
        drop(guard);
        assert_eq!(ctx.phase(), AttemptPhase::TeardownDone);
        assert!(!mgr.has_live_attempt("ep"), "正常完成必须移除条目");
        assert_eq!(
            mgr.current_gen("ep"),
            Some(5),
            "完成代次凭证必须保留（否则 await_settled 永远没有基准）"
        );
        assert_eq!(
            mgr.gen_floor("ep"),
            0,
            "手工登记不经过 next_gen，floor 仍为 0"
        );
        // 完成后允许全新 Start（这里只验证门不再拦：驱动不存在 ⇒ 报 driver not found）。
        let err = mgr.start_endpoint(cfg("ep")).unwrap_err();
        assert!(!err.starts_with("ATTEMPT_FAILED_UNKNOWN"), "{err}");
        assert!(!err.starts_with("STOP_IN_PROGRESS"), "{err}");
    }

    /// 契约行 2：Stopping 期间 Start 必须同步拒绝，且**未**产生第二个 attempt。
    #[tokio::test]
    async fn start_rejected_while_stopping() {
        let mgr = empty_manager("stopping");
        let guard = register(&mgr, "ep", 1);
        mgr.begin_stop("ep");
        let err = mgr.start_endpoint(cfg("ep")).unwrap_err();
        assert!(err.starts_with("STOP_IN_PROGRESS"), "{err}");
        assert_eq!(
            mgr.attempts.lock().unwrap().live.len(),
            1,
            "不得产生第二个 attempt（无孤儿）"
        );
        let _ = guard.ctx.outcome.set(Ok(true));
        guard.commit();
        drop(guard);
    }

    /// 代次单调且永不回收；`gen_floor` 是"已发放的最大值"（墓碑下界）。
    #[tokio::test]
    async fn generation_is_monotonic_and_floor_is_last_issued() {
        let mgr = empty_manager("generation");
        assert_eq!(mgr.next_gen("ep"), 1);
        assert_eq!(mgr.next_gen("ep"), 2);
        assert_eq!(mgr.gen_floor("ep"), 2);
        assert_eq!(mgr.next_gen("other"), 1, "代次是 per-endpoint 的");
        assert_eq!(mgr.gen_floor("missing"), 0);
        let _ = AtomicBool::new(true);
    }

    /// 隔离态复核只接受"明确已死"：`Unknown`（进程查询异常）**不得**被当成收敛。
    ///
    /// 这是 Windows 上"查询异常 ⇒ 隔离态误判"的算法层回归点：旧实现把
    /// `process_alive()==false` 直接当 `pid_dead`，而权限不足时它也是 false。
    #[test]
    fn pid_convergence_requires_proof_of_death() {
        use crate::process::ProcessStatus;
        assert!(pid_converged(ProcessStatus::Dead));
        assert!(!pid_converged(ProcessStatus::Alive));
        assert!(
            !pid_converged(ProcessStatus::Unknown),
            "无法证明已死时必须继续隔离（fail-closed），不得放行新 Start"
        );
    }

    // ---- Issue #102：epoch 来源分类（评审边界） ----

    /// 记录每次 epoch 意图的 (序号, 来源)。
    struct EpochRecorder(std::sync::Mutex<Vec<(u64, &'static str)>>);

    impl crate::attempt_audit::AttemptAuditSink for EpochRecorder {
        fn record_epoch_intent(
            &self,
            intent: &crate::attempt_audit::EpochIntent,
        ) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((intent.connection_sequence, intent.source.as_str()));
            Ok(())
        }
        fn record_epoch_outcome(
            &self,
            _request_id: &str,
            _outcome: &crate::attempt_audit::EpochOutcome,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    /// 第一次意图写入失败（审计故障）不得吃掉连接序号。
    struct FailFirstEpochIntent(AtomicBool);

    impl crate::attempt_audit::AttemptAuditSink for FailFirstEpochIntent {
        fn record_epoch_intent(
            &self,
            _intent: &crate::attempt_audit::EpochIntent,
        ) -> Result<(), String> {
            if !self.0.swap(true, Ordering::SeqCst) {
                return Err("store busy".into());
            }
            Ok(())
        }
        fn record_epoch_outcome(
            &self,
            _request_id: &str,
            _outcome: &crate::attempt_audit::EpochOutcome,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    fn attempt_with_sink(
        sink: std::sync::Arc<dyn crate::attempt_audit::AttemptAuditSink>,
    ) -> AttemptCtx {
        AttemptCtx::new(
            7,
            crate::attempt_audit::AttemptContext::audited(
                "lc-endpoint.start-ep1-1",
                "local-api",
                sink,
            ),
        )
    }

    /// 评审边界回归：首连意图已落盘但 `StartConnection` 失败时，后续重试**不得**被标成
    /// `internal_reconnect`（那会暗示"曾经连上过"）。
    ///
    /// 复现的是完整状态机：意图落盘（序号提交）→ 连接失败（不 mark established）→
    /// 下一次意图落盘 ⇒ 来源必须是 `initial_connection_retry`；只有真的连上过之后的重试
    /// 才是 `internal_reconnect`。
    #[test]
    fn retry_after_failed_first_connection_is_not_internal_reconnect() {
        let rec = Arc::new(EpochRecorder(std::sync::Mutex::new(Vec::new())));
        let ctx = attempt_with_sink(rec.clone());

        // 第 1 次：意图落盘成功（序号 1），随后 StartConnection 失败 ⇒ 不 mark established
        let rid1 = ctx.try_audit_epoch_intent("ep1", 100).unwrap().unwrap();
        assert!(rid1.ends_with(":epoch:1"), "{rid1}");
        // 第 2 次（内部重试）：仍未连上过
        let rid2 = ctx.try_audit_epoch_intent("ep1", 200).unwrap().unwrap();
        assert!(rid2.ends_with(":epoch:2"), "{rid2}");
        // 模拟某次连接真的成功：之后的重连才是 internal_reconnect
        ctx.mark_connection_established();
        let rid3 = ctx.try_audit_epoch_intent("ep1", 300).unwrap().unwrap();
        assert!(rid3.ends_with(":epoch:3"), "{rid3}");

        assert_eq!(
            rec.0.lock().unwrap().clone(),
            vec![
                (1, "initial_connection"),
                (2, "initial_connection_retry"),
                (3, "internal_reconnect"),
            ],
            "来源分类必须区分「首连重试」与「已连上后的断线重连」"
        );
    }

    /// 评审边界回归（配套）：意图写入失败时序号不被消耗，下一次成功仍是 `:epoch:1` /
    /// `initial_connection`（不会被伪装成重试或重连）。
    #[test]
    fn failed_epoch_intent_does_not_consume_sequence() {
        let ctx = attempt_with_sink(Arc::new(FailFirstEpochIntent(AtomicBool::new(false))));

        let err = ctx.try_audit_epoch_intent("ep1", 100).unwrap_err();
        assert!(err.contains("store busy"), "{err}");
        assert_eq!(ctx.peek_connection_sequence(), 1, "失败不得推进序号");

        let rid = ctx.try_audit_epoch_intent("ep1", 200).unwrap().unwrap();
        assert!(rid.ends_with(":epoch:1"), "重试仍应拿到首连序号: {rid}");
        assert!(!ctx.has_established_connection());
    }
}
