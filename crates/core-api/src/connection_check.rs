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
//!
//! # C3-B：`mode=probe` 的执行路径（复用 C1 编排，不另建生命周期）
//!
//! 两种模式共用同一条前缀链（读 Endpoint → Descriptor → 草稿解析 → 统一校验），
//! 只有在**校验通过之后**才分叉：[`run_probe`] 把内存中已物化的连接序列化后交给
//! [`MesaManager::probe()`]，即 C1 的临时进程 / Probe RPC / 分层预算（Open 5s、
//! Query `4×timeout_ms+3s` 上限 45s、Close 5s、清理 6s）/ 进程回收。
//! Core 这一层只做三件事：守住「载荷中不得残留 marker」的红线、序列化、映射结果。
//!
//! 出口脱敏（本轮评审新增的边界）：进入 Driver 前的脱敏**不足以**保证不泄漏——
//! 真实驱动会把收到的凭据写进认证失败的错误/警告文本（`auth failed for
//! password=...`），而这类文本一路透传到 HTTP 响应。因此映射之后还要按
//! Descriptor 的 Secret 字段做**值级擦除**（[`redact_plaintext`]）。
//!
//! NOTE（已核查的残留日志路径，本轮未改动其行为）：`driver-manager` 的
//! `probe.rs` 在 `CloseConnection` 失败时会把 Driver 的原始错误文本以 debug
//! 记入日志（`probe close temp connection failed (ignored)`）。该文本理论上可能
//! 含凭据，但它既不属于 Probe 报告也不属于错误响应，且真实驱动的关闭错误是
//! 传输级诊断（不含凭据）。C3 的值级擦除只作用于 HTTP 响应体，不接管 C1 的
//! 日志文案——无证据就改动 C1 的排障输出不划算。若后续真机发现驱动在关闭错误里
//! 回显凭据，应在 `driver-manager` 侧按同一个「值级擦除」思路处理。
//!
//! # Secret 继承（与 PUT 同源，两条 fail-closed 边界）
//!
//! 草稿省略某个 Secret 字段时的继承**逐步对齐** `update_endpoint()` 的缺失分支：
//! 先按 Descriptor 字段名查 SecretStore（唯一读入口 [`read_stored_secret`]，恒定带
//! `endpoint_id`），**查无记录才**回退历史 `connection_json`（兼容明文残留/marker）。
//! 两条 fail-closed 边界：
//! ① 「确实需要继承」而已保存 `connection_json` 不是合法 JSON 对象 → 503
//!    `PERSISTED_CONNECTION_INVALID`，绝不把「读不到已保存配置」当成「没有旧凭据」
//!    继续匿名检查/探测（C1 `materialize_connection()` 同款口径）；
//! ② SecretStore 读不出来 → 503 `SECRET_STORE_UNAVAILABLE`，不得降级为匿名连接。
//! 反之，草稿已给全所有 Secret 字段时不需要继承，损坏的历史记录**不得**拦下检查。

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use mesa_config_store::{ConfigStore, StoreError};
use mesa_core_types::SchemaDescriptor;
use serde::Deserialize;

