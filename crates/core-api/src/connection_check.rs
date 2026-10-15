//! C3-A：Endpoint-bound 连接检查
//! （`POST /api/v1/endpoints/{endpoint_id}/connection-check`）。
//!
//! 冻结契约见 `docs/design/c3-endpoint-connection-check.md`（重点 §3/§4/§5）。
//!
//! # 为什么不能复用 `runtime::build_runtime_endpoint()`
//!
//! 该函数读的是**已持久化**的完整 Endpoint 配置，并施加 Data/Event Tasks 门禁。
//! C3 检查的对象是用户**尚未保存的连接草稿**，直接复用会同时犯两个错：
//! ① 测的是旧参数而不是用户刚改的草稿；② 已保存任务的合法性会干扰「只检查
//! 连接」这件事。因此这里只复用 C1 的 Secret 物化底层原语
//! （[`materialize_secret_markers_in_place`] / [`read_stored_secret`]，坏 JSON
//! 上 fail-closed），并沿用 core-types 的统一 Schema 校验
//! （[`mesa_core_types::SchemaDescriptor::validate_instance`]）——不新建
//! SecretStore、通用 Resolver 框架，也不引入第二套连接模型。
//!
//! # 无副作用（冻结）
//!
//! 只读 ConfigStore 与 Descriptor：不写 `connection_json` / `endpoint_secrets` /
//! `desired_running` / 任务 / 运行态，不启停 Endpoint，不连接真实设备。
//! 明文只允许在本进程内存中短暂存在（C3-B 交给临时 Driver 用），**绝不**进入
//! HTTP 响应体、`tracing` 日志或错误文本。

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use mesa_config_store::{ConfigStore, StoreError};
use mesa_core_types::SchemaDescriptor;
use serde::Deserialize;

use crate::{
    AppState, SecretRefError, json_error, materialize_secret_markers_in_place, secret_field_keys,
};

// ---------------------------------------------------------------------------
// 请求形状
// ---------------------------------------------------------------------------

/// 检查模式（冻结）：`validate` 只校验、**不连设备**；`probe` 走临时探测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CheckMode {
    Validate,
    Probe,
}

/// 请求体（冻结形状）：`{"mode": "validate"|"probe", "connection": {...}}`。
///
/// `deny_unknown_fields` 在这里是**安全边界**而不是风格选择：`endpoint_id` 与
/// `driver_id` 都只能来自 URL 指向的 Endpoint 已保存记录；请求体里出现
/// `driver_id`（或任何其他字段）一律 400。否则调用方可以拿 A 设备已保存的
/// 凭据去检查 B 驱动，Endpoints 的归属约束形同虚设。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionCheckReq {
    mode: CheckMode,
    /// 尚未保存的连接草稿（Descriptor 语义下的 connection 对象）。
    connection: serde_json::Value,
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 连接检查失败（结构化，REST 直接映射状态码；禁止靠文本匹配判类型）。
#[derive(Debug)]
struct CheckError {
    status: StatusCode,
    code: String,
    message: String,
}

impl CheckError {
    fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }

    /// 转 REST 错误体（与既有 `json_error` 外形一致）。
    fn into_error_response(self) -> (StatusCode, Json<serde_json::Value>) {
        (self.status, Json(json_error(&self.code, &self.message)))
    }
}

/// 复用既有 `store_err_to_response` 的状态码/错误码口径，避免另立第二套映射表。
fn check_error_from_store(e: StoreError) -> CheckError {
    let (status, Json(body)) = crate::store_err_to_response(e);
    let err = body.get("error");
    CheckError::new(
        status,
        err.and_then(|e| e.get("code"))
            .and_then(|c| c.as_str())
            .unwrap_or("INTERNAL"),
        err.and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("存储读取失败"),
    )
}

// ---------------------------------------------------------------------------
// 草稿中的 Secret 形态（四种语义 + 非法形态）
// ---------------------------------------------------------------------------

