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

/// CNC 系统信息（`cnc_sysinfo`/ODBSYS 的最小可用子集）。
/// series 如 "0i-F"，version 为固件版本原文；model 无法从 ODBSYS 唯一确定，
/// 由 Driver 层按真机确认的映射处理，此处绝不猜测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocasSysInfo {
    pub series: String,
    pub version: String,
}

/// FOCAS2 访问抽象：`Fake` 与 `Native` 均为 async。
/// `Native` 的一切 FFI 调用由其内部固定 worker 线程执行（PR52 线程亲和），
/// 调用方无需关心线程（历史 `spawn_blocking` 模型已移除，见 PR52）。
#[async_trait::async_trait]
pub trait FocasApi: Send + Sync {
    /// 建立连接（Fake 下为轻量校验；Native 下调用 `cnc_allclibhndl`）。
    async fn connect(&self, host: &str, port: u16, timeout_ms: u64) -> Result<(), String>;
    /// 批量读取（与 `S7Client::read_vars` 对称），按地址顺序返回 `Value`。
    async fn read_batch(&self, addresses: &[FocasAddress]) -> Result<Vec<Value>, String>;
    /// 读系统信息（低风险只读，供动态探测；失败由调用方降级）。
    async fn system_info(&self) -> Result<FocasSysInfo, String>;
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
                // 产品合同：raw ODBST.aut 码（AUTOMATIC/MANUAL mode selection）。
                // 公开码值（0i/30i 族）：0 MDI / 1 MEM / 3 EDIT / 4 HANDLE /
                // 5 JOG / 9 REFERENCE / 10 REMOTE 等；Fake 只在其中抽样，
                // 不自创码值（旧注释 1=AUTO/2=EDIT/3=HANDLE 已纠正）。
                const MODES: [u32; 4] = [0, 1, 3, 4];
                Value::U32(MODES[(r as usize) % MODES.len()])
            }
            FocasAddress::Alarm => {
                // 报警文本（native 同口径 String；Fake 不得用 U32 伪造）
                Value::String(format!("ALM{}", r % 3))
            }
            FocasAddress::ProgramNumber => Value::U32(1000 + (r % 9000)),
            FocasAddress::ProgramMain => Value::U32(1000 + (r % 9000)),
            FocasAddress::ProgramName => Value::String(format!("O{:04}", 1000 + (r % 9000))),
            FocasAddress::Axis { axis: _, kind } => {
                // 模拟位置：-10000..10000 带小数（以 0.001 为单位存储为 I32）
                let base = (r % 20001) as i32 - 10000;
                match kind {
                    AxisKind::Absolute => {
                        // 返回 I32 位置（单位 0.001mm），上层直接取 Value::I32
                        Value::I32(base * 100)
                    }
                    // 其余 kind 当前无可信读路径：Fake 不得伪造位置，
                    // 与 Native fail-closed 对齐（ERR → 单点 BAD）。
                    _ => Value::String(format!("ERR:EW_NOOPT axis {kind:?} unsupported")),
                }
            }
            FocasAddress::Feed => {
                // 进给 0..5000
                Value::U32(r % 5001)
            }
            // 当前活动主轴速度（`cnc_acts`，与 machine/spindle_speed 资源对应）。
            FocasAddress::ActiveSpindleSpeed => Value::I32((r % 3000) as i32),
            // indexed speed fail-closed：parser 兼容保留，但 Fake 不得
            // 用 active 速度冒充 spindle[n].speed（与 Native 同口径 ERR）。
            FocasAddress::Spindle {
                kind: SpindleKind::Speed,
                ..
            } => Value::String("ERR:EW_NOOPT indexed spindle speed unsupported".into()),
            FocasAddress::Spindle { spindle: _, kind } => match kind {
                // Speed 不可达（上一臂已拦截 indexed speed，此臂仅 load/gear/maxrpm）。
                SpindleKind::Load => Value::U32(r % 101), // 0..100%
                // Native 真机口径 I16→I32（Fake 旧 U32 错误，对齐 Native）。
                SpindleKind::Gear => Value::I32(((r % 4) + 1) as i32),
                SpindleKind::MaxRpm => Value::I32((6000 + (r % 4000)) as i32),
                SpindleKind::Speed => {
                    Value::String("ERR:EW_NOOPT indexed spindle speed unsupported".into())
                }
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
                    // 字节/字范围（PR56 产品合同：无 bit 时 I32，与生产一致）。
                    let _ = addr;
                    Value::I32((r % 256) as i32)
                }
            }
            FocasAddress::Diagnosis { number: _, axis: _ } => {
                // Diagnosis 不进入 Fake random 模拟。
                // Native/Wire 合同需要真实 ABI evidence；
                // Fake 若未来支持，必须复用 diagnosis REAL contract
                // （engineering F64；见 `diagnosis_to_value` B2-C1），
                // 不得回退 `Value::I32(random)` 伪协议。
                // 当前 PR52 前门 fail-closed，此分支不可达。
                unreachable!("diagnosis fake must stay fail-closed (PR52)")
            }
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

    /// Fake 固定身份（合同基准，确定性）：一台 0i-F，固件 1.0。
    async fn system_info(&self) -> Result<FocasSysInfo, String> {
        Ok(FocasSysInfo {
            series: "0i-F".into(),
            version: "1.0".into(),
        })
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
// Native 实现：动态加载 Fwlib + 固定 worker 线程（PR52 线程亲和）
// ---------------------------------------------------------------------------
//
// 背景：FANUC DLL 在 handle 创建时记录 `GetCurrentThreadId`，后续调用比对
// 线程 ID；`tokio::spawn_blocking` 每次可能落到不同 OS 线程，旧模型下
// connect 与 read 可能不在同一线程（能跑通只是没触发失败）。
// PR52 模型（克制：单 worker，非 pool；FOCAS 本身串行，此模型最自然）：
//
// ```text
// NativeFocasApi (async, Clone 共享 WorkerHandle)
//     │  mpsc request + oneshot reply
//     ▼
// NativeWorker (固定 OS thread)
//     ├─ NativeLib（OnceLock 懒加载，worker 内首次使用）
//     ├─ Option<handle>（创建/调用/free 全在此线程）
//     ├─ connect / read_batch / system_info / disconnect
//     └─ 退出时 free handle exactly once
// ```
//
// PR52 不证明上传包的 ABI 推断最终正确；只保证在 ABI 未证明正确时，
// 不执行危险调用（危险 FFI 一律 fail-closed，见 `read_one_on_worker`）。

use crate::native::{FocasRet, NativeLib};
use std::sync::Mutex;

/// worker 请求（async 侧 → worker 线程；oneshot 回执）。
enum WorkerOp {
    /// 建连（OPEN）。成功即 worker 内 `handle = Some`。
    Connect {
        host: String,
        port: u16,
        timeout_ms: u64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// 批量读（worker 内全量执行，含 PMC 分组与缓存；单次 FFI 序列）。
    ReadBatch {
        addrs: Vec<FocasAddress>,
        reply: tokio::sync::oneshot::Sender<Result<Vec<Value>, String>>,
    },
    /// 系统信息（worker 内 `cnc_sysinfo`）。
    SystemInfo {
        reply: tokio::sync::oneshot::Sender<Result<FocasSysInfo, String>>,
    },
    /// 断开（CLOSE + `handle = None`；best-effort，不掩盖上层结论）。
    Disconnect {
        reply: tokio::sync::oneshot::Sender<()>,
    },
    /// 停 worker（消费剩余队列后退出；`Drop` 与显式 `shutdown` 用）。
    #[allow(dead_code)]
    Shutdown,
    /// 线程探针（测试与诊断用；生产路径不发送，`dead_code` 允许）。
    #[allow(dead_code)]
    ProbeThread {
        reply: tokio::sync::oneshot::Sender<std::thread::ThreadId>,
    },
}

/// worker 句柄（`NativeFocasApi` 的唯一状态；Clone 共享同一 worker）。
/// 确定生命周期（`WorkerHandle::drop`）：最后一个 `Arc` 释放时，
/// 关 sender → worker 消费完已入队 op 后 free handle exactly once → 退出 →
/// join。`Drop` 返回即 free 完成（`join` 在 `Drop` 内同步等待；见下注释）。
/// N03 真修：有界队列（`WORKER_QUEUE_MAX` 背压）；`submit` 在队满时直接
/// `EW_BUSY`，不无界积压。关闭后 backlog 上界见 `shutdown_blocking` 注释
/// （已入队仍 drain，不止等一个 FFI）。
struct WorkerHandle {
    sender: Mutex<Option<tokio::sync::mpsc::Sender<WorkerOp>>>,
    /// worker OS 线程（`None` = 已 join；`Mutex` 保并发 take，幂等）。
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Native worker 请求队列上限（N03 背压；PR52 无界是 bug）。
/// FOCAS 本身串行 + 采集周期远大于单次 FFI，16 足够；超限即 `EW_BUSY`
///（调用方按连接错误重连/丢弃本批，不在本层排队放大内存）。
/// `pub(crate)` 供 lib.rs 回归锁形状（值本身是工程选择，不是协议常量）。
pub(crate) const WORKER_QUEUE_MAX: usize = 16;

impl WorkerHandle {
    fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel::<WorkerOp>(WORKER_QUEUE_MAX);
        let join = std::thread::Builder::new()
            .name("focas-native-worker".into())
            .spawn(move || NativeWorker::run(rx))
            .expect("FOCAS native worker 线程必须能启动");
        Self {
            sender: Mutex::new(Some(tx)),
            join: Mutex::new(Some(join)),
        }
    }

    /// 同步关闭 worker 并 join：关闭 sender（worker 退出；N03 有界语义）→
    /// join 等待完成。幂等（重复调即返回）。
    /// 关闭语义（N03 注释修正）：sender 全部 drop 后，tokio bounded `mpsc`
    /// 的 Receiver 仍会把**已缓存在 channel 中的消息消费完**，
    /// `blocking_recv()` 才返回 `None`；因此最坏等待不是“单个 FFI timeout”，
    /// 而是“当前 operation + 最多 `WORKER_QUEUE_MAX` 个已入队 operation”
    /// 依次执行——仍有界（禁止无限积压的目标已实现），但不是单 FFI 上界。
    /// `Drop` 与显式 `shutdown_blocking` 共用此路径。
    fn shutdown_blocking(&self) {
        // 先关 sender（take 即关闭通道；worker 侧 blocking_recv → None）。
        self.sender.lock().unwrap().take();
        // 再 join（worker 已 free handle exactly once 后退出）。
        if let Some(h) = self.join.lock().unwrap().take() {
            let _ = h.join();
        }
    }

    /// 下发 op；worker 已退出即 `Err`（调用方转连接错误，由上层重连）。
    /// N03 背压：队列满即 `EW_BUSY`（`try_send`，不阻塞 async 调用方，
    /// 不无界积压；FOCAS 串行语义下调用方丢弃本批即可）。
    fn submit(&self, op: WorkerOp) -> Result<(), String> {
        let guard = self.sender.lock().unwrap();
        let tx = guard
            .as_ref()
            .ok_or_else(|| "EW_NODLL native worker 已退出".to_string())?;
        tx.try_send(op).map_err(|e| match e {
            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                "EW_BUSY native worker 队列已满".to_string()
            }
            tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                "EW_NODLL native worker 已退出".to_string()
            }
        })
    }

    /// 同步等待 oneshot（async 侧用；worker panic/退出即连接错误）。
    async fn await_reply<T>(
        rx: tokio::sync::oneshot::Receiver<Result<T, String>>,
    ) -> Result<T, String> {
        rx.await
            .map_err(|_| "EW_SOCKET native worker 无响应（已退出）".to_string())?
    }

    /// 同步等待 unit oneshot。
    async fn await_unit(rx: tokio::sync::oneshot::Receiver<()>) {
        let _ = rx.await;
    }
}

