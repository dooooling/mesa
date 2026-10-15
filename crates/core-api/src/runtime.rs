//! 运行期 Endpoint 构造的**唯一共享路径**（C1 P0）。
//!
//! 为什么必须共享：同一条存储配置在三条入口上被构造：
//!
//! 1. `POST /endpoints/{id}/start`（REST 启动）；
//! 2. `POST /device-bootstrap` 且 `start=true`（建档即启动）；
//! 3. `mesad` 开机恢复 `desired_running=true` 的 Endpoint。
//!
//! 前两条各自实现了 Descriptor 查询 + Secret 物化 + 任务门禁，第三条**什么都没做**：
//! 直接把持久化的 `connection_json`（其中 Secret 字段是 `{"secret_set": true}`
//! marker）交给 Driver。后果不是“少一个优化”，而是**重启后带认证的连接必然失败**：
//! marker 不是密码，Driver 拿到它只能报 `BAD_CONFIG`，而配置类失败是终态
//! `ConfigurationFailed`（不重试），端点会永久停在 FAILED。
//!
//! 因此这里把「读存储 → 取 Descriptor → 物化 Secret → 读任务 → 跑门禁 →
//! 造 BuiltinEndpoint」收敛成一个函数，三条入口只允许调用它。
//!
//! 边界：
//! - 本模块**不做** side effect：不 `start_endpoint`、不改 `desired_running`、
//!   不做 bootstrap 失败补偿——那些仍归各自入口；
//! - 明文 Secret 只存在于返回值的 `connection_json` 中（仅内存），
//!   不得写日志、不得回 HTTP；
//! - fail-closed：Endpoint 不存在、配置 JSON 非法、marker 缺对应 Secret、
//!   Descriptor 不可用且配置含 marker，一律返回错误，绝不降级为匿名连接。

use axum::Json;
use axum::http::StatusCode;
use mesa_config_store::{EndpointRecord, StoreError};
use mesa_driver_manager::endpoint::BuiltinEndpoint;

use crate::{
    AppState, gate_data_tasks, gate_event_tasks, materialize_connection, secret_field_keys,
};

/// 运行期 Endpoint 构造失败（结构化：REST 可映射状态码，Mesad 可记录原因）。
#[derive(Debug, Clone)]
pub struct RuntimeBuildError {
    /// 供 REST 直接使用的状态码。
    pub status: StatusCode,
    /// 稳定错误码（禁止用文本匹配代替）。
    pub code: String,
    pub message: String,
}

impl RuntimeBuildError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }

    fn not_found(endpoint_id: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("endpoint `{endpoint_id}`"),
        )
    }

    /// 转 REST 错误体（与既有 `json_error` 外形一致）。
    pub fn to_json(&self) -> Json<serde_json::Value> {
        Json(crate::json_error(&self.code, &self.message))
    }
}

impl std::fmt::Display for RuntimeBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}: {}",
            self.status.as_u16(),
            self.code,
            self.message
        )
    }
}

impl std::error::Error for RuntimeBuildError {}

/// `StoreError` → 运行期构造错误（沿用既有 HTTP 映射口径）。
fn store_error(e: StoreError) -> RuntimeBuildError {
    let (status, code) = match e {
        StoreError::Duplicate(_) => (StatusCode::CONFLICT, "CONFLICT"),
        StoreError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
        StoreError::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
        StoreError::Validation(_) => (StatusCode::BAD_REQUEST, "VALIDATION_ERROR"),
        StoreError::Json(_) => (StatusCode::BAD_REQUEST, "INVALID_JSON"),
        StoreError::Sqlite(_) => (StatusCode::INTERNAL_SERVER_ERROR, "SQLITE_ERROR"),
    };
    RuntimeBuildError::new(status, code, e.to_string())
}

/// 门禁函数返回的 `(StatusCode, Json)` → 运行期构造错误。
fn gate_error((status, Json(body)): (StatusCode, Json<serde_json::Value>)) -> RuntimeBuildError {
    let err = body.get("error");
    let code = err
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
        .unwrap_or("VALIDATION_ERROR");
    let message = err
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("任务校验失败");
    RuntimeBuildError::new(status, code, message)
}

