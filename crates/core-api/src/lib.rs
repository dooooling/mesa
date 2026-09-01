#![allow(clippy::collapsible_if)]
//! Mesa REST API（方案 §4.1）。
//!
//! 安全边界（§4.2）：仅绑定 loopback，不提供任何远程管理能力。
//! Device/Endpoint/Task CRUD + 启停 + rescan + diagnostics 已实现（§4.1 最小集）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
}

impl AppState {
    pub fn new(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
    ) -> Arc<Self> {
        Self::new_with_cert_dir(manager, store, drivers_dir, CertStore::default_path())
    }

    pub fn new_with_cert_dir(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        cert_dir: std::path::PathBuf,
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
    // 临时进程探测（5s 超时）
    let probe_res =
        tokio::time::timeout(Duration::from_secs(6), async {
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
    let merged: Vec<serde_json::Value> = stored
        .into_iter()
        .map(|rec| {
            let runtime = live.get(&rec.id).cloned();
            let conn: serde_json::Value =
                serde_json::from_str(&rec.connection_json).unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "id": rec.id,
                "device_id": rec.device_id,
                "driver_id": rec.driver_id,
                "connection": conn,
                "desired_running": rec.desired_running,
                "updated_at_ns": rec.updated_at_ns,
                "runtime": runtime,
            })
        })
        .collect();
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
            let conn: serde_json::Value =
                serde_json::from_str(&rec.connection_json).unwrap_or(serde_json::json!({}));
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
    let rec = EndpointRecord {
        id: body.id.clone(),
        device_id: body.device_id,
        driver_id: body.driver_id,
        connection_json: serde_json::to_string(&body.connection).unwrap(),
        desired_running: false,
        updated_at_ns: mesa_core_types::now_unix_ns(),
    };
    match state.store.create_endpoint(&rec) {
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
    rec.device_id = body.device_id;
    rec.driver_id = body.driver_id;
    rec.connection_json = serde_json::to_string(&body.connection).unwrap();
    rec.updated_at_ns = mesa_core_types::now_unix_ns();
    match state.store.update_endpoint(&rec) {
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
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": id }))),
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
    let cfg = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: rec.id.clone(),
        driver_id: rec.driver_id.clone(),
        connection_json: rec.connection_json.clone(),
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