/// 一个 Secret 字段在草稿/已保存记录中的取值形态。
#[derive(Debug, PartialEq, Eq)]
enum SecretValue {
    /// 明文（用户新输入，或历史记录中的明文残留）：保留在内存草稿里。
    Plaintext,
    /// `{"secret_set": true}`：引用**该 Endpoint、该字段**的已存 Secret。
    Reference,
    /// 明确清除：`{"clear_secret": true}` / 空字符串 / `null`。
    Clear,
    /// 非法形态或非法 marker：400 拒绝，**绝不**原样透传给 Driver。
    Invalid(&'static str),
}

/// 严格判定 Secret 字段上的取值形态。
///
/// # 为什么必须严格
///
/// marker 只允许两种规范形态。任何「长得像 marker 但不是」的值
/// （`{"secret_set": "yes"}`、`{"secret_set": true, "x": 1}`、
/// `{"secret_set": false}`、`{"clear_secret": true, "secret_set": true}`、
/// 裸对象、数字、数组）都必须**拒绝**而不是猜意图：猜错的代价要么是把它当
/// 明文发给 Driver（把 marker 当密码去连设备），要么是把「引用凭据」误判成
/// 「清除凭据」。两者都是安全缺陷，没有第三种可接受结果。
///
/// # 为什么 `null` / `""` 算「明确清除」
///
/// C2 前端对「空值」的定义是 `isEmptyConnectionValue`（`undefined`/`null`/`""`），
/// 其含义就是清除意图。契约 §2 明令「用户明确清除 Secret → 不允许偷偷回退到
/// 旧密码」；若把 `null`/`""` 当成「未提供」去继承旧值，正好触发这条被禁止的
/// 回退。因此这里把三者统一按**清除**处理（清除优先级最高，见
/// [`resolve_connection_draft`]）。
fn classify_secret_value(v: &serde_json::Value) -> SecretValue {
    match v {
        serde_json::Value::String(s) if s.is_empty() => SecretValue::Clear,
        serde_json::Value::String(_) => SecretValue::Plaintext,
        serde_json::Value::Null => SecretValue::Clear,
        serde_json::Value::Object(m) => {
            let has_set = m.contains_key("secret_set");
            let has_clear = m.contains_key("clear_secret");
            if has_set && has_clear {
                return SecretValue::Invalid("secret_set 与 clear_secret 不得同时出现");
            }
            if has_set {
                if m.len() == 1 && m.get("secret_set") == Some(&serde_json::Value::Bool(true)) {
                    SecretValue::Reference
                } else {
                    SecretValue::Invalid("secret_set 只允许 { \"secret_set\": true } 这一种形状")
                }
            } else if has_clear {
                if m.len() == 1 && m.get("clear_secret") == Some(&serde_json::Value::Bool(true)) {
                    SecretValue::Clear
                } else {
                    SecretValue::Invalid(
                        "clear_secret 只允许 { \"clear_secret\": true } 这一种形状",
                    )
                }
            } else {
                // 非 marker 的裸对象不可能是凭据：明确拒绝，不猜它是不是「某个
                // 未来的凭据结构」。
                SecretValue::Invalid("Secret 字段只接受明文字符串或 secret_set/clear_secret 标记")
            }
        }
        _ => SecretValue::Invalid("Secret 字段只接受明文字符串或 secret_set/clear_secret 标记"),
    }
}

/// 递归探测 marker 形状（深度受限），与 C2 前端 `containsSecretMarker` 同一判据。
///
/// 为什么按**形状**而不是字段名判断：凭据字段命名会演进
/// （`password` / `api_token` / `credentials` …），白名单必然漏。只要值里出现
/// marker 形状，它就不是可用明文，**绝不能**进入 Driver 的 `OpenConnection`
/// 或 Probe RPC。落在非 Secret 字段上的 marker 只有一种可能——调用方搞错了
/// 字段声明或前端把 marker 放错了位置——必须 400，而不是交给 Driver 去猜。
fn contains_marker_shape(v: &serde_json::Value, depth: usize) -> bool {
    // 深度上限与前端一致：Schema 没有对象类型，正常配置不可能有这么深的嵌套；
    // 超深结构只可能是异常输入，不再往下追（不构成放行条件，因为外层 marker
    // 已在上层被判出）。
    if depth > 4 {
        return false;
    }
    match v {
        serde_json::Value::Object(m) => {
            if m.contains_key("secret_set") || m.contains_key("clear_secret") {
                return true;
            }
            m.values().any(|x| contains_marker_shape(x, depth + 1))
        }
        serde_json::Value::Array(a) => a.iter().any(|x| contains_marker_shape(x, depth + 1)),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// 草稿解析
// ---------------------------------------------------------------------------

/// 解析结果。
///
/// `connection` 是**含明文**的完整连接配置，只允许在本进程内存中短暂存在
/// （C3-B 交给临时 Driver 进程用），不得序列化进响应或日志。
/// 其余字段只记录**字段名**，用于日志与断言，不含任何凭据内容。
struct ResolvedDraft {
    connection: serde_json::Value,
    /// 本次由请求体明文覆盖的 Secret 字段名。
    inline_fields: Vec<String>,
    /// 本次从 SecretStore 物化的 Secret 字段名（含继承而来的引用）。
    stored_fields: Vec<String>,
    /// 本次被明确清除的 Secret 字段名。
    cleared_fields: Vec<String>,
}

/// 手写 `Debug`：`connection` 可能含**明文凭据**，绝不能经 `{:?}` 泄漏到测试
/// 失败输出、panic 消息或日志里。这里只暴露字段名与「已脱敏」这一事实。
impl std::fmt::Debug for ResolvedDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedDraft")
            .field("connection", &"[redacted]")
            .field("inline_fields", &self.inline_fields)
            .field("stored_fields", &self.stored_fields)
            .field("cleared_fields", &self.cleared_fields)
            .finish()
    }
}

/// 解析内存草稿，把 Secret 引用物化为明文（冻结 §4 四语义）。
///
/// 顺序本身承载语义：
/// 1. **先**做 marker 位置红线（只有 Descriptor 声明为 Secret 的字段能带标记）；
/// 2. **再**逐字段处理草稿里显式给出的 Secret 值——`clear` 分支在第一层就
///    把字段从草稿里删掉，因此**永远**走不到「读旧值」的代码，这是「明确清除
///    优先级最高」的实现方式，而不是靠后续判断补救；
/// 3. 草稿省略的 Secret 字段才按 PUT 契约继承旧值；
/// 4. 最后统一走 C1 的物化原语，所有引用（无论来自草稿还是继承）在同一处
///    解析，失败分类只有两种：引用无效（400）/ SecretStore 故障（503）。
///
/// NOTE（非 Secret 字段不做继承）：既有 PUT 契约对非 Secret 字段是**整体替换**
/// （见 C2 `preserveHiddenFields` 的注释与后端 `update_endpoint()`），草稿省略
/// 的可见字段应当由统一校验按 `required` 裁决，而不是由服务端悄悄拿旧值补上——
/// 否则「用户看到的草稿」与「被检查的连接」会不一致。缺省继承只适用于契约
/// §4 明确规定的 Secret 字段。
fn resolve_connection_draft(
    draft: &serde_json::Value,
    endpoint_id: &str,
    persisted_connection_json: &str,
    schema: &SchemaDescriptor,
    store: &ConfigStore,
) -> Result<ResolvedDraft, CheckError> {
    let Some(draft_obj) = draft.as_object() else {
        return Err(CheckError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            "connection 必须为 JSON 对象",
        ));
    };
    let secret_keys = secret_field_keys(schema);

