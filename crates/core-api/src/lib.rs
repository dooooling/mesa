//! ForgeLink REST API（方案 §4.1）。
//!
//! 安全边界（§4.2）：仅绑定 loopback，不提供任何远程管理能力。
//! Phase B 补齐：Device/Endpoint/Task CRUD + 启停 + rescan + diagnostics（§4.1 最小集）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use forgelink_config_store::{ConfigStore, DeviceRecord, EndpointRecord, StoreError};
use forgelink_core_types::AcquisitionTask;
use forgelink_driver_manager::{ForgeLinkManager, Snapshot};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// 共享状态
// ---------------------------------------------------------------------------

pub struct AppState {
    pub snapshot: Arc<Snapshot>,
    pub manager: Arc<ForgeLinkManager>,
    pub store: Arc<ConfigStore>,
    /// 驱动目录（供 rescan 使用）。
    pub drivers_dir: String,
    pub start_time: Instant,
}

impl AppState {
    pub fn new(
        manager: Arc<ForgeLinkManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
    ) -> Arc<Self> {
        let snapshot = manager.snapshot();
        Arc::new(Self {
            snapshot,
            manager,
            store,
            drivers_dir,
            start_time: Instant::now(),
        })
    }
}

// ---------------------------------------------------------------------------
// 通用响应辅助
// ---------------------------------------------------------------------------

fn json_error(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

fn store_err_to_response(e: StoreError) -> (StatusCode, Json<serde_json::Value>) {
    match e {
        StoreError::Duplicate(msg) => (StatusCode::CONFLICT, Json(json_error("CONFLICT", &msg))),
        StoreError::NotFound(msg) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &msg))),
        StoreError::Conflict(msg) => (StatusCode::CONFLICT, Json(json_error("CONFLICT", &msg))),
        StoreError::Validation(msg) => (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", &msg))),
        StoreError::Json(err) => (StatusCode::BAD_REQUEST, Json(json_error("INVALID_JSON", &err.to_string()))),
        StoreError::Sqlite(err) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json_error("INTERNAL", &err.to_string()))),
    }
}

// ---------------------------------------------------------------------------
// 只读查询（沿用 M0）
// ---------------------------------------------------------------------------

async fn list_drivers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "drivers": state.snapshot.drivers() }))
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
            let conn: serde_json::Value = serde_json::from_str(&rec.connection_json).unwrap_or(serde_json::json!({}));
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
    Json(serde_json::json!({ "points": state.snapshot.latest_all(), "count": state.snapshot.latest_all().len() }))
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
        Ok(None) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("device `{id}`")))),
        Err(e) => store_err_to_response(e),
    }
}

async fn create_device(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDeviceReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = DeviceRecord { id: body.id.clone(), name: body.name, profile: body.profile };
    match state.store.create_device(&rec) {
        Ok(()) => (StatusCode::CREATED, Json(serde_json::to_value(&rec).unwrap())),
        Err(e) => store_err_to_response(e),
    }
}

async fn update_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDeviceReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = DeviceRecord { id: id.clone(), name: body.name, profile: body.profile };
    match state.store.update_device(&rec) {
        Ok(true) => (StatusCode::OK, Json(serde_json::to_value(&rec).unwrap())),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("device `{id}`")))),
        Err(e) => store_err_to_response(e),
    }
}

async fn delete_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.store.delete_device(&id) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": id }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("device `{id}`")))),
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
            let conn: serde_json::Value = serde_json::from_str(&rec.connection_json).unwrap_or(serde_json::json!({}));
            (StatusCode::OK, Json(serde_json::json!({
                "id": rec.id, "device_id": rec.device_id, "driver_id": rec.driver_id,
                "connection": conn, "desired_running": rec.desired_running,
                "updated_at_ns": rec.updated_at_ns, "runtime": runtime,
            })))
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("endpoint `{id}`")))),
        Err(e) => store_err_to_response(e),
    }
}

