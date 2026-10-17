//! I-2 管理面**夹具驱动**（阶段 2 合同测试专用）。
//!
//! 为什么需要它：阶段 2 要验收的是「Manager + Core API 的通道与资源边界」，
//! 而真正的 NCK 定义目录实现属阶段 3。若拿 `test-driver` 当被测对象，只能覆盖
//! 501 分支（它走 SDK 默认实现），无法覆盖 200 / 分页 / 超时 / 超大载荷这些
//! 通道语义。`tests/support/test-driver` 的源码已冻结（只允许同步 minor），
//! 因此夹具放在**合同测试自己的 bin**里——与既有 `hang_helper`/`orphan_helper`
//! 同一形态，不进生产依赖图。
//!
//! **模式选择方式**：同一个二进制被 staged 成多个**短名**（`def-ok`、`def-paged`、
//! `def-big`、`def-slow`、`def-hang`、`def-silent`），启动时按 `argv[0]` 的 stem 判定
//! 模式。为什么不用环境变量：合同测试在**同一进程内并行**跑多个用例，env 是进程级
//! 共享状态，会互相串味；文件名是每个驱动目录独立的，天然隔离。
//!
//! 各模式行为：
//! - `ok`：42 条目录（单页），预览回显请求 JSON 的**逐字**副本；
//! - `paged`：250 条目录，按 `limit`/`cursor` 真翻页（验 I37-③）；
//! - `big`：4200 条**紧凑**目录（> 4096 硬顶、但 < 256 KiB 字节门）⇒ 验 E4 条目门；
//! - `slow`：延迟 1.5s 再应答（验并发预算 A1/A2：4 个在飞同时成立）；
//! - `hang`：握手成功后**永不**应答目录 RPC ⇒ 验 E3 的 5s 预算；
//! - `silent`：accept 后**永不发 Hello** ⇒ 验 E2 的 6s 启动 deadline。

use std::time::Duration;

use mesa_core_types::ErrorKind;
use mesa_driver_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR};
use mesa_driver_sdk::{Driver, DriverConnection, SdkDriverError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Ok,
    Paged,
    Big,
    Slow,
    Hang,
    Silent,
    Fail,
    /// 返回**非法 JSON**（验 E4-b 格式门）
    Invalid,
    /// 返回合法 JSON 但**缺 `definitions` 数组**（验 E4-b 结构门）
    InvalidShape,
    /// 业务拒绝：Revision 不支持（验「Configuration ⇒ 400」的映射）
    Revision,
}

/// 按可执行文件名判定模式（staged 时的文件名即模式名）。
///
/// 文件名刻意取**短名**（`def-ok` / `def-paged` …）：Linux 的 `/proc/<pid>/comm`
/// 会截断到 15 字符，长名会让合同测试的进程枚举（按 comm 匹配）全部撞在一起，
/// 导致「无残留进程」断言被静默旁路（`subprocess_orphan_guard.rs:236` 有同样约束）。
fn mode_from_argv0() -> Mode {
    let stem = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
        })
        .unwrap_or_default();
    match stem.as_str() {
        "def-paged" => Mode::Paged,
        "def-big" => Mode::Big,
        "def-slow" => Mode::Slow,
        "def-hang" => Mode::Hang,
        // `def-cancel` 与 `def-hang` 行为相同，只是**进程名不同**：取消路径的
        // 「无残留进程」断言需要按名字做 PID 快照，两个用例并行跑时同名进程会
        // 互相干扰（快照里混进对方的新进程）。
        "def-cancel" => Mode::Hang,
        "def-silent" => Mode::Silent,
        "def-fail" => Mode::Fail,
        "def-invalid" => Mode::Invalid,
        "def-shape" => Mode::InvalidShape,
        "def-revision" => Mode::Revision,
        // 缺省按 ok 处理：手工调试时不至于直接 panic
        _ => Mode::Ok,
    }
}