    // ① marker 位置红线：非 Secret 字段上的 marker 形状一律拒绝。
    for (key, value) in draft_obj {
        if secret_keys.iter().any(|k| k == key) {
            continue;
        }
        if contains_marker_shape(value, 0) {
            return Err(CheckError::new(
                StatusCode::BAD_REQUEST,
                "INVALID_SECRET_MARKER",
                format!("field `{key}` 不是 Secret 字段，不得携带 secret_set/clear_secret 标记"),
            ));
        }
    }

    let mut resolved = draft.clone();
    let mut inline_fields = Vec::new();
    let mut cleared_fields = Vec::new();

    let obj = resolved
        .as_object_mut()
        .expect("connection 已确认是对象，clone 后仍是对象");

    // ② 草稿显式给出的 Secret 字段：清除优先，绝不到「继承」分支取旧值。
    for key in &secret_keys {
        let Some(cur) = obj.get(key).cloned() else {
            continue;
        };
        match classify_secret_value(&cur) {
            // 新明文只用于本次检查：这里**只**保留在内存草稿里，不调用任何
            // 写入接口，SecretStore 内容不变（C3-05）。
            SecretValue::Plaintext => inline_fields.push(key.clone()),
            // 引用保留为 marker，交给 ④ 的统一物化处理。
            SecretValue::Reference => {}
            SecretValue::Clear => {
                obj.remove(key);
                cleared_fields.push(key.clone());
            }
            SecretValue::Invalid(reason) => {
                return Err(CheckError::new(
                    StatusCode::BAD_REQUEST,
                    "INVALID_SECRET_MARKER",
                    format!("field `{key}`: {reason}"),
                ));
            }
        }
    }

    // ③ 草稿省略的 Secret 字段：按 PUT 契约继承旧值（SecretStore 优先；兼容
    //    历史记录里的明文残留）。无旧值就保持缺失，由统一校验按 required 裁决
    //    （必填凭据缺失 → REQUIRED → 400）。
    //
    //    被 ② 明确清除的字段**不参与**继承：清除是用户的显式意图，若允许它从
    //    已保存记录里回填，就等于「用户清了密码，服务端偷偷拿旧密码去连」——
    //    契约 §2/§4 明令禁止。这一条是「清除优先」真正落地的地方。
    let missing: Vec<&String> = secret_keys
        .iter()
        .filter(|k| !obj.contains_key(*k) && !cleared_fields.contains(*k))
        .collect();
    if !missing.is_empty() {
        // 只有确实需要继承时才解析已保存记录：草稿完整时它不参与检查，
        // 一份损坏的历史记录也就不该拦下「用户已经填全」的检查。
        let persisted = serde_json::from_str::<serde_json::Value>(persisted_connection_json)
            .ok()
            .filter(serde_json::Value::is_object);
        if let Some(persisted_obj) = persisted.as_ref().and_then(|p| p.as_object()) {
            for key in missing {
                let Some(v) = persisted_obj.get(key) else {
                    continue;
                };
                match classify_secret_value(v) {
                    SecretValue::Plaintext => {
                        obj.insert(key.clone(), v.clone());
                    }
                    // 历史记录里的引用同样是「必须能解析」的引用：读不到就
                    // 明确失败，绝不退化成「本次不带凭据去连」。
                    SecretValue::Reference => {
                        obj.insert(key.clone(), v.clone());
                    }
                    // 旧记录本身就是空的 → 没有可继承的值。
                    SecretValue::Clear => {}
                    SecretValue::Invalid(reason) => {
                        return Err(CheckError::new(
                            StatusCode::BAD_REQUEST,
                            "INVALID_SECRET_MARKER",
                            format!("field `{key}`（已保存记录）: {reason}"),
                        ));
                    }
                }
            }
        }
    }

