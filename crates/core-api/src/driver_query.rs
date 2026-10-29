//! I-2 只读管理路由（driver 作用域）：定义目录 + 权威身份预览。
//!
//! 三条不可越过的边界（DC §3.3 / §6.2 / §7.1）：
//!
//! 1. **零厂商语义**：本文件不得出现任何 NCK 专有类型或码字面量 —— P9 护栏
//!    （`tests/driver-contract/tests/resource_contract.rs`）会逐行扫描
//!    `crates/core-api/src` 的生产代码。
//! 2. **只按 `ErrorKind` 映射 HTTP**：状态码由 [`error_kind_to_status`] **一处**
//!    决定，绝不按 `code` 特判（否则 Core 就得理解各驱动的码表）。
//! 3. **不解释载荷**：目录/预览载荷都是**不透明 JSON**，Core 只做长度/形状校验与
//!    **逐字**转发（不重排、不裁剪、不改编码）。
//!
//! 为什么这两条路由挂在 driver 作用域（`/drivers/{id}/...`）而不是 Endpoint 作用域：
//! 定义目录与身份预览都是**驱动能力**查询，与「哪台机床、哪条连接」无关 ——
//! 因此不需要 Endpoint 记录、不打开 CNC 连接、不读机床实时值（DC §6.3 的 R8–R11）。

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::AppState;

/// 预览请求体上限（DC §6.4-①-① / E30）。
///
/// 取值依据：合法请求的最大形态（原生 12 字段白名单 + 信封）量级 < 2 KiB，
/// 64 KiB 给出 ≥ 30× 余量；框架默认门是 2 MB，本路由只是**收紧**。
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;

/// 分页缺省页大小（DC §6.4-①-③）。
const DEFAULT_LIMIT: u32 = 100;
/// 分页上限：超出即**夹取**（不是错误，与 Browse 的 `min(1000)` 同口径）。
const MAX_LIMIT: u32 = 500;

/// 目录查询参数（`GET`，§4.1）。
///
/// 全部按**字符串**接收：`limit`/`revision` 需要区分「缺省」与「非法形态」
/// （`0`、`-1`、`abc` 都必须能被判成 `INVALID_QUERY`，而不是被 serde 直接吞掉）。
#[derive(Debug, Default, Deserialize)]
pub(crate) struct DefinitionsQuery {
    family: Option<String>,
    variable_id: Option<String>,
    revision: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
    include_history: Option<String>,
}

/// `GET /api/v1/drivers/{id}/definitions`（§4.1 / §4.2）。
///
/// 本函数只做**查询信封**的形状判定（正整数、互斥关系、夹取），然后把参数组装成
/// 一段不透明 JSON 交给 Manager 逐字转发。**不判断** family 是否已声明、变量是否
/// 存在、revision 是否可用 —— 那些是驱动侧的目录语义（E8/E10/E11）。
pub(crate) async fn definitions(
    State(state): State<Arc<AppState>>,
    Path(driver_id): Path<String>,
    Query(q): Query<DefinitionsQuery>,
) -> Response {
    // ① limit：缺省 100；非正整数 ⇒ 400；超出上限 ⇒ 夹取到 500（不是错误）
    let limit = match q.limit.as_deref() {
        None => DEFAULT_LIMIT,
        Some(raw) => match parse_positive_u32(raw) {
            Some(v) => v.min(MAX_LIMIT),
            None => return invalid_query("limit 必须是正整数"),
        },
    };

    // ② revision：必须与 variable_id 同时出现，且为正整数（拒绝「全目录过滤 revision」
    //    这种语义不明确的查询，DC §4.1）
    if let Some(raw) = q.revision.as_deref() {
        if q.variable_id.is_none() {
            return invalid_query("revision 必须与 variable_id 同时出现");
        }
        if parse_positive_u32(raw).is_none() {
            return invalid_query("revision 必须是正整数");
        }
    }

    // ③ include_history：只接受显式 true/false；默认 false 保持普通目录的最新版本视图。
    let include_history = match q.include_history.as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return invalid_query("include_history 必须是 true 或 false"),
    };

    // ③ cursor 与 variable_id 互斥（游标语义 = 上一页末条，不能再叠加精确过滤）
    if q.cursor.is_some() && q.variable_id.is_some() {
        return invalid_query("cursor 与 variable_id 互斥");
    }

    // 组装查询信封：键名是 §4.1 冻结的**查询参数**，不是厂商语义；
    // 缺省项不出现，避免给驱动发送「显式 null」造成歧义。
    let mut body = serde_json::Map::new();
    if let Some(family) = &q.family {
        body.insert("family".into(), serde_json::Value::from(family.clone()));
    }
    if let Some(variable_id) = &q.variable_id {
        body.insert(
            "variable_id".into(),
            serde_json::Value::from(variable_id.clone()),
        );
    }
    if let Some(revision) = &q.revision {
        // 已校验为正整数，故这里必定可解析
        body.insert(
            "revision".into(),
            serde_json::Value::from(parse_positive_u32(revision).unwrap_or_default()),
        );
    }
    body.insert("limit".into(), serde_json::Value::from(limit));
    if let Some(cursor) = &q.cursor {
        body.insert("cursor".into(), serde_json::Value::from(cursor.clone()));
    }
    if include_history {
        body.insert("include_history".into(), serde_json::Value::Bool(true));
    }
    let query_json = serde_json::Value::Object(body).to_string();

    match state.manager.definitions(&driver_id, &query_json).await {
        // 逐字回填：驱动返回的目录 JSON 原样作为响应体（不再 serde 往返，
        // 否则键序/空白会被改写，违反 §7.1 的「逐字透传」）
        Ok(json) => raw_json(StatusCode::OK, json),
        Err(e) => query_error(e),
    }
}