/// Secret 物化（仅内存）。抽成独立函数供 runtime 与 Browse 复用同一语义。
///
/// Descriptor 不可用时的处理是**有意保守**的：无法判断哪些字段是 Secret，
/// 因此只要配置里出现 marker 就 fail-closed（`503`），而不是把 marker
/// 当明文交给 Driver。
///
/// 无论是否存在 Secret 字段，配置都必须先是**合法 JSON 对象**：坏配置在
/// Core 侧即被挡住（`INVALID_JSON`），避免把不可解析的文本交给 Driver，
/// 也避免「配置损坏」被静默当作「没有连接参数」。Secret 读回失败
/// （缺记录 / 密钥不匹配 / 密文损坏）同样原样上抛，绝不降级为匿名连接。
pub(crate) async fn materialize_for_runtime(
    state: &AppState,
    rec: &EndpointRecord,
) -> Result<String, RuntimeBuildError> {
    let parsed: serde_json::Value = serde_json::from_str(&rec.connection_json).map_err(|e| {
        RuntimeBuildError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_JSON",
            format!("endpoint `{}` 的 connection_json 非法: {e}", rec.id),
        )
    })?;
    if !parsed.is_object() {
        return Err(RuntimeBuildError::new(
            StatusCode::BAD_REQUEST,
            "INVALID_JSON",
            format!("endpoint `{}` 的 connection_json 必须是 JSON 对象", rec.id),
        ));
    }
    match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(desc) => {
            let secret_keys = secret_field_keys(&desc.connection);
            if secret_keys.is_empty() {
                return Ok(rec.connection_json.clone());
            }
            materialize_connection(&rec.connection_json, &rec.id, &state.store, &secret_keys)
                .map_err(store_error)
        }
        Err(e) => {
            if rec.connection_json.contains("secret_set") {
                Err(RuntimeBuildError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    e.code,
                    e.message,
                ))
            } else {
                Ok(rec.connection_json.clone())
            }
        }
    }
}

