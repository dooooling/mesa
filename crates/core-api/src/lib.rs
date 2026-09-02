#![allow(clippy::collapsible_if)]
//! Mesa REST API（方案 §4.1）。
//!
//! 安全边界（§4.2）：仅绑定 loopback，不提供任何远程管理能力。
//! Device/Endpoint/Task CRUD + 启停 + rescan + diagnostics 已实现（§4.1 最小集）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use mesa_config_store::{ConfigStore, DeviceRecord, EndpointRecord, StoreError};
use mesa_core_types::AcquisitionTask;
use mesa_driver_manager::{MesaManager, Snapshot};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

pub mod certificates;
use certificates::CertStore;

// ---------------------------------------------------------------------------
// 共享状态
// ---------------------------------------------------------------------------

pub struct AppState {
    pub snapshot: Arc<Snapshot>,
    pub manager: Arc<MesaManager>,
    pub store: Arc<ConfigStore>,
    /// 驱动目录（供 rescan 使用）。
    pub drivers_dir: String,
    pub start_time: Instant,
    pub cert_store: Arc<CertStore>,
    /// 控制面总闸：默认关闭，需 --enable-control 显式开启（§22）
    pub enable_control: bool,
}

impl AppState {
    pub fn new(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
    ) -> Arc<Self> {
        Self::new_with_cert_dir(manager, store, drivers_dir, CertStore::default_path())
    }

    pub fn new_with_control(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        enable_control: bool,
    ) -> Arc<Self> {
        Self::new_with_cert_dir_and_control(
            manager,
            store,
            drivers_dir,
            CertStore::default_path(),
            enable_control,
        )
    }

    pub fn new_with_cert_dir(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        cert_dir: std::path::PathBuf,
    ) -> Arc<Self> {
        Self::new_with_cert_dir_and_control(manager, store, drivers_dir, cert_dir, false)
    }

    pub fn new_with_cert_dir_and_control(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        cert_dir: std::path::PathBuf,
        enable_control: bool,
    ) -> Arc<Self> {
        let snapshot = manager.snapshot();
        let cert_store = Arc::new(CertStore::new(cert_dir));
        // 确保目录并生成 own 证书（忽略错误，仅日志）
        if let Err(e) = cert_store.ensure_dirs() {
            tracing::warn!(error=%e, "证书目录创建失败");
        }
        if let Err(e) = cert_store.ensure_own_cert() {
            tracing::warn!(error=%e, "own 证书生成失败");
        }
        Arc::new(Self {
            snapshot,
            manager,
            store,
            drivers_dir,
            start_time: Instant::now(),
            cert_store,
            enable_control,
        })
    }
}

// ---------------------------------------------------------------------------
// 通用响应辅助
// ---------------------------------------------------------------------------