/// `--port <u16>`（Core 的 spawn 形态：`args(["--port", ...])`）。
fn port_from_args() -> u16 {
    std::env::args()
        .nth(2)
        .or_else(|| std::env::args().nth(1))
        .expect("usage: definitions-fixture --port <PORT>")
        .parse()
        .expect("port must be u16")
}

fn variable_id(i: usize) -> String {
    format!("fixture.variable.{i:04}")
}

/// 一条目录条目（紧凑模式只留两个字段，用于把 4200 条压在 256 KiB 字节门之下）。
fn entry(i: usize, compact: bool) -> serde_json::Value {
    let id = variable_id(i);
    if compact {
        return serde_json::json!({ "variable_id": id, "revision": 1 });
    }
    serde_json::json!({
        "variable_id": id,
        "revision": 1,
        "family": "828d",
        "scope": "collect",
        "semantics_status": "verified",
        "label": { "zh": format!("夹具变量 {i}"), "en": format!("fixture variable {i}") },
        "source": [ { "kind": "manual", "reference": "contract-test fixture" } ],
        "targets": ["channel"],
        "target_required": ["channel"],
        "arguments": [],
        "identity_template": format!("{}@channel={{channel}}", variable_id(i)),
        "identity_keys": [ { "path": "channel", "required": true } ],
        "golden_ref": [ { "area": "C", "block": "M", "variable": "FIXTURE", "entry": "nc_variable" } ],
        "digest": format!("{:064x}", i),
    })
}

/// 渲染目录响应。
///
/// 这里**允许**解析 `request_json`：本文件扮演的是 **Driver** 角色，
/// 查询参数（family/variable_id/revision/limit/cursor）的语义本来就只属于驱动层。
fn render_catalog(total: usize, request_json: &str, mode: Mode) -> String {
    let q: serde_json::Value =
        serde_json::from_str(request_json).unwrap_or(serde_json::Value::Null);
    let limit = q.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
    let cursor = q.get("cursor").and_then(|v| v.as_str()).unwrap_or("");

    // 只有 paged 模式真翻页：其余模式单页返回全部（今天的真实目录也只有 42 条）
    let (start, end) = if mode == Mode::Paged {
        let start = if cursor.is_empty() {
            0
        } else {
            (0..total)
                .find(|i| variable_id(*i) == cursor)
                .map(|i| i + 1)
                .unwrap_or(0)
        };
        (start, (start + limit).min(total))
    } else {
        (0, total)
    };

    let compact = mode == Mode::Big;
    let definitions: Vec<serde_json::Value> = (start..end).map(|i| entry(i, compact)).collect();
    // next_cursor = 本页最后一条 variable_id（不透明游标，按字典序稳定）
    let next_cursor = if end < total && end > start {
        serde_json::Value::from(variable_id(end - 1))
    } else {
        serde_json::Value::Null
    };

    serde_json::json!({
        "format_version": 1,
        "family": q.get("family").and_then(|v| v.as_str()).unwrap_or("828d"),
        "revision": 1,
        "driver_id": "definitions-fixture",
        "driver_version": "0.0.0",
        "note": serde_json::Value::Null,
        "total": total,
        "limit": limit,
        "next_cursor": next_cursor,
        "definitions": definitions,
    })
    .to_string()
}

/// 渲染预览响应：`echo_request_json` 是请求原文的**逐字**副本，
/// 用来证明 Core/Manager 没有对请求做任何重排或裁剪。
fn render_preview(request_json: &str) -> String {
    let echoed = serde_json::to_string(request_json).unwrap_or_else(|_| "\"\"".into());
    format!(
        concat!(
            r#"{{"point_key":"fixture.variable.0001@channel=i:1","#,
            r#""identity_mode":"derived","scope":"collect","#,
            r#""variable_id":"fixture.variable.0001","definition_revision":1,"#,
            r#""definition_digest":"{}","echo_request_json":{}}}"#
        ),
        "0".repeat(64),
        echoed
    )
}