async fn create_endpoint(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateEndpointReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !body.connection.is_object() {
        return (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", "connection 必须为 JSON 对象")));
    }
    let rec = EndpointRecord {
        id: body.id.clone(),
        device_id: body.device_id,
        driver_id: body.driver_id,
        connection_json: serde_json::to_string(&body.connection).unwrap(),
        desired_running: false,
        updated_at_ns: forgelink_core_types::now_unix_ns(),
    };
    match state.store.create_endpoint(&rec) {
        Ok(()) => (StatusCode::CREATED, Json(serde_json::json!({ "id": rec.id }))),
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
        return (StatusCode::CONFLICT, Json(json_error("CONFLICT", "endpoint 正在运行，请先停止后再修改配置")));
    }
    if !body.connection.is_object() {
        return (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", "connection 必须为 JSON 对象")));
    }
    let Some(mut rec) = (match state.store.get_endpoint(&id) { Ok(v) => v, Err(e) => return store_err_to_response(e) }) else {
        return (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))));
    };
    rec.device_id = body.device_id;
    rec.driver_id = body.driver_id;
    rec.connection_json = serde_json::to_string(&body.connection).unwrap();
    rec.updated_at_ns = forgelink_core_types::now_unix_ns();
    match state.store.update_endpoint(&rec) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "updated": id }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("endpoint `{id}`")))),
        Err(e) => store_err_to_response(e),
    }
}

async fn delete_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.manager.is_running(&id) {
        return (StatusCode::CONFLICT, Json(json_error("CONFLICT", "endpoint 正在运行，请先停止后再删除")));
    }
    match state.store.delete_endpoint(&id) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": id }))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("endpoint `{id}`")))),
        Err(e) => store_err_to_response(e),
    }
}

async fn start_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let rec = match state.store.get_endpoint(&id) {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("endpoint `{id}`")))),
        Err(e) => return store_err_to_response(e),
    };
    if state.manager.is_running(&id) {
        return (StatusCode::CONFLICT, Json(json_error("CONFLICT", "endpoint 已在运行")));
    }
    // 构造 BuiltinEndpoint
    let tasks = match state.store.list_tasks(&id) { Ok(v) => v, Err(e) => return store_err_to_response(e) };
    let cfg = forgelink_driver_manager::endpoint::BuiltinEndpoint {
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
        Err(msg) => (StatusCode::BAD_REQUEST, Json(json_error("START_FAILED", &msg))),
    }
}

async fn stop_endpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if !state.manager.is_running(&id) {
        // 即使未运行也更新期望态，保持幂等
        let _ = state.store.set_desired_running(&id, false);
        return (StatusCode::OK, Json(serde_json::json!({ "stopped": id, "was_running": false })));
    }
    let was = state.manager.stop_endpoint(&id).await;
    let _ = state.store.set_desired_running(&id, false);
    (StatusCode::OK, Json(serde_json::json!({ "stopped": id, "was_running": was })))
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
        return (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", "缺少查询参数 endpoint")));
    };
    match state.store.list_tasks(&ep) {
        Ok(v) => {
            let rev = state.store.current_revision(&ep).unwrap_or(0);
            (StatusCode::OK, Json(serde_json::json!({ "endpoint_id": ep, "revision": rev, "tasks": v })))
        }
        Err(e) => store_err_to_response(e),
    }
}