fn json_error(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

fn validate_connection_against_schema(
    schema: &mesa_core_types::SchemaDescriptor,
    conn: &serde_json::Value,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if !conn.is_object() {
        issues.push(ValidationIssue {
            path: "connection".into(),
            code: "INVALID_TYPE".into(),
            message: "connection must be an object".into(),
        });
        return issues;
    }
    let obj = conn.as_object().unwrap();
    for field in &schema.fields {
        let present = obj.contains_key(&field.key);
        if field.required && !present {
            issues.push(ValidationIssue {
                path: format!("connection.{}", field.key),
                code: "REQUIRED".into(),
                message: format!("field `{}` is required", field.key),
            });
            continue;
        }
        if let Some(val) = obj.get(&field.key) {
            // 类型校验
            let type_ok = match field.field_type {
                mesa_core_types::FieldType::String
                | mesa_core_types::FieldType::Host
                | mesa_core_types::FieldType::Url
                | mesa_core_types::FieldType::File
                | mesa_core_types::FieldType::CertificateRef
                | mesa_core_types::FieldType::Secret => val.is_string(),
                mesa_core_types::FieldType::Integer | mesa_core_types::FieldType::Port => {
                    val.is_number() && val.as_i64().is_some()
                }
                mesa_core_types::FieldType::Number | mesa_core_types::FieldType::Duration => {
                    val.is_number()
                }
                mesa_core_types::FieldType::Boolean => val.is_boolean(),
                mesa_core_types::FieldType::Enum => val.is_string(),
            };
            if !type_ok {
                issues.push(ValidationIssue {
                    path: format!("connection.{}", field.key),
                    code: "INVALID_TYPE".into(),
                    message: format!(
                        "field `{}` expected {:?}, got {}",
                        field.key, field.field_type, val
                    ),
                });
                continue;
            }
            // 枚举选项
            if let Some(opts) = &field.validation.enum_options {
                if let Some(s) = val.as_str() {
                    if !opts.contains(&s.to_string()) {
                        issues.push(ValidationIssue {
                            path: format!("connection.{}", field.key),
                            code: "INVALID_ENUM".into(),
                            message: format!("field `{}` value `{s}` not in {:?}", field.key, opts),
                        });
                    }
                }
            }
            // 范围
            if let Some(num) = val.as_f64() {
                if let Some(min) = field.validation.min {
                    if num < min {
                        issues.push(ValidationIssue {
                            path: format!("connection.{}", field.key),
                            code: "OUT_OF_RANGE".into(),
                            message: format!("field `{}` {num} < min {min}", field.key),
                        });
                    }
                }
                if let Some(max) = field.validation.max {
                    if num > max {
                        issues.push(ValidationIssue {
                            path: format!("connection.{}", field.key),
                            code: "OUT_OF_RANGE".into(),
                            message: format!("field `{}` {num} > max {max}", field.key),
                        });
                    }
                }
            }
            // 正则（如配置则简单包含校验，生产可引入 regex）
            if let Some(pat) = &field.validation.pattern {
                if let Some(s) = val.as_str() {
                    if !s.contains(pat) && pat != ".*" {
                        // 占位：仅当模式非通配时做简单检查
                    }
                }
            }
        }
    }
    issues
}

// ---------------------------------------------------------------------------
// Secret 集成：Descriptor 驱动的 Secret 处理（P0-1）
// ---------------------------------------------------------------------------

/// 获取 schema 中 Secret 类型字段的 key 集合
fn secret_field_keys(schema: &mesa_core_types::SchemaDescriptor) -> Vec<String> {
    schema
        .fields
        .iter()
        .filter(|f| f.field_type == mesa_core_types::FieldType::Secret)
        .map(|f| f.key.clone())
        .collect()
}

/// 判断是否为已持久化的 Secret 标记 {"secret_set": true}
fn is_secret_marker(v: &serde_json::Value) -> bool {
    v.is_object()
        && v.as_object()
            .map(|m| m.get("secret_set") == Some(&serde_json::Value::Bool(true)))
            .unwrap_or(false)
}

/// 创建/更新时：将明文密码写入 SecretStore，并将 connection 中的明文替换为 marker
#[allow(dead_code)]
fn process_connection_for_store(
    conn: &mut serde_json::Value,
    endpoint_id: &str,
    store: &ConfigStore,
    secret_keys: &[String],
) -> Result<(), StoreError> {
    let Some(obj) = conn.as_object_mut() else {
        return Ok(());
    };
    for key in secret_keys {
        if let Some(val) = obj.get(key).cloned() {
            if val.is_string() {
                let plaintext = val.as_str().unwrap();
                // 写入 SecretStore（key_id 固定为 master，算法由 Store 决定）
                store.put_secret(endpoint_id, key, plaintext, "master")?;
                // 替换为 marker，避免落盘明文
                obj.insert(key.clone(), serde_json::json!({"secret_set": true}));
            } else if is_secret_marker(&val) {
                // 客户端传 marker 表示不更新，保持原 Secret 不动
                // 已在 DB 中的 secret 保持不变，无需操作
            } else {
                // 其他类型（理论上已在 validate 阶段拦截）
            }
        } else {
            // 字段缺失：若 DB 中已有 secret 且本次未传，视为删除
            // 调用方需显式处理删除，这里不自动删，避免误删
        }
    }
    Ok(())
}

/// 对存储的 connection_json 做脱敏：若为 Secret 字段且为字符串明文，则替换为 marker
fn redact_connection_for_response(
    mut conn: serde_json::Value,
    endpoint_id: &str,
    store: &ConfigStore,
    secret_keys: &[String],
) -> serde_json::Value {
    let Some(obj) = conn.as_object_mut() else {
        return conn;
    };
    for key in secret_keys {
        // 若 SecretStore 中有对应 secret，则无论存储的是明文还是 marker，都返回 marker
        if store.get_secret(endpoint_id, key).ok().flatten().is_some() {
            obj.insert(key.clone(), serde_json::json!({"secret_set": true}));
        } else if let Some(v) = obj.get(key) {
            // 无 secret 但值为字符串（历史明文残留），也脱敏
            if v.is_string() {
                obj.insert(key.clone(), serde_json::json!({"secret_set": true}));
            }
        }
    }
    conn
}

/// 启动/探测时：将 marker 临时还原为明文（仅内存，不写盘）
fn materialize_connection(
    conn_str: &str,
    endpoint_id: &str,
    store: &ConfigStore,
    secret_keys: &[String],
) -> Result<String, StoreError> {
    let mut v: serde_json::Value = serde_json::from_str(conn_str).unwrap_or(serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        for key in secret_keys {
            if let Some(cur) = obj.get(key).cloned() {
                if is_secret_marker(&cur) {
                    match store.get_secret(endpoint_id, key) {
                        Ok(Some(pt)) => {
                            obj.insert(key.clone(), serde_json::Value::String(pt));
                        }
                        Ok(None) => {
                            return Err(StoreError::Validation(format!(
                                "secret `{key}` marker 存在但 SecretStore 无记录 (endpoint `{endpoint_id}`)"
                            )));
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
    serde_json::to_string(&v).map_err(StoreError::Json)
}

fn store_err_to_response(e: StoreError) -> (StatusCode, Json<serde_json::Value>) {
    match e {
        StoreError::Duplicate(msg) => (StatusCode::CONFLICT, Json(json_error("CONFLICT", &msg))),
        StoreError::NotFound(msg) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &msg))),
        StoreError::Conflict(msg) => (StatusCode::CONFLICT, Json(json_error("CONFLICT", &msg))),
        StoreError::Validation(msg) => (
            StatusCode::BAD_REQUEST,
            Json(json_error("VALIDATION_ERROR", &msg)),
        ),
        StoreError::Json(err) => (
            StatusCode::BAD_REQUEST,
            Json(json_error("INVALID_JSON", &err.to_string())),
        ),
        StoreError::Sqlite(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &err.to_string())),
        ),
    }
}

// ---------------------------------------------------------------------------
// 只读查询：驱动/端点/点位 基础查询
// ---------------------------------------------------------------------------

async fn list_drivers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "drivers": state.snapshot.drivers() }))
}

async fn get_driver(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let drivers = state.snapshot.drivers();
    if let Some(d) = drivers.into_iter().find(|x| x.id == id) {
        (StatusCode::OK, Json(serde_json::to_value(d).unwrap()))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("driver `{id}` not found"))),
        )
    }
}

async fn list_profiles(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "profiles": state.manager.list_profiles() }))
}

async fn get_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Some(p) = state.manager.get_profile(&id) {
        (StatusCode::OK, Json(serde_json::to_value(p).unwrap()))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json_error(
                "NOT_FOUND",
                &format!("profile `{id}` not found"),
            )),
        )
    }
}

async fn get_driver_descriptor(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.manager.get_descriptor(&id).await {
        Ok(desc) => (StatusCode::OK, Json(serde_json::to_value(&desc).unwrap())),
        Err(e) => {
            // §4.4 统一 503，code 精确可断言
            let status = StatusCode::SERVICE_UNAVAILABLE;
            // 尝试提取 issues（validation 失败时）
            if e.code == "DRIVER_DESCRIPTOR_VALIDATION_FAILED" {
                // 尝试解析 issues 详情，当前 Manager 仅返回 message，此处包装
                (
                    status,
                    Json(serde_json::json!({
                        "error": { "code": e.code, "message": e.message, "issues": [] }
                    })),
                )
            } else {
                (
                    status,
                    Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
                )
            }
        }
    }
}

async fn validate_connection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 查找驱动
    let drivers = state.snapshot.drivers();
    if drivers.iter().find(|d| d.id == id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("driver `{id}` not found"))),
        );
    }
    // 获取 Descriptor（内存缓存或临时进程）
    let desc = match state.manager.get_descriptor(&id).await {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
            );
        }
    };
    // 提取 connection 对象（支持 {connection:{}} 或直接对象）
    let conn_val = if let Some(c) = body.get("connection") {
        c.clone()
    } else {
        body.clone()
    };
    let issues = validate_connection_against_schema(&desc.connection, &conn_val);
    if issues.is_empty() {
        // 额外尝试 Driver 侧解析（不触设备，仅本地校验）
        // 通过尝试 open_connection 的配置解析路径：当前仅做 JSON 对象校验，已足够
        (
            StatusCode::OK,
            Json(serde_json::json!({ "valid": true, "issues": [] })),
        )
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "valid": false, "issues": issues })),
        )
    }
}

