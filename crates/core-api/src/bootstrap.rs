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
use std::time::Duration;

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

/// 启动 side effect 的失败分类（R-1：**必须**区分这两种）。
///
/// 二者在调用方看来都是"start 失败"，但副作用完全不同：
/// - `NotStarted`：没有任何 attempt 被创建 ⇒ 直接补偿删除是安全的；
/// - `StartedButIntentNotPersisted`：`commit_start` 已经 `start_endpoint` 成功，
///   只是随后的 `desired_running=true` 落盘失败。此时**驱动任务正在执行**，
///   若直接删配置就会留下"配置已删除、驱动仍在跑"的孤儿 ⇒ 必须先停止 attempt。
enum StartFailure {
    NotStarted(String),
    StartedButIntentNotPersisted(String),
}

impl StartFailure {
    fn message(&self) -> &str {
        match self {
            StartFailure::NotStarted(m) | StartFailure::StartedButIntentNotPersisted(m) => m,
        }
    }

    fn started(&self) -> bool {
        matches!(self, StartFailure::StartedButIntentNotPersisted(_))
    }

    fn code(&self) -> &'static str {
        match self {
            StartFailure::NotStarted(_) => "START_FAILED",
            StartFailure::StartedButIntentNotPersisted(_) => "START_INTENT_NOT_PERSISTED",
        }
    }
}

/// 启动 side effect（`start_endpoint` 的核心逻辑抽出：DB 已落库后执行）。
/// 失败返回 [`StartFailure`]（调用方据此决定"直接补偿"还是"先停再补偿"）。
///
/// C1 P0：连接构造（Descriptor / Secret 物化 / 任务门禁）**不得**在本文件里
/// 再实现一遍——它与 REST Start、Mesad 开机恢复共用
/// [`super::runtime::build_runtime_endpoint`]，否则三条入口的 Secret 语义会
/// 再次分叉。
///
/// G7 §4-③：bootstrap 的"建档即启动"与 REST Start 同构，**必须一并接入门**，
/// 否则它同样能覆盖并发的 Stop（W1）。因此这里走 `admit_start` →
/// `build_runtime_endpoint` → `commit_start` 同一条两阶段路径：
/// - 构造期间 Stop 被受理 ⇒ `SUPERSEDED_BY_STOP`（启动失败，走补偿删除）；
/// - 旧 attempt 在拆解 ⇒ `STOP_IN_PROGRESS`（不排队）；
/// - `desired_running=true` 只在 commit 成功后由门写入（§6.10.2 第 6 条），
///   建档事务里一律写 false（"刚建好 = 未运行"，第 5 条）。
async fn start_created_endpoint(state: &AppState, endpoint_id: &str) -> Result<(), StartFailure> {
    let ticket = state.lifecycle.admit_start(endpoint_id);
    let cfg = super::runtime::build_runtime_endpoint(state, endpoint_id)
        .await
        .map_err(|e| StartFailure::NotStarted(e.to_string()))?;
    match state.lifecycle.commit_start(&ticket, cfg) {
        Ok(()) => Ok(()),
        // R-1：这是唯一一种"启动已发生"的失败。attempt 已在跑，不能当普通失败补偿。
        Err(e) if e.code == "START_INTENT_NOT_PERSISTED" => Err(
            StartFailure::StartedButIntentNotPersisted(format!("{}: {}", e.code, e.message)),
        ),
        Err(e) => Err(StartFailure::NotStarted(format!(
            "{}: {}",
            e.code, e.message
        ))),
    }
}