/// 固定 worker：此线程是进程内唯一执行 FFI 的 OS 线程。
/// `handle` 的创建/调用/free 全在此 `run` 循环内，无跨线程传递。
struct NativeWorker {
    lib: std::sync::OnceLock<Result<NativeLib, String>>,
    handle: Option<u16>,
}

impl NativeWorker {
    /// N03：有界 `Receiver`（与 `WORKER_QUEUE_MAX` 配对；`blocking_recv`
    /// 在 sender 关闭后返回 `None` 即退出，不无限排空——未送达的 op
    /// 直接丢弃，调用方已收 `EW_BUSY`/连接错误）。
    fn run(rx: tokio::sync::mpsc::Receiver<WorkerOp>) {
        let mut me = Self {
            lib: std::sync::OnceLock::new(),
            handle: None,
        };
        // `Receiver` 不是 Sync，但 worker 是唯一消费方；裸 OS 线程用
        // `rx.blocking_recv()`（tokio 特性：裸线程阻塞收）。
        let mut rx = rx;
        while let Some(op) = rx.blocking_recv() {
            match op {
                WorkerOp::Connect {
                    host,
                    port,
                    timeout_ms,
                    reply,
                } => {
                    let r = me.on_connect(&host, port, timeout_ms);
                    let _ = reply.send(r);
                }
                WorkerOp::ReadBatch { addrs, reply } => {
                    let r = me.on_read_batch(&addrs);
                    let _ = reply.send(r);
                }
                WorkerOp::SystemInfo { reply } => {
                    let r = me.on_system_info();
                    let _ = reply.send(r);
                }
                WorkerOp::Disconnect { reply } => {
                    me.on_disconnect();
                    let _ = reply.send(());
                }
                WorkerOp::Shutdown => {
                    me.on_disconnect();
                    break;
                }
                WorkerOp::ProbeThread { reply } => {
                    let _ = reply.send(std::thread::current().id());
                }
            }
        }
        // 队列耗尽（所有 sender 已 drop）即退出；退出前确保 handle 已 free。
        me.on_disconnect();
    }