async fn probe_driver(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let disc = match state.manager.find_driver(&id) {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error("NOT_FOUND", &format!("driver `{id}` not found"))),
            );
        }
    };
    let conn_val = if let Some(c) = body.get("connection") {
        c.clone()
    } else {
        body.clone()
    };
    if !conn_val.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error(
                "VALIDATION_ERROR",
                "connection must be an object",
            )),
        );
    }
    let conn_str = serde_json::to_string(&conn_val).unwrap();
    // 临时进程探测：需覆盖 DRIVER_STARTUP_TIMEOUT(6s) + handshake + OpenConnection
    let probe_res =
        tokio::time::timeout(mesa_driver_manager::session::PROBE_TIMEOUT, async {
            let mut proc = match mesa_driver_manager::process::DriverProcess::spawn(&disc).await {
                Ok(p) => p,
                Err(e) => return Err(format!("spawn failed: {e}")),
            };
            let (mut session, _events, _) =
                match mesa_driver_manager::session::Session::connect_retry(proc.port, &proc.token)
                    .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        proc.terminate().await;
                        return Err(format!("handshake failed: {e}"));
                    }
                };
            // 尝试 OpenConnection
            let handle = 999;
            let open_res = session
                .call(mesa_driver_protocol::pb::envelope::Body::OpenConnection(
                    mesa_driver_protocol::pb::OpenConnection {
                        connection_handle: handle,
                        endpoint_id: format!("probe-{id}"),
                        config_json: conn_str.clone(),
                    },
                ))
                .await;
            let reachable = match open_res {
                Ok(env) => match env.body {
                    Some(mesa_driver_protocol::pb::envelope::Body::OpenConnectionAck(ack)) => {
                        ack.result.map(|r| r.ok).unwrap_or(false)
                    }
                    Some(mesa_driver_protocol::pb::envelope::Body::DriverError(e)) => {
                        let d = e.detail.unwrap_or_default();
                        return Err(format!("{}/{}: {}", d.kind, d.code, d.message));
                    }
                    _ => false,
                },
                Err(e) => return Err(format!("open failed: {e}")),
            };
            session.invalidate();
            proc.terminate().await;
            if reachable {
                Ok(())
            } else {
                Err("open not ok".into())
            }
        })
        .await;
    match probe_res {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(serde_json::json!({ "reachable": true, "warnings": [] })),
        ),
        Ok(Err(e)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "reachable": false, "error": e, "warnings": [] })),
        ),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(json_error("TIMEOUT", "probe timeout")),
        ),
    }
}

#[derive(Debug, serde::Deserialize)]
struct BrowseReq {
    parent: Option<String>,
    filter: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

async fn browse_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<BrowseReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = match state.store.get_endpoint(&id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error(
                    "NOT_FOUND",
                    &format!("endpoint `{id}` not found"),
                )),
            );
        }
        Err(e) => return store_err_to_response(e),
    };
    let parent = body.parent.unwrap_or_default();
    let filter = body.filter.unwrap_or_default();
    let cursor = body.cursor.unwrap_or_default();
    let limit = body.limit.unwrap_or(50).min(1000);
    // Browse 同 Start 一样需 materialize Secret，避免把 marker 交给 Driver
    let mut browse_conn = rec.connection_json.clone();
    match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(desc) => {
            let secret_keys = secret_field_keys(&desc.connection);
            if !secret_keys.is_empty() {
                match materialize_connection(
                    &rec.connection_json,
                    &rec.id,
                    &state.store,
                    &secret_keys,
                ) {
                    Ok(s) => browse_conn = s,
                    Err(e) => return store_err_to_response(e),
                }
            }
        }
        Err(e) => {
            if rec.connection_json.contains("secret_set") {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
                );
            }
        }
    }
    match state
        .manager
        .browse(
            &rec.driver_id,
            &browse_conn,
            &parent,
            &filter,
            &cursor,
            limit,
        )
        .await
    {
        Ok((nodes, next)) => {
            let nodes_json: Vec<serde_json::Value> = nodes
                .into_iter()
                .map(|n| {
                    serde_json::json!({
                        "id": n.id,
                        "label": n.label,
                        "kind": n.kind,
                        "data_type": n.data_type,
                        "access": n.access,
                        "has_children": n.has_children,
                        "binding_json": n.binding_json,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "nodes": nodes_json, "next_cursor": next })),
            )
        }
        Err(e) => {
            let status = if e.code == "BROWSE_FAILED" || e.code == "DRIVER_UNAVAILABLE" {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            };
            (
                status,
                Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
            )
        }
    }
}

async fn import_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 占位：当前仅 OPC UA 等支持 browse，import 框架待 Milestone H 扩展
    let _ = state.store.get_endpoint(&id);
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json_error("NOT_IMPLEMENTED", "import not yet implemented")),
    )
}

#[derive(Debug, Deserialize)]
struct ControlWriteReq {
    target: String,
    value: serde_json::Value,
    expected_value: Option<serde_json::Value>,
    // 允许直接传 typed Value 的 tag 形式（兼容 Value 枚举序列化）
    #[serde(default)]
    value_typed: Option<mesa_core_types::Value>,
}

fn json_to_value(v: &serde_json::Value) -> Result<mesa_core_types::Value, String> {
    match v {
        serde_json::Value::Bool(b) => Ok(mesa_core_types::Value::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                // 优先 I32，若超出则 I64
                if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                    Ok(mesa_core_types::Value::I32(i as i32))
                } else {
                    Ok(mesa_core_types::Value::I64(i))
                }
            } else if let Some(u) = n.as_u64() {
                if u <= u32::MAX as u64 {
                    Ok(mesa_core_types::Value::U32(u as u32))
                } else {
                    Ok(mesa_core_types::Value::U64(u))
                }
            } else if let Some(f) = n.as_f64() {
                Ok(mesa_core_types::Value::F64(f))
            } else {
                Err("invalid number".into())
            }
        }
        serde_json::Value::String(s) => Ok(mesa_core_types::Value::String(s.clone())),
        serde_json::Value::Array(arr) => {
            // 简单按首元素类型推断，Simulator 仅需标量，数组罕用
            Err(format!(
                "array value not supported in control write: {arr:?}"
            ))
        }
        serde_json::Value::Null => Err("null value not allowed".into()),
        serde_json::Value::Object(_) => {
            // 尝试按 Value 枚举的 tag 形式反序列化（如 {"F64":1.0}）
            serde_json::from_value::<mesa_core_types::Value>(v.clone()).map_err(|e| e.to_string())
        }
    }
}