/// R-1：停止"启动已发生"的 attempt，返回是否**确认**已无活 attempt。
///
/// 这里直接走 `manager.stop_endpoint` 而不是门：门在 `commit_start` 失败后已经
/// 把该端点标成 `INTENT_UNPERSISTED`（503），再经门只会拿到 503 而不是停止。
/// 停止动作本身不依赖 DB（安全方向），因此不受落盘失败影响。
///
/// 返回值语义：**只有确认条目已移除才算"停住了"**。超时/失败一律按"未确认"，
/// 调用方据此保留配置（宁可留一个可见的端点，也不留一个无主的驱动进程）。
async fn stop_started_attempt(state: &AppState, endpoint_id: &str) -> bool {
    match tokio::time::timeout(
        Duration::from_secs(30),
        state.manager.stop_endpoint(endpoint_id),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::error!(
            endpoint = %endpoint_id, error = %e,
            "bootstrap 补偿前停止已启动 attempt 失败"
        ),
        Err(_) => tracing::error!(
            endpoint = %endpoint_id,
            "bootstrap 补偿前停止已启动 attempt 超时（30s）"
        ),
    }
    !state.manager.has_live_attempt(endpoint_id)
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
            // 与 gate_data_tasks **同源**：Task Binding Kind 路由、绑定解析与
            // Endpoint-wide point_key 唯一性全部走 `binding_gate` 的唯一实现，
            // 不得在此再写第二份白名单——否则会出现「保存通过、bootstrap 拒绝」）。
            if let Some(acq) = &body.acquisition {
                if !acq.tasks.is_empty() {
                    // ① Task Binding Kind 路由 + nck.points.v1 信封层校验。
                    let (route_issues, nck_bindings) =
                        super::binding_gate::route_data_task_kinds(&acq.tasks, "acquisition.tasks");
                    if !route_issues.is_empty() {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({ "valid": false, "issues": route_issues })),
                        );
                    }
                    // ② 解析 mesa.resources.v1 绑定。
                    let (mut issues, parsed) = super::binding_gate::parse_generic_bindings(
                        &acq.tasks,
                        "acquisition.tasks",
                    );
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
                    let modes: Vec<mesa_core_types::TaskMode> =
                        parsed.iter().map(|(i, _)| acq.tasks[*i].mode()).collect();
                    let inputs: Vec<(
                        &mesa_core_types::TaskMode,
                        &[mesa_core_types::ResourceSelection],
                        &str,
                    )> = parsed
                        .iter()
                        .zip(roots.iter())
                        .zip(modes.iter())
                        .map(|(((_i, b), r), m)| (m, &b.selections[..], r.as_str()))
                        .collect();
                    // ③ resources 集合级语义校验。
                    issues.extend(mesa_core_types::validate_task_set_against(&desc, &inputs));
                    // ④ 跨 kind 的 Endpoint-wide point_key 唯一性。
                    issues.extend(super::binding_gate::cross_kind_point_key_issues(
                        &parsed,
                        &nck_bindings,
                        "acquisition.tasks",
                    ));
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
        // G7 §6.10.2 第 5 条：建档一律写 `false`（刚建好 = 未运行）。
        // `start=true` 的启动意图由 `commit_start` 在**启动成功后**写入，
        // 收益：DB 里不再可能出现"从未经过门却为 true"的行（否则一次并发 Stop
        // 会被建档时的 true 覆盖回运行意图）。
        desired_running: false,
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
    let mut start_failed_code = "START_FAILED";
    let mut compensated = false;
    let mut attempt_stopped = false;
    if want_start {
        if let Err(failure) = start_created_endpoint(&state, &endpoint_rec.id).await {
            let started = failure.started();
            let m = failure.message().to_string();
            start_failed_code = failure.code();
            // R-1：**启动已发生**时不得直接删配置——驱动任务还在跑。
            // 先停掉已启动的 attempt，并且只有"确认没有活 attempt"才允许补偿删除，
            // 否则保留配置并如实报告（配置在、驱动在，至少不会变成无主进程）。
            let mut may_delete = true;
            if started {
                attempt_stopped = stop_started_attempt(&state, &endpoint_rec.id).await;
                if !attempt_stopped {
                    may_delete = false;
                    tracing::error!(
                        device_id = %device_rec.id,
                        endpoint_id = %endpoint_rec.id,
                        reason = %m,
                        "device-bootstrap 启动意图未持久化且停止未确认；保留配置，禁止补偿删除"
                    );
                }
            }
            if may_delete {
                // 补偿：删 endpoint → device（顺序不可反，device 有 RESTRICT）。
                // R1.1：补偿结果必须检查——失败也写 compensated:true 是谎报；
                // 如实报告，调用方凭 device_id/endpoint_id 定位残留。
                match state
                    .store
                    .bootstrap_compensate(&device_rec.id, &endpoint_rec.id)
                {
                    Ok(()) => {
                        compensated = true;
                        // 端点已不存在，围栏（若在启动意图落盘失败时写入）已无意义；
                        // 不清除会在重启时留下指向不存在端点的永久围栏项。
                        let _ = state.store.clear_stop_intent_unpersisted(&endpoint_rec.id);
                        tracing::warn!(
                            device_id = %device_rec.id,
                            endpoint_id = %endpoint_rec.id,
                            reason = %m,
                            attempt_stopped,
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
                state
                    .snapshot
                    .remove_endpoint(&endpoint_rec.id, state.manager.gen_floor(&endpoint_rec.id));
            }
            start_failed = Some(m);
        }
    }
    // 注：start=false 时 desired_running 已在事务内写 false，无需再写一次；
    // start=true 的 true 由 commit_start 写入（见 start_created_endpoint）。

    if let Some(m) = start_failed {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": { "code": start_failed_code, "message": m },
                "device_id": device_rec.id,
                "endpoint_id": endpoint_rec.id,
                "compensated": compensated,
                // R-1：调用方可据此区分"启动已发生、但已先停掉"与"启动前就失败"。
                "attempt_stopped": attempt_stopped,
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