struct FixtureDriver {
    mode: Mode,
}

#[async_trait::async_trait]
impl Driver for FixtureDriver {
    fn metadata(&self) -> mesa_core_types::DriverMetadata {
        mesa_core_types::DriverMetadata {
            driver_id: "definitions-fixture".into(),
            name: "Definitions Fixture".into(),
            version: "0.0.0".into(),
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
        }
    }

    async fn open_connection(
        &self,
        _endpoint_id: &str,
        _config_json: &str,
    ) -> Result<Box<dyn DriverConnection>, SdkDriverError> {
        // 夹具**永远**不开连接：I-2 的两条 RPC 必须在零 Connection 条目下应答
        Err(SdkDriverError::new(
            ErrorKind::Unsupported,
            "FIXTURE_NO_CONNECTION",
            "definitions fixture never opens connections",
        ))
    }

    async fn definitions(&self, request_json: &str) -> Result<String, SdkDriverError> {
        // E4-b：格式/结构违约必须由 Manager 判为 DEFINITION_RESPONSE_INVALID
        //（而不是体积门 TOO_LARGE），因此这里直接返回坏载荷。
        if self.mode == Mode::Invalid {
            return Ok("<html>definition catalog unavailable</html>".to_string());
        }
        if self.mode == Mode::InvalidShape {
            return Ok(r#"{"format_version":1,"total":0}"#.to_string());
        }
        // Revision 不支持的映射固定为 `Configuration → 400`（带 available[]）
        if self.mode == Mode::Revision {
            return Err(SdkDriverError::new(
                ErrorKind::Configuration,
                "UNSUPPORTED_DEFINITION_REVISION",
                "revision 9 unavailable for `fixture.variable.0001`; available=[1]",
            ));
        }
        // 业务拒绝（E12 的 Driver 侧路径）：kind/code 必须原样过线到 REST
        if self.mode == Mode::Fail {
            return Err(SdkDriverError::new(
                ErrorKind::Configuration,
                "INVALID_QUERY",
                "fixture rejects every query",
            ));
        }
        let total = match self.mode {
            Mode::Paged => 250,
            // 4200 > 4096 硬顶，但紧凑条目总量仍 < 256 KiB ⇒ 精确命中「条目门」而非「字节门」
            Mode::Big => 4200,
            _ => 42,
        };
        match self.mode {
            Mode::Slow => tokio::time::sleep(Duration::from_millis(1500)).await,
            Mode::Hang => {
                // 永不回包：Core 侧 5s 预算必须把它判成 E3（而不是干等 10s）
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            _ => {}
        }
        Ok(render_catalog(total, request_json, self.mode))
    }

    async fn preview_point_key(&self, request_json: &str) -> Result<String, SdkDriverError> {
        Ok(render_preview(request_json))
    }
}

fn main() {
    let mode = mode_from_argv0();
    let port = port_from_args();
    let token = mesa_driver_sdk::read_session_token_from_stdin();
    mesa_driver_sdk::spawn_parent_liveness_guard();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap_or_else(|e| panic!("bind 127.0.0.1:{port} failed: {e}"));
        if mode == Mode::Silent {
            // 只 accept、不发 Hello：Core 的 Hello 读超时（5s）必须判死。
            // 必须把 stream 绑到**具名变量**：`let _ = ...` 会立即 drop 连接，
            // Core 只会看到 early EOF（快速失败），测不到「超时」这条路径。
            let _stream = listener.accept().await.expect("accept core");
            tokio::time::sleep(Duration::from_secs(3600)).await;
            return;
        }
        let shutdown = tokio_util::sync::CancellationToken::new();
        if let Err(e) =
            mesa_driver_sdk::serve(FixtureDriver { mode }, listener, token, shutdown).await
        {
            eprintln!("definitions fixture exited with error: {e}");
            std::process::exit(1);
        }
    });
}