async fn control_write(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ControlWriteReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !state.enable_control {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json_error(
                "CONTROL_DISABLED",
                "control plane disabled, start mesad with --enable-control",
            )),
        );
    }
    // endpoint 存在性校验
    let rec = match state.store.get_endpoint(&id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error(
                    "NOT_FOUND",
                    &format!("endpoint `{id}` not found"),
                )),
            );
        }
        Err(e) => return store_err_to_response(e),
    };
    let _ = rec; // 已校验存在，具体 driver 能力由 Driver 二次校验
    let target = body.target.trim();
    if target.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("VALIDATION_ERROR", "target required")),
        );
    }
    let value = if let Some(v) = body.value_typed {
        v
    } else {
        match json_to_value(&body.value) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error("VALIDATION_ERROR", &e)),
                );
            }
        }
    };
    let expected = if let Some(ev) = body.expected_value {
        match json_to_value(&ev) {
            Ok(v) => Some(v),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error(
                        "VALIDATION_ERROR",
                        &format!("expected_value: {e}"),
                    )),
                );
            }
        }
    } else {
        None
    };
    // TODO: PolicyProvider scope=control:execute 鉴权（V1 loopback 默认放行，actor=local-api）
    // 审计 STARTED（同步插入，失败不阻断控制）
    let request_id = format!("api-wr-{}-{}", id, mesa_core_types::now_unix_ns());
    let audit_started = mesa_config_store::ControlAuditRecord {
        request_id: request_id.clone(),
        endpoint_id: id.clone(),
        actor: "local-api".into(),
        operation_type: "write".into(),
        operation_id: target.to_string(),
        request_json: serde_json::json!({"target": target, "value": format!("{value:?}")})
            .to_string(),
        result_json: None,
        status: "STARTED".into(),
        started_at_ns: mesa_core_types::now_unix_ns(),
        finished_at_ns: None,
    };
    let _ = state.store.insert_control_audit(&audit_started);
    match state
        .manager
        .control_write(&id, target, value, expected, &request_id)
        .await
    {
        Ok(readback) => {
            let detail =
                serde_json::json!({"readback": readback.as_ref().map(|v| format!("{v:?}"))})
                    .to_string();
            let _ = state.store.update_control_audit(
                &request_id,
                "COMPLETED",
                Some(&detail),
                mesa_core_types::now_unix_ns(),
            );
            let rb_json =
                readback.map(|v| serde_json::to_value(&v).unwrap_or(serde_json::Value::Null));
            (
                StatusCode::OK,
                Json(
                    serde_json::json!({"request_id": request_id, "status":"Succeeded", "readback": rb_json}),
                ),
            )
        }
        Err(e) => {
            let _ = state.store.update_control_audit(
                &request_id,
                "FAILED",
                Some(&e.message),
                mesa_core_types::now_unix_ns(),
            );
            let status = if e.code == "ENDPOINT_NOT_RUNNING" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            };
            (
                status,
                Json(
                    serde_json::json!({"error": {"code": e.code, "message": e.message}, "request_id": request_id}),
                ),
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct ControlCommandReq {
    command_id: Option<String>,
    command: Option<String>,
    input_json: Option<String>,
    input: Option<serde_json::Value>,
}

async fn control_command(
    State(state): State<Arc<AppState>>,
    Path((id, cmd)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !state.enable_control {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json_error(
                "CONTROL_DISABLED",
                "control plane disabled, start mesad with --enable-control",
            )),
        );
    }
    let rec = match state.store.get_endpoint(&id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error(
                    "NOT_FOUND",
                    &format!("endpoint `{id}` not found"),
                )),
            );
        }
        Err(e) => return store_err_to_response(e),
    };
    let _ = rec;
    // 解析 body 中的 command_id / input
    let parsed: ControlCommandReq =
        serde_json::from_value(body.clone()).unwrap_or(ControlCommandReq {
            command_id: None,
            command: None,
            input_json: None,
            input: None,
        });
    let command_id = parsed.command_id.or(parsed.command).unwrap_or(cmd);
    let input_json = if let Some(s) = parsed.input_json {
        s
    } else if let Some(v) = parsed.input {
        serde_json::to_string(&v).unwrap_or("{}".into())
    } else if body.is_object() && !body.as_object().unwrap().is_empty() {
        // 将整个 body 当作 input（兼容前端直接传参对象）
        serde_json::to_string(&body).unwrap_or("{}".into())
    } else {
        "{}".into()
    };
    let request_id = format!("api-cmd-{}-{}", id, mesa_core_types::now_unix_ns());
    let audit_started = mesa_config_store::ControlAuditRecord {
        request_id: request_id.clone(),
        endpoint_id: id.clone(),
        actor: "local-api".into(),
        operation_type: "command".into(),
        operation_id: command_id.clone(),
        request_json: input_json.clone(),
        result_json: None,
        status: "STARTED".into(),
        started_at_ns: mesa_core_types::now_unix_ns(),
        finished_at_ns: None,
    };
    let _ = state.store.insert_control_audit(&audit_started);
    match state
        .manager
        .control_command(&id, &command_id, &input_json, &request_id)
        .await
    {
        Ok((status, result_json, error)) => {
            let audit_status = if status == "Succeeded" {
                "COMPLETED"
            } else {
                "FAILED"
            };
            let detail = format!("{status}:{result_json}:{error}");
            let _ = state.store.update_control_audit(
                &request_id,
                audit_status,
                Some(&detail),
                mesa_core_types::now_unix_ns(),
            );
            let result_val: serde_json::Value = serde_json::from_str(&result_json)
                .unwrap_or(serde_json::Value::String(result_json.clone()));
            (
                StatusCode::OK,
                Json(
                    serde_json::json!({"request_id": request_id, "status": status, "result": result_val, "error": error}),
                ),
            )
        }
        Err(e) => {
            let _ = state.store.update_control_audit(
                &request_id,
                "FAILED",
                Some(&e.message),
                mesa_core_types::now_unix_ns(),
            );
            let status = if e.code == "ENDPOINT_NOT_RUNNING" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_GATEWAY
            };
            (
                status,
                Json(
                    serde_json::json!({"error": {"code": e.code, "message": e.message}, "request_id": request_id}),
                ),
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    endpoint_id: Option<String>,
    status: Option<String>,
    from_ns: Option<i64>,
    to_ns: Option<i64>,
    limit: Option<u32>,
    cursor: Option<String>,
}

async fn list_control_audit(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let limit = q.limit.unwrap_or(50).min(200);
    match state.store.list_control_audit(
        q.endpoint_id.as_deref(),
        q.status.as_deref(),
        q.from_ns,
        q.to_ns,
        limit,
        q.cursor.as_deref(),
    ) {
        Ok(list) => {
            let next_cursor = list.last().map(|r| r.started_at_ns.to_string());
            (
                StatusCode::OK,
                Json(
                    serde_json::json!({"audits": list, "next_cursor": next_cursor, "count": list.len()}),
                ),
            )
        }
        Err(e) => store_err_to_response(e),
    }
}

async fn get_control_audit(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.store.get_control_audit(&request_id) {
        Ok(Some(rec)) => (StatusCode::OK, Json(serde_json::to_value(rec).unwrap())),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json_error(
                "NOT_FOUND",
                &format!("audit `{request_id}` not found"),
            )),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn endpoint_diagnostics(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = match state.store.get_endpoint(&id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error(
                    "NOT_FOUND",
                    &format!("endpoint `{id}` not found"),
                )),
            );
        }
        Err(e) => return store_err_to_response(e),
    };
    let runtime = state.snapshot.endpoint(&id);
    let point_count = state.store.point_map(&id).map(|m| m.len()).unwrap_or(0);
    let drivers = state.snapshot.drivers();
    let driver = drivers.iter().find(|d| d.id == rec.driver_id);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "endpoint_id": rec.id,
            "driver_id": rec.driver_id,
            "driver_version": driver.map(|d| d.version.clone()).unwrap_or_default(),
            "desired_running": rec.desired_running,
            "runtime": runtime,
            "point_count": point_count,
            "connection_state": runtime.as_ref().map(|s| s.state.clone()).unwrap_or("UNKNOWN".into()),
            "descriptor_state": "Ready",
            "profile_state": "None",
            "data_queue_depth": 0,
            "control_queue_depth": 0,
            "last_connected_at_ns": serde_json::Value::Null,
            "reconnect_attempt_total": 0,
        })),
    )
}