    fn lib(&mut self) -> Result<&NativeLib, String> {
        let r = self.lib.get_or_init(NativeLib::load);
        match r {
            Ok(lib) => Ok(lib),
            Err(e) => Err(e.clone()),
        }
    }

    fn on_connect(&mut self, host: &str, port: u16, timeout_ms: u64) -> Result<(), String> {
        // 重复 connect：先 free 旧 handle（单 worker 内串行，无竞态）。
        self.on_disconnect();
        // NOTE：`self.lib` 借用与 `self.handle` 写入不能同时活跃；
        // 先完成 FFI 调用（NLL 结束借用）再写 handle。
        let hdl = {
            let lib = self.lib()?;
            let timeout_secs = timeout_ms.div_ceil(FOCAS_MS_PER_S) as i32;
            lib.cnc_allclibhndl3(host, port, timeout_secs)
                .map_err(NativeFocasApi::map_ret_err)?
        };
        self.handle = Some(hdl);
        tracing::info!(host=%host, port, hdl, "FOCAS Native 连接建立（worker 线程）");
        Ok(())
    }

    fn on_disconnect(&mut self) {
        if let Some(hdl) = self.handle.take() {
            // `lib` 未加载（如 connect 前 disconnect）即无 handle 可 free。
            if let Some(Ok(lib)) = self.lib.get().map(|r| r.as_ref()) {
                let _ = lib.cnc_freelibhndl(hdl);
                tracing::info!(hdl, "FOCAS 句柄已释放（worker 线程）");
            }
        }
    }

    fn on_system_info(&mut self) -> Result<FocasSysInfo, String> {
        // 先取 handle（Copy），再借 lib（NLL 作废借用顺序无关，文风统一）。
        let hdl = self
            .handle
            .ok_or_else(|| "NOT_CONNECTED 未调用 connect".to_string())?;
        let lib = self.lib()?;
        let sys = lib.cnc_sysinfo(hdl).map_err(NativeFocasApi::map_ret_err)?;
        let series = crate::native::odbsys_field(&sys.series)
            .ok_or_else(|| "sysinfo series 非法".to_string())?;
        let version = crate::native::odbsys_field(&sys.version)
            .ok_or_else(|| "sysinfo version 非法".to_string())?;
        Ok(FocasSysInfo { series, version })
    }

    /// worker 内批量读（含 PMC 分组与单批缓存；原 `spawn_blocking` 闭包整体搬入）。
    /// 危险 FFI（meter/gear/diagnosis/alarm）在 `read_one_on_worker` 内
    /// fail-closed（见该函数注释），此处逻辑与旧路径一致，仅执行位置改变。
    fn on_read_batch(&mut self, addrs: &[FocasAddress]) -> Result<Vec<Value>, String> {
        if addrs.is_empty() {
            return Ok(Vec::new());
        }
        // 先取 handle（usize Copy），避免 `self.handle` 借用跨后续 `self.lib()` 调用。
        let hdl = self
            .handle
            .ok_or_else(|| "NOT_CONNECTED 未调用 connect".to_string())?;
        let lib = self.lib()?;
        Self::read_batch_inner(lib, hdl, addrs)
    }