    // ④ 统一物化：所有残留 marker 走 C1 同一原语（仅内存）。
    let stored_fields =
        match materialize_secret_markers_in_place(obj, endpoint_id, store, &secret_keys) {
            Ok(names) => names,
            // 引用无效（该 Endpoint 上没有这条 Secret）→ 400：界面上显示「已保存
            // 凭据」而服务端查无记录，必须明说，不能悄悄换成匿名连接。
            Err(SecretRefError::Missing(key)) => {
                return Err(CheckError::new(
                    StatusCode::BAD_REQUEST,
                    "SECRET_NOT_FOUND",
                    format!("field `{key}` 引用的已存 Secret 不存在（endpoint `{endpoint_id}`）"),
                ));
            }
            // SecretStore 故障 → 503（红线：不得降级为匿名连接，也不得显示成设备不可达）。
            Err(SecretRefError::Store(e)) => {
                return Err(CheckError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "SECRET_STORE_UNAVAILABLE",
                    format!("SecretStore 不可用，拒绝以不明凭据状态检查连接: {e}"),
                ));
            }
        };

    // 继承而来的 marker 无需在这里补记：③ 已把它们写进对象，④ 的统一物化会
    // 一并返回字段名，因此 `stored_fields` 天然覆盖「草稿引用」与「继承引用」。
    Ok(ResolvedDraft {
        connection: resolved,
        inline_fields,
        stored_fields,
        cleared_fields,
    })
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /api/v1/endpoints/{endpoint_id}/connection-check`
///
/// 状态码口径（冻结 §5）：200 校验通过；400 参数/Secret 引用/必填凭据问题；
/// 404 Endpoint 不存在；501 `probe` 待 C3-B 接入（Driver 不支持探测同码）；
/// 503 Driver / SecretStore / 探测基础设施不可用；504 探测超时（C3-B）。
pub(crate) async fn connection_check(
    State(state): State<Arc<AppState>>,
    Path(endpoint_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 形状优先：`driver_id` 等额外字段在这里就被拒（deny_unknown_fields），
    // 因此不存在「请求体覆盖 driver_id」的代码路径。
    let req: ConnectionCheckReq = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => {
            return CheckError::new(
                StatusCode::BAD_REQUEST,
                "VALIDATION_ERROR",
                format!("请求体形状非法: {e}"),
            )
            .into_error_response();
        }
    };
    run_check(&state, &endpoint_id, req).await
}

async fn run_check(
    state: &AppState,
    endpoint_id: &str,
    req: ConnectionCheckReq,
) -> (StatusCode, Json<serde_json::Value>) {
    if !req.connection.is_object() {
        return CheckError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            "connection 必须为 JSON 对象",
        )
        .into_error_response();
    }

    // ① Endpoint 只按 URL 里的 ID 查；不存在即 404（不猜测、不按名字匹配）。
    let rec = match state.store.get_endpoint(endpoint_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return CheckError::new(
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                format!("endpoint `{endpoint_id}` not found"),
            )
            .into_error_response();
        }
        Err(e) => return check_error_from_store(e).into_error_response(),
    };

    // ② driver_id 只读**已保存记录**；Descriptor 不可用即 503（不猜 Schema，
    //    也就不可能在没有 Descriptor 的情况下判断哪些字段是 Secret）。
    let desc = match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(d) => d,
        Err(e) => {
            return CheckError::new(StatusCode::SERVICE_UNAVAILABLE, e.code, e.message)
                .into_error_response();
        }
    };

    // ③ 内存草稿解析：Secret 四语义 + marker 位置红线。
    let resolved = match resolve_connection_draft(
        &req.connection,
        endpoint_id,
        &rec.connection_json,
        &desc.connection,
        &state.store,
    ) {
        Ok(r) => r,
        Err(e) => return e.into_error_response(),
    };

    // ④ 统一 Schema 校验（core-types 唯一实现，与 C2/PUT 同一套语义）。
    //    跑在**已物化**的逻辑值上，因此 Secret 的 pattern/enum 规则同样生效；
    //    Secret 值在 issue 文本里统一显示为 [redacted]（core-types 保证）。
    //    这一步不做任何设备 I/O：`validate` 模式因此天然「零 Probe」。
    let issues = desc
        .connection
        .validate_instance("connection", &resolved.connection);
    if !issues.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "valid": false, "issues": issues })),
        );
    }

    tracing::debug!(
        endpoint = %endpoint_id,
        driver = %rec.driver_id,
        mode = ?req.mode,
        inline_secret_fields = ?resolved.inline_fields,
        stored_secret_fields = ?resolved.stored_fields,
        cleared_secret_fields = ?resolved.cleared_fields,
        "endpoint connection check"
    );

    match req.mode {
        // `validate`：仅校验，不连接设备。响应形状与既有
        // `POST /api/v1/drivers/{id}/validate-connection` 对齐（§3 不另立模型）。
        CheckMode::Validate => (
            StatusCode::OK,
            Json(serde_json::json!({ "valid": true, "issues": [] })),
        ),
        // TODO(C3-B)：接入 `MesaManager::probe()` 的**临时**探测（复用 C1 分层
        // 预算），并回归「凭据归属、超时、进程回收、对持久化状态零影响」。
        //
        // 在那之前这里必须明确返回「未实现」，绝不返回模拟成功：C3-B 未落地时
        // 任何 `reachable` 结论都是假的。也**绝不**改道既有的
        // `POST /api/v1/drivers/{id}/probe`——那条路径不接受 Secret marker，
        // 改道等于绕过本接口刚建立的 Secret 安全边界。
        CheckMode::Probe => CheckError::new(
            StatusCode::NOT_IMPLEMENTED,
            "PROBE_NOT_IMPLEMENTED",
            "mode=probe 的临时探测尚未接入（待 C3-B）；本轮不返回模拟结果",
        )
        .into_error_response(),
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mesa_config_store::{DeviceRecord, EndpointRecord};
    use mesa_core_types::{FieldDescriptor, FieldType};

    /// 测试用 Schema：`endpoint_url` 必填 + `password` Secret（required 可控）。
    ///
    /// 为什么自带 Schema 而不是用真 Driver：C3-A 的 Secret 四语义是**服务端
    /// 逻辑**，与协议无关；用真实驱动反而把断言绑在某个驱动的字段清单上。
    /// `required=true` 的形态（必填凭据被清除 → 400）在现有四个驱动里并不存在
    /// （OPC UA password 是可选的），只能在这里构造。
    fn test_schema(secret_required: bool) -> SchemaDescriptor {
        SchemaDescriptor::new(vec![
            FieldDescriptor::new("endpoint_url", "Endpoint URL", FieldType::Url).required(true),
            FieldDescriptor::new("password", "Password", FieldType::Secret)
                .required(secret_required),
        ])
    }

    /// 内存库 + 一个 Device + 一个 Endpoint（`connection_json` 按需给定）。
    fn store_with_endpoint(id: &str, connection_json: &str) -> ConfigStore {
        let store = ConfigStore::open_in_memory().unwrap();
        store
            .create_device(&DeviceRecord {
                id: "d1".into(),
                name: "D".into(),
            })
            .unwrap();
        store
            .create_endpoint(&EndpointRecord {
                id: id.into(),
                name: id.into(),
                device_id: "d1".into(),
                driver_id: "opcua".into(),
                connection_json: connection_json.into(),
                desired_running: false,
                updated_at_ns: 0,
            })
            .unwrap();
        store
    }

    // -----------------------------------------------------------------------
    // 语义 1/2：marker 读取当前 Endpoint 的明文；草稿优先于持久化配置
    // -----------------------------------------------------------------------

    /// 验收 1：用**当前草稿**校验，而不是已持久化的旧参数。
    /// 验收 2：`{"secret_set": true}` 解析为**该 Endpoint** 的明文。
    #[test]
    fn draft_wins_over_persisted_and_marker_resolves_stored_plaintext() {
        let store = store_with_endpoint(
            "e1",
            r#"{"endpoint_url":"opc.tcp://old:4840","password":{"secret_set":true}}"#,
        );
        store
            .put_secret("e1", "password", "pw-1", "master")
            .unwrap();

        let out = resolve_connection_draft(
            &serde_json::json!({
                "endpoint_url": "opc.tcp://new:4840",
                "password": {"secret_set": true},
            }),
            "e1",
            r#"{"endpoint_url":"opc.tcp://old:4840","password":{"secret_set":true}}"#,
            &test_schema(false),
            &store,
        )
        .expect("引用有效 + 草稿合法，必须解析成功");

        assert_eq!(
            out.connection["endpoint_url"], "opc.tcp://new:4840",
            "必须是本次草稿的 URL，而不是持久化里的旧值"
        );
        assert_eq!(
            out.connection["password"], "pw-1",
            "marker 必须物化为该 Endpoint 的明文"
        );
        assert_eq!(out.stored_fields, vec!["password".to_string()]);
        assert!(out.inline_fields.is_empty());
        // 物化只在内存：持久化行仍然是 marker。
        let rec = store.get_endpoint("e1").unwrap().unwrap();
        assert!(rec.connection_json.contains("secret_set"));
        assert!(!rec.connection_json.contains("pw-1"));
    }

    // -----------------------------------------------------------------------
    // C3-07：跨 Endpoint 隔离
    // -----------------------------------------------------------------------

    /// 验收 3（C3-07）：同一驱动、同一 `field_path` 的两个 Endpoint 各读各的
    /// 凭据；另一个 Endpoint 没有该 Secret 时，marker 必须报引用无效。
    #[test]
    fn secrets_are_never_read_across_endpoints() {
        let store = store_with_endpoint("e1", r#"{"password":{"secret_set":true}}"#);
        store
            .create_endpoint(&EndpointRecord {
                id: "e2".into(),
                name: "e2".into(),
                device_id: "d1".into(),
                driver_id: "opcua".into(),
                connection_json: r#"{"password":{"secret_set":true}}"#.into(),
                desired_running: false,
                updated_at_ns: 0,
            })
            .unwrap();
        store
            .put_secret("e1", "password", "pw-of-e1", "master")
            .unwrap();
        store
            .put_secret("e2", "password", "pw-of-e2", "master")
            .unwrap();

        let schema = test_schema(true);
        let d = serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"secret_set": true}});
        let a = resolve_connection_draft(&d, "e1", "{}", &schema, &store).unwrap();
        let b = resolve_connection_draft(&d, "e2", "{}", &schema, &store).unwrap();
        assert_eq!(a.connection["password"], "pw-of-e1");
        assert_eq!(b.connection["password"], "pw-of-e2");

        // schema 层面同形，但 SecretStore 里没有 e3 的记录 → 必须明确失败，
        // 绝不能「按 field_path 全局兜底」拿到 e1/e2 的密码。
        let err = resolve_connection_draft(&d, "e3", "{}", &schema, &store).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, "SECRET_NOT_FOUND");
        assert!(
            !err.message.contains("pw-of-e1") && !err.message.contains("pw-of-e2"),
            "错误文本不得回显任何 Endpoint 的明文: {}",
            err.message
        );
    }

    // -----------------------------------------------------------------------
    // C3-05：新明文只影响本次检查
    // -----------------------------------------------------------------------

    /// 验收 4（C3-05）：用户新输入的明文只用于本次检查，**不更新** SecretStore。
    #[test]
    fn inline_plaintext_is_used_but_never_written_back() {
        let store = store_with_endpoint("e1", r#"{"password":{"secret_set":true}}"#);
        store
            .put_secret("e1", "password", "old-pw", "master")
            .unwrap();
        let before_record = store.get_endpoint("e1").unwrap().unwrap();
        let before_fields = store.list_secret_fields("e1").unwrap();
        let before_secret = store.get_secret("e1", "password").unwrap();

        let out = resolve_connection_draft(
            &serde_json::json!({
                "endpoint_url": "opc.tcp://a:4840",
                "password": "new-pw",
            }),
            "e1",
            &before_record.connection_json,
            &test_schema(false),
            &store,
        )
        .unwrap();

        assert_eq!(out.connection["password"], "new-pw", "本次检查必须用新明文");
        assert_eq!(out.inline_fields, vec!["password".to_string()]);
        // SecretStore 与持久化记录**逐字节**不变。
        assert_eq!(store.get_secret("e1", "password").unwrap(), before_secret);
        assert_eq!(
            store.get_secret("e1", "password").unwrap().as_deref(),
            Some("old-pw")
        );
        assert_eq!(store.list_secret_fields("e1").unwrap(), before_fields);
        assert_eq!(store.get_endpoint("e1").unwrap().unwrap(), before_record);
    }

    // -----------------------------------------------------------------------
    // 红线 1：明确清除优先
    // -----------------------------------------------------------------------

    /// 验收 5（红线 1）：`clear_secret` / 空字符串 / `null` 都表示明确清除，
    /// 一律不得继承旧值；必填凭据被清除后由统一校验报 REQUIRED（→ 400）。
    #[test]
    fn clear_never_falls_back_to_stored_secret() {
        let persisted = r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#;
        let store = store_with_endpoint("e1", persisted);
        store
            .put_secret("e1", "password", "old-pw", "master")
            .unwrap();

        for clear_form in [
            serde_json::json!({"clear_secret": true}),
            serde_json::json!(""),
            serde_json::Value::Null,
        ] {
            let d = serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": clear_form});
            let out = resolve_connection_draft(&d, "e1", persisted, &test_schema(false), &store)
                .expect("可选凭据被清除后仍应通过解析");
            assert!(
                out.connection.get("password").is_none(),
                "明确清除后草稿里不得再有 password（has_password={}）",
                out.connection.get("password").is_some()
            );
            assert_eq!(out.cleared_fields, vec!["password".to_string()]);
            assert!(out.stored_fields.is_empty(), "清除优先，不得去读旧值");
            assert_eq!(
                store.get_secret("e1", "password").unwrap().as_deref(),
                Some("old-pw"),
                "检查不得修改 SecretStore"
            );

            // 必填凭据被清除 → 统一校验报 REQUIRED（handler 映射 400）。
            let issues = test_schema(true).validate_instance("connection", &out.connection);
            assert!(
                issues
                    .iter()
                    .any(|i| i.code == "REQUIRED" && i.path == "connection.password"),
                "必填凭据被清除必须报 REQUIRED，实际 {issues:?}"
            );
        }
    }

    /// 验收 5（另一半）：字段**缺失**时按 PUT 契约继承旧值；无旧值则保持缺失。
    #[test]
    fn missing_secret_field_inherits_old_value_only_when_it_exists() {
        let persisted = r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#;
        let store = store_with_endpoint("e1", persisted);
        store
            .put_secret("e1", "password", "old-pw", "master")
            .unwrap();

        // 缺失 + 旧值存在 → 继承。
        let d = serde_json::json!({"endpoint_url": "opc.tcp://a:4840"});
        let out =
            resolve_connection_draft(&d, "e1", persisted, &test_schema(false), &store).unwrap();
        assert_eq!(out.connection["password"], "old-pw", "缺失应按契约继承旧值");
        assert_eq!(out.stored_fields, vec!["password".to_string()]);

        // 缺失 + 无旧值 → 保持缺失（不报错，交由统一校验裁决）。
        let store2 = store_with_endpoint("e9", r#"{"endpoint_url":"opc.tcp://a:4840"}"#);
        let out2 = resolve_connection_draft(
            &d,
            "e9",
            r#"{"endpoint_url":"opc.tcp://a:4840"}"#,
            &test_schema(false),
            &store2,
        )
        .unwrap();
        assert!(out2.connection.get("password").is_none());

        // 缺失 + 已保存记录里有引用但 SecretStore 查无记录 → 引用无效（400），
        // **不得**退化成「本次不带凭据去连」。
        let store3 = store_with_endpoint(
            "e8",
            r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#,
        );
        let err = resolve_connection_draft(
            &d,
            "e8",
            r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#,
            &test_schema(false),
            &store3,
        )
        .unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, "SECRET_NOT_FOUND");
    }

    // -----------------------------------------------------------------------
    // 红线 3：marker 位置与形状
    // -----------------------------------------------------------------------

    /// 红线 3：只有 Descriptor 声明为 Secret 的字段可以带 marker；非 Secret
    /// 字段（含嵌套）上的 marker 形状必须 400，绝不透传给 Driver。
    #[test]
    fn marker_on_non_secret_field_is_rejected() {
        let store = store_with_endpoint("e1", "{}");
        for bad in [
            serde_json::json!({"endpoint_url": {"secret_set": true}}),
            serde_json::json!({"endpoint_url": {"clear_secret": true}}),
            // 嵌套形状同样拒绝：Schema 没有对象类型，这只能是放错位置。
            serde_json::json!({"endpoint_url": {"a": {"b": {"secret_set": true}}}}),
        ] {
            let err = resolve_connection_draft(&bad, "e1", "{}", &test_schema(false), &store)
                .unwrap_err();
            assert_eq!(err.status, StatusCode::BAD_REQUEST, "输入 {bad}");
            assert_eq!(err.code, "INVALID_SECRET_MARKER", "输入 {bad}");
        }
    }

    /// 红线 3：不合法的 marker 形状必须拒绝，绝不「猜意图」。
    #[test]
    fn malformed_secret_marker_shapes_are_rejected() {
        let store = store_with_endpoint("e1", "{}");
        store.put_secret("e1", "password", "pw", "master").unwrap();
        for bad in [
            serde_json::json!({"secret_set": "yes"}),
            serde_json::json!({"secret_set": true, "extra": 1}),
            serde_json::json!({"secret_set": false}),
            serde_json::json!({"clear_secret": false}),
            serde_json::json!({"clear_secret": true, "secret_set": true}),
            serde_json::json!({"clear_secret": true, "extra": 1}),
            // 裸对象/数字/布尔/数组都不是凭据形态。
            serde_json::json!({"credentials": {"user": "u"}}),
            serde_json::json!(123),
            serde_json::json!(true),
            serde_json::json!([1, 2]),
        ] {
            let d = serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": bad});
            let err =
                resolve_connection_draft(&d, "e1", "{}", &test_schema(false), &store).unwrap_err();
            assert_eq!(err.status, StatusCode::BAD_REQUEST, "输入 {bad}");
            assert_eq!(err.code, "INVALID_SECRET_MARKER", "输入 {bad}");
        }
    }

    // -----------------------------------------------------------------------
    // 红线 2：SecretStore 故障不得降级
    // -----------------------------------------------------------------------

    /// 验收 6（红线 2）：Secret 缺失是 400（引用无效），而密钥损坏/解密失败是
    /// **503 SecretStore 基础设施故障**——不得降级为匿名连接，不得显示成参数
    /// 错误或设备不可达。
    #[test]
    fn missing_secret_is_400_but_broken_store_is_503() {
        // ① 缺记录 → 400 SECRET_NOT_FOUND
        let store = store_with_endpoint("e1", r#"{"password":{"secret_set":true}}"#);
        let d = serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"secret_set": true}});
        let err =
            resolve_connection_draft(&d, "e1", "{}", &test_schema(false), &store).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, "SECRET_NOT_FOUND");

        // ② 真实文件库 + 被替换的 master.key → 解密失败 → 503。
        //    用真库而不是构造 StoreError：分类正确性取决于「错误来自哪一层」，
        //    只看变体类型是测不出真实行为的。
        let base = std::env::temp_dir().join(format!(
            "mesa-c3-check-store-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        let db = base.join("mesa.db");
        {
            let s = ConfigStore::open(&db).unwrap();
            s.create_device(&DeviceRecord {
                id: "d1".into(),
                name: "D".into(),
            })
            .unwrap();
            s.create_endpoint(&EndpointRecord {
                id: "e1".into(),
                name: "e1".into(),
                device_id: "d1".into(),
                driver_id: "opcua".into(),
                connection_json: r#"{"password":{"secret_set":true}}"#.into(),
                desired_running: false,
                updated_at_ns: 0,
            })
            .unwrap();
            s.put_secret("e1", "password", "top-secret-pw", "master")
                .unwrap();
        }
        // 换成另一把**合法**密钥 → AEAD 认证失败（密钥丢失/轮换的典型形态）。
        std::fs::write(base.join("master.key"), [0x5Au8; 32]).unwrap();
        let broken = ConfigStore::open(&db).unwrap();
        let err = resolve_connection_draft(
            &d,
            "e1",
            r#"{"password":{"secret_set":true}}"#,
            &test_schema(false),
            &broken,
        )
        .unwrap_err();
        assert_eq!(
            err.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "SecretStore 解密失败必须是 503，实际 {} {}",
            err.status,
            err.code
        );
        assert_eq!(err.code, "SECRET_STORE_UNAVAILABLE");
        assert!(
            !err.message.contains("top-secret-pw"),
            "错误文本不得回显明文: {}",
            err.message
        );
        // 同时确认底层确实是「读失败」而不是「没有记录」。
        let read_err = broken
            .get_secret("e1", "password")
            .expect_err("预置：master.key 被替换后解密必须失败");
        let read_msg = read_err.to_string();
        assert!(
            !read_msg.contains("top-secret-pw"),
            "底层错误也不得回显明文: {read_msg}"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    // -----------------------------------------------------------------------
    // 无副作用 / 脱敏
    // -----------------------------------------------------------------------

    /// 验收 7：检查前后 `connection_json`、`endpoint_secrets`、`desired_running`、
    /// 任务、运行态一致（运行态在 handler 层由契约测试覆盖，这里比对持久化面）。
    #[test]
    fn persisted_state_is_identical_before_and_after_check() {
        let persisted = r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#;
        let store = store_with_endpoint("e1", persisted);
        store
            .put_secret("e1", "password", "old-pw", "master")
            .unwrap();

        let before_record = store.get_endpoint("e1").unwrap().unwrap();
        let before_secret = store.get_secret("e1", "password").unwrap();
        let before_fields = store.list_secret_fields("e1").unwrap();
        let before_tasks = store.list_tasks("e1").unwrap();
        let before_events = store.list_event_tasks("e1").unwrap();

        // 四种语义各跑一遍（含写意图最强的「新明文」与「清除」）。
        for d in [
            serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"secret_set": true}}),
            serde_json::json!({"endpoint_url": "opc.tcp://b:4840", "password": "new-pw"}),
            serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"clear_secret": true}}),
            serde_json::json!({"endpoint_url": "opc.tcp://a:4840"}),
        ] {
            let out = resolve_connection_draft(&d, "e1", persisted, &test_schema(false), &store)
                .expect("四种语义都必须解析成功");
            let _ = test_schema(false).validate_instance("connection", &out.connection);
        }

        assert_eq!(store.get_endpoint("e1").unwrap().unwrap(), before_record);
        assert_eq!(store.get_secret("e1", "password").unwrap(), before_secret);
        assert_eq!(store.list_secret_fields("e1").unwrap(), before_fields);
        assert_eq!(store.list_tasks("e1").unwrap(), before_tasks);
        assert_eq!(store.list_event_tasks("e1").unwrap(), before_events);
    }

    /// 验收 8：错误文本不得包含明文；统一校验在 Secret 字段上必须显示
    /// `[redacted]`（core-types 既有保证，这里锁死 C3 路径不会绕过它）。
    #[test]
    fn plaintext_never_appears_in_error_messages() {
        const PW: &str = "top-secret-pw";
        // 让 Secret 字段带 pattern：明文不匹配 → 走「校验失败」这条最容易回显
        // 用户输入的路径。
        let mut schema = test_schema(false);
        schema.fields[1].validation.pattern = Some("^[A-Z]+$".to_string());
        let store = store_with_endpoint("e1", "{}");

        let d = serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": PW});
        let out = resolve_connection_draft(&d, "e1", "{}", &schema, &store).unwrap();
        let issues = schema.validate_instance("connection", &out.connection);
        let body = serde_json::json!({"valid": false, "issues": issues}).to_string();
        assert!(
            body.contains("PATTERN_MISMATCH"),
            "预置：必须走到 pattern 失败"
        );
        assert!(
            !body.contains(PW),
            "校验失败的响应体不得回显 Secret 明文: {body}"
        );
        // core-types 在 Secret 字段上不把值写进 issue 文本（只说明哪条规则不匹配）；
        // 这里把它锁死，防止日后「为了好排查」把值加回消息里。
        assert!(
            body.contains("不匹配") && !body.contains("\"value\""),
            "Secret 值的展示必须脱敏（只报规则，不回显值）: {body}"
        );

        // 各类错误文本同样不得回显明文（错误里只允许出现字段名）。
        for bad in [
            serde_json::json!({"password": {"secret_set": true}}),
            serde_json::json!({"password": {"secret_set": "yes"}}),
        ] {
            let d = serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": bad, "username": PW});
            let err =
                resolve_connection_draft(&d, "e1", "{}", &test_schema(false), &store).unwrap_err();
            let (_, Json(body)) = err.into_error_response();
            let text = body.to_string();
            assert!(!text.contains(PW), "错误响应不得回显明文: {text}");
        }
    }

    // -----------------------------------------------------------------------
    // Handler 形状（HTTP 层其余语义见 management_api 契约测试）
    // -----------------------------------------------------------------------

    fn test_state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!(
            "mesa-c3-check-handler-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let drivers_dir = dir.join("drivers");
        std::fs::create_dir_all(&drivers_dir).unwrap();
        let store = Arc::new(store_with_endpoint("e1", "{}"));
        let manager = Arc::new(mesa_driver_manager::MesaManager::discover(&drivers_dir));
        AppState::try_new_with_cert_dir_and_control(
            manager,
            store,
            drivers_dir.to_string_lossy().to_string(),
            dir.join("cert"),
            false,
        )
        .unwrap()
    }

    async fn post(
        state: &Arc<AppState>,
        endpoint_id: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let (status, Json(v)) = connection_check(
            State(Arc::clone(state)),
            Path(endpoint_id.to_string()),
            Json(body),
        )
        .await;
        (status, v)
    }

    /// `endpoint_id` 只来自 URL、`driver_id` 只读已保存记录：请求体里的任何
    /// 额外字段（尤其 `driver_id`）必须 400，mode 缺失/非法同样 400。
    #[tokio::test]
    async fn request_shape_rejects_driver_id_override_and_bad_mode() {
        let state = test_state();
        for bad in [
            serde_json::json!({"mode": "validate", "connection": {}, "driver_id": "s7"}),
            serde_json::json!({"mode": "validate", "connection": {}, "endpoint_id": "e2"}),
            serde_json::json!({"connection": {}}),
            serde_json::json!({"mode": "probe2", "connection": {}}),
            serde_json::json!({"mode": "validate", "connection": "not-an-object"}),
        ] {
            let (status, v) = post(&state, "e1", bad.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "输入 {bad} → {v}");
            assert_eq!(v["error"]["code"], "VALIDATION_ERROR", "输入 {bad}");
            assert!(
                !v.to_string().contains("s7"),
                "不得因请求体里的 driver_id 而取得驱动信息: {v}"
            );
        }
    }

    /// Endpoint 不存在 → 404（URL 是唯一 Endpoint 来源）。
    #[tokio::test]
    async fn unknown_endpoint_is_404() {
        let state = test_state();
        let (status, v) = post(
            &state,
            "nope",
            serde_json::json!({"mode": "validate", "connection": {}}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body: {v}");
        assert_eq!(v["error"]["code"], "NOT_FOUND");
    }

    /// Descriptor 不可用（此处 drivers 目录为空）→ 503，不得猜 Schema 放行。
    #[tokio::test]
    async fn descriptor_unavailable_is_503() {
        let state = test_state();
        let (status, v) = post(
            &state,
            "e1",
            serde_json::json!({"mode": "validate", "connection": {}}),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "body: {v}");
        assert_eq!(v["error"]["code"], "DRIVER_UNAVAILABLE");
    }

    // -----------------------------------------------------------------------
    // 辅助判定函数本身也要锁住语义（它们是红线 3 的判据）
    // -----------------------------------------------------------------------

    #[test]
    fn secret_value_classification_is_strict() {
        assert_eq!(
            classify_secret_value(&serde_json::json!("pw")),
            SecretValue::Plaintext
        );
        assert_eq!(
            classify_secret_value(&serde_json::json!("")),
            SecretValue::Clear
        );
        assert_eq!(
            classify_secret_value(&serde_json::Value::Null),
            SecretValue::Clear
        );
        assert_eq!(
            classify_secret_value(&serde_json::json!({"secret_set": true})),
            SecretValue::Reference
        );
        assert_eq!(
            classify_secret_value(&serde_json::json!({"clear_secret": true})),
            SecretValue::Clear
        );
        assert!(matches!(
            classify_secret_value(&serde_json::json!({"secret_set": true, "x": 1})),
            SecretValue::Invalid(_)
        ));
        assert!(matches!(
            classify_secret_value(&serde_json::json!({"secret_set": true, "clear_secret": true})),
            SecretValue::Invalid(_)
        ));
        // 既有 marker 判定函数与严格判定保持同一方向（C1 路径继续可用）。
        assert!(crate::is_secret_marker(
            &serde_json::json!({"secret_set": true})
        ));
        assert!(crate::is_clear_secret_marker(
            &serde_json::json!({"clear_secret": true})
        ));
    }
}