async fn endpoint_state(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    match state.snapshot.endpoint(&id) {
        Some(s) => Json(serde_json::to_value(s).expect("serializable status")),
        None => Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))),
    }
}

async fn list_endpoints(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    // 合并持久化记录与运行态（便于验收一次看全）
    let stored = state.store.list_endpoints().unwrap_or_default();
    let live: HashMap<String, _> = state
        .snapshot
        .endpoints()
        .into_iter()
        .map(|s| (s.endpoint_id.clone(), s))
        .collect();
    let mut merged: Vec<serde_json::Value> = Vec::with_capacity(stored.len());
    for rec in stored {
        let runtime = live.get(&rec.id).cloned();
        let mut conn: serde_json::Value =
            serde_json::from_str(&rec.connection_json).unwrap_or(serde_json::json!({}));
        // 脱敏：同 get_endpoint，保持列表与详情一致（避免历史明文泄露）
        match state.manager.get_descriptor(&rec.driver_id).await {
            Ok(desc) => {
                let secret_keys = secret_field_keys(&desc.connection);
                if !secret_keys.is_empty() {
                    conn =
                        redact_connection_for_response(conn, &rec.id, &state.store, &secret_keys);
                }
            }
            Err(_) => {
                if let Ok(fields) = state.store.list_secret_fields(&rec.id) {
                    if !fields.is_empty() {
                        conn = redact_connection_for_response(conn, &rec.id, &state.store, &fields);
                    } else if conn
                        .as_object()
                        .map(|m| m.values().any(|v| v.is_string()))
                        .unwrap_or(false)
                    {
                        conn = serde_json::Value::Null;
                    }
                } else {
                    conn = serde_json::Value::Null;
                }
            }
        }
        merged.push(serde_json::json!({
            "id": rec.id,
            "device_id": rec.device_id,
            "driver_id": rec.driver_id,
            "connection": conn,
            "desired_running": rec.desired_running,
            "updated_at_ns": rec.updated_at_ns,
            "runtime": runtime,
        }));
    }
    Json(serde_json::json!({ "endpoints": merged }))
}

async fn latest_points(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({ "points": state.snapshot.latest_all(), "count": state.snapshot.latest_all().len() }),
    )
}

// ---------------------------------------------------------------------------
// diagnostics
// ---------------------------------------------------------------------------