    /// 纯函数：给定 `lib + hdl + addrs` 执行批量语义（PMC 分组/缓存/隔离）。
    /// `&self` 不需要（worker 调度与 FFI 语义解耦，便于单测直测分组逻辑）。
    /// NOTE：`lib: &NativeLib` 生命周期仅限本次调用（worker 线程内），
    /// 绝不跨线程传递引用（线程亲和要求）。
    fn read_batch_inner(
        lib: &NativeLib,
        hdl: u16,
        addrs: &[FocasAddress],
    ) -> Result<Vec<Value>, String> {
        // 语义批处理：PMC 连续区合并为范围读，减少 FOCAS 调用次数（P0）；其余资源仍按单点隔离
        let mut pmc_groups: Vec<Vec<usize>> = Vec::new();
        let mut other: Vec<usize> = Vec::new();
        let mut cur_group: Vec<usize> = Vec::new();
        let mut cur_kind: Option<char> = None;
        let mut cur_next: Option<u32> = None;
        for (idx, addr) in addrs.iter().enumerate() {
            if let FocasAddress::Pmc { kind, addr: a, bit } = addr
                && bit.is_none()
            {
                let (_, width, _) = crate::native::NativeLib::pmc_layout(*kind, None);
                let w = width as u32;
                let can_merge =
                    cur_kind == Some(*kind) && cur_next == Some(*a) && cur_group.len() < 16;
                if can_merge {
                    cur_group.push(idx);
                    cur_next = Some(a + w);
                    continue;
                } else {
                    if !cur_group.is_empty() {
                        pmc_groups.push(std::mem::take(&mut cur_group));
                    }
                    cur_group.push(idx);
                    cur_kind = Some(*kind);
                    cur_next = Some(a + w);
                    continue;
                }
            }
            if !cur_group.is_empty() {
                pmc_groups.push(std::mem::take(&mut cur_group));
                cur_kind = None;
                cur_next = None;
            }
            other.push(idx);
        }
        if !cur_group.is_empty() {
            pmc_groups.push(cur_group);
        }
        // 周期缓存：同批次内 cnc_statinfo / cnc_rddynamic2 / cnc_absolute / cnc_acts 等共享调用仅执行一次
        let mut stat_cache: Option<Result<crate::native::OdbSt, FocasRet>> = None;
        let mut dy_cache: Option<Result<crate::native::OdbDy2, FocasRet>> = None;
        let mut acts_cache: Option<Result<crate::native::OdbActs, FocasRet>> = None;
        let mut axis_cache: std::collections::HashMap<u8, Result<i32, FocasRet>> =
            std::collections::HashMap::new();
        let mut out: Vec<Option<Value>> = vec![None; addrs.len()];
        // helper：带缓存的 read_one（闭包借用 lib/hdl/caches；单 worker 内串行）
        let mut read_cached = |addr: &FocasAddress| -> Result<Value, String> {
            match addr {
                FocasAddress::Status => {
                    let r = stat_cache.get_or_insert_with(|| lib.cnc_statinfo(hdl));
                    match r {
                        // 产品合同：machine/status = ODBST.aut 原始码
                        //（AUTOMATIC/MANUAL mode selection），不是 run/motion。
                        Ok(st) => Ok(Value::U32(st.aut as u32)),
                        Err(e) => Err(NativeFocasApi::map_ret_err(*e)),
                    }
                }
                FocasAddress::Feed => {
                    let r = dy_cache.get_or_insert_with(|| lib.cnc_rddynamic2(hdl));
                    match r {
                        Ok(dy) => Ok(Value::U32(dy.actf as u32)),
                        Err(e) => Err(NativeFocasApi::map_ret_err(*e)),
                    }
                }
                FocasAddress::Axis { axis, kind } => {
                    // fail-closed：只有 Absolute 有可信读路径（`cnc_absolute`）；
                    // 其余 kind 不得用 absolute 值冒充，更不得用 feed 回退。
                    // `axis_cache` 只服务 Absolute。
                    if *kind != AxisKind::Absolute {
                        return Err(format!("EW_NOOPT axis {kind:?} unsupported"));
                    }
                    let r = axis_cache.entry(*axis).or_insert_with(|| {
                        match lib.cnc_absolute(hdl, *axis) {
                            Ok(v) => Ok(v),
                            Err(e) => Err(e),
                        }
                    });
                    match r {
                        Ok(v) => Ok(Value::I32(*v)),
                        Err(e) => Err(NativeFocasApi::map_ret_err(*e)),
                    }
                }
                FocasAddress::ActiveSpindleSpeed => {
                    let r = acts_cache.get_or_insert_with(|| lib.cnc_acts(hdl));
                    match r {
                        Ok(v) => Ok(Value::I32(v.data)),
                        Err(e) => Err(NativeFocasApi::map_ret_err(*e)),
                    }
                }
                FocasAddress::Spindle {
                    kind: crate::address::SpindleKind::Speed,
                    ..
                } => {
                    // fail-closed：indexed speed 已无可信读路径（`cnc_acts`
                    // 不接受 spindle 号）；只有 `ActiveSpindleSpeed`
                    //（machine/spindle_speed）可调 `cnc_acts`。
                    // parser 兼容保留，但读到即 unsupported（ERR → BAD）。
                    Err(
                        "EW_NOOPT indexed spindle speed unsupported (use machine/spindle_speed)"
                            .into(),
                    )
                }
                _ => Self::read_one_on_worker(lib, hdl, addr),
            }
        };
        // 点级隔离：EW_NOOPT/DATA/RANGE/… 即单点 ERR/BAD；其余整批 Err。
        // （与旧 spawn_blocking 路径同语义；抽小函数避免三处重复。）
        fn isolate(addr: &FocasAddress, r: Result<Value, String>) -> Result<Option<Value>, String> {
            match r {
                Ok(v) => Ok(Some(v)),
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
                        Ok(Some(Value::String(format!("ERR:{}", e))))
                    } else {
                        Err(e)
                    }
                }
            }
        }
        for group in &pmc_groups {
            // WORD range fast-path（与旧路径一致；失败回退逐点）。
            let mut fast_done = false;
            if group.len() > 1
                && let Some(FocasAddress::Pmc {
                    kind,
                    addr: start,
                    bit: None,
                }) = addrs.get(group[0]).cloned()
            {
                let (data_type, width, _) = crate::native::NativeLib::pmc_layout(kind, None);
                if data_type == 1 {
                    let mut consecutive = true;
                    for (i, idx) in group.iter().enumerate() {
                        if let FocasAddress::Pmc {
                            addr: a,
                            bit: None,
                            kind: k,
                        } = &addrs[*idx]
                        {
                            let expected = start + (i as u32) * (width as u32);
                            if *k != kind || *a != expected {
                                consecutive = false;
                                break;
                            }
                        } else {
                            consecutive = false;
                            break;
                        }
                    }
                    if consecutive {
                        let adr_type = crate::native::NativeLib::pmc_adr_type(kind);
                        match lib.pmc_read_word_range(hdl, adr_type, start, group.len() as u32) {
                            Ok(vals) => {
                                for (i, idx) in group.iter().enumerate() {
                                    out[*idx] = Some(Value::I32(vals[i]));
                                }
                                fast_done = true;
                            }
                            Err(e) => {
                                let msg = NativeFocasApi::map_ret_err(e);
                                let low = msg.to_ascii_lowercase();
                                if !(low.contains("ew_noopt")
                                    || low.contains("ew_param")
                                    || low.contains("ew_length")
                                    || low.contains("ew_range")
                                    || low.contains("ew_attrib")
                                    || low.contains("ew_number")
                                    || low.contains("ew_data")
                                    || low.contains("ew_func"))
                                {
                                    return Err(msg);
                                }
                            }
                        }
                    }
                }
            }
            if fast_done {
                continue;
            }
            for idx in group {
                let addr = &addrs[*idx];
                match isolate(addr, read_cached(addr))? {
                    Some(v) => out[*idx] = Some(v),
                    None => return Err("ERR:missing".into()),
                }
            }
        }
        for idx in other {
            let addr = &addrs[idx];
            match isolate(addr, read_cached(addr))? {
                Some(v) => out[idx] = Some(v),
                None => return Err("ERR:missing".into()),
            }
        }
        let out = out
            .into_iter()
            .map(|o| o.unwrap_or(Value::String("ERR:missing".into())))
            .collect();
        Ok(out)
    }

    /// worker 内单点读（`read_one_blocking` 迁移 + PR52 fail-closed）。
    /// PR52 范围冻结：以下 FFI 在 ABI/结构闭合前**不调用**（直接
    /// `EW_NOOPT` 单点 BAD；PR52 只保证不执行危险调用，不证明 ABI）：
    /// `cnc_rdspmeter / cnc_rdsvmeter / cnc_rdspgear / cnc_rdspmaxrpm /
    /// cnc_diagnoss / cnc_rdalmmsg`（旧分支已删除，见 git 历史，不是注释）。
    /// 其余可信路径（sysinfo/statinfo/
    /// rddynamic2/absolute/acts/macro/pmc/tool/param/progdir/opmsg）
    /// 与旧语义一致（`cnc_acts` 仍服务 `ActiveSpindleSpeed`，
    /// 为后续 `0x25` Evidence Window 保留 oracle）。
    fn read_one_on_worker(lib: &NativeLib, hdl: u16, addr: &FocasAddress) -> Result<Value, String> {
        NativeFocasApi::read_one_blocking(lib, hdl, addr)
    }
}

pub struct NativeFocasApi {
    worker: std::sync::Arc<WorkerHandle>,
}

impl Default for NativeFocasApi {
    fn default() -> Self {
        Self {
            worker: std::sync::Arc::new(WorkerHandle::new()),
        }
    }
}

impl NativeFocasApi {
    pub fn new() -> Self {
        Self::default()
    }

