//! ForgeLink 最小 REST API（方案 §4.1 的 M0 子集）。
//!
//! 安全边界（§4.2）：仅绑定 loopback，不提供任何远程管理能力。
//! M0 只暴露只读接口；Device/Endpoint/Task CRUD 随 ConfigStore 在 Phase 1 引入。

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use forgelink_driver_manager::Snapshot;
use tokio_util::sync::CancellationToken;

/// GET /api/v1/drivers —— 已发现驱动清单。
async fn list_drivers(State(snapshot): State<std::sync::Arc<Snapshot>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "drivers": snapshot.drivers() }))
}

/// GET /api/v1/endpoints/{id}/state —— 单端点运行状态。
async fn endpoint_state(
    State(snapshot): State<std::sync::Arc<Snapshot>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    match snapshot.endpoint(&id) {
        Some(s) => Json(serde_json::to_value(s).expect("serializable status")),
        None => Json(not_found(&format!("endpoint `{id}`"))),
    }
}

/// 列出全部端点状态，便于验收时一次查看。
async fn list_endpoints(State(snapshot): State<std::sync::Arc<Snapshot>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "endpoints": snapshot.endpoints() }))
}

/// GET /api/v1/points/latest —— 最新值缓存快照。
async fn latest_points(State(snapshot): State<std::sync::Arc<Snapshot>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "points": snapshot.latest_all(), "count": snapshot.latest_all().len() }))
}

fn not_found(what: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": "NOT_FOUND", "message": what } })
}

pub fn router(snapshot: std::sync::Arc<Snapshot>) -> Router {
    Router::new()
        .route("/api/v1/drivers", get(list_drivers))
        .route("/api/v1/endpoints", get(list_endpoints))
        .route("/api/v1/endpoints/{id}/state", get(endpoint_state))
        .route("/api/v1/points/latest", get(latest_points))
        // 健康探针：服务化部署（§25）的存活检查入口
        .route("/healthz", get(|| async { "ok" }))
        .with_state(snapshot)
}

/// 启动 HTTP 服务。绑定严格限定 loopback（§4.2），shutdown 触发后优雅退出。
pub async fn serve(
    snapshot: std::sync::Arc<Snapshot>,
    port: u16,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    let app = router(snapshot);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
}