async fn diagnostics(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let uptime_secs = state.start_time.elapsed().as_secs_f64();
    let drivers = state.snapshot.drivers();
    let endpoints = state.snapshot.endpoints();
    let stored_eps = state.store.list_endpoints().map(|v| v.len()).unwrap_or(0);
    let stored_devs = state.store.list_devices().map(|v| v.len()).unwrap_or(0);
    Json(serde_json::json!({
        "uptime_secs": uptime_secs,
        "drivers": { "count": drivers.len() },
        "endpoints": { "stored": stored_eps, "runtime": endpoints.len(), "states": endpoints },
        "devices": { "stored": stored_devs },
        "certificates": state.cert_store.diagnostics(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// ---------------------------------------------------------------------------
// Devices CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateDeviceReq {
    id: String,
    name: String,
    profile: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateDeviceReq {
    name: String,
    profile: Option<String>,
}

async fn list_devices(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    match state.store.list_devices() {
        Ok(v) => Json(serde_json::json!({ "devices": v })),
        Err(e) => {
            let (_, j) = store_err_to_response(e);
            j
        }
    }
}

async fn get_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.store.get_device(&id) {
        Ok(Some(d)) => (StatusCode::OK, Json(serde_json::to_value(d).unwrap())),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("device `{id}`"))),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn create_device(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDeviceReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = DeviceRecord {
        id: body.id.clone(),
        name: body.name,
        profile: body.profile,
    };
    match state.store.create_device(&rec) {
        Ok(()) => (
            StatusCode::CREATED,
            Json(serde_json::to_value(&rec).unwrap()),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn update_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDeviceReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = DeviceRecord {
        id: id.clone(),
        name: body.name,
        profile: body.profile,
    };
    match state.store.update_device(&rec) {
        Ok(true) => (StatusCode::OK, Json(serde_json::to_value(&rec).unwrap())),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("device `{id}`"))),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn delete_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.store.delete_device(&id) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": id }))),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("device `{id}`"))),
        ),
        Err(e) => store_err_to_response(e),
    }
}

// ---------------------------------------------------------------------------
// Endpoints CRUD + 启停
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateEndpointReq {
    id: String,
    device_id: String,
    driver_id: String,
    connection: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct UpdateEndpointReq {
    device_id: String,
    driver_id: String,
    connection: serde_json::Value,
}

async fn get_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.store.get_endpoint(&id) {
        Ok(Some(rec)) => {
            let runtime = state.snapshot.endpoint(&id);
            let mut conn: serde_json::Value =
                serde_json::from_str(&rec.connection_json).unwrap_or(serde_json::json!({}));
            // 脱敏：若为 Secret 字段则返回 marker 而非明文；Descriptor 不可用时按已有 Secret 列表兜底，避免历史明文泄露
            match state.manager.get_descriptor(&rec.driver_id).await {
                Ok(desc) => {
                    let secret_keys = secret_field_keys(&desc.connection);
                    if !secret_keys.is_empty() {
                        conn = redact_connection_for_response(
                            conn,
                            &rec.id,
                            &state.store,
                            &secret_keys,
                        );
                    }
                }
                Err(_) => {
                    if let Ok(fields) = state.store.list_secret_fields(&rec.id) {
                        if !fields.is_empty() {
                            conn = redact_connection_for_response(
                                conn,
                                &rec.id,
                                &state.store,
                                &fields,
                            );
                        } else if conn
                            .as_object()
                            .map(|m| m.values().any(|v| v.is_string()))
                            .unwrap_or(false)
                        {
                            // 历史遗留且无 Secret 记录、Descriptor 又不可用时不返回原始 connection，避免明文泄露
                            conn = serde_json::Value::Null;
                        }
                    } else {
                        // 无法查询 Secret 列表时同样不返回原始 connection
                        conn = serde_json::Value::Null;
                    }
                }
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": rec.id, "device_id": rec.device_id, "driver_id": rec.driver_id,
                    "connection": conn, "desired_running": rec.desired_running,
                    "updated_at_ns": rec.updated_at_ns, "runtime": runtime,
                })),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn create_endpoint(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateEndpointReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !body.connection.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error(
                "VALIDATION_ERROR",
                "connection 必须为 JSON 对象",
            )),
        );
    }
    // Secret 集成（P0）：Descriptor 必须可用，否则 fail-closed，避免明文绕过 SecretStore
    let mut conn_val = body.connection.clone();
    let mut secrets_to_upsert: Vec<(String, String)> = Vec::new();
    match state.manager.get_descriptor(&body.driver_id).await {
        Ok(desc) => {
            let secret_keys = secret_field_keys(&desc.connection);
            if !secret_keys.is_empty() {
                if let Some(obj) = conn_val.as_object_mut() {
                    for sk in &secret_keys {
                        if let Some(v) = obj.get(sk).cloned() {
                            if let Some(s) = v.as_str() {
                                secrets_to_upsert.push((sk.clone(), s.to_string()));
                                obj.insert(sk.clone(), serde_json::json!({"secret_set": true}));
                            } else if is_secret_marker(&v) {
                                // Create 时不允许 marker（无历史 Secret 可复用）
                                return (
                                    StatusCode::BAD_REQUEST,
                                    Json(json_error(
                                        "VALIDATION_ERROR",
                                        &format!(
                                            "field `{sk}`: create 时需提供明文，marker 仅更新时可用"
                                        ),
                                    )),
                                );
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
            );
        }
    }
    let rec = EndpointRecord {
        id: body.id.clone(),
        device_id: body.device_id,
        driver_id: body.driver_id,
        connection_json: serde_json::to_string(&conn_val).unwrap(),
        desired_running: false,
        updated_at_ns: mesa_core_types::now_unix_ns(),
    };
    match state
        .store
        .create_endpoint_with_secrets(&rec, &secrets_to_upsert)
    {
        Ok(()) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": rec.id })),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn update_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateEndpointReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 运行中禁止热改（§6.2：必须 Stop→Configure→Start）
    if state.manager.is_running(&id) {
        return (
            StatusCode::CONFLICT,
            Json(json_error(
                "CONFLICT",
                "endpoint 正在运行，请先停止后再修改配置",
            )),
        );
    }
    if !body.connection.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error(
                "VALIDATION_ERROR",
                "connection 必须为 JSON 对象",
            )),
        );
    }
    let Some(mut rec) = (match state.store.get_endpoint(&id) {
        Ok(v) => v,
        Err(e) => return store_err_to_response(e),
    }) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))),
        );
    };
    let old_driver_id = rec.driver_id.clone();
    rec.device_id = body.device_id.clone();
    rec.driver_id = body.driver_id.clone();
    // Secret 集成（P0）：fail-closed + 单事务
    let mut conn_val = body.connection.clone();
    let mut secrets_to_upsert: Vec<(String, String)> = Vec::new();
    let mut secrets_to_delete: Vec<String> = Vec::new();
    match state.manager.get_descriptor(&body.driver_id).await {
        Ok(desc) => {
            let secret_keys = secret_field_keys(&desc.connection);
            if !secret_keys.is_empty() {
                if let Some(obj) = conn_val.as_object_mut() {
                    for sk in &secret_keys {
                        if let Some(v) = obj.get(sk).cloned() {
                            if let Some(s) = v.as_str() {
                                secrets_to_upsert.push((sk.clone(), s.to_string()));
                                obj.insert(sk.clone(), serde_json::json!({"secret_set": true}));
                            } else if is_secret_marker(&v) {
                                // Driver 切换时同名 Secret 禁止复用（需重新明文）
                                if old_driver_id != body.driver_id {
                                    return (
                                        StatusCode::BAD_REQUEST,
                                        Json(json_error(
                                            "SECRET_VALUE_REQUIRED",
                                            &format!(
                                                "field `{sk}` driver 已切换，需重新提供明文而非 marker"
                                            ),
                                        )),
                                    );
                                }
                                // marker 需校验旧 Secret 是否存在
                                match state.store.get_secret(&id, sk) {
                                    Ok(Some(_)) => {}
                                    Ok(None) => {
                                        return (
                                            StatusCode::BAD_REQUEST,
                                            Json(json_error(
                                                "SECRET_NOT_FOUND",
                                                &format!("field `{sk}` marker 存在但无对应 Secret"),
                                            )),
                                        );
                                    }
                                    Err(e) => return store_err_to_response(e),
                                }
                            }
                        } else {
                            // 未包含该 Secret 字段 → 显式删除
                            secrets_to_delete.push(sk.clone());
                        }
                    }
                }
            }
            // Driver 切换：清理旧 Driver 残留的 Secret（不在新 Descriptor 中的字段）
            if let Ok(existing) = state.store.list_secret_fields(&id) {
                for ef in existing {
                    if !secret_keys.contains(&ef) && !secrets_to_delete.contains(&ef) {
                        secrets_to_delete.push(ef);
                    }
                }
            }
        }
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
            );
        }
    }
    rec.connection_json = serde_json::to_string(&conn_val).unwrap();
    rec.updated_at_ns = mesa_core_types::now_unix_ns();
    match state
        .store
        .update_endpoint_with_secrets(&rec, &secrets_to_upsert, &secrets_to_delete)
    {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "updated": id }))),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn delete_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.manager.is_running(&id) {
        return (
            StatusCode::CONFLICT,
            Json(json_error(
                "CONFLICT",
                "endpoint 正在运行，请先停止后再删除",
            )),
        );
    }
    match state.store.delete_endpoint(&id) {
        Ok(true) => {
            // 清理 Snapshot 中的残留状态（latest/status）
            state.snapshot.remove_endpoint(&id);
            (StatusCode::OK, Json(serde_json::json!({ "deleted": id })))
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn start_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = match state.store.get_endpoint(&id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))),
            );
        }
        Err(e) => return store_err_to_response(e),
    };
    if state.manager.is_running(&id) {
        return (
            StatusCode::CONFLICT,
            Json(json_error("CONFLICT", "endpoint 已在运行")),
        );
    }
    // 构造 BuiltinEndpoint
    let tasks = match state.store.list_tasks(&id) {
        Ok(v) => v,
        Err(e) => return store_err_to_response(e),
    };
    // Secret 集成：启动时临时还原明文（仅内存），fail-closed
    let mut materialized_json = rec.connection_json.clone();
    match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(desc) => {
            let secret_keys = secret_field_keys(&desc.connection);
            if !secret_keys.is_empty() {
                match materialize_connection(
                    &rec.connection_json,
                    &rec.id,
                    &state.store,
                    &secret_keys,
                ) {
                    Ok(s) => materialized_json = s,
                    Err(e) => return store_err_to_response(e),
                }
            }
        }
        Err(e) => {
            // 若存储的 connection 包含 secret marker，则 Descriptor 不可用视为安全失败
            if rec.connection_json.contains("secret_set") {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
                );
            }
        }
    }
    let cfg = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: rec.id.clone(),
        driver_id: rec.driver_id.clone(),
        connection_json: materialized_json,
        tasks,
    };
    match state.manager.start_endpoint(cfg) {
        Ok(()) => {
            let _ = state.store.set_desired_running(&id, true);
            (StatusCode::OK, Json(serde_json::json!({ "started": id })))
        }
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(json_error("START_FAILED", &msg)),
        ),
    }
}