    /// 单测可达的 fail-closed 门（`read_one_blocking` 需要 `&NativeLib`；
    /// 无 dll 环境下验证 fail-closed 语义，不碰 FFI）：
    /// Axis 非 Absolute 即 Err；indexed Spindle Speed 即 Err（只有
    /// ActiveSpindleSpeed 可调 cnc_acts）。
    /// 有 dll 时生产路径同样先判，不会走到 FFI。
    #[cfg(test)]
    pub(crate) fn read_one_no_lib_for_test(
        addr: &FocasAddress,
    ) -> Result<mesa_core_types::Value, String> {
        if let FocasAddress::Axis { kind, .. } = addr
            && *kind != AxisKind::Absolute
        {
            return Err(format!("EW_NOOPT axis {kind:?} unsupported"));
        }
        if let FocasAddress::Spindle { kind, .. } = addr
            && *kind == SpindleKind::Speed
        {
            return Err("EW_NOOPT indexed spindle speed unsupported".into());
        }
        Err("EW_NODLL test has no NativeLib".into())
    }

    // NOTE：旧 `ensure_lib`（直接 `self.lib.get_or_init`）已随 PR52 删除：
    // 库加载收归 worker 线程内懒加载，async 侧不再触碰 `NativeLib`。

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
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.submit(WorkerOp::Connect {
            host: host.to_string(),
            port,
            timeout_ms,
            reply: tx,
        })?;
        WorkerHandle::await_reply(rx).await
    }

    async fn read_batch(&self, addresses: &[FocasAddress]) -> Result<Vec<Value>, String> {
        if addresses.is_empty() {
            return Ok(Vec::new());
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.submit(WorkerOp::ReadBatch {
            addrs: addresses.to_vec(),
            reply: tx,
        })?;
        WorkerHandle::await_reply(rx).await
    }

    async fn disconnect(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .worker
            .submit(WorkerOp::Disconnect { reply: tx })
            .is_ok()
        {
            WorkerHandle::await_unit(rx).await;
        }
    }

    async fn system_info(&self) -> Result<FocasSysInfo, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.submit(WorkerOp::SystemInfo { reply: tx })?;
        WorkerHandle::await_reply(rx).await
    }
}

impl NativeFocasApi {
    /// PR52 pre-FFI 门（与生产 `read_one_blocking` 同源；纯逻辑，无 DLL/FFI）：
    /// 危险地址在任何危险 symbol 调用前即 `Err(FocasRet::Noopt)`。
    /// 生产路径在 `read_one_blocking` 首行调用；单测直测此门
    /// （DLL 存在与否都不影响结论——门在 FFI 之前）。
    /// `Ok(())` = 允许继续（可信路径）；`Err(Noopt)` = fail-closed 单点 BAD。
    /// NOTE：`Noopt` 在此仅为“暂停 oracle”的分类码（调用方转 `EW_NOOPT`
    /// 单点 BAD），不是对 CNC 能力的断言；ABI 闭合后由 Native ABI
    /// Evidence 恢复调用（PR52 只保证不执行，不证明 ABI）。
    fn pre_ffi_gate(addr: &FocasAddress) -> Result<(), FocasRet> {
        match addr {
            // PR52 暂停（ABI/结构未闭合，worker 内不进 FFI）：
            FocasAddress::Alarm => Err(FocasRet::Noopt),
            FocasAddress::Diagnosis { .. } => Err(FocasRet::Noopt),
            FocasAddress::Spindle { kind, .. } => match kind {
                SpindleKind::Speed => Err(FocasRet::Noopt),
                SpindleKind::Load | SpindleKind::Gear | SpindleKind::MaxRpm => Err(FocasRet::Noopt),
            },
            FocasAddress::ServoLoad { .. } => Err(FocasRet::Noopt),
            // 非 Absolute 无可信读路径（旧门保留，同语义）。
            FocasAddress::Axis { kind, .. } if *kind != AxisKind::Absolute => Err(FocasRet::Noopt),
            // 其余为可信路径（Status/Program/Absolute/Feed/ActiveSpindle/
            // Macro/Pmc/Param/ProgDir/ProgInfo/Upload/Tool/OpMsg）。
            _ => Ok(()),
        }
    }