use crate::{
    AppState, SecretRefError, json_error, materialize_secret_markers_in_place, read_stored_secret,
    secret_field_keys,
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

    // ③ 草稿省略的 Secret 字段：与 PUT **完全一致**地继承旧值。
    //
    //    对齐依据是 `update_endpoint()` 的缺失分支（`lib.rs:2033-2050` 原文）：
    //
    //    ```ignore
    //    None => {                                   // 草稿里该 Secret 字段缺失
    //        match state.store.get_secret(&id, sk) { // ← 直接查 SecretStore，且 sk
    //            Ok(Some(pt)) => { /* 继承真实值参检；落盘写回 marker */ }  // 取自
    //            Ok(None) => {}                       // secret_field_keys(desc)
    //            Err(e) => return store_err_to_response(e),
    //        }
    //    }
    //    ```
    //
    //    ⇒ PUT **不解析 `connection_json`**。因此这里也必须**先查 SecretStore**
    //    （同一入口 [`crate::read_stored_secret`]、同一参数 `(endpoint_id, field_path)`、
    //    同一字段名来源 `secret_field_keys()`），旧 `connection_json` **只**在
    //    SecretStore 查无记录时作为历史明文/marker 的兼容回退。C3-A 早期实现只依赖
    //    旧 JSON 里存在 marker，于是出现「SecretStore 有记录、旧 JSON 无 marker ⇒
    //    PUT 会继承、检查却不继承」的偏差：同一份草稿在两条路径上结论不同。
    //
    //    被 ② 明确清除的字段**不参与**继承：清除是用户的显式意图，若允许它从
    //    已保存记录里回填，就等于「用户清了密码，服务端偷偷拿旧密码去连」——
    //    契约 §2/§4 明令禁止。这一条是「清除优先」真正落地的地方（红线①）。
    let missing: Vec<&String> = secret_keys
        .iter()
        .filter(|k| !obj.contains_key(*k) && !cleared_fields.contains(*k))
        .collect();
    if !missing.is_empty() {
        // 需要继承 ⇒ 已保存记录必须是一份**可信的合法 JSON 对象**，否则 fail-closed
        // （503），绝不退化成「读不到旧凭据 ⇒ 本次不带凭据去连」。
        //
        // 为什么不能沿用旧的 `.ok()` 静默吞掉：那会把「已保存配置损坏」降级成
        // 「没有旧凭据」，随后以**空凭据**继续校验/探测——即匿名连接，与 C1
        // `materialize_connection()` 在坏 JSON 上的 fail-closed 口径相矛盾，也正是
        // 评审指出的安全边界缺口。注意 Secret 可能是**可选**字段，此时匿名形态本身
        // 就是合法配置，只有 fail-closed 才拦得住。
        //
        // 为什么只在 `missing` 非空时解析：草稿已给全所有 Secret 字段时，历史记录的
        // 好坏与被检查的连接无关，不该拦下「用户已经填全」的检查。
        //
        // 为什么错误文本不回显原始 JSON：历史 `connection_json` 可能含明文残留，
        // 回显等于把凭据带进响应/日志（脱敏红线）；这里只报「损坏」这一事实。
        let persisted = serde_json::from_str::<serde_json::Value>(persisted_connection_json)
            .ok()
            .filter(serde_json::Value::is_object);
        let Some(persisted_obj) = persisted.as_ref().and_then(|p| p.as_object()) else {
            return Err(CheckError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "PERSISTED_CONNECTION_INVALID",
                format!(
                    "endpoint `{endpoint_id}` 已保存的连接配置损坏（非法 JSON 或非对象），\
                     无法安全继承凭据；请在本次请求中显式提供相关 Secret 字段后重试"
                ),
            ));
        };

        for key in missing {
            // ③-1 SecretStore 优先（PUT 同款查询，恒定带 endpoint_id ⇒ C3-07 隔离）。
            match read_stored_secret(store, endpoint_id, key) {
                Ok(Some(_)) => {
                    // 保留 marker 形态交给 ④ 统一物化，而不是把明文直接塞进草稿：
                    // ①「引用 → 明文」只有一处实现，不会与运行期/C1 分叉；② 与 PUT
                    // 继承时写回的落盘形态一致；③ `stored_fields` 因此天然记录到本次
                    // 由 SecretStore 提供的字段（只有字段名、无明文）。多发一次同一条
                    // 记录的读请求没有任何副作用（本接口全程只读）。
                    obj.insert(key.clone(), serde_json::json!({ "secret_set": true }));
                    continue;
                }
                // 查无记录 → 与 PUT 的 `Ok(None) => {}` 一致：**不报错**，保持缺失，
                // 是否合法交由 ④ 之后的统一校验（required 等）裁决；这里转去做历史
                // JSON 兼容回退。
                Ok(None) => {}
                // 读不出来（master.key 丢失、AEAD 解密失败、SQLite 故障）→ 503
                // （红线②）：绝不当作「没有旧凭据」。
                //
                // NOTE（与 PUT 的**一处刻意差异**）：PUT 在同情形走
                // `store_err_to_response()`——AEAD 解密失败是 `StoreError::Validation`，
                // 会被映射成 400 VALIDATION_ERROR。这里保持契约 §5 冻结的 503
                // `SECRET_STORE_UNAVAILABLE`：密钥丢失既不是参数错误也不是设备不可达，
                // 报 400 会把用户引向改参数，而不是去修密钥。差异点与理由记录在此，
                // 以便日后统一时不会误判为遗漏。
                Err(e) => {
                    return Err(CheckError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "SECRET_STORE_UNAVAILABLE",
                        format!("SecretStore 不可用，拒绝以不明凭据状态检查连接: {e}"),
                    ));
                }
            }

            // ③-2 历史兼容回退（**仅** SecretStore 查无记录时生效）：兼容 C1 之前的
            //      明文落盘形态，以及旧记录里留下的 marker。
            let Some(v) = persisted_obj.get(key) else {
                continue;
            };
            match classify_secret_value(v) {
                // 历史明文残留：只用在本次内存草稿里，不写回任何地方。
                SecretValue::Plaintext => {
                    obj.insert(key.clone(), v.clone());
                }
                // 历史记录里的引用同样是「必须能解析」的引用：交给 ④；读不到就
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
// 出口脱敏：Probe 结果里的凭据明文
// ---------------------------------------------------------------------------

/// 收集本次草稿中所有 Secret 字段的**明文值**（只有值，不含字段名以外的信息）。
///
/// 为什么按 Descriptor 的 Secret 字段取值而不是全文搜索：擦除必须是**值级**的，
/// 只有 Schema 知道哪些字段是凭据。全文猜密文形态会把 URL、型号等正常内容也误伤。
///
/// 为什么排除空字符串：空串在 [`redact_plaintext`] 里会匹配任何位置，把整份响应
/// 打成碎片（而空串本身不构成凭据泄漏——契约里它就是「明确清除」）。
fn secret_plaintexts(connection: &serde_json::Value, schema: &SchemaDescriptor) -> Vec<String> {
    let Some(obj) = connection.as_object() else {
        return Vec::new();
    };
    secret_field_keys(schema)
        .into_iter()
        .filter_map(|k| {
            obj.get(&k)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// 值级擦除：把 JSON 中每个字符串里出现的凭据明文替换为 `[redacted]`。
///
/// 为什么是「包含即替换」而不是「相等才替换」：驱动常把凭据拼进更长的诊断文本
/// （`"open failed: bad password 'pw1'"`），只比对整串会漏掉最典型的泄漏形态。
///
/// 只遍历**值**、不改 key：响应体的键集合是我们自己固定的形状（`error.code`、
/// `warnings[].message` 等），改动键会破坏既有契约；凭据不可能出现在键上。
fn redact_plaintext(v: &mut serde_json::Value, plaintexts: &[String]) {
    match v {
        serde_json::Value::String(s) => {
            for p in plaintexts {
                if s.contains(p.as_str()) {
                    *s = s.replace(p.as_str(), "[redacted]");
                }
            }
        }
        serde_json::Value::Array(a) => {
            for item in a.iter_mut() {
                redact_plaintext(item, plaintexts);
            }
        }
        serde_json::Value::Object(m) => {
            for item in m.values_mut() {
                redact_plaintext(item, plaintexts);
            }
        }
        // 数字/布尔/null 不承载凭据：Secret 在 Schema 里只有 string 一种合法形态，
        // 而且把数字节点改成字符串会破坏响应形状（如 `capabilities` 四态）。
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /api/v1/endpoints/{endpoint_id}/connection-check`
///
/// 状态码口径（冻结 §5）：200 校验通过 / 探测完成（含 `reachable:false`）；
/// 400 参数、Secret 引用或必填凭据问题（含 Driver 侧 ConfigurationError）；
/// 404 Endpoint 不存在；501 Driver 不支持探测；503 Driver / SecretStore /
/// 探测基础设施不可用；504 探测超时。
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
        // `probe`：把**已通过统一校验**的内存连接交给 C1 的临时探测编排。
        //
        // 为什么绝不改道既有的 `POST /api/v1/drivers/{id}/probe`：那条路径接受
        // 原始 connection JSON 且不做 Secret 物化，改道等于绕过本接口刚建立的
        // Secret 安全边界（marker 直接进 Driver）。这里复用它的**编排与映射**，
        // 而不是它的入口。
        CheckMode::Probe => {
            run_probe(
                state,
                endpoint_id,
                &rec.driver_id,
                &desc.connection,
                resolved,
            )
            .await
        }
    }
}

/// `mode=probe` 的真实执行路径（C3-B）。
///
/// 与 `validate` 的分工是冻结的：前四步（读 Endpoint → Descriptor → 草稿解析 →
/// 统一 Schema 校验）完全共用，只有校验**通过之后**才分叉——「Schema 不合法就
/// 绝不发起探测」因此是结构上的事实，而不是在探测侧再判一次。
async fn run_probe(
    state: &AppState,
    endpoint_id: &str,
    driver_id: &str,
    schema: &SchemaDescriptor,
    resolved: ResolvedDraft,
) -> (StatusCode, Json<serde_json::Value>) {
    // ① 红线复核（纵深防御）：交给 Driver 的载荷里**不允许**残留任何 marker
    //    形状。`resolve_connection_draft` 已从结构上排除它，这里再核一次：日后
    //    若有人在解析链上加了新分支而漏了物化，宁可 500（内部不变量被破坏）
    //    也绝不把 marker 当密码送给设备，更不能报成「设备不可达」。
    if contains_marker_shape(&resolved.connection, 0) {
        return CheckError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "SECRET_MARKER_LEAK",
            "内存连接草稿中仍存在 Secret 标记，拒绝发起探测（内部不变量被破坏）",
        )
        .into_error_response();
    }

    // ② 序列化**已物化**的连接：明文只从本进程内存流向临时 Driver 进程的 IPC，
    //    不经过任何持久化与日志。
    let conn_json = match serde_json::to_string(&resolved.connection) {
        Ok(s) => s,
        // `serde_json::Value` 的序列化在实践中不会失败（无 NaN / 非字符串 key）；
        // 真失败说明值形态异常，报 500 而不是 panic 掉整个 worker。
        Err(e) => {
            return CheckError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL",
                format!("连接草稿无法序列化: {e}"),
            )
            .into_error_response();
        }
    };

    // ③ 复用 C1 编排：临时进程 / Probe RPC / 分层预算 / 进程回收全部收敛在
    //    `MesaManager::probe()`，本层不另建探测生命周期（契约 §5 冻结）。
    let result = state.manager.probe(driver_id, &conn_json).await;

    // ④ 复用既有映射：状态码与响应形状与 `/drivers/{id}/probe` 同源（唯一实现
    //    在 `probe_result_to_response`），因此 C3-10 的分层不会与既有入口分叉。
    let (status, Json(mut body)) = crate::probe_result_to_response(result);

    // ⑤ 出口脱敏：Driver 回传的 warning / 错误文本里可能带着它收到的凭据明文
    //    （真实驱动同样会把密码写进认证失败消息）。这里是**唯一**能擦掉它的位置，
    //    因为只有本层同时掌握 Schema 与本次物化出来的明文。
    let plaintexts = secret_plaintexts(&resolved.connection, schema);
    redact_plaintext(&mut body, &plaintexts);

    // 只记录分类与字段名，绝不记录响应体（可能含 Driver 回传的诊断文本）。
    tracing::debug!(
        endpoint = %endpoint_id,
        driver = %driver_id,
        status = %status,
        redacted_secret_fields = plaintexts.len(),
        "endpoint connection probe finished"
    );
    (status, Json(body))
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
    // 修复 1：需要继承而 connection_json 损坏 ⇒ fail-closed（不得匿名降级）
    // -----------------------------------------------------------------------

    /// 需要继承 Secret，而持久化 `connection_json` 损坏（非法 JSON / 非对象）时
    /// 必须 503 `PERSISTED_CONNECTION_INVALID`：**不得**把「读不到旧配置」当成
    /// 「没有旧凭据」继续走匿名校验/探测。
    ///
    /// 「没有走匿名路径」的断言方式：匿名形态在这份 Schema（Secret 可选）下**本身
    /// 合法**，因此唯一的拦截点就是这里——每个用例都要求 `Err`；而在 `run_check`
    /// 中只有拿到 `Ok(draft)` 才可能返回 200 `{"valid": true}`（匿名检查成功的
    /// 形态）。预置断言把这一点钉死，避免日后有人「顺手」把错误吞掉后测试仍然绿。
    ///
    /// 为什么坏串直接作为参数传入而不是写进库：`config-store` 的三条写路径都会
    /// 校验 `connection_json` 必须是合法 JSON 对象（`lib.rs:951-1075` 等），坏记录
    /// 只可能来自**历史遗留行**（早于写时校验）/带外修改/存储损坏——这正是 C1
    /// `materialize_connection()` 在坏 JSON 上 fail-closed 的理由。这里按
    /// `run_check()` 的真实形态，把坏串当作 `rec.connection_json` 交给解析函数。
    #[test]
    fn persisted_json_invalid_and_secret_needs_inheritance_fails_closed() {
        const LEAK_MARKER: &str = "leak-marker";
        const OLD_PW: &str = "old-pw";
        /// 库中那一行仍是合法对象：只读路径必须保持它逐字节不变。
        const STORED: &str = r#"{"endpoint_url":"opc.tcp://a:4840"}"#;

        let schema = test_schema(false);
        let draft = serde_json::json!({ "endpoint_url": "opc.tcp://a:4840" });
        assert!(
            schema.validate_instance("connection", &draft).is_empty(),
            "预置：匿名形态是合法配置，只能靠 fail-closed 拦住，不能指望校验兜底"
        );

        // 损坏形态 × SecretStore 有无记录：每一种都必须失败（有记录时更不能因为
        // 「凭据其实读得到」就放行——需要继承即代表已保存配置必须可信）。
        for (label, broken, has_record) in [
            (
                "非法 JSON（截断）",
                r#"{"endpoint_url":"opc.tcp://leak-marker:4840","password":"old-pw"#,
                true,
            ),
            (
                "非法 JSON（多余括号）",
                r#"{"endpoint_url":"opc.tcp://leak-marker:4840"}}"#,
                true,
            ),
            ("非对象（数组）", r#"[{"password":"old-pw"}]"#, true),
            ("非对象（字符串）", r#""old-pw""#, true),
            ("非对象（null）", "null", true),
            ("空字符串", "", true),
            ("非法 JSON 且无记录", "{oops", false),
        ] {
            let store = store_with_endpoint("e1", STORED);
            if has_record {
                store
                    .put_secret("e1", "password", OLD_PW, "master")
                    .unwrap();
            }
            let err = resolve_connection_draft(&draft, "e1", broken, &schema, &store).expect_err(
                &format!("{label}: 需要继承 + 已保存配置损坏 ⇒ 必须 fail-closed，不得匿名放行"),
            );
            assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE, "{label}");
            assert_eq!(err.code, "PERSISTED_CONNECTION_INVALID", "{label}");
            // 脱敏：错误文本不得回显历史配置的任何内容（可能含明文残留）。
            assert!(
                !err.message.contains(LEAK_MARKER) && !err.message.contains(OLD_PW),
                "{label}: 错误文本不得回显已保存配置内容: {}",
                err.message
            );
            // 无副作用：拒绝路径不得「顺手修好」或改写持久化记录。
            assert_eq!(
                store.get_endpoint("e1").unwrap().unwrap().connection_json,
                STORED,
                "{label}: 检查失败也不得写入 connection_json"
            );
        }
    }

    /// 反向用例：草稿已给全**所有** Secret 字段（明文 / 明确清除 / 可用引用）时
    /// 不需要继承，损坏的历史记录**不得**拦下检查——保持「只有确实需要继承时才
    /// 解析已保存记录」的既有设计意图。
    #[test]
    fn corrupt_persisted_json_does_not_block_check_when_draft_supplies_all_secrets() {
        const STORED: &str = r#"{"endpoint_url":"opc.tcp://a:4840"}"#;
        let schema = test_schema(true);
        for (label, broken) in [
            ("非法 JSON", "{oops"),
            ("非对象（数组）", "[1,2]"),
            ("空字符串", ""),
        ] {
            for (what, draft) in [
                (
                    "草稿给新明文",
                    serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": "new-pw"}),
                ),
                (
                    "草稿明确清除",
                    serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"clear_secret": true}}),
                ),
                (
                    "草稿引用已存 Secret",
                    serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"secret_set": true}}),
                ),
            ] {
                let store = store_with_endpoint("e1", STORED);
                store
                    .put_secret("e1", "password", "old-pw", "master")
                    .unwrap();
                let out = resolve_connection_draft(&draft, "e1", broken, &schema, &store)
                    .unwrap_or_else(|e| {
                        panic!("{label} + {what}: 不需要继承时损坏记录不得拦下检查: {e:?}")
                    });
                match what {
                    "草稿给新明文" => assert_eq!(out.connection["password"], "new-pw"),
                    "草稿明确清除" => {
                        assert!(out.connection.get("password").is_none());
                        assert_eq!(out.cleared_fields, vec!["password".to_string()]);
                    }
                    // 引用由 ④ 统一物化，与损坏的历史记录无关。
                    _ => assert_eq!(out.connection["password"], "old-pw"),
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 修复 2：缺失继承必须与 PUT 的缺失分支逐条一致
    // -----------------------------------------------------------------------

    /// SecretStore 有记录、而旧 `connection_json` 里**没有**该字段（评审指出的
    /// 偏差场景）时，缺失继承必须成功：PUT 的缺失分支就是直接查 SecretStore。
    #[test]
    fn missing_secret_inherits_from_store_even_when_old_json_lacks_the_field() {
        // 旧 JSON 完全没有 password key，也没有任何 marker。
        let persisted = r#"{"endpoint_url":"opc.tcp://a:4840"}"#;
        let store = store_with_endpoint("e1", persisted);
        store
            .put_secret("e1", "password", "old-pw", "master")
            .unwrap();

        let out = resolve_connection_draft(
            &serde_json::json!({"endpoint_url": "opc.tcp://a:4840"}),
            "e1",
            persisted,
            &test_schema(false),
            &store,
        )
        .expect("SecretStore 有记录就必须继承，不得依赖旧 JSON 里是否留了 marker");

        assert_eq!(
            out.connection["password"], "old-pw",
            "继承来源是 SecretStore，而不是旧 connection_json"
        );
        assert_eq!(out.stored_fields, vec!["password".to_string()]);
        assert!(out.inline_fields.is_empty());
        // 只读：SecretStore 与持久化行都逐值不变。
        assert_eq!(
            store.get_secret("e1", "password").unwrap().as_deref(),
            Some("old-pw")
        );
        assert_eq!(
            store.get_endpoint("e1").unwrap().unwrap().connection_json,
            persisted
        );
    }

    /// 对齐性测试：同一场景下 connection-check 的缺失继承与 PUT 缺失分支结论一致。
    ///
    /// 为什么不去直接驱动 PUT handler：`update_endpoint()` 取 Descriptor 依赖
    /// `MesaManager::get_descriptor()`，而它必须**临时 spawn 真实驱动进程**；core-api
    /// 单测环境里没有可启动的驱动二进制（既有 `runtime.rs` 测试正是靠「Descriptor
    /// 必然失败」验证 fail-closed 分支），强行拉起 PUT 只能得到 503
    /// `DRIVER_UNAVAILABLE`，根本走不到 Secret 分支。
    ///
    /// 因此这里改为断言**两侧的继承来源是同一个调用**：PUT 缺失分支（`lib.rs:2040`）
    /// 的原文是 `match read_stored_secret(&state.store, &id, sk)`，`sk` 取自
    /// `secret_field_keys(&desc.connection)`；测试用同一 `(endpoint_id, field_path)`、
    /// 同一字段名来源执行它，并与 connection-check 的解析结果逐值比对。两侧自本次
    /// 修复起共用 `read_stored_secret`，因此「读到/读不到」的分支归属也必然一致。
    #[test]
    fn missing_secret_inheritance_matches_put_missing_branch() {
        // 预置：两侧的字段名来源都是 Descriptor 的 `secret_field_keys()`。
        assert_eq!(
            secret_field_keys(&test_schema(false)),
            vec!["password".to_string()],
            "PUT 与 connection-check 的字段名都必须取自 Descriptor"
        );
        let persisted = r#"{"endpoint_url":"opc.tcp://a:4840"}"#;
        let draft = serde_json::json!({"endpoint_url": "opc.tcp://a:4840"});

        // 场景 1：旧 JSON 无该字段 + SecretStore 有记录 → 两侧都继承同一个值。
        let store = store_with_endpoint("e1", persisted);
        store
            .put_secret("e1", "password", "old-pw", "master")
            .unwrap();
        let put_inherits = read_stored_secret(&store, "e1", "password").unwrap();
        assert_eq!(
            put_inherits.as_deref(),
            Some("old-pw"),
            "预置：PUT 的缺失分支（lib.rs:2041 `Ok(Some(pt))`）会继承该值"
        );
        let check = resolve_connection_draft(&draft, "e1", persisted, &test_schema(false), &store)
            .expect("检查侧必须继承");
        assert_eq!(
            check.connection["password"], "old-pw",
            "同一场景下 PUT 与 connection-check 必须继承同一个值"
        );

        // 场景 2：PUT 保存**之后**的落盘形态（缺失继承会把 marker 写回
        // connection_json）再喂给检查，仍必须解析出同一个值——这是
        // 「PUT 保存 → 再检查」的真实序列，两侧不得有落差。
        let after_put = r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#;
        let check_after_put =
            resolve_connection_draft(&draft, "e1", after_put, &test_schema(false), &store)
                .expect("PUT 落盘的 marker 形态必须能被检查解析");
        assert_eq!(check_after_put.connection["password"], "old-pw");

        // 场景 3（反向对齐）：SecretStore 无记录 ⇒ PUT 的 `Ok(None) => {}` 什么都不
        // 继承；检查侧同样保持缺失而**不报错**（是否必填由统一校验裁决）。
        let store2 = store_with_endpoint("e9", persisted);
        let put_inherits_nothing = read_stored_secret(&store2, "e9", "password").unwrap();
        assert!(
            put_inherits_nothing.is_none(),
            "预置：PUT 侧 `Ok(None)`，无旧值可继承"
        );
        let check2 =
            resolve_connection_draft(&draft, "e9", persisted, &test_schema(false), &store2)
                .expect("无旧值不是错误：与 PUT 的 Ok(None) 分支一致");
        assert!(
            check2.connection.get("password").is_none(),
            "与 PUT 一致：无旧值就保持缺失，绝不发明凭据"
        );
    }

    /// 红线②（继承路径）：**缺失继承**读 SecretStore 失败时同样是 503，绝不当作
    /// 「没有旧凭据」继续匿名检查。旧实现只解析旧 JSON，这条路径根本不读
    /// SecretStore，因此坏密钥会被静默忽略——本用例把该缺口钉死。
    #[test]
    fn store_failure_during_inheritance_is_503_not_anonymous() {
        let base = std::env::temp_dir().join(format!(
            "mesa-c3-check-inherit-{}-{}",
            std::process::id(),
            mesa_core_types::now_unix_ns()
        ));
        let db = base.join("mesa.db");
        // 旧 JSON **不含** password：继承只能来自 SecretStore。
        let persisted = r#"{"endpoint_url":"opc.tcp://a:4840"}"#;
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
                connection_json: persisted.into(),
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
            &serde_json::json!({"endpoint_url": "opc.tcp://a:4840"}),
            "e1",
            persisted,
            &test_schema(false),
            &broken,
        )
        .expect_err("SecretStore 读失败时不得退化成「没有旧凭据」的匿名检查");
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "SECRET_STORE_UNAVAILABLE");
        assert!(
            !err.message.contains("top-secret-pw"),
            "错误文本不得回显明文: {}",
            err.message
        );
        std::fs::remove_dir_all(&base).ok();
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

    // -----------------------------------------------------------------------
    // C3-B：探测载荷与结果映射（HTTP 层见 management_api 契约测试）
    // -----------------------------------------------------------------------

    /// C3-10（分层，映射级）：`probe` 的状态码分类是**唯一实现**
    /// （`crate::probe_result_to_response`），既有 `/drivers/{id}/probe` 与 C3
    /// 入口共用它。504 只在这里覆盖是刻意的取舍：真实超时至少要耗满 Query 下限
    /// 5s + Close 5s + 启动/清理预算（单例 ≥ 20s），放进合同套件不划算；而真实
    /// RpcTimeout 的**进程回收**已由 A32 用例覆盖。两条合起来才是完整证据。
    #[test]
    fn probe_http_layering_classification() {
        use mesa_core_types::{CapabilityItem, CapabilityState, ProbeReport, ProbeWarning};
        use mesa_driver_manager::probe::ProbeError;
        use mesa_driver_manager::session::StartupStage;

        // 200：可达（含完整事实形状）
        let reachable = ProbeReport {
            reachable: true,
            vendor: Some("Mesa".into()),
            family: None,
            model: None,
            firmware: None,
            model_confidence: Some("high".into()),
            capabilities: vec![CapabilityItem {
                id: "read".into(),
                state: CapabilityState::Available,
                detail: None,
            }],
            warnings: vec![ProbeWarning {
                code: "W".into(),
                message: "m".into(),
            }],
        };
        let (s, Json(v)) = crate::probe_result_to_response(Ok(reachable));
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["reachable"], true);
        assert_eq!(v["device"]["vendor"], "Mesa");
        assert_eq!(v["model_confidence"], "high");
        assert_eq!(v["capabilities"][0]["id"], "read");
        assert_eq!(v["warnings"][0]["code"], "W");

        // 200：不可达是**探测结果**（不是错误，因此不得是 5xx）
        let (s, Json(v)) = crate::probe_result_to_response(Ok(ProbeReport::unreachable(
            "CONNECTION_FAILED",
            "refused",
        )));
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["reachable"], false);
        assert_eq!(v["warnings"][0]["code"], "CONNECTION_FAILED");

        // 逐分支锁定状态码与错误码（与既有 handler 契约逐字一致）
        for (err, want_status, want_code) in [
            (
                ProbeError::DriverNotFound("x".into()),
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
            ),
            (
                ProbeError::Unsupported("minor 1".into()),
                StatusCode::NOT_IMPLEMENTED,
                "PROBE_UNSUPPORTED",
            ),
            (
                ProbeError::InvalidInput {
                    code: "BAD_CONFIG".into(),
                    message: "m".into(),
                },
                StatusCode::BAD_REQUEST,
                "BAD_CONFIG",
            ),
            (
                ProbeError::Spawn("boom".into()),
                StatusCode::SERVICE_UNAVAILABLE,
                "DRIVER_UNAVAILABLE",
            ),
            (
                ProbeError::Handshake {
                    stage: StartupStage::Connect,
                    message: "reset".into(),
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "DRIVER_UNAVAILABLE",
            ),
            (
                ProbeError::Rpc("ConnectionError/CONNECT_FAIL: m".into()),
                StatusCode::SERVICE_UNAVAILABLE,
                "PROBE_FAILED",
            ),
            (
                ProbeError::RpcTimeout,
                StatusCode::GATEWAY_TIMEOUT,
                "PROBE_TIMEOUT",
            ),
            (
                ProbeError::Timeout { budget_ms: 27_000 },
                StatusCode::GATEWAY_TIMEOUT,
                "PROBE_TIMEOUT",
            ),
        ] {
            let (s, Json(v)) = crate::probe_result_to_response(Err(err));
            assert_eq!(s, want_status, "body: {v}");
            assert_eq!(v["error"]["code"], want_code, "body: {v}");
            assert!(
                v.get("reachable").is_none(),
                "错误分支不得携带任何可达性结论: {v}"
            );
        }
    }

    /// 出口脱敏（评审特别点出）：Driver 返回的 **warning 与错误文本**都可能带着
    /// 它收到的凭据明文。这里锁定两条：值级擦除覆盖「子串」形态（驱动常把凭据拼
    /// 进更长的诊断句），且空串不做替换（否则整份响应会被打成碎片）。
    #[test]
    fn probe_result_redacts_secret_plaintext_in_warnings_and_errors() {
        use mesa_core_types::{ProbeReport, ProbeWarning};
        use mesa_driver_manager::probe::ProbeError;

        const PW: &str = "pw-echo-never-return";
        let secret_keys = vec!["password".to_string()];

        // 预置：Driver 真的把明文回灌了（否则本用例是空断言）。
        let report = ProbeReport {
            reachable: true,
            vendor: Some("Mesa".into()),
            family: None,
            model: None,
            firmware: None,
            model_confidence: None,
            capabilities: vec![],
            warnings: vec![ProbeWarning {
                code: "SECRET_ECHO".into(),
                message: format!("driver received secret: {PW} (auth rejected)"),
            }],
        };
        let connection = serde_json::json!({"password": PW});
        let plaintexts = secret_plaintexts(&connection, &test_schema(false));
        assert_eq!(
            plaintexts,
            vec![PW.to_string()],
            "Secret 字段的明文必须被收集（且只收集 Secret 字段）"
        );

        let (s, Json(mut body)) = crate::probe_result_to_response(Ok(report));
        assert_eq!(s, StatusCode::OK);
        assert!(
            body.to_string().contains(PW),
            "预置：未脱敏前 warning 文本里确实带着明文（本用例才有意义）"
        );
        redact_plaintext(&mut body, &plaintexts);
        let text = body.to_string();
        assert!(!text.contains(PW), "warning 不得回显凭据明文: {text}");
        assert!(text.contains("[redacted]"), "必须留下脱敏标记: {text}");
        assert!(
            text.contains("driver received secret: [redacted] (auth rejected)"),
            "只擦凭据本身，保留其余诊断（否则排障信息全丢）: {text}"
        );

        // 错误文本同样必须擦除（400 分支最容易把密码带回响应）。
        let (s, Json(mut body)) = crate::probe_result_to_response(Err(ProbeError::InvalidInput {
            code: "PROBE_REJECTED_BY_DRIVER".into(),
            message: format!("open failed: bad password '{PW}'"),
        }));
        assert_eq!(s, StatusCode::BAD_REQUEST);
        redact_plaintext(&mut body, &plaintexts);
        let text = body.to_string();
        assert_eq!(body["error"]["code"], "PROBE_REJECTED_BY_DRIVER");
        assert!(!text.contains(PW), "错误文本不得回显凭据明文: {text}");
        assert!(text.contains("[redacted]"), "{text}");

        // 空串不参与擦除：`{"password": ""}` 是「明确清除」，若把空串当明文
        // 会让每个字符之间都插入 [redacted]，响应直接不可读。
        let cleared = serde_json::json!({"password": ""});
        assert!(secret_plaintexts(&cleared, &test_schema(false)).is_empty());
        let mut body = serde_json::json!({"error": {"message": "abc"}});
        redact_plaintext(&mut body, &secret_plaintexts(&cleared, &test_schema(false)));
        assert_eq!(body["error"]["message"], "abc");

        // 非 Secret 字段即便与凭据同值也不参与（值级擦除只认 Descriptor 的
        // Secret 字段；这里锁住收集函数的来源）。
        let other = serde_json::json!({"endpoint_url": PW, "password": "pw2"});
        assert_eq!(
            secret_plaintexts(&other, &test_schema(false)),
            vec!["pw2".to_string()]
        );
        let _ = secret_keys;
    }

    /// 红线：**marker 绝不进入 Driver**。四种 Secret 语义（引用 / 新明文 /
    /// 明确清除 / 缺失继承）解析完成后，即将序列化给 Driver 的载荷里都不得残留
    /// 任何 marker 形状——[`run_probe`] 的纵深防御守卫正是用同一判据。
    #[test]
    fn probe_payload_never_contains_marker_shape() {
        let persisted = r#"{"endpoint_url":"opc.tcp://a:4840","password":{"secret_set":true}}"#;
        let store = store_with_endpoint("e1", persisted);
        store
            .put_secret("e1", "password", "pw-marker-guard", "master")
            .unwrap();

        for (label, draft) in [
            (
                "引用已存凭据",
                serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"secret_set": true}}),
            ),
            (
                "本次新明文",
                serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": "pw-inline"}),
            ),
            (
                "明确清除",
                serde_json::json!({"endpoint_url": "opc.tcp://a:4840", "password": {"clear_secret": true}}),
            ),
            (
                "字段缺失（继承）",
                serde_json::json!({"endpoint_url": "opc.tcp://a:4840"}),
            ),
        ] {
            let out =
                resolve_connection_draft(&draft, "e1", persisted, &test_schema(false), &store)
                    .unwrap_or_else(|e| panic!("{label}: 应解析成功: {e:?}"));
            assert!(
                !contains_marker_shape(&out.connection, 0),
                "{label}: 即将发给 Driver 的载荷不得含 marker 形状: {:?}",
                out.connection
            );
            // 文本级再核一次：marker 的两种键名都不得出现在序列化结果里。
            let payload = serde_json::to_string(&out.connection).unwrap();
            assert!(
                !payload.contains("secret_set") && !payload.contains("clear_secret"),
                "{label}: 序列化载荷不得含 marker: {payload}"
            );
        }
    }
}