async fn stop_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !state.manager.is_running(&id) {
        // 即使未运行也更新期望态，保持幂等
        let _ = state.store.set_desired_running(&id, false);
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "stopped": id, "was_running": false })),
        );
    }
    let was = state.manager.stop_endpoint(&id).await;
    let _ = state.store.set_desired_running(&id, false);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "stopped": id, "was_running": was })),
    )
}

// ---------------------------------------------------------------------------
// Tasks（全量快照语义 §6.2）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TasksQuery {
    endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReplaceTasksReq {
    endpoint_id: Option<String>,
    tasks: Vec<AcquisitionTask>,
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TasksQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(ep) = q.endpoint else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("VALIDATION_ERROR", "缺少查询参数 endpoint")),
        );
    };
    match state.store.list_tasks(&ep) {
        Ok(v) => {
            let rev = state.store.current_revision(&ep).unwrap_or(0);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "endpoint_id": ep, "revision": rev, "tasks": v })),
            )
        }
        Err(e) => store_err_to_response(e),
    }
}

async fn replace_tasks(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReplaceTasksReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(ep) = body.endpoint_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("VALIDATION_ERROR", "缺少 endpoint_id")),
        );
    };
    if state.manager.is_running(&ep) {
        return (
            StatusCode::CONFLICT,
            Json(json_error(
                "CONFLICT",
                "endpoint 正在运行，请先停止后再修改任务",
            )),
        );
    }
    match state.store.replace_tasks(&ep, &body.tasks) {
        Ok(rev) => (
            StatusCode::OK,
            Json(serde_json::json!({ "endpoint_id": ep, "revision": rev })),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn put_tasks_for_endpoint(
    State(state): State<Arc<AppState>>,
    Path(endpoint_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    // body 可为 {tasks:[...]} 或直接 [...]
    let tasks: Vec<AcquisitionTask> = if let Some(arr) = body.get("tasks") {
        match serde_json::from_value(arr.clone()) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error("VALIDATION_ERROR", &e.to_string())),
                );
            }
        }
    } else if body.is_array() {
        match serde_json::from_value(body) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error("VALIDATION_ERROR", &e.to_string())),
                );
            }
        }
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error(
                "VALIDATION_ERROR",
                "body 需为 {tasks:[...]} 或 [...]",
            )),
        );
    };
    if state.manager.is_running(&endpoint_id) {
        return (
            StatusCode::CONFLICT,
            Json(json_error(
                "CONFLICT",
                "endpoint 正在运行，请先停止后再修改任务",
            )),
        );
    }
    match state.store.replace_tasks(&endpoint_id, &tasks) {
        Ok(rev) => (
            StatusCode::OK,
            Json(serde_json::json!({ "endpoint_id": endpoint_id, "revision": rev })),
        ),
        Err(e) => store_err_to_response(e),
    }
}

async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path((endpoint_id, task_id)): Path<(String, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.manager.is_running(&endpoint_id) {
        return (
            StatusCode::CONFLICT,
            Json(json_error(
                "CONFLICT",
                "endpoint 正在运行，请先停止后再修改任务",
            )),
        );
    }
    let mut tasks = match state.store.list_tasks(&endpoint_id) {
        Ok(v) => v,
        Err(e) => return store_err_to_response(e),
    };
    let before = tasks.len();
    tasks.retain(|t| t.id != task_id);
    if tasks.len() == before {
        return (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("task `{task_id}`"))),
        );
    }
    match state.store.replace_tasks(&endpoint_id, &tasks) {
        Ok(rev) => (
            StatusCode::OK,
            Json(serde_json::json!({ "deleted": task_id, "revision": rev })),
        ),
        Err(e) => store_err_to_response(e),
    }
}

// ---------------------------------------------------------------------------
// drivers rescan
// ---------------------------------------------------------------------------

async fn rescan_drivers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let infos = state
        .manager
        .rescan(std::path::Path::new(&state.drivers_dir));
    Json(serde_json::json!({ "drivers": infos }))
}

// ---------------------------------------------------------------------------
// 证书管理（§19.3）
// ---------------------------------------------------------------------------

async fn list_cert_store(
    State(state): State<Arc<AppState>>,
    Path(store): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let valid = ["own", "trusted", "issuers", "rejected"];
    if !valid.contains(&store.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error(
                "VALIDATION_ERROR",
                "store 需为 own/trusted/issuers/rejected",
            )),
        );
    }
    match state.cert_store.list(&store) {
        Ok(list) => (
            StatusCode::OK,
            Json(serde_json::json!({ "store": store, "count": list.len(), "certs": list })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e)),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct AddTrustedReq {
    pem: String,
}

async fn add_trusted_cert(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddTrustedReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    if body.pem.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error("VALIDATION_ERROR", "pem 不能为空")),
        );
    }
    match state.cert_store.add_trusted(&body.pem) {
        Ok(tp) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "thumbprint": tp })),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json_error("INVALID_CERT", &e)),
        ),
    }
}