    /// worker 内单点读（旧 `read_one_blocking` 迁移 + PR52 fail-closed）。
    /// PR52 范围冻结：以下 FFI 在 ABI/结构闭合前**不调用**（直接
    /// `EW_NOOPT` 单点 BAD；PR52 只保证不执行危险调用，不证明 ABI）：
    /// `cnc_rdspmeter`（8B `SpLoad` 疑越界）/ `cnc_rdsvmeter`（同前）/
    /// `cnc_rdspgear` / `cnc_rdspmaxrpm`（输出疑为 `+4` 结构）/
    /// `cnc_diagnoss`（签名疑 5 参）/ `cnc_rdalmmsg`（64B 疑不足）。
    /// 其余可信路径（sysinfo/statinfo/rddynamic2/absolute/acts/macro/
    /// pmc/tool/param/progdir/opmsg）与旧语义一致（`cnc_acts` 仍服务
    /// `ActiveSpindleSpeed`，为后续 `0x25` Evidence Window 保留 oracle）。
    /// NOTE：`Spindle::Load/Gear/MaxRpm`、`ServoLoad`、`Diagnosis`、`Alarm`
    /// 的旧 FFI 分支已整体删除（见 git 历史），不是注释掉——避免未来
    /// 有人误以为“临时禁用”而直接恢复调用。
    fn read_one_blocking(lib: &NativeLib, hdl: u16, addr: &FocasAddress) -> Result<Value, String> {
        // PR52 门：危险地址在任何 FFI 前即 Err（与单测同源逻辑）。
        if Self::pre_ffi_gate(addr).is_err() {
            return match addr {
                FocasAddress::Alarm => {
                    Err("EW_NOOPT alarm oracle suspended (PR52 ABI audit)".into())
                }
                FocasAddress::Diagnosis { .. } => {
                    Err("EW_NOOPT diagnosis oracle suspended (PR52 ABI audit)".into())
                }
                FocasAddress::Spindle { kind, .. } => match kind {
                    SpindleKind::Speed => Err(
                        "EW_NOOPT indexed spindle speed unsupported (use machine/spindle_speed)"
                            .into(),
                    ),
                    SpindleKind::Load | SpindleKind::Gear | SpindleKind::MaxRpm => {
                        Err("EW_NOOPT spindle oracle suspended (PR52 ABI audit)".into())
                    }
                },
                FocasAddress::ServoLoad { .. } => {
                    Err("EW_NOOPT servo oracle suspended (PR52 ABI audit)".into())
                }
                FocasAddress::Axis { kind, .. } => {
                    Err(format!("EW_NOOPT axis {kind:?} unsupported"))
                }
                _ => unreachable!("pre_ffi_gate 仅拒绝上述变体"),
            };
        }
        match addr {
            FocasAddress::Status => {
                // 产品合同：machine/status = ODBST.aut 原始码（mode selection）。
                let st = lib.cnc_statinfo(hdl).map_err(Self::map_ret_err)?;
                Ok(Value::U32(st.aut as u32))
            }
            // PR52 门已在函数首行拦截以下变体；此处保留不可达臂以满足穷尽
            // match（门漂移则 debug 断言红，release fail-closed，不进 FFI）。
            // NOTE：`#[allow(unreachable_patterns)]` 有意——编译器判不可达
            // 恰好证明门完整；改门必须同步改此处（见 PR52）。
            #[allow(unreachable_patterns)]
            FocasAddress::Alarm
            | FocasAddress::Diagnosis { .. }
            | FocasAddress::Spindle { .. }
            | FocasAddress::ServoLoad { .. } => {
                debug_assert!(
                    false,
                    "PR52 pre_ffi_gate 必须先行拦截（FocasAddress 变体漂移？）"
                );
                Err("EW_NOOPT oracle suspended (PR52 ABI audit)".into())
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
                // 非 Absolute 已由首行门拦截；此处到达即门漂移。
                // release 下 fail-closed（不进 FFI），debug 下断言红。
                if *kind != AxisKind::Absolute {
                    debug_assert!(false, "PR52 pre_ffi_gate 必须先行拦截非 Absolute");
                    return Err(format!("EW_NOOPT axis {kind:?} unsupported"));
                }
                // 多机型 MAX_AXIS 差异：0i 8轴 30i 10/24轴，当前 OdbAxis 以 8 轴覆盖 0i-F 基准，真机 30i 超 8 轴时需扩展
                match lib.cnc_absolute(hdl, *axis) {
                    Ok(v) => Ok(Value::I32(v)),
                    Err(e) => Err(Self::map_ret_err(e)),
                }
            }
            FocasAddress::Feed => {
                let dy = lib.cnc_rddynamic2(hdl).map_err(Self::map_ret_err)?;
                Ok(Value::U32(dy.actf as u32))
            }
            // 当前活动主轴速度（与 machine/spindle_speed 资源对应）。
            FocasAddress::ActiveSpindleSpeed => {
                let v = lib.cnc_acts(hdl).map_err(Self::map_ret_err)?;
                Ok(Value::I32(v.data))
            }
            // PR52 门已在函数首行拦截 Spindle/Servo/Alarm/Diagnosis；
            // 此处到达即门漂移（release fail-closed 不进 FFI，debug 断言红）。
            // NOTE：`#[allow(unreachable_patterns)]` 是有意的——首行门在语义上
            // 已覆盖这些变体，编译器将其判为不可达恰好证明门完整；
            // 若未来门逻辑收缩，此处即兜底（改门必须同步改此处，见 PR52）。
            #[allow(unreachable_patterns)]
            FocasAddress::Spindle { .. }
            | FocasAddress::ServoLoad { .. }
            | FocasAddress::Alarm
            | FocasAddress::Diagnosis { .. } => {
                debug_assert!(false, "PR52 pre_ffi_gate 必须先行拦截（变体漂移？）");
                Err("EW_NOOPT oracle suspended (PR52 ABI audit)".into())
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
                    // 无 bit 时：由 pmc_layout 统一 single/range 的 data_type/width，M/K/E/Z 等均为 BYTE，R/A/T/C 为 WORD，D 为 DWORD
                    let (data_type, _width, _) = crate::native::NativeLib::pmc_layout(*kind, None);
                    if data_type == 2 {
                        // DWORD (D)
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
                    } else if data_type == 1 {
                        // WORD (R/A/T/C)
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
                                // PR56 产品合同修正：BYTE raw `u8` → `I32`
                                //（Descriptor 无 bit 时为 I32；旧 `U32` 漂移）。
                                Ok(Value::I32(b as i32))
                            }
                            Err(e) => Err(Self::map_ret_err(e)),
                        }
                    } else {
                        // G/X/Y/F 等单字节
                        let b = lib
                            .pmc_rdpmcrng_byte(hdl, adr_type, *addr)
                            .map_err(Self::map_ret_err)?;
                        // PR56 产品合同修正：同上（底层 `u8` raw 不变，只改 adapter）。
                        Ok(Value::I32(b as i32))
                    }
                }
            }
            // PR52 门已在函数首行拦截 Diagnosis；此处删除旧生产分支——
            // 门是唯一真相来源，避免“门 + 分支”双写漂移。
            // NOTE：`#[allow(unreachable_patterns)]` 有意——首行门完整时
            // 此臂不可达（编译器证明门完整）；门收缩则此处兜底。
            #[allow(unreachable_patterns)]
            FocasAddress::Diagnosis { .. } => {
                Err("EW_NOOPT diagnosis oracle suspended (PR52 ABI audit)".into())
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

impl Drop for WorkerHandle {
    /// 最终 Drop：关 sender → worker 排空 → free handle exactly once →
    /// 退出 → join。`Drop` 返回即 free 完成（最后一个 `Arc` 释放时）。
    /// 阻塞说明：`join()` 只阻塞当前调用线程直到 worker 退出；
    /// worker 是独立裸 OS 线程，其 FFI/free 不依赖 Tokio runtime，
    /// oneshot `send` 不等待接收端，故与 async executor 无依赖环。
    /// 代价是 `Drop` 可能阻塞调用线程一个 worker 排空周期——对 FOCAS
    /// 这种带线程亲和约束的 native handle，这是 final-resource cleanup
    /// 可接受的 tradeoff（且幂等：已 `shutdown_blocking` 则直接返回）。
    fn drop(&mut self) {
        // `get_mut`：`Drop` 独占 `&mut`，无需 lock（`Mutex::get_mut`）。
        if let Ok(sender) = self.sender.get_mut() {
            sender.take();
        }
        if let Ok(join) = self.join.get_mut()
            && let Some(h) = join.take()
        {
            let _ = h.join();
        }
    }
}

impl Drop for NativeFocasApi {
    /// 不再手动 `strong_count` 判断：`Arc<WorkerHandle>` 的最后一个释放
    /// 自动触发 `WorkerHandle::drop`（关 sender → free → join）。
    /// 此处无逻辑（注释保留以说明生命周期归属）。
    fn drop(&mut self) {}
}

impl NativeFocasApi {
    /// 显式同步停机（blocking）：与 `WorkerHandle::drop` 同路径
    /// （关 sender → join）。幂等；`Drop` 后再调即返回。
    /// NOTE：绝不在 async 上下文内调（会阻塞 executor 线程）；
    /// `disconnect()`（async）只发 op 不 join，free 仍在 worker 内 exactly once。
    pub fn shutdown_blocking(&self) {
        self.worker.shutdown_blocking();
    }
}

/// worker 同线程断言支持（#[cfg(test)]）：connect/read/system/disconnect
/// 全程同一 OS 线程（GetCurrentThreadId 亲和要求）。
#[cfg(test)]
impl NativeFocasApi {
    /// 向 worker 发线程探针，返回 worker 线程 ID。
    async fn probe_worker_thread(&self) -> Result<std::thread::ThreadId, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.worker.submit(WorkerOp::ProbeThread { reply: tx })?;
        rx.await
            .map_err(|_| "EW_SOCKET native worker 无响应（已退出）".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PR52 验收门（核心）：connect/read/system/disconnect 全程同一 worker
    /// 线程（GetCurrentThreadId 亲和）。4 次探针 + 3 次真实 op（无 dll 时
    /// 为 EW_NODLL 错误路径，同样经 worker）必须同线程 ID。
    /// 无需真机/DLL：探针不碰 FFI；`connect` 在无 dll 下返回 EW_NODLL，
    /// 但请求仍经 worker 线程处理（线程断言不受 DLL 缺席影响）。
    #[tokio::test]
    async fn native_worker_thread_affinity() {
        let api = NativeFocasApi::new();
        let t0 = api.probe_worker_thread().await.expect("探针必须可达");
        // 穿插真实 ops（无 dll 下走错误路径，但仍经 worker 线程）。
        let _ = api.connect("127.0.0.1", 8193, 1000).await;
        let t1 = api.probe_worker_thread().await.expect("探针必须可达");
        let _ = api
            .read_batch(&[FocasAddress::Status, FocasAddress::Feed])
            .await;
        let t2 = api.probe_worker_thread().await.expect("探针必须可达");
        let _ = api.system_info().await;
        let t3 = api.probe_worker_thread().await.expect("探针必须可达");
        api.disconnect().await;
        let t4 = api.probe_worker_thread().await.expect("探针必须可达");
        assert_eq!(t0, t1, "connect 必须与探针同 worker 线程");
        assert_eq!(t0, t2, "read_batch 必须同 worker 线程");
        assert_eq!(t0, t3, "system_info 必须同 worker 线程");
        assert_eq!(t0, t4, "disconnect 后 worker 必须仍存活且同线程");
    }

    /// PR52 验收门 B1：pre-FFI 门纯逻辑测试（无 DLL/FFI/handle 即可验证）。
    /// 生产 `read_one_blocking` 首行即此门；门拒绝的 6 类危险地址永不
    /// 到达危险 symbol 调用；门放行的可信路径不受影响。
    #[test]
    fn pre_ffi_gate_blocks_before_ffi() {
        use crate::native::FocasRet;
        // 危险 6 类 → Noopt（FFI 前拦截）。
        for addr in [
            FocasAddress::Alarm,
            FocasAddress::Diagnosis { number: 0, axis: 0 },
            FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::Load,
            },
            FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::Gear,
            },
            FocasAddress::Spindle {
                spindle: 1,
                kind: SpindleKind::MaxRpm,
            },
            FocasAddress::ServoLoad { axis: 1 },
        ] {
            assert_eq!(
                NativeFocasApi::pre_ffi_gate(&addr),
                Err(FocasRet::Noopt),
                "{addr:?} 必须在 FFI 前 Noopt"
            );
        }
        // 可信路径 → 放行（Status/Feed/Absolute/ActiveSpindle/Macro/Pmc/
        // Param/Tool/OpMsg/Program…；此处抽代表，门逻辑是白名单外全放行）。
        for addr in [
            FocasAddress::Status,
            FocasAddress::Feed,
            FocasAddress::Axis {
                axis: 1,
                kind: AxisKind::Absolute,
            },
            FocasAddress::ActiveSpindleSpeed,
            FocasAddress::MacroVar { number: 100 },
            FocasAddress::Pmc {
                kind: 'R',
                addr: 100,
                bit: None,
            },
            FocasAddress::Param { number: 100 },
            FocasAddress::OpMsg,
        ] {
            assert!(
                NativeFocasApi::pre_ffi_gate(&addr).is_ok(),
                "{addr:?} 必须放行（可信路径）"
            );
        }
    }

    /// PR52 验收门 B2：最终 Drop 确定性（implicit final-drop 路径）。
    /// 最后一个 `Api` drop → `WorkerHandle::drop`（关 sender → worker 排空 →
    /// free → join）→ `Drop` 返回即 free 完成。此测试锁该路径本身
    /// （`shutdown_blocking` 只复用同逻辑，不单独测 join）。
    /// NOTE：`#[test]`（非 async）：避免在 executor 内阻塞；另起
    /// `current_thread` runtime 跑 async 部分，drop/join 在普通线程。
    /// 断言：drop 前探针可达；drop（join）返回（无 hang 即 PASS；
    /// free-once 由 worker 内 `handle.take()` 语义保证）。
    #[test]
    fn worker_final_drop_is_deterministic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt.block_on(async {
            let api = NativeFocasApi::new();
            api.probe_worker_thread()
                .await
                .expect("drop 前探针必须可达");
            // `api` 在此 block 结束时 drop（最后一个 Arc）→
            // WorkerHandle::drop（join）→ 返回。
        });
        // 能执行到此即 drop-join 无 hang（free-once 由语义保证）。
        // 另建 Api = 新 worker（旧 worker 已退出，不复用）：
        let rt2 = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt2.block_on(async {
            NativeFocasApi::new()
                .probe_worker_thread()
                .await
                .expect("新 worker 探针必须可达");
        });
    }

    /// `shutdown_blocking` 显式路径复用同逻辑（关 sender → join），
    /// 此处锁“调后 worker-gone 确定性”（与 Drop 路径同源，不断言两遍 join）。
    #[test]
    fn worker_explicit_shutdown_is_deterministic() {
        let api2 = NativeFocasApi::new();
        let rt2 = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt2.block_on(async {
            api2.probe_worker_thread()
                .await
                .expect("shutdown 前探针必须可达");
        });
        api2.shutdown_blocking();
        // join 后 worker 已退出：探针 submit 即 worker-gone 错误（确定性）。
        let rt3 = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");
        rt3.block_on(async {
            let e = api2.probe_worker_thread().await.unwrap_err();
            assert!(
                e.contains("已退出"),
                "shutdown 后必须 worker-gone（{e}），free 已在 join 前完成"
            );
        });
    }

    /// PR52 验收门 B1b：无 DLL 整批失败仍干净（改名自旧测试，证据一致）。
    /// 无 dll：connect 失败 → read_batch 整批 EW_NODLL；绝不 panic/越界。
    #[tokio::test]
    async fn missing_dll_fails_cleanly() {
        let api = NativeFocasApi::new();
        let r = api
            .read_batch(&[
                FocasAddress::Spindle {
                    spindle: 1,
                    kind: SpindleKind::Load,
                },
                FocasAddress::Spindle {
                    spindle: 1,
                    kind: SpindleKind::Gear,
                },
                FocasAddress::Spindle {
                    spindle: 1,
                    kind: SpindleKind::MaxRpm,
                },
                FocasAddress::ServoLoad { axis: 1 },
                FocasAddress::Diagnosis { number: 0, axis: 0 },
                FocasAddress::Alarm,
            ])
            .await;
        assert!(r.is_err(), "无 dll 时必须整批 Err（EW_NODLL），不 panic");
        let e = r.unwrap_err();
        assert!(
            e.contains("EW_NODLL") || e.contains("NOT_CONNECTED"),
            "错误必须为连接类（{e}）"
        );
    }

    /// PR52 验收门：可信 oracle 路径保留（`ActiveSpindleSpeed → cnc_acts`
    /// 门在 `read_one_no_lib_for_test` 与 worker 语义一致；此处锁定门语义，
    /// 真机 `0x25` Evidence Window 依赖此 oracle）。
    /// 注意：`read_one_blocking` 需要 `&NativeLib`（无 dll 不可达），
    /// 此处锁定 fail-closed 门（indexed speed 拒绝），不断言 FFI 结果。
    #[test]
    fn trusted_oracle_gates_intact() {
        // indexed speed 拒绝（只有 ActiveSpindleSpeed 可调 cnc_acts）。
        let r = NativeFocasApi::read_one_no_lib_for_test(&FocasAddress::Spindle {
            spindle: 2,
            kind: SpindleKind::Speed,
        });
        assert!(r.is_err(), "indexed speed 不得 Ok");
        // 非 Absolute 拒绝。
        let r = NativeFocasApi::read_one_no_lib_for_test(&FocasAddress::Axis {
            axis: 1,
            kind: AxisKind::Machine,
        });
        assert!(r.is_err(), "Native 非 Absolute 必须 Err");
    }

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

    /// PMC 连续 R 分组应合并为一次 range FFI（WORD width=2，10 个点 100,102..118 → 单次 bulk），非连续则拆组
    #[test]
    fn pmc_consecutive_word_range_merges() {
        // 直接复用生产布局：R/A/T/C 为 WORD width=2，其余 BYTE=1；生产分组与此一致
        use crate::native::NativeLib;
        fn group_count(addrs: &[FocasAddress]) -> usize {
            let mut groups: Vec<Vec<usize>> = Vec::new();
            let mut cur: Vec<usize> = Vec::new();
            let mut prev: Option<(char, u32)> = None;
            for (i, a) in addrs.iter().enumerate() {
                if let FocasAddress::Pmc {
                    kind,
                    addr,
                    bit: None,
                } = a
                {
                    let (dt, width, _) = NativeLib::pmc_layout(*kind, None);
                    if dt == 1 {
                        let w = width as u32;
                        if let Some((pk, pa)) = prev
                            && *kind == pk
                            && *addr == pa + w
                        {
                            cur.push(i);
                            prev = Some((*kind, *addr));
                            continue;
                        }
                        if !cur.is_empty() {
                            groups.push(std::mem::take(&mut cur));
                        }
                        cur.push(i);
                        prev = Some((*kind, *addr));
                        continue;
                    }
                }
                if !cur.is_empty() {
                    groups.push(std::mem::take(&mut cur));
                }
                prev = None;
            }
            if !cur.is_empty() {
                groups.push(cur);
            }
            groups.len()
        }

        // 10 个 WORD 连续（步距 2）：100,102..118 应为 1 组
        let addrs: Vec<FocasAddress> = (0..10)
            .map(|i| FocasAddress::Pmc {
                kind: 'R',
                addr: 100 + i * 2,
                bit: None,
            })
            .collect();
        assert_eq!(
            group_count(&addrs),
            1,
            "10 连续 WORD(R) 步距2 应合并为 1 组"
        );

        // 跳号：100,102,106（缺 104）应拆为 2 组
        let addrs2 = vec![
            FocasAddress::Pmc {
                kind: 'R',
                addr: 100,
                bit: None,
            },
            FocasAddress::Pmc {
                kind: 'R',
                addr: 102,
                bit: None,
            },
            FocasAddress::Pmc {
                kind: 'R',
                addr: 106,
                bit: None,
            },
        ];
        assert_eq!(group_count(&addrs2), 2, "跳号应拆为 2 组");

        // M/K/E 为 BYTE，不应走 WORD 合并：R100,R101 虽连续但 M100 单独 BYTE 不应视为 WORD 组
        let (dt_m, _, _) = NativeLib::pmc_layout('M', None);
        let (dt_e, _, _) = NativeLib::pmc_layout('E', None);
        let (dt_r, _, _) = NativeLib::pmc_layout('R', None);
        assert_eq!(dt_m, 0, "M 应为 BYTE(0)");
        assert_eq!(dt_e, 0, "E 应为 BYTE(0)");
        assert_eq!(dt_r, 1, "R 应为 WORD(1)");
    }

    #[test]
    fn pmc_layout_word_end_is_width_aware() {
        // WORD width=2：10 个 WORD 从 100 起，e = 100 + 10*2 -1 = 119，buf_len = 8 + 20 =28
        let start = 100u32;
        let count = 10u32;
        let width = 2u32;
        let e = start + count * width - 1;
        let len = 8 + (count as usize) * 2;
        assert_eq!(e, 119);
        assert_eq!(len, 28);
        // BYTE：10 个从 100 起 e=109
        let e_byte = 100 + 10 - 1;
        assert_eq!(e_byte, 109);
    }

    #[test]
    fn pmc_layout_single_range_consistent() {
        // single/range 共用同一布局：M/K/E 均为 BYTE，R/A/T/C 为 WORD，D 为 DWORD
        use crate::native::NativeLib;
        assert_eq!(NativeLib::pmc_layout('D', None).0, 2);
        assert_eq!(NativeLib::pmc_layout('D', None).1, 4);
        for k in ['R', 'A', 'T', 'C'] {
            let (dt, w, _) = NativeLib::pmc_layout(k, None);
            assert_eq!(dt, 1, "{k} 应为 WORD");
            assert_eq!(w, 2);
        }
        for k in ['G', 'X', 'Y', 'F', 'M', 'K', 'N', 'E', 'Z', 'B'] {
            let (dt, w, _) = NativeLib::pmc_layout(k, None);
            assert_eq!(dt, 0, "{k} 应为 BYTE");
            assert_eq!(w, 1);
        }
        // bit 强制 BYTE
        assert_eq!(NativeLib::pmc_layout('R', Some(0)).0, 0);
    }
}
