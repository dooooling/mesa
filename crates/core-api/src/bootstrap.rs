//! M5 device-bootstrap：一次完成 Device + Endpoint + Tasks + Start 的原子接口。
//!
//! 事务边界（多轮评审结论）：
//! - DB 事务内：Device + Endpoint(+Secrets) + Tasks + revision（单事务，
//!   要么全落库要么全回滚，不存在半提交）；
//! - DB 事务外（runtime side effect）：Driver start。start 失败时补偿删除
//!   已落库的 endpoint → device，并明确报告（`compensated: true` +
//!   `start_failed: true`），调用方不得把“DB 建了但没跑起来”当成功。
//! - 幂等：`Idempotency-Key` header（或 body `idempotency_key`）+ 请求指纹。
//!   同 key 同指纹直接返回上次结果（`replayed: true`）；同 key 不同指纹 409。

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use mesa_config_store::{DeviceRecord, EndpointRecord};
use serde::{Deserialize, Serialize};

use super::{AppState, json_error, secret_field_keys, store_err_to_response};

/// Bootstrap 请求体。`acquisition.tasks` 为空表示不配置任务（纯连接设备）；
/// `start` 为 false 时建完保持 STOPPED（不执行 start side effect）。
#[derive(Debug, Deserialize, Serialize)]
pub struct BootstrapRequest {
    pub device: BootstrapDevice,
    pub endpoint: BootstrapEndpoint,
    pub acquisition: Option<BootstrapAcquisition>,
    pub start: Option<bool>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BootstrapDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BootstrapEndpoint {
    pub id: String,
    pub name: String,
    pub driver_id: String,
    pub connection: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BootstrapAcquisition {
    pub tasks: Vec<mesa_core_types::AcquisitionTask>,
}

/// 请求指纹：归一化 JSON（键排序）后哈希。secret 明文字段参与指纹——
///
/// 同一 key 换了密码必须视为不同请求（409），否则重放会返回“成功”但密码没换。
fn request_fingerprint(body: &serde_json::Value) -> String {
    let canonical = canonical_json(body);
    let mut h = 0xcbf29ce484222325u64;
    for b in canonical.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        canonical_json(&m[*k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        _ => serde_json::to_string(v).unwrap_or_default(),
    }
}

fn idempotency_key(headers: &HeaderMap, body: &BootstrapRequest) -> Option<String> {
    if let Some(v) = headers.get("idempotency-key").and_then(|v| v.to_str().ok()) {
        let t = v.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    body.idempotency_key
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 启动 side effect（`start_endpoint` 的核心逻辑抽出：DB 已落库后执行）。
/// 失败返回 message（调用方补偿删除并报告）。
async fn start_created_endpoint(state: &AppState, endpoint_id: &str) -> Result<(), String> {
    let rec = match state.store.get_endpoint(endpoint_id) {
        Ok(Some(r)) => r,
        Ok(None) => return Err(format!("endpoint `{endpoint_id}` 不存在（事务后丢失）")),
        Err(e) => return Err(e.to_string()),
    };
    let tasks = state
        .store
        .list_tasks(endpoint_id)
        .map_err(|e| e.to_string())?;
    if let Err((_, j)) = super::gate_data_tasks(state, endpoint_id, &tasks).await {
        return Err(j
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("任务校验失败")
            .to_string());
    }
    // Secret 还原（与 start_endpoint 同逻辑）
    let mut materialized_json = rec.connection_json.clone();
    match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(desc) => {
            let secret_keys = secret_field_keys(&desc.connection);
            if !secret_keys.is_empty() {
                match super::materialize_connection(
                    &rec.connection_json,
                    &rec.id,
                    &state.store,
                    &secret_keys,
                ) {
                    Ok(s) => materialized_json = s,
                    Err(e) => return Err(e.to_string()),
                }
            }
        }
        Err(e) => {
            if rec.connection_json.contains("secret_set") {
                return Err(e.message);
            }
        }
    }
    let event_tasks = state
        .store
        .list_event_tasks(endpoint_id)
        .map_err(|e| e.to_string())?;
    if let Err((_, j)) = super::gate_event_tasks(state, endpoint_id, &event_tasks).await {
        return Err(j
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("事件任务校验失败")
            .to_string());
    }
    let cfg = mesa_driver_manager::endpoint::BuiltinEndpoint {
        endpoint_id: rec.id.clone(),
        driver_id: rec.driver_id.clone(),
        connection_json: materialized_json,
        tasks,
        event_tasks,
    };
    state.manager.start_endpoint(cfg)?;
    // 期望态落盘失败不翻转 start 成功（驱动已在运行，DB 期望态下次对账可纠）；
    // 但必须记日志，否则重启恢复时该 endpoint 不会被自动拉起，静默丢运行态。
    if let Err(e) = state.store.set_desired_running(endpoint_id, true) {
        tracing::error!(
            endpoint_id = %endpoint_id,
            error = %e,
            "device-bootstrap start 成功但期望态落盘失败，重启后可能不自动恢复"
        );
    }
    Ok(())
}

pub async fn device_bootstrap(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BootstrapRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    device_bootstrap_inner(State(state), headers, body).await
}

async fn device_bootstrap_inner(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: BootstrapRequest,
) -> (StatusCode, Json<serde_json::Value>) {
    // 幂等键 + 指纹（body 指纹排除 idempotency_key 自身，避免“同请求换 key”
    // 视为不同指纹；secret 明文参与指纹——同一 key 换密码必须 409）。
    let mut fp_value = serde_json::to_value(&body).unwrap_or(serde_json::Value::Null);
    if let Some(o) = fp_value.as_object_mut() {
        o.remove("idempotency_key");
    }
    let fingerprint = request_fingerprint(&fp_value);
    let idem_key = idempotency_key(&headers, &body);

    if let Some(key) = &idem_key {
        match state.store.bootstrap_idempotency_get(key) {
            Ok(Some((hash, result))) if hash == fingerprint => {
                let mut v: serde_json::Value =
                    serde_json::from_str(&result).unwrap_or(serde_json::json!({}));
                if let Some(o) = v.as_object_mut() {
                    o.insert("replayed".to_string(), serde_json::Value::Bool(true));
                }
                return (StatusCode::OK, Json(v));
            }
            Ok(Some(_)) => {
                return (
                    StatusCode::CONFLICT,
                    Json(json_error(
                        "IDEMPOTENCY_CONFLICT",
                        "同一幂等键对应不同请求内容，请更换幂等键",
                    )),
                );
            }
            Ok(None) => {}
            Err(e) => return store_err_to_response(e),
        }
    }

    if !body.endpoint.connection.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error(
                "VALIDATION_ERROR",
                "connection 必须为 JSON 对象",
            )),
        );
    }

    // 1) Descriptor 门禁：connection 校验 + secret 拆分（与 create_endpoint 同语义）
    let mut conn_val = body.endpoint.connection.clone();
    let mut secrets_plain: Vec<(String, String)> = Vec::new();
    match state.manager.get_descriptor(&body.endpoint.driver_id).await {
        Ok(desc) => {
            let conn_issues = desc.connection.validate_instance("connection", &conn_val);
            if !conn_issues.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "valid": false, "issues": conn_issues })),
                );
            }
            let secret_keys = secret_field_keys(&desc.connection);
            if !secret_keys.is_empty() {
                if let Some(obj) = conn_val.as_object_mut() {
                    for sk in &secret_keys {
                        if let Some(v) = obj.get(sk).cloned() {
                            if let Some(s) = v.as_str() {
                                secrets_plain.push((sk.clone(), s.to_string()));
                                obj.insert(sk.clone(), serde_json::json!({"secret_set": true}));
                            } else if super::is_secret_marker(&v) {
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
            // acquisition 集合校验（endpoint 尚未落库，直接用 descriptor 校验，
            // 与 gate_data_tasks 同规则；无 generic 任务直接放行）
            if let Some(acq) = &body.acquisition {
                if acq
                    .tasks
                    .iter()
                    .any(|t| t.binding.kind == mesa_core_types::GENERIC_BINDING_KIND)
                {
                    let mut parsed = Vec::new();
                    let mut issues = Vec::new();
                    for (i, task) in acq.tasks.iter().enumerate() {
                        if task.binding.kind != mesa_core_types::GENERIC_BINDING_KIND {
                            continue;
                        }
                        match mesa_core_types::GenericBinding::from_json(&task.binding.config) {
                            Ok(b) => parsed.push((i, b)),
                            Err(e) => issues.push(mesa_core_types::ValidationIssue {
                                path: format!("acquisition.tasks[{i}].binding"),
                                code: "INVALID_BINDING_CONFIG".into(),
                                message: e,
                            }),
                        }
                    }
                    if !issues.is_empty() {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({ "valid": false, "issues": issues })),
                        );
                    }
                    let roots: Vec<String> = parsed
                        .iter()
                        .map(|(i, _)| format!("acquisition.tasks[{i}].selections"))
                        .collect();
                    let inputs: Vec<(
                        &mesa_core_types::TaskMode,
                        &[mesa_core_types::ResourceSelection],
                        &str,
                    )> = parsed
                        .iter()
                        .zip(roots.iter())
                        .map(|((i, b), r)| (&acq.tasks[*i].mode, &b.selections[..], r.as_str()))
                        .collect();
                    issues.extend(mesa_core_types::validate_task_set_against(&desc, &inputs));
                    if !issues.is_empty() {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({ "valid": false, "issues": issues })),
                        );
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

    // 2) 单事务落库（secrets 明文由 store 层在事务内加密落库，
    //    与 create_endpoint_with_secrets 同语义）
    let device_rec = DeviceRecord {
        id: body.device.id.clone(),
        name: body.device.name.clone(),
    };
    let endpoint_rec = EndpointRecord {
        id: body.endpoint.id.clone(),
        name: body.endpoint.name.clone(),
        device_id: body.device.id.clone(),
        driver_id: body.endpoint.driver_id.clone(),
        connection_json: serde_json::to_string(&conn_val).unwrap(),
        desired_running: body.start.unwrap_or(true),
        updated_at_ns: mesa_core_types::now_unix_ns(),
    };
    let tasks: &[mesa_core_types::AcquisitionTask] = body
        .acquisition
        .as_ref()
        .map(|a| a.tasks.as_slice())
        .unwrap_or(&[]);
    let revision = match state.store.bootstrap_device_tx_with_plaintext(
        &device_rec,
        &endpoint_rec,
        &secrets_plain,
        tasks,
    ) {
        Ok(r) => r,
        Err(e) => return store_err_to_response(e),
    };

    // 3) start side effect（可选）
    let want_start = body.start.unwrap_or(true);
    let mut start_failed: Option<String> = None;
    let mut compensated = false;
    if want_start {
        if let Err(m) = start_created_endpoint(&state, &endpoint_rec.id).await {
            // 补偿：删 endpoint → device（顺序不可反，device 有 RESTRICT）。
            // R1.1：补偿结果必须检查——失败也写 compensated:true 是谎报；
            // 如实报告，调用方凭 device_id/endpoint_id 定位残留。
            match state
                .store
                .bootstrap_compensate(&device_rec.id, &endpoint_rec.id)
            {
                Ok(()) => {
                    compensated = true;
                    tracing::warn!(
                        device_id = %device_rec.id,
                        endpoint_id = %endpoint_rec.id,
                        reason = %m,
                        "device-bootstrap start 失败，已补偿删除"
                    );
                }
                Err(e) => {
                    compensated = false;
                    tracing::error!(
                        device_id = %device_rec.id,
                        endpoint_id = %endpoint_rec.id,
                        reason = %m,
                        compensate_error = %e,
                        "device-bootstrap start 失败且补偿删除失败，存在残留，需人工处理"
                    );
                }
            }
            state.snapshot.remove_endpoint(&endpoint_rec.id);
            start_failed = Some(m);
        }
    }
    // 注：start=false 时 desired_running 已在事务内写 false，无需再写一次。

    if let Some(m) = start_failed {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": { "code": "START_FAILED", "message": m },
                "device_id": device_rec.id,
                "endpoint_id": endpoint_rec.id,
                "compensated": compensated,
            })),
        );
    }

    let result = serde_json::json!({
        "device_id": device_rec.id,
        "endpoint_id": endpoint_rec.id,
        "revision": revision,
        "started": want_start,
    });
    // 幂等语义（R1.1 明确）：只记成功结果。失败（400/409/502/503）不记——
    // 调用方重试即重新执行，事务回滚/补偿删除保证无残留；start 失败多为运行
    // 时状态问题，稍后重试可能成功，重放旧失败会藏掉恢复机会。
    if let Some(key) = &idem_key {
        let result_str = serde_json::to_string(&result).unwrap_or_default();
        if let Err(e) = state.store.bootstrap_idempotency_put(
            key,
            &fingerprint,
            &device_rec.id,
            &endpoint_rec.id,
            &result_str,
        ) {
            // 幂等记录写失败不影响本次成功（设备已建好并启动）；记日志，
            // 调用方重试同 key 会重新执行到 device Duplicate 409（无双建）。
            tracing::warn!(
                device_id = %device_rec.id,
                endpoint_id = %endpoint_rec.id,
                error = %e,
                "device-bootstrap 成功但幂等记录写入失败，重试可能返回 409 而非重放"
            );
        }
    }
    tracing::info!(
        device_id = %device_rec.id,
        endpoint_id = %endpoint_rec.id,
        revision = revision,
        started = want_start,
        "device-bootstrap 成功"
    );
    (StatusCode::CREATED, Json(result))
}