/// 构造运行期 Endpoint（**唯一**路径，见模块文档）。
///
/// 顺序与既有 REST 启动路径一致：Endpoint → Data Tasks → Data 门禁 →
/// Secret 物化 → Event Tasks → Event 门禁。任一环节失败即返回错误，
/// 不产生半成品 cfg。
pub async fn build_runtime_endpoint(
    state: &AppState,
    endpoint_id: &str,
) -> Result<BuiltinEndpoint, RuntimeBuildError> {
    let rec = state
        .store
        .get_endpoint(endpoint_id)
        .map_err(store_error)?
        .ok_or_else(|| RuntimeBuildError::not_found(endpoint_id))?;

    // Data Tasks：读失败不得静默当空任务（否则会以“无任务”启动一个
    // 实际配置了点位的端点，且不给任何提示）。
    let tasks = state.store.list_tasks(endpoint_id).map_err(store_error)?;
    if let Err(resp) = gate_data_tasks(state, endpoint_id, &tasks).await {
        return Err(gate_error(resp));
    }

    let connection_json = materialize_for_runtime(state, &rec).await?;

    let event_tasks = state
        .store
        .list_event_tasks(endpoint_id)
        .map_err(store_error)?;
    if let Err(resp) = gate_event_tasks(state, endpoint_id, &event_tasks).await {
        return Err(gate_error(resp));
    }

    Ok(BuiltinEndpoint {
        endpoint_id: rec.id.clone(),
        driver_id: rec.driver_id.clone(),
        connection_json,
        tasks,
        event_tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_config_store::{ConfigStore, DeviceRecord};
    use std::sync::Arc;

    /// 独立临时目录（每个用例一个，避免并发用例互踩）。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "mesa-runtime-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 构造一个含单个 Endpoint 的 AppState。`drivers_dir` 为空目录，
    /// 因此 `get_descriptor` 必然失败——正好用来验证 fail-closed 分支。
    fn app_with_endpoint(connection_json: &str, tag: &str) -> (Arc<AppState>, Arc<ConfigStore>) {
        let store = Arc::new(ConfigStore::open_in_memory().unwrap());
        store
            .create_device(&DeviceRecord {
                id: "dev-1".into(),
                name: "dev".into(),
            })
            .unwrap();
        store
            .create_endpoint(&mesa_config_store::EndpointRecord {
                id: "ep-1".into(),
                name: "ep".into(),
                device_id: "dev-1".into(),
                driver_id: "opcua".into(),
                connection_json: connection_json.to_string(),
                desired_running: false,
                updated_at_ns: 0,
            })
            .unwrap();
        let drivers_dir = temp_dir(&format!("{tag}-drivers"));
        let manager = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
        let cert_dir = temp_dir(&format!("{tag}-cert"));
        let state = AppState::try_new_with_cert_dir_and_control(
            manager,
            Arc::clone(&store),
            drivers_dir.to_string_lossy().to_string(),
            cert_dir,
            false,
        )
        .unwrap();
        (state, store)
    }

    #[tokio::test]
    async fn missing_endpoint_is_not_found() {
        let (state, _store) = app_with_endpoint("{}", "missing");
        let err = build_runtime_endpoint(&state, "nope").await.unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.code, "NOT_FOUND");
    }

    /// P0：配置含 Secret marker 而 Descriptor 不可用时，**无法**判断哪些字段是
    /// Secret，必须 fail-closed（绝不把 marker 当密码交给 Driver）。
    #[tokio::test]
    async fn secret_marker_without_descriptor_is_fail_closed() {
        let (state, _store) = app_with_endpoint(
            r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#,
            "marker",
        );
        let err = build_runtime_endpoint(&state, "ep-1").await.unwrap_err();
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            !err.message.contains("secret_set"),
            "错误消息不得回显 marker 细节之外的内容"
        );
    }

    /// 无 Secret 的配置在 Descriptor 不可用时仍可构造（不引入新的硬依赖）。
    #[tokio::test]
    async fn plain_connection_survives_descriptor_unavailable() {
        let conn = r#"{"endpoint_url":"opc.tcp://a:4840"}"#;
        let (state, _store) = app_with_endpoint(conn, "plain");
        let cfg = build_runtime_endpoint(&state, "ep-1").await.unwrap();
        assert_eq!(cfg.endpoint_id, "ep-1");
        assert_eq!(cfg.driver_id, "opcua");
        assert_eq!(cfg.connection_json, conn);
        assert!(cfg.tasks.is_empty());
        assert!(cfg.event_tasks.is_empty());
    }

    /// A31 / 配置损坏（两层防线）：
    /// (1) 写入侧：`create_endpoint` 拒绝非法/非对象的 `connection_json`，
    ///     因此坏配置无法经正常 API 落库；
    /// (2) 读取侧：运行期构造仍独立校验一次（纵深防御，覆盖直改 SQLite、
    ///     未来迁移等旁路），失败码为 `INVALID_JSON`，绝不把不可解析文本
    ///     交给 Driver，也不伪装成「没有连接参数」。
    ///
    /// 这里断言 (1)；(2) 的纯函数分支由 `malformed_connection_json_is_rejected`
    /// 直接覆盖（经 API 不可达，属于防御性代码）。
    #[test]
    fn store_rejects_malformed_connection_json_at_write_time() {
        let store = ConfigStore::open_in_memory().unwrap();
        store
            .create_device(&DeviceRecord {
                id: "dev-1".into(),
                name: "dev".into(),
            })
            .unwrap();
        for bad in ["{not json", "[1,2,3]", "\"just-a-string\"", "null", "42"] {
            let r = store.create_endpoint(&mesa_config_store::EndpointRecord {
                id: format!("ep-{bad}"),
                name: "ep".into(),
                device_id: "dev-1".into(),
                driver_id: "opcua".into(),
                connection_json: bad.to_string(),
                desired_running: false,
                updated_at_ns: 0,
            });
            assert!(
                r.is_err(),
                "connection_json={bad} 必须在写入侧被拒绝，实际 Ok"
            );
        }
    }

    /// 坏 JSON 不再被 `unwrap_or({})` 悄悄变成空对象。
    #[test]
    fn malformed_connection_json_is_rejected() {
        let store = ConfigStore::open_in_memory().unwrap();
        let keys = vec!["password".to_string()];
        let err = crate::materialize_connection("{not json", "ep-1", &store, &keys).unwrap_err();
        assert!(matches!(err, StoreError::Json(_)), "实际: {err:?}");
        let err = crate::materialize_connection("[1,2,3]", "ep-1", &store, &keys).unwrap_err();
        assert!(matches!(err, StoreError::Validation(_)), "实际: {err:?}");
    }

    /// marker 有对应 Secret → 只在内存中还原为明文；缺 Secret → 明确失败。
    #[test]
    fn marker_is_materialized_in_memory_only() {
        let store = ConfigStore::open_in_memory().unwrap();
        store
            .create_device(&DeviceRecord {
                id: "dev-1".into(),
                name: "dev".into(),
            })
            .unwrap();
        store
            .create_endpoint(&mesa_config_store::EndpointRecord {
                id: "ep-1".into(),
                name: "ep".into(),
                device_id: "dev-1".into(),
                driver_id: "opcua".into(),
                connection_json: r#"{"password":{"secret_set":true}}"#.into(),
                desired_running: false,
                updated_at_ns: 0,
            })
            .unwrap();
        let keys = vec!["password".to_string()];
        // 缺 Secret → 明确失败（不得匿名降级）。
        let err = crate::materialize_connection(
            r#"{"password":{"secret_set":true}}"#,
            "ep-1",
            &store,
            &keys,
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::Validation(_)), "实际: {err:?}");
        // 存入 Secret 后 → 还原为明文，且存储行本身仍只有 marker。
        store
            .put_secret("ep-1", "password", "s3cr3t", "master")
            .unwrap();
        let out = crate::materialize_connection(
            r#"{"password":{"secret_set":true}}"#,
            "ep-1",
            &store,
            &keys,
        )
        .unwrap();
        assert!(out.contains("s3cr3t"), "运行期配置必须拿到明文");
        let stored = store.get_endpoint("ep-1").unwrap().unwrap();
        assert!(
            stored.connection_json.contains("secret_set"),
            "物化只发生在内存，持久化行不得写入明文"
        );
        assert!(!stored.connection_json.contains("s3cr3t"));
    }
}