/// `POST /api/v1/drivers/{id}/definitions/identity-preview`（§5.1 / §5.2）。
///
/// **`POST` 表示「带参数的只读计算」，不是写入**：不建 Endpoint、不开 CNC 连接、
/// 不读机床实时值、不改采集计划、不写任何存储（DC §6.3 的 R8–R11 逐条断言）。
///
/// 形状校验只到**信封层**：`family`/`kind`/`parameters` 三字段必须存在且类型正确，
/// 未知顶层字段（含被明令禁止的 `point_key`）一律 400。`parameters` 内部的语义
/// 校验（D2–D10）全部在驱动侧，Core 连键名都不解释。
pub(crate) async fn identity_preview(
    State(state): State<Arc<AppState>>,
    Path(driver_id): Path<String>,
    body: Bytes,
) -> Response {
    let raw = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => return invalid_request("请求体必须是 UTF-8 编码的 JSON"),
    };
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return invalid_request(&format!("请求体不是合法 JSON: {e}")),
    };
    let Some(obj) = value.as_object() else {
        return invalid_request("请求体必须是 JSON 对象");
    };

    // 顶层白名单：三个字段，其余一律拒绝（含 point_key —— 身份正是要被算出来的东西）
    const ALLOWED: [&str; 3] = ["family", "kind", "parameters"];
    for key in obj.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return invalid_request(&format!("未知顶层字段 `{key}`"));
        }
    }
    for key in ALLOWED {
        if !obj.contains_key(key) {
            return invalid_request(&format!("缺少必需字段 `{key}`"));
        }
    }
    if !obj["family"].is_string() || obj["family"].as_str().is_some_and(str::is_empty) {
        return invalid_request("`family` 必须是非空字符串（本端点无缺省值）");
    }
    if !obj["kind"].is_string() || obj["kind"].as_str().is_some_and(str::is_empty) {
        return invalid_request("`kind` 必须是非空字符串");
    }
    if !obj["parameters"].is_object() {
        return invalid_request("`parameters` 必须是 JSON 对象");
    }

    // 逐字转发原始 body（**不重新序列化**）：驱动看到的就是客户端发来的那一段
    match state.manager.preview_point_key(&driver_id, raw).await {
        Ok(json) => raw_json(StatusCode::OK, json),
        Err(e) => query_error(e),
    }
}