async fn remove_trusted_cert(
    State(state): State<Arc<AppState>>,
    Path(thumbprint): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.cert_store.remove_trusted(&thumbprint) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({ "deleted": thumbprint })),
        ),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error(
                "NOT_FOUND",
                &format!("证书 {thumbprint} 不存在"),
            )),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e)),
        ),
    }
}

async fn trust_rejected_cert(
    State(state): State<Arc<AppState>>,
    Path(thumbprint): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.cert_store.trust_rejected(&thumbprint) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({ "trusted": thumbprint })),
        ),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error(
                "NOT_FOUND",
                &format!("rejected 证书 {thumbprint} 不存在"),
            )),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e)),
        ),
    }
}

async fn cert_diagnostics(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(state.cert_store.diagnostics())
}

async fn list_own(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    match state.cert_store.list("own") {
        Ok(list) => (
            StatusCode::OK,
            Json(serde_json::json!({ "store": "own", "count": list.len(), "certs": list })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e)),
        ),
    }
}
async fn list_trusted(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    match state.cert_store.list("trusted") {
        Ok(list) => (
            StatusCode::OK,
            Json(serde_json::json!({ "store": "trusted", "count": list.len(), "certs": list })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e)),
        ),
    }
}
async fn list_issuers(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    match state.cert_store.list("issuers") {
        Ok(list) => (
            StatusCode::OK,
            Json(serde_json::json!({ "store": "issuers", "count": list.len(), "certs": list })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e)),
        ),
    }
}
async fn list_rejected(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.cert_store.list("rejected") {
        Ok(list) => (
            StatusCode::OK,
            Json(serde_json::json!({ "store": "rejected", "count": list.len(), "certs": list })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e)),
        ),
    }
}

// ---------------------------------------------------------------------------
// 路由装配
// ---------------------------------------------------------------------------

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // 只读 / 诊断
        .route("/api/v1/drivers", get(list_drivers))
        .route("/api/v1/drivers/rescan", post(rescan_drivers))
        .route("/api/v1/drivers/{id}", get(get_driver))
        .route(
            "/api/v1/drivers/{id}/descriptor",
            get(get_driver_descriptor),
        )
        .route(
            "/api/v1/drivers/{id}/validate-connection",
            post(validate_connection),
        )
        .route("/api/v1/drivers/{id}/probe", post(probe_driver))
        .route("/api/v1/profiles", get(list_profiles))
        .route("/api/v1/profiles/{id}", get(get_profile))
        .route(
            "/api/v1/endpoints",
            get(list_endpoints).post(create_endpoint),
        )
        .route(
            "/api/v1/endpoints/{id}",
            get(get_endpoint)
                .put(update_endpoint)
                .delete(delete_endpoint),
        )
        .route("/api/v1/endpoints/{id}/state", get(endpoint_state))
        .route(
            "/api/v1/endpoints/{id}/diagnostics",
            get(endpoint_diagnostics),
        )
        .route("/api/v1/endpoints/{id}/browse", post(browse_endpoint))
        .route("/api/v1/endpoints/{id}/import", post(import_endpoint))
        .route("/api/v1/endpoints/{id}/write", post(control_write))
        .route(
            "/api/v1/endpoints/{id}/commands/{command}",
            post(control_command),
        )
        .route("/api/v1/control/audit", get(list_control_audit))
        .route("/api/v1/control/audit/{request_id}", get(get_control_audit))
        .route("/api/v1/points/latest", get(latest_points))
        .route("/api/v1/diagnostics", get(diagnostics))
        // Devices CRUD
        .route("/api/v1/devices", get(list_devices).post(create_device))
        .route(
            "/api/v1/devices/{id}",
            get(get_device).put(update_device).delete(delete_device),
        )
        // 启停
        .route("/api/v1/endpoints/{id}/start", post(start_endpoint))
        .route("/api/v1/endpoints/{id}/stop", post(stop_endpoint))
        // Tasks（全量快照）
        .route("/api/v1/tasks", get(list_tasks).post(replace_tasks))
        .route("/api/v1/tasks/{endpoint_id}", put(put_tasks_for_endpoint))
        .route("/api/v1/tasks/{endpoint_id}/{task_id}", delete(delete_task))
        // 证书管理 §19.3（显式路由避免 Axum 参数与静态路径 405 冲突）
        .route(
            "/api/v1/certificates/opcua/diagnostics",
            get(cert_diagnostics),
        )
        .route("/api/v1/certificates/opcua/own", get(list_own))
        .route(
            "/api/v1/certificates/opcua/trusted",
            get(list_trusted).post(add_trusted_cert),
        )
        .route("/api/v1/certificates/opcua/issuers", get(list_issuers))
        .route("/api/v1/certificates/opcua/rejected", get(list_rejected))
        .route(
            "/api/v1/certificates/opcua/trusted/{thumbprint}",
            delete(remove_trusted_cert),
        )
        .route(
            "/api/v1/certificates/opcua/rejected/{thumbprint}/trust",
            post(trust_rejected_cert),
        )
        // 兼容参数路由（保留）
        .route("/api/v1/certificates/opcua/{store}", get(list_cert_store))
        // 健康探针
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

/// 兼容旧入口：仅含只读路由（供不需要持久化的单测使用）。
pub fn router_readonly(snapshot: Arc<Snapshot>) -> Router {
    Router::new()
        .route(
            "/api/v1/drivers",
            get({
                let s = snapshot.clone();
                move |_: State<Arc<Snapshot>>| {
                    let s = s.clone();
                    async move { Json(serde_json::json!({ "drivers": s.drivers() })) }
                }
            }),
        )
        .route(
            "/api/v1/endpoints",
            get({
                let s = snapshot.clone();
                move |_: State<Arc<Snapshot>>| {
                    let s = s.clone();
                    async move { Json(serde_json::json!({ "endpoints": s.endpoints() })) }
                }
            }),
        )
        .route("/healthz", get(|| async { "ok" }))
        .with_state(snapshot)
}

/// 启动 HTTP 服务（绑定严格限定 loopback §4.2）。
pub async fn serve(
    state: Arc<AppState>,
    port: u16,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let app = router(state);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
}

/// 旧签名兼容：直接以 AppState 构造并启动（Mesad 使用新签名，保留此函数避免孤立调用）。
pub async fn serve_snapshot(
    snapshot: Arc<Snapshot>,
    port: u16,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let app = router_readonly(snapshot);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
}