async fn replace_tasks(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReplaceTasksReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(ep) = body.endpoint_id else {
        return (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", "缺少 endpoint_id")));
    };
    if state.manager.is_running(&ep) {
        return (StatusCode::CONFLICT, Json(json_error("CONFLICT", "endpoint 正在运行，请先停止后再修改任务")));
    }
    match state.store.replace_tasks(&ep, &body.tasks) {
        Ok(rev) => (StatusCode::OK, Json(serde_json::json!({ "endpoint_id": ep, "revision": rev }))),
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
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", &e.to_string()))),
        }
    } else if body.is_array() {
        match serde_json::from_value(body) {
            Ok(v) => v,
            Err(e) => return (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", &e.to_string()))),
        }
    } else {
        return (StatusCode::BAD_REQUEST, Json(json_error("VALIDATION_ERROR", "body 需为 {tasks:[...]} 或 [...]")));
    };
    if state.manager.is_running(&endpoint_id) {
        return (StatusCode::CONFLICT, Json(json_error("CONFLICT", "endpoint 正在运行，请先停止后再修改任务")));
    }
    match state.store.replace_tasks(&endpoint_id, &tasks) {
        Ok(rev) => (StatusCode::OK, Json(serde_json::json!({ "endpoint_id": endpoint_id, "revision": rev }))),
        Err(e) => store_err_to_response(e),
    }
}

async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path((endpoint_id, task_id)): Path<(String, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.manager.is_running(&endpoint_id) {
        return (StatusCode::CONFLICT, Json(json_error("CONFLICT", "endpoint 正在运行，请先停止后再修改任务")));
    }
    let mut tasks = match state.store.list_tasks(&endpoint_id) {
        Ok(v) => v,
        Err(e) => return store_err_to_response(e),
    };
    let before = tasks.len();
    tasks.retain(|t| t.id != task_id);
    if tasks.len() == before {
        return (StatusCode::NOT_FOUND, Json(json_error("NOT_FOUND", &format!("task `{task_id}`"))));
    }
    match state.store.replace_tasks(&endpoint_id, &tasks) {
        Ok(rev) => (StatusCode::OK, Json(serde_json::json!({ "deleted": task_id, "revision": rev }))),
        Err(e) => store_err_to_response(e),
    }
}

// ---------------------------------------------------------------------------
// drivers rescan
// ---------------------------------------------------------------------------

async fn rescan_drivers(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let infos = state.manager.rescan(std::path::Path::new(&state.drivers_dir));
    Json(serde_json::json!({ "drivers": infos }))
}

// ---------------------------------------------------------------------------
// 路由装配
// ---------------------------------------------------------------------------

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // 只读 / 诊断
        .route("/api/v1/drivers", get(list_drivers))
        .route("/api/v1/drivers/rescan", post(rescan_drivers))
        .route("/api/v1/endpoints", get(list_endpoints).post(create_endpoint))
        .route("/api/v1/endpoints/{id}", get(get_endpoint).put(update_endpoint).delete(delete_endpoint))
        .route("/api/v1/endpoints/{id}/state", get(endpoint_state))
        .route("/api/v1/points/latest", get(latest_points))
        .route("/api/v1/diagnostics", get(diagnostics))
        // Devices CRUD
        .route("/api/v1/devices", get(list_devices).post(create_device))
        .route("/api/v1/devices/{id}", get(get_device).put(update_device).delete(delete_device))
        // 启停
        .route("/api/v1/endpoints/{id}/start", post(start_endpoint))
        .route("/api/v1/endpoints/{id}/stop", post(stop_endpoint))
        // Tasks（全量快照）
        .route("/api/v1/tasks", get(list_tasks).post(replace_tasks))
        .route("/api/v1/tasks/{endpoint_id}", put(put_tasks_for_endpoint))
        .route("/api/v1/tasks/{endpoint_id}/{task_id}", delete(delete_task))
        // 健康探针
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

/// 兼容旧入口：仅含只读路由（供不需要持久化的单测使用）。
pub fn router_readonly(snapshot: Arc<Snapshot>) -> Router {
    Router::new()
        .route("/api/v1/drivers", get({
            let s = snapshot.clone();
            move |_: State<Arc<Snapshot>>| {
                let s = s.clone();
                async move { Json(serde_json::json!({ "drivers": s.drivers() })) }
            }
        }))
        .route("/api/v1/endpoints", get({
            let s = snapshot.clone();
            move |_: State<Arc<Snapshot>>| {
                let s = s.clone();
                async move { Json(serde_json::json!({ "endpoints": s.endpoints() })) }
            }
        }))
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

/// 旧签名兼容：直接以 AppState 构造并启动（forgelinkd 使用新签名，保留此函数避免孤立调用）。
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