/// `ErrorKind → HTTP`（DC §6.2）：**全仓唯一**的管理面映射点。
///
/// 为什么只按 kind：Core 一旦按 `code` 特判，就必须理解每个驱动的码表 ——
/// 那正是 P9 护栏（Core 零厂商语义）禁止的事。因此映射函数里只允许出现
/// `ErrorKind` 变体，不得出现任何码字面量。
pub(crate) fn error_kind_to_status(kind: mesa_core_types::ErrorKind) -> StatusCode {
    use mesa_core_types::ErrorKind as K;
    match kind {
        K::Configuration => StatusCode::BAD_REQUEST,
        K::Address => StatusCode::NOT_FOUND,
        K::Unsupported => StatusCode::NOT_IMPLEMENTED,
        K::Timeout => StatusCode::GATEWAY_TIMEOUT,
        K::Connection | K::Device | K::Protocol | K::Decode | K::Internal => {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// 把 `DefaultBodyLimit` 的框架级 413 改写成统一错误信封（E30）。
///
/// 为什么仍用框架门：DC §11-16 明确拒绝「handler 内手动读 body」——那会放弃
/// 框架级门、更容易漏。这里只改写**响应体**，拒绝动作依旧发生在进入 handler 之前。
pub(crate) async fn rewrite_payload_too_large(resp: Response) -> Response {
    if resp.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return resp;
    }
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(serde_json::json!({
            "error": {
                "code": "REQUEST_TOO_LARGE",
                "kind": mesa_core_types::ErrorKind::Configuration.as_str(),
                "message": format!("请求体超过 {MAX_REQUEST_BODY_BYTES} 字节上限"),
            }
        })),
    )
        .into_response()
}

/// 管理面错误 → HTTP：状态码来自 kind，`code`/`message` 原样透出。
///
/// 响应体同时带 `kind`，让前端与运维能一眼区分「协议不支持（501）/ 输入问题（400）/
/// 基础设施（503）」——这正是 DC §6.2.1 要求的「每条错误都有确定 ErrorKind」。
fn query_error(e: mesa_driver_manager::manager::DefinitionQueryError) -> Response {
    let status = error_kind_to_status(e.kind);
    (
        status,
        Json(serde_json::json!({
            "error": {
                "code": e.code,
                "message": e.message,
                "kind": e.kind.as_str(),
            }
        })),
    )
        .into_response()
}

/// 400 `INVALID_QUERY`（查询信封形态非法）。
fn invalid_query(message: &str) -> Response {
    shape_error("INVALID_QUERY", message)
}

/// 400 `INVALID_REQUEST`（预览请求信封形态非法，E13）。
fn invalid_request(message: &str) -> Response {
    shape_error("INVALID_REQUEST", message)
}

fn shape_error(code: &str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": {
                "code": code,
                "kind": mesa_core_types::ErrorKind::Configuration.as_str(),
                "message": message,
            }
        })),
    )
        .into_response()
}

/// 成功响应：把驱动返回的 JSON **原样**作为响应体（零 serde 往返）。
fn raw_json(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// 严格正整数解析：`"0"` / `"-1"` / `"abc"` 一律拒绝（`u32` 的 `FromStr` 本身
/// 接受前导 `+`，故 `"+1"` 会被解析为 1 —— 这属可接受的宽松，不影响任何断言）。
fn parse_positive_u32(raw: &str) -> Option<u32> {
    let v: u32 = raw.parse().ok()?;
    (v >= 1).then_some(v)
}
#[cfg(test)]
mod tests {
    use super::*;
    use mesa_core_types::ErrorKind as K;

    /// **映射冻结**（阶段 2 定向补丁）：`ErrorKind → HTTP` 是 Core 唯一的状态码来源，
    /// 只允许出现 kind 变体。这里把两个容易混淆的边界钉死：
    /// `Internal → 503`（I-2 的 Manager 预算/契约类错误）与 `Timeout → 504`
    ///（真正的 RPC 超时）。若有人把 `DEFINITION_QUERY_TIMEOUT` 的 kind 改成
    /// `Timeout`，端到端状态会从 503 变成 504 —— 本断言 + `manager.rs` 的
    /// `definition_query_error_contracts_are_pinned` 会同时报警。
    #[test]
    fn error_kind_to_status_is_pinned() {
        assert_eq!(
            error_kind_to_status(K::Internal),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            error_kind_to_status(K::Timeout),
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(
            error_kind_to_status(K::Configuration),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(error_kind_to_status(K::Address), StatusCode::NOT_FOUND);
        assert_eq!(
            error_kind_to_status(K::Unsupported),
            StatusCode::NOT_IMPLEMENTED
        );
        for kind in [K::Connection, K::Device, K::Protocol, K::Decode] {
            assert_eq!(error_kind_to_status(kind), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    /// 查询信封形态解析：正整数判定不接受 0 / 负数 / 非数字。
    #[test]
    fn positive_u32_parsing_is_strict() {
        assert_eq!(parse_positive_u32("1"), Some(1));
        assert_eq!(parse_positive_u32("500"), Some(500));
        assert_eq!(parse_positive_u32("0"), None);
        assert_eq!(parse_positive_u32("-1"), None);
        assert_eq!(parse_positive_u32("abc"), None);
    }
}
