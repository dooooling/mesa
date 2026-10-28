#![allow(clippy::collapsible_if)]
//! Mesa REST API（方案 §4.1）。
//!
//! 安全边界（§4.2）：仅绑定 loopback，不提供任何远程管理能力。
//! Device/Endpoint/Task CRUD + 启停 + rescan + diagnostics 已实现（§4.1 最小集）。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use mesa_config_store::{ConfigStore, DeviceRecord, EndpointRecord, StoreError};
use mesa_core_types::{AcquisitionTask, EventTask};
use mesa_driver_manager::{MesaManager, Snapshot};
use mesa_event_store::{EventFilter, EventServices, StoredEvent};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

/// Data Task 绑定门禁的唯一实现（保存门禁与 bootstrap 共用，消除双份白名单）。
pub(crate) mod binding_gate;
pub mod bootstrap;
pub mod certificates;
/// C3-A：Endpoint-bound 连接检查（尚未保存的连接草稿，只读、无副作用）。
pub(crate) mod connection_check;
/// I-2 只读管理路由（定义目录 / 权威身份预览）：零厂商语义、只按 `ErrorKind` 映射。
pub(crate) mod driver_query;
/// G7：生命周期门（Start 两阶段提交 / Stop 合流 / 停止判据观察器）。
pub mod lifecycle;
/// Issue #102：端点生命周期审计（意图先行、结果如实、epoch 可归因）。
pub mod lifecycle_audit;
pub mod recovery;
/// Issue #102：开机恢复的统一入口（审计 + 生命周期门 + 产品策略跳过记录）。
pub mod restore;
pub mod runtime;
use certificates::CertStore;

// ---------------------------------------------------------------------------
// 共享状态
// ---------------------------------------------------------------------------

pub struct AppState {
    pub snapshot: Arc<Snapshot>,
    pub manager: Arc<MesaManager>,
    pub store: Arc<ConfigStore>,
    /// G7：生命周期门（唯一排序器；不持有生命周期真值）。
    pub lifecycle: Arc<lifecycle::LifecycleGate>,
    /// 驱动目录（供 rescan 使用）。
    pub drivers_dir: String,
    pub start_time: Instant,
    pub cert_store: Arc<CertStore>,
    /// 控制面总闸：默认关闭，需 --enable-control 显式开启（§22）
    pub enable_control: bool,
    /// 事件面服务（PR7 v1.1 §20）。`None` = EventStore 不可用（§22）：
    /// `/events*` 系接口 503，Data 面不受影响。
    /// `OnceLock` 保证只注入一次；构造器签名零 churn，老调用方默认无事件。
    pub events: std::sync::OnceLock<Option<Arc<EventServices>>>,
    /// 测试专用（Issue #102 并发回归）：Bootstrap 在「事务已提交、Start 尚未提交」
    /// 处等待放行，用于**确定性**复现"Stop 先于 bootstrap Start 提交"的交错。
    /// 生产构建里该字段不存在，也不存在任何等待路径。
    #[cfg(feature = "test-hooks")]
    pub test_bootstrap_start_gate: std::sync::OnceLock<Option<Arc<tokio::sync::Semaphore>>>,
}

impl AppState {
    /// 注入事件面服务（Mesad 在 EventStore 打开后调一次；测试按需调）。
    pub fn set_event_services(&self, services: Arc<EventServices>) {
        let _ = self.events.set(Some(services));
    }

    /// 取事件面服务（`None` 即不可用，调用方转 503）。
    pub fn event_services(&self) -> Option<Arc<EventServices>> {
        self.events.get().cloned().flatten()
    }

    /// 生命周期审计写入器（Issue #102）。按需构造，只克隆一个 `Arc`，不新增状态。
    pub fn audit(&self) -> lifecycle_audit::LifecycleAudit {
        lifecycle_audit::LifecycleAudit::new(Arc::clone(&self.store))
    }

    /// 运行期审计 sink（交给 `MesaManager` 写 epoch / runtime_lost 记录）。
    pub fn audit_sink(&self) -> Arc<dyn mesa_driver_manager::attempt_audit::AttemptAuditSink> {
        Arc::new(self.audit())
    }

    /// 测试专用（`test-hooks`）：安装/清除 Bootstrap 的"事务后、Start 提交前"放行闸门。
    /// 0 许可 = 一律阻塞；测试用 `add_permits(1)` 放行一个。
    #[cfg(feature = "test-hooks")]
    pub fn set_test_bootstrap_start_gate(&self, gate: Option<Arc<tokio::sync::Semaphore>>) {
        let _ = self.test_bootstrap_start_gate.set(gate);
    }

    /// 测试专用（`test-hooks`）：读取放行闸门（生产构建不存在该字段）。
    #[cfg(feature = "test-hooks")]
    pub fn test_bootstrap_start_gate(&self) -> Option<Arc<tokio::sync::Semaphore>> {
        self.test_bootstrap_start_gate.get().cloned().flatten()
    }
}

impl AppState {
    #[allow(deprecated)]
    pub fn new(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
    ) -> Arc<Self> {
        Self::new_with_cert_dir(manager, store, drivers_dir, CertStore::default_path())
    }

    pub fn try_new_with_control(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        enable_control: bool,
    ) -> Result<Arc<Self>, String> {
        Self::try_new_with_cert_dir_and_control(
            manager,
            store,
            drivers_dir,
            CertStore::default_path(),
            enable_control,
        )
    }

    #[allow(deprecated)]
    #[deprecated(
        since = "0.1.0",
        note = "use try_new_with_control (fail-closed); fail-open constructor is test-only"
    )]
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

    #[allow(deprecated)]
    #[deprecated(
        since = "0.1.0",
        note = "use try_new_with_cert_dir_and_control; fail-open is test-only"
    )]
    pub fn new_with_cert_dir(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        cert_dir: std::path::PathBuf,
    ) -> Arc<Self> {
        Self::new_with_cert_dir_and_control(manager, store, drivers_dir, cert_dir, false)
    }

    pub fn try_new_with_cert_dir_and_control(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        cert_dir: std::path::PathBuf,
        enable_control: bool,
    ) -> Result<Arc<Self>, String> {
        let snapshot = manager.snapshot();
        let cert_store = Arc::new(CertStore::new(cert_dir));
        cert_store
            .ensure_dirs()
            .map_err(|e| format!("证书目录创建失败: {e}"))?;
        cert_store
            .ensure_own_cert()
            .map_err(|e| format!("own 证书初始化失败: {e}"))?;
        Ok(Arc::new(Self {
            snapshot,
            lifecycle: Arc::new(lifecycle::LifecycleGate::new(
                Arc::clone(&store),
                Arc::clone(&manager),
            )),
            manager,
            store,
            drivers_dir,
            start_time: Instant::now(),
            cert_store,
            enable_control,
            events: std::sync::OnceLock::new(),
            #[cfg(feature = "test-hooks")]
            test_bootstrap_start_gate: std::sync::OnceLock::new(),
        }))
    }

    #[deprecated(
        since = "0.1.0",
        note = "use try_new_with_cert_dir_and_control (fail-closed); fail-open is test-only"
    )]
    pub fn new_with_cert_dir_and_control(
        manager: Arc<MesaManager>,
        store: Arc<ConfigStore>,
        drivers_dir: String,
        cert_dir: std::path::PathBuf,
        enable_control: bool,
    ) -> Arc<Self> {
        let manager_c = manager.clone();
        let store_c = store.clone();
        let drivers_dir_c = drivers_dir.clone();
        let cert_dir_c = cert_dir.clone();
        Self::try_new_with_cert_dir_and_control(
            manager,
            store,
            drivers_dir,
            cert_dir,
            enable_control,
        )
        .unwrap_or_else(move |e| {
            tracing::error!(error=%e, "证书初始化失败（兼容旧路径，仍启动但已告警）");
            // 兼容旧 warn 路径：极端情况下仍返回实例，避免单测因证书目录不可写而崩
            let snapshot = manager_c.snapshot();
            let cert_store = Arc::new(CertStore::new(cert_dir_c));
            Arc::new(Self {
                snapshot,
                lifecycle: Arc::new(lifecycle::LifecycleGate::new(
                    Arc::clone(&store_c),
                    Arc::clone(&manager_c),
                )),
                manager: manager_c,
                store: store_c,
                drivers_dir: drivers_dir_c,
                start_time: Instant::now(),
                cert_store,
                enable_control,
                events: std::sync::OnceLock::new(),
                #[cfg(feature = "test-hooks")]
                test_bootstrap_start_gate: std::sync::OnceLock::new(),
            })
        })
    }
}

// ---------------------------------------------------------------------------
// 通用响应辅助
// ---------------------------------------------------------------------------

/// 控制面鉴权 V1（最小实现）：REST 默认仅绑定 loopback，调用方恒为本机。
/// 当前策略：actor 显式记录进审计，scope=control:execute 默认放行本机调用。
/// V2 在此接入 PolicyProvider（非 loopback / 多租户 actor 时拒绝）。
fn authorize_control(actor: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    // V1 loopback 默认放行，actor=local-api；空 actor 视为未知调用方，拒绝。
    if actor.trim().is_empty() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json_error(
                "FORBIDDEN",
                "control:execute denied: empty actor",
            )),
        ));
    }
    Ok(())
}

/// 审计更新失败只告警不阻断（控制已执行、结果已产生；STARTED 记录仍在，
/// 查询侧可识别为“未完成”，不得静默吞掉）。
fn warn_audit_update(what: &str, request_id: &str, e: mesa_config_store::StoreError) {
    tracing::warn!(request_id = %request_id, error = %e, "{what} 审计更新失败（控制结果不受影响）");
}

fn json_error(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

// ---------------------------------------------------------------------------
// Task 保存门禁：Core 统一 Descriptor 校验（PR4）
// ---------------------------------------------------------------------------

/// Data Task 保存门禁（Foundation-2 单路径 + NCK 语义点位 PR-A 双 kind 路由）：
/// 所有任务必须走 Descriptor 集合级统一校验（`mesa.resources.v1`）或点位信封层
/// 校验（`nck.points.v1`；空数组 `tasks=[]` 清空直接放行，不碰 Descriptor）。
/// issues 为空即通过；endpoint 缺失/descriptor 不可用分别映射 404/503。
///
/// **唯一实现**：Task Binding Kind 路由、绑定解析、跨 kind 的 Endpoint-wide
/// `point_key` 唯一性全部来自 [`binding_gate`]；本函数只负责编排顺序与 HTTP 映射。
/// bootstrap 创建门禁（`bootstrap.rs`）调用同一组函数，**不得**再写第二份判定。
///
/// 顺序契约（保持既有错误优先级，`mesa.resources.v1` 行为逐字不变）：
/// ① kind 路由 + nck 信封层（Descriptor 无关）→ 400；
/// ② Endpoint → 404 / Descriptor → 503；
/// ③ resources 绑定解析 → 400；④ resources 集合级语义校验；
/// ⑤ 跨 kind 的 Endpoint-wide point_key 唯一性。
async fn gate_data_tasks(
    state: &AppState,
    endpoint_id: &str,
    tasks: &[mesa_core_types::AcquisitionTask],
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let err = |code: StatusCode, v: serde_json::Value| Err((code, Json(v)));
    // 空快照 = 清空，直接放行
    if tasks.is_empty() {
        return Ok(());
    }
    // ① Task Binding Kind 路由 + nck.points.v1 信封层校验。
    // 非白名单 kind 即拒绝（legacy 已删除，无 bypass）；Core 不建 Point Kind 白名单。
    let mut issues = Vec::new();
    let (route_issues, nck_bindings) = binding_gate::route_data_task_kinds(tasks, "tasks");
    issues.extend(route_issues);
    if !issues.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "valid": false, "issues": issues }),
        );
    }
    let rec = match state.store.get_endpoint(endpoint_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err(
                StatusCode::NOT_FOUND,
                json_error("NOT_FOUND", &format!("endpoint `{endpoint_id}`")),
            );
        }
        Err(e) => return Err(store_err_to_response(e)),
    };
    let desc = match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(d) => d,
        Err(e) => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({ "error": { "code": e.code, "message": e.message } }),
            );
        }
    };
    // ③ 解析全部 generic 绑定（`mesa.resources.v1`）。
    let (parse_issues, parsed) = binding_gate::parse_generic_bindings(tasks, "tasks");
    issues.extend(parse_issues);
    // generic 解析失败即返（避免 roots 与 parsed 错位误报）
    if !issues.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "valid": false, "issues": issues }),
        );
    }
    let roots: Vec<String> = parsed
        .iter()
        .map(|(i, _)| format!("tasks[{i}].selections"))
        .collect();
    // Foundation-2 单真值：mode 由 schedule 派生（validate_task_set_against
    // 取 &TaskMode 作能力投影；此处收集 owned 值避免借用 tasks 与 parsed 交叉）。
    let modes: Vec<mesa_core_types::TaskMode> =
        parsed.iter().map(|(i, _)| tasks[*i].mode()).collect();
    let set_inputs: Vec<(
        &mesa_core_types::TaskMode,
        &[mesa_core_types::ResourceSelection],
        &str,
    )> = parsed
        .iter()
        .zip(roots.iter())
        .zip(modes.iter())
        .map(|(((_i, b), r), m)| (m, &b.selections[..], r.as_str()))
        .collect();
    // ④ resources 集合级语义校验（含 resources 内部的 Endpoint-wide 唯一性）。
    issues.extend(mesa_core_types::validate_task_set_against(
        &desc,
        &set_inputs,
    ));
    // ⑤ 跨 kind 的 Endpoint-wide point_key 唯一性（nck 内部 + nck×resources）。
    issues.extend(binding_gate::cross_kind_point_key_issues(
        &parsed,
        &nck_bindings,
        "tasks",
    ));
    if issues.is_empty() {
        Ok(())
    } else {
        err(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "valid": false, "issues": issues }),
        )
    }
}

/// 事件 Task 保存门禁（Foundation-2 单路径）：所有事件任务必须走
/// `desc.events` 目录校验（`mesa.events.v1` 唯一合法 kind；空数组直接放行）。
async fn gate_event_tasks(
    state: &AppState,
    endpoint_id: &str,
    tasks: &[mesa_core_types::EventTask],
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let err = |code: StatusCode, v: serde_json::Value| Err((code, Json(v)));
    if tasks.is_empty() {
        return Ok(());
    }
    // 非 generic kind 即拒绝（legacy 已删除，无 bypass）
    let mut issues = Vec::new();
    for (i, task) in tasks.iter().enumerate() {
        if task.binding.kind != mesa_core_types::GENERIC_EVENT_BINDING_KIND {
            issues.push(mesa_core_types::ValidationIssue {
                path: format!("event_tasks[{i}].binding.kind"),
                code: "UNSUPPORTED_BINDING".into(),
                message: format!(
                    "binding kind `{}` 不再支持，仅允许 {}",
                    task.binding.kind,
                    mesa_core_types::GENERIC_EVENT_BINDING_KIND,
                ),
            });
        }
    }
    if !issues.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "valid": false, "issues": issues }),
        );
    }
    let rec = match state.store.get_endpoint(endpoint_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err(
                StatusCode::NOT_FOUND,
                json_error("NOT_FOUND", &format!("endpoint `{endpoint_id}`")),
            );
        }
        Err(e) => return Err(store_err_to_response(e)),
    };
    let desc = match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(d) => d,
        Err(e) => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({ "error": { "code": e.code, "message": e.message } }),
            );
        }
    };
    //（前置 kind 门已保证全为 generic，此处不再跳过任何任务）
    for (i, task) in tasks.iter().enumerate() {
        let root = format!("event_tasks[{i}]");
        match mesa_core_types::GenericEventBinding::from_json(&task.binding.config) {
            Ok(binding) => issues.extend(mesa_core_types::validate_event_binding_against(
                &desc.events,
                &task.mode,
                &binding,
                &root,
            )),
            Err(e) => issues.push(mesa_core_types::ValidationIssue {
                path: format!("{root}.binding"),
                code: "INVALID_BINDING_CONFIG".into(),
                message: e,
            }),
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        err(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "valid": false, "issues": issues }),
        )
    }
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

/// 判断是否为显式删除 Secret 标记 {"clear_secret": true}
fn is_clear_secret_marker(v: &serde_json::Value) -> bool {
    v.is_object()
        && v.as_object()
            .map(|m| m.get("clear_secret") == Some(&serde_json::Value::Bool(true)))
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

/// Secret 引用失败的两种语义。**必须分开映射，不得合并**：
///
/// - [`SecretRefError::Missing`]：`{"secret_set": true}` 指向的 Secret 在**该
///   Endpoint、该字段**上不存在。调用方按「引用无效」处理（REST 400）。用户在
///   界面上看到「已保存凭据」而服务端却查无记录时必须明说，绝不能静默降级成
///   不带凭据的连接（那会把「凭据丢了」伪装成「设备不需要认证」）。
/// - [`SecretRefError::Store`]：SecretStore 基础设施故障（`master.key` 缺失或
///   内容非法、AEAD 解密失败、SQLite 错误）。调用方**一律**映射 503，这是硬
///   红线：密钥丢失/密文损坏既不是参数错误也不是设备不可达，把它显示成后者会
///   引导用户去查网络，而真正要修的是密钥；更不允许降级为匿名连接。
#[derive(Debug)]
pub(crate) enum SecretRefError {
    /// 引用无效：marker 存在但该 Endpoint 上没有这条 Secret（字段名不含明文）。
    Missing(String),
    /// SecretStore 不可用（原因保留在 StoreError 内，绝不篡改分类）。
    Store(StoreError),
}

/// SecretStore 读取的**唯一**底层入口（C1 运行期构造、PUT 的 Secret 分支与 C3
/// 草稿检查共用）。
///
/// 为什么值得单独抽一层：调用方需要区分的是「没有这条 Secret」（`Ok(None)`，
/// 语义由调用方决定）与「读不出来」（`Err`，一律基础设施故障），而
/// `StoreError` 自身无法表达这个区别（缺记录与解密失败在业务语义上完全不同）。
///
/// 为什么 PUT 也要走这里：`update_endpoint()` 的「marker 保留」与「缺失继承」
/// 两个分支都直接查 SecretStore，而 C3 的 `connection-check` 必须与它逐字段
/// 同源（否则会出现「PUT 会继承、检查却不继承」的偏差）。让两侧共用同一个函数
/// 后，这种对齐是**代码级事实**而不是两段相似代码的巧合；函数体是直通的
/// `get_secret`，行为与错误类型完全不变。
pub(crate) fn read_stored_secret(
    store: &ConfigStore,
    endpoint_id: &str,
    field: &str,
) -> Result<Option<String>, StoreError> {
    store.get_secret(endpoint_id, field)
}

/// C1 Secret 物化原语（仅内存）：把对象上 `{"secret_set": true}` marker 就地
/// 还原为明文，返回被物化的字段名（**只有字段名，不含明文**，供诊断与测试）。
///
/// 为什么是共享函数而不是各自实现：运行期 Endpoint 构造
/// （[`materialize_connection`]）与 C3 的**内存草稿**检查必须使用完全相同的
/// marker 语义。两处各写一遍，迟早会在「什么算 marker」「marker 缺 Secret 时
/// 算失败还是算空」上分叉，而这类分叉的后果就是把 marker 当密码发给 Driver，
/// 或把丢凭证伪装成匿名连接。本函数只做物化、不做策略：缺记录与读失败一律
/// 上抛，由调用方映射状态码。
pub(crate) fn materialize_secret_markers_in_place(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    endpoint_id: &str,
    store: &ConfigStore,
    secret_keys: &[String],
) -> Result<Vec<String>, SecretRefError> {
    let mut materialized = Vec::new();
    for key in secret_keys {
        if let Some(cur) = obj.get(key).cloned() {
            if is_secret_marker(&cur) {
                match read_stored_secret(store, endpoint_id, key).map_err(SecretRefError::Store)? {
                    Some(pt) => {
                        obj.insert(key.clone(), serde_json::Value::String(pt));
                        materialized.push(key.clone());
                    }
                    None => return Err(SecretRefError::Missing(key.clone())),
                }
            }
        }
    }
    Ok(materialized)
}

/// 启动/探测时：将 marker 临时还原为明文（仅内存，不写盘）。
///
/// fail-closed（C1 P0）：`connection_json` 必须是**合法 JSON 对象**。此前用
/// `unwrap_or(json!({}))` 把解析失败悄悄变成空对象，等于把一份坏配置当成
/// “没有连接参数”继续启动——既掩盖了存储损坏，也可能让 Driver 回落到自己的
/// 默认连接目标。坏 JSON / 非对象一律明确失败。
fn materialize_connection(
    conn_str: &str,
    endpoint_id: &str,
    store: &ConfigStore,
    secret_keys: &[String],
) -> Result<String, StoreError> {
    let mut v: serde_json::Value = serde_json::from_str(conn_str).map_err(StoreError::Json)?;
    let Some(obj) = v.as_object_mut() else {
        return Err(StoreError::Validation(format!(
            "connection_json 必须是 JSON 对象（endpoint `{endpoint_id}`）"
        )));
    };
    match materialize_secret_markers_in_place(obj, endpoint_id, store, secret_keys) {
        Ok(_) => {}
        // 错误映射逐字保持 C1 既有行为（错误文本被既有测试与运维检索依赖）
        Err(SecretRefError::Missing(key)) => {
            return Err(StoreError::Validation(format!(
                "secret `{key}` marker 存在但 SecretStore 无记录 (endpoint `{endpoint_id}`)"
            )));
        }
        Err(SecretRefError::Store(e)) => return Err(e),
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
    // 统一 Schema Validator（Core 唯一实现）：required/类型/enum/min/max/
    // pattern（真 regex）/未知字段，一次返回全部问题。
    let issues = desc.connection.validate_instance("connection", &conn_val);
    if issues.is_empty() {
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

/// 探测结果的**唯一** HTTP 映射点（既有 `/drivers/{id}/probe` 与 C3
/// `POST /endpoints/{id}/connection-check` 的 `mode=probe` 共用）。
///
/// 为什么必须共用而不是各写一套：状态码分层是冻结契约（§5：200 可达/不可达、
/// 400 输入、404 驱动不存在、501 不支持探测、503 基础设施、504 超时）。两份
/// 相似代码迟早会在某个分支上分叉，而分叉的表现是「同一个探测在两个入口给出
/// 不同诊断」，用户与运维都会据此得出错误结论。
///
/// 分层口径：
/// - 200：探测**完成**（`reachable:false` 也是探测结果，不是错误）；
/// - 404/400：Driver 不存在 / 输入问题（含 Driver 侧 ConfigurationError）；
/// - 501：Driver 不支持探测；503：spawn/握手/RPC 等基础设施失败；504：超时。
///
/// NOTE（脱敏职责不在这里）：本函数只做形状与状态码映射，**不做**凭据擦除。
/// 调用方（C3 路径）负责在下发前按 Descriptor 的 Secret 字段做值级擦除——
/// 因为只有它同时掌握 Schema 与本次物化出来的明文；既有 `/drivers/{id}/probe`
/// 的调用方自带明文（无 SecretStore 参与），返回值也只回到该调用方。
pub(crate) fn probe_result_to_response(
    result: Result<mesa_core_types::ProbeReport, mesa_driver_manager::probe::ProbeError>,
) -> (StatusCode, Json<serde_json::Value>) {
    match result {
        Ok(r) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "reachable": r.reachable,
                "device": {
                    "vendor": r.vendor,
                    "family": r.family,
                    "model": r.model,
                    "firmware": r.firmware,
                },
                "model_confidence": r.model_confidence,
                "capabilities": r.capabilities,
                "warnings": r.warnings,
            })),
        ),
        Err(e) => {
            use mesa_driver_manager::probe::ProbeError;
            match e {
                ProbeError::DriverNotFound(_) => (
                    StatusCode::NOT_FOUND,
                    Json(json_error("NOT_FOUND", &e.to_string())),
                ),
                ProbeError::Unsupported(_) => (
                    StatusCode::NOT_IMPLEMENTED,
                    Json(json_error("PROBE_UNSUPPORTED", &e.to_string())),
                ),
                ProbeError::InvalidInput { code, message } => {
                    (StatusCode::BAD_REQUEST, Json(json_error(&code, &message)))
                }
                ProbeError::Spawn(_) | ProbeError::Handshake { .. } => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json_error("DRIVER_UNAVAILABLE", &e.to_string())),
                ),
                ProbeError::Rpc(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json_error("PROBE_FAILED", &e.to_string())),
                ),
                ProbeError::RpcTimeout | ProbeError::Timeout { .. } => (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(json_error("PROBE_TIMEOUT", &e.to_string())),
                ),
            }
        }
    }
}

/// 动态探测（§8）：纯 HTTP 映射，编排（临时进程/Probe RPC/匹配/清理）
/// 全部收敛在 `MesaManager::probe()`，本层只做形状转换与状态码映射。
/// - 200：探测完成（含设备不可达 `reachable:false`，那是探测结果）；
/// - 404/400：输入问题；501：Driver 不支持；503：基础设施失败；504：超时。
async fn probe_driver(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
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
    probe_result_to_response(state.manager.probe(&id, &conn_str).await)
}

#[derive(Debug, serde::Deserialize)]
struct BrowseReq {
    parent: Option<String>,
    filter: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// Browse 节点 → REST JSON（**语义中立**的纯透传层）。
///
/// 语义中立是硬约束（AGENTS.md「Core 不懂协议」）：本函数只做字段搬运，不解释、
/// 不补齐、不过滤。IPC 1.8 的选择契约三字段同样如此：
///
/// - `selection_mode` 原样给**数值**（`0` = 旧驱动未声明）：Core 不把它映射成
///   字符串、不判断「可不可选」，也不因为它是 0 就补默认值——那是驱动与消费端
///   的事；
/// - `selection_payload_json` 是驱动私有 JSON：Core 不解析、不校验、不裁剪
///   （载荷畸形必须由消费端 fail-closed）；
/// - `diagnostics[].code` 是驱动自定义稳定码：Core 不维护码表、不改写文案。
///
/// 旧驱动（IPC < 1.8）不填这三项 ⇒ JSON 里就是 `0` / `""` / `[]`，与 1.7 的
/// 语义完全等价（消费端据此走既有通用规则），因此**普通资源浏览不受影响**。
///
/// 这里出现任何品牌/协议字面量都属架构错误——这是契约测试要拦的事。
fn browse_nodes_json(nodes: Vec<mesa_driver_protocol::pb::BrowseNode>) -> Vec<serde_json::Value> {
    nodes
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
                "selection_mode": n.selection_mode,
                "selection_payload_json": n.selection_payload_json,
                "diagnostics": n
                    .diagnostics
                    .into_iter()
                    .map(|d| {
                        serde_json::json!({
                            "code": d.code,
                            "message": d.message,
                            "severity": d.severity,
                        })
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
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
    // Browse 同 Start 一样需 materialize Secret，避免把 marker 交给 Driver。
    // 复用运行期构造的同一物化语义（含坏 JSON / 缺 Secret 的 fail-closed）。
    let browse_conn = match runtime::materialize_for_runtime(&state, &rec).await {
        Ok(s) => s,
        Err(e) => return (e.status, e.to_json()),
    };
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
            let nodes_json = browse_nodes_json(nodes);
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

/// Foundation-3 #5.1 门：Control 请求入口比普通配置更 fail-closed。
/// 未知字段（尤其 `expected_valeu` 这类 CAS 拼写错误）必须 400，
/// 绝不静默退化为无条件写（expected=None）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlWriteReq {
    // Foundation-3 单真值：结构化三元组（旧 string target 已删除，不留 shim）。
    // value 唯一入口（JSON 标量经 json_to_value 映射，tag-object 经 Value
    // 反序列化；第二入口 value_typed 已删除——终审 #4：双 value 真值歧义）。
    target: mesa_core_types::WriteTarget,
    value: serde_json::Value,
    expected_value: Option<serde_json::Value>,
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
    let target = body.target;
    if target.resource_id.trim().is_empty() || target.output.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json_error(
                "VALIDATION_ERROR",
                "target.resource_id/output required",
            )),
        );
    }
    let value = match json_to_value(&body.value) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json_error("VALIDATION_ERROR", &e)),
            );
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
    // Foundation-3 Core Descriptor 门禁（结构化 WriteTarget）：
    // descriptor → capabilities.write → resource/output/access →
    // parameters schema → value/expected 类型。issues 非空即 400，
    // 绝不把非法 target 送到 Driver。
    let desc = match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
            );
        }
    };
    let (issues, _) =
        mesa_core_types::gate_write_against(&desc, &target, &value, &expected, "target");
    if !issues.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "valid": false, "issues": issues })),
        );
    }
    // 控制面鉴权（V1 最小实现：本机 actor 放行并记录；见 authorize_control）。
    if let Err(e) = authorize_control("local-api") {
        return e;
    }
    // 审计 STARTED（同步插入；插入失败则拒绝控制——记不下来就不执行，
    // 绝不产生“执行了但无审计”的控制）。
    // operation_id = resource_id/output 稳定身份（parameters 进 request_json）；
    // Driver 私有地址字符串彻底退出审计。
    let request_id = format!("api-wr-{}-{}", id, mesa_core_types::now_unix_ns());
    let audit_started = mesa_config_store::ControlAuditRecord {
        request_id: request_id.clone(),
        endpoint_id: id.clone(),
        actor: "local-api".into(),
        operation_type: "write".into(),
        operation_id: target.audit_id(),
        request_json: serde_json::json!({"target": target, "value": format!("{value:?}"), "expected": expected.as_ref().map(|v| format!("{v:?}"))})
            .to_string(),
        result_json: None,
        status: "STARTED".into(),
        started_at_ns: mesa_core_types::now_unix_ns(),
        finished_at_ns: None,
    };
    if let Err(e) = state.store.insert_control_audit(&audit_started) {
        tracing::error!(request_id = %request_id, error = %e, "控制审计 STARTED 写入失败，拒绝执行控制");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json_error(
                "AUDIT_UNAVAILABLE",
                "control audit store unavailable, control refused",
            )),
        );
    }
    match state
        .manager
        .control_write(&id, &target, value, expected, &request_id)
        .await
    {
        Ok(readback) => {
            let detail =
                serde_json::json!({"readback": readback.as_ref().map(|v| format!("{v:?}"))})
                    .to_string();
            if let Err(e) = state.store.update_control_audit(
                &request_id,
                "COMPLETED",
                Some(&detail),
                mesa_core_types::now_unix_ns(),
            ) {
                warn_audit_update("control_audit COMPLETED", &request_id, e);
            }
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
            // write 路径：精确码直透（#5.2）。
            if let Err(e) = state.store.update_control_audit(
                &request_id,
                "FAILED",
                Some(&e.message),
                mesa_core_types::now_unix_ns(),
            ) {
                warn_audit_update("control_audit FAILED", &request_id, e);
            }
            // #5.2：精确码直透（EXPECTED_MISMATCH / EXPECTED_VALUE_UNSUPPORTED /
            // TARGET_NOT_FOUND / OUTPUT_NOT_WRITABLE 等）；ENDPOINT_NOT_RUNNING
            // 为 409；CONTROL_FAILED/CONTROL_MODEL_UNSUPPORTED 等传输/模型层
            // 才为 502。客户端按 error.code 做 machine-readable 路由。
            let status = if e.code == "ENDPOINT_NOT_RUNNING" {
                StatusCode::CONFLICT
            } else if e.code == "CONTROL_FAILED" || e.code == "CONTROL_MODEL_UNSUPPORTED" {
                StatusCode::BAD_GATEWAY
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
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
#[serde(deny_unknown_fields)]
struct ControlCommandReq {
    // Foundation-3 单真值：input 唯一合法形态（command_id 取 URL path）。
    // 旧 command_id/command/input_json 形态已删除，不留 shim。
    // 反序列化失败（含未知字段）即 400，不 fallback 为空 input（#5.1）。
    #[serde(default)]
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
    // command_id 真值 = URL path；input 真值 = body.input（缺省 {}）。
    // 旧 body 形态（command_id/command/input_json）整体拒绝——双真值已删除。
    if let Some(obj) = body.as_object() {
        for legacy in ["command_id", "command", "input_json"] {
            if obj.contains_key(legacy) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json_error(
                        "VALIDATION_ERROR",
                        &format!(
                            "body.{legacy} 已删除：command_id 取 URL path，input 取 body.input"
                        ),
                    )),
                );
            }
        }
    }
    let command_id = cmd;
    // #5.1：反序列化失败（含未知字段/拼写错误）即 400，不 fallback 空 input。
    let parsed: ControlCommandReq = match serde_json::from_value(body.clone()) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json_error("VALIDATION_ERROR", &e.to_string())),
            );
        }
    };
    let input = parsed.input.unwrap_or(serde_json::json!({}));
    let input_json = serde_json::to_string(&input).unwrap_or("{}".into());
    // Foundation-3 Core Descriptor 门禁：capabilities.method → command 声明 →
    // input 过 input_schema。不存在的 command 绝不送 Driver。
    let desc = match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": { "code": e.code, "message": e.message } })),
            );
        }
    };
    let issues = mesa_core_types::gate_command_against(&desc, &command_id, &input, "command");
    if !issues.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "valid": false, "issues": issues })),
        );
    }
    // 控制面鉴权（V1 最小实现，与 control_write 同策略）。
    if let Err(e) = authorize_control("local-api") {
        return e;
    }
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
    if let Err(e) = state.store.insert_control_audit(&audit_started) {
        tracing::error!(request_id = %request_id, error = %e, "控制审计 STARTED 写入失败，拒绝执行控制");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json_error(
                "AUDIT_UNAVAILABLE",
                "control audit store unavailable, control refused",
            )),
        );
    }
    match state
        .manager
        .control_command(&id, &command_id, &input_json, &request_id)
        .await
    {
        Ok((status, result_json, error)) => {
            // Foundation-3 结果门禁（终审 #2 修正 + status 白名单 fail-closed）：
            // 先验证 status（proto 只允许 Succeeded | Failed | TimedOut |
            // Rejected | Cancelled）：未知 status 即 DRIVER_CONTRACT_VIOLATION
            //（审计 FAILED，HTTP 502），不得当正常业务失败接受。
            // 只有 Succeeded 才检查 result_schema；Failed 等正常终态直通。
            // HTTP 语义：Succeeded(+schema 通过)→200；业务失败→200+status；
            // schema 违反/未知 status→502 DRIVER_CONTRACT_VIOLATION。
            let result_val: serde_json::Value = serde_json::from_str(&result_json)
                .unwrap_or(serde_json::Value::String(result_json.clone()));
            match classify_command_status(&status) {
                CommandStatusClass::BusinessFailure => {
                    let detail = format!("{status}:{result_json}:{error}");
                    if let Err(e) = state.store.update_control_audit(
                        &request_id,
                        "FAILED",
                        Some(&detail),
                        mesa_core_types::now_unix_ns(),
                    ) {
                        warn_audit_update("control_audit result", &request_id, e);
                    }
                    return (
                        StatusCode::OK,
                        Json(
                            serde_json::json!({"request_id": request_id, "status": status, "result": result_val, "error": error}),
                        ),
                    );
                }
                CommandStatusClass::Unknown => {
                    let detail = format!(
                        "DRIVER_CONTRACT_VIOLATION:unknown command status `{status}`:{result_json}:{error}"
                    );
                    if let Err(e) = state.store.update_control_audit(
                        &request_id,
                        "FAILED",
                        Some(&detail),
                        mesa_core_types::now_unix_ns(),
                    ) {
                        warn_audit_update("control_audit result", &request_id, e);
                    }
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(
                            serde_json::json!({"error": {"code": "DRIVER_CONTRACT_VIOLATION", "message": detail}, "request_id": request_id}),
                        ),
                    );
                }
                CommandStatusClass::Succeeded => {}
            }
            let result_issues = mesa_core_types::gate_command_result_against(
                &desc,
                &command_id,
                &result_val,
                "command",
            );
            if result_issues.is_empty() {
                // 能到这里 status 必为 Succeeded（白名单已分流），审计 COMPLETED。
                let detail = format!("{status}:{result_json}:{error}");
                if let Err(e) = state.store.update_control_audit(
                    &request_id,
                    "COMPLETED",
                    Some(&detail),
                    mesa_core_types::now_unix_ns(),
                ) {
                    warn_audit_update("control_audit result", &request_id, e);
                }
                (
                    StatusCode::OK,
                    Json(
                        serde_json::json!({"request_id": request_id, "status": status, "result": result_val, "error": error}),
                    ),
                )
            } else {
                let detail = format!("DRIVER_CONTRACT_VIOLATION:{result_json}:{error}");
                if let Err(e) = state.store.update_control_audit(
                    &request_id,
                    "FAILED",
                    Some(&detail),
                    mesa_core_types::now_unix_ns(),
                ) {
                    warn_audit_update("control_audit result", &request_id, e);
                }
                (
                    StatusCode::BAD_GATEWAY,
                    Json(
                        serde_json::json!({"error": {"code": "DRIVER_CONTRACT_VIOLATION", "message": detail}, "request_id": request_id}),
                    ),
                )
            }
        }
        Err(e) => {
            if let Err(e) = state.store.update_control_audit(
                &request_id,
                "FAILED",
                Some(&e.message),
                mesa_core_types::now_unix_ns(),
            ) {
                warn_audit_update("control_audit FAILED", &request_id, e);
            }
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

/// Command status 白名单分类（终审 fail-closed）：proto 只允许
/// `Succeeded | Failed | TimedOut | Rejected | Cancelled`。
/// 未知 status（如 `""` / `"Failure"` / `"Bogus"`，含大小写漂移）即
/// Driver contract violation，不得当正常业务失败接受（HTTP 200）。
#[derive(Debug, PartialEq)]
enum CommandStatusClass {
    Succeeded,
    BusinessFailure,
    Unknown,
}

fn classify_command_status(status: &str) -> CommandStatusClass {
    match status {
        "Succeeded" => CommandStatusClass::Succeeded,
        "Failed" | "TimedOut" | "Rejected" | "Cancelled" => CommandStatusClass::BusinessFailure,
        _ => CommandStatusClass::Unknown,
    }
}

#[cfg(test)]
mod command_status_tests {
    use super::*;

    /// 终审回归门：status 白名单 fail-closed。
    /// `Bogus`/`""`/`"Failure"`（含大小写漂移）必须判 Unknown，
    /// 绝不能按普通 Failed 返回 200。
    #[test]
    fn command_status_whitelist_is_fail_closed() {
        assert_eq!(
            classify_command_status("Succeeded"),
            CommandStatusClass::Succeeded
        );
        for s in ["Failed", "TimedOut", "Rejected", "Cancelled"] {
            assert_eq!(
                classify_command_status(s),
                CommandStatusClass::BusinessFailure,
                "{s}"
            );
        }
        for s in [
            "",
            "Failure",
            "Bogus",
            "succeeded",
            "FAILED",
            "Timeout",
            "Cancel",
        ] {
            assert_eq!(
                classify_command_status(s),
                CommandStatusClass::Unknown,
                "{s}"
            );
        }
    }
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    endpoint_id: Option<String>,
    status: Option<String>,
    /// `write` / `command` / `lifecycle`（Issue #102）：只取某一类审计。
    operation_type: Option<String>,
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
        q.operation_type.as_deref(),
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
    // R4：descriptor 按 driver 缓存（同 driver 只拉一次；500 endpoints 同
    // driver 时 N+1 await 是 541ms 的主因）。失败缓存 None（保守置空 connection）。
    let mut desc_cache: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let mut merged: Vec<serde_json::Value> = Vec::with_capacity(stored.len());
    for rec in stored {
        let runtime = live.get(&rec.id).cloned();
        let mut conn: serde_json::Value =
            serde_json::from_str(&rec.connection_json).unwrap_or(serde_json::json!({}));
        // 脱敏：同 get_endpoint，保持列表与详情一致（避免历史明文泄露）
        let secret_keys = match desc_cache.get(&rec.driver_id) {
            Some(cached) => cached.clone(),
            None => {
                let keys = match state.manager.get_descriptor(&rec.driver_id).await {
                    Ok(desc) => Some(secret_field_keys(&desc.connection)),
                    Err(_) => None,
                };
                desc_cache.insert(rec.driver_id.clone(), keys.clone());
                keys
            }
        };
        match secret_keys {
            Some(keys) if !keys.is_empty() => {
                conn = redact_connection_for_response(conn, &rec.id, &state.store, &keys);
            }
            None => {
                // 保守策略：Descriptor 不可用则直接不返回 connection
                conn = serde_json::Value::Null;
            }
            _ => {}
        }
        merged.push(serde_json::json!({
            "id": rec.id,
            "name": rec.name,
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
    // 单次快照：points 与 count 同源（旧实现调了两次 latest_all，O(2N)）。
    let points = state.snapshot.latest_all();
    let count = points.len();
    Json(serde_json::json!({ "points": points, "count": count }))
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
}

#[derive(Debug, Deserialize)]
struct UpdateDeviceReq {
    name: String,
}

/// P2 改名请求：Some(名) 设置；None 清除（回落 point_key）。
/// 空字符串由 store 层拒绝（400）。
/// fail-closed：字段缺失（`{}`）必须 400——`{}` 与 `{"display_name": null}`
/// 都是“清除”将是隐式 destructive。双层 Option 显式区分：
/// 缺失（None）→ 400，null（Some(None)）→ 清除，string → 设置。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePointDisplayNameReq {
    #[serde(default, deserialize_with = "de_display_name_arg")]
    display_name: Option<Option<String>>,
}

/// 反序列化显示名参数：null → Some(None)；string → Some(Some)；其他 → 400。
/// 字段缺失不经过这里（`default` 给 None）。
fn de_display_name_arg<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

#[cfg(test)]
mod control_auth_tests {
    use super::*;

    /// 控制面鉴权 V1：本机 actor 放行，空 actor 拒绝（fail-closed）。
    #[test]
    fn local_actor_allowed_empty_denied() {
        assert!(authorize_control("local-api").is_ok());
        let err = authorize_control("").unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        let err2 = authorize_control("   ").unwrap_err();
        assert_eq!(err2.0, StatusCode::FORBIDDEN);
    }
}

#[cfg(test)]
mod display_name_req_tests {
    use super::*;

    /// P2 收4：`{}`（缺失）与 `null`（明确清除）必须区分；其他类型 400。
    #[test]
    fn missing_null_string_other_are_distinct() {
        let missing: UpdatePointDisplayNameReq = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.display_name, None, "{{}} 解析为缺失");
        let null: UpdatePointDisplayNameReq =
            serde_json::from_str(r#"{"display_name":null}"#).unwrap();
        assert_eq!(null.display_name, Some(None), "null 解析为清除");
        let set: UpdatePointDisplayNameReq =
            serde_json::from_str(r#"{"display_name":"名"}"#).unwrap();
        assert_eq!(
            set.display_name,
            Some(Some("名".to_string())),
            "string 解析为设置"
        );
        assert!(
            serde_json::from_str::<UpdatePointDisplayNameReq>(r#"{"display_name":123}"#).is_err()
        );
    }
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

/// P2：点展示名改名（用户元数据）。
/// - body `{"display_name": "名"}` 设置；`{"display_name": null}` 清除（回落 point_key）。
/// - 空字符串 400（意图清空请传 null）；point 不存在/墓碑 404。
/// - 只写 registry + 同步 snapshot；point_id/key/label 不动；Driver 无感知。
async fn update_point_display_name(
    State(state): State<Arc<AppState>>,
    Path((endpoint_id, point_key)): Path<(String, String)>,
    Json(body): Json<UpdatePointDisplayNameReq>,
) -> (StatusCode, Json<serde_json::Value>) {
    // 三态：Missing → 400（`{}` 不得隐式清除）；Null → 清除；String → 设置。
    let name: Option<String> = match body.display_name {
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json_error(
                    "VALIDATION_ERROR",
                    "display_name 字段缺失（设置传 string，清除传 null）",
                )),
            );
        }
        Some(inner) => inner,
    };
    match state
        .store
        .set_point_display_name(&endpoint_id, &point_key, name.as_deref())
    {
        Ok(()) => {}
        Err(e) => return store_err_to_response(e),
    }
    // 同步运行时快照（point 不存在于 snapshot 即尚未 register，update 静默无操作）。
    let pid = state
        .store
        .point_map(&endpoint_id)
        .ok()
        .and_then(|m| m.get(&point_key).copied());
    if let Some(id) = pid {
        state
            .snapshot
            .update_display_name(&endpoint_id, id, name.clone());
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "endpoint_id": endpoint_id,
            "point_key": point_key,
            "display_name": name,
        })),
    )
}

async fn delete_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.store.delete_device(&id) {
        // RC2 修3：删除设备同时清理其 bootstrap 幂等记录（第二层防御）。
        // 主防御是前端 per-operation key（删后重建即新 key）；此处防止表
        // 永久增长 + 旧 key 残留。清理失败不翻转删除成功（设备已删是事实），
        // 只记日志（幂等表不可写时旧记录可能残留，同 key 仍可能 replay；
        // V2 UI 已用 per-operation key，删后重建即新 key，不受此路径影响）。
        Ok(true) => {
            if let Err(e) = state.store.bootstrap_idempotency_delete_by_device(&id) {
                tracing::warn!(device_id = %id, error = %e, "delete_device 成功但幂等记录清理失败");
            }
            (StatusCode::OK, Json(serde_json::json!({ "deleted": id })))
        }
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
    name: String,
    device_id: String,
    driver_id: String,
    connection: serde_json::Value,
}

/// Update shape（PR25）：`driver_id` 已从形状移除（创建后不可变），传入即
/// `deny_unknown_fields` 拒绝，不再是“传入后检查不能变”。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateEndpointReq {
    name: String,
    device_id: String,
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
            // 脱敏：若为 Secret 字段则返回 marker 而非明文；Descriptor 不可用时一律不返回 connection，避免历史明文泄露
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
                    // 保守策略：Descriptor 不可用则无法判断哪些字段为 Secret，直接不返回 connection
                    conn = serde_json::Value::Null;
                }
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": rec.id, "name": rec.name,
                    "device_id": rec.device_id, "driver_id": rec.driver_id,
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
            // PR4 门禁：connection 先过统一校验（明文 secret 即 string，合法形态）
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
        name: body.name.clone(),
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
    // driver_id 创建后不可变：Update 形状已移除该字段（deny_unknown_fields），
    // 此处无任何 driver_id 写入路径；Store UPDATE 同样永不触碰该列。
    rec.device_id = body.device_id.clone();
    rec.name = body.name.clone();
    // Secret 正式语义（P0）：明文→更新；marker→保留（须存在）；
    // 缺失→保留旧值；显式 {"clear_secret": true}→删除。
    // 先构造 logical candidate（真实值）跑统一校验，通过后再落盘 marker 形态。
    let mut conn_val = body.connection.clone();
    let mut secrets_to_upsert: Vec<(String, String)> = Vec::new();
    let mut secrets_to_delete: Vec<String> = Vec::new();
    match state.manager.get_descriptor(&rec.driver_id).await {
        Ok(desc) => {
            let secret_keys = secret_field_keys(&desc.connection);
            // logical candidate：各 secret 字段填真实值（明文/物化旧值/继承旧值）
            let mut logical = conn_val.clone();
            if !secret_keys.is_empty() {
                let mut logical_obj = logical.as_object_mut();
                let Some(obj) = conn_val.as_object_mut() else {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json_error(
                            "VALIDATION_ERROR",
                            "connection 必须为 JSON 对象",
                        )),
                    );
                };
                for sk in &secret_keys {
                    match obj.get(sk).cloned() {
                        Some(v) if v.is_string() => {
                            // 明文：logical 用真实新值；persistence 换 marker
                            secrets_to_upsert.push((sk.clone(), v.as_str().unwrap().to_string()));
                            obj.insert(sk.clone(), serde_json::json!({"secret_set": true}));
                        }
                        Some(v) if is_secret_marker(&v) => {
                            // marker：须存在旧值；logical 物化真实旧值参检
                            //（走唯一读入口 `read_stored_secret`，与 C3 检查同源）
                            match read_stored_secret(&state.store, &id, sk) {
                                Ok(Some(pt)) => {
                                    if let Some(lo) = logical_obj.as_mut() {
                                        lo.insert(sk.clone(), serde_json::Value::String(pt));
                                    }
                                }
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
                        Some(v) if is_clear_secret_marker(&v) => {
                            // 显式删除：logical 与 persistence 同时移除
                            secrets_to_delete.push(sk.clone());
                            obj.remove(sk);
                            if let Some(lo) = logical_obj.as_mut() {
                                lo.remove(sk);
                            }
                        }
                        Some(v) => {
                            // 非 string 非 marker 非 clear（如数字/数组）：保持原样，
                            // 交给统一校验报 INVALID_TYPE，不在这里猜测意图
                            if let Some(lo) = logical_obj.as_mut() {
                                lo.insert(sk.clone(), v);
                            }
                        }
                        None => {
                            // 缺失 → 保留旧值：旧值存在则 logical 继承真实值参检，
                            // persistence 保持 marker；无旧值则两侧都缺失
                            //（required 与否由统一校验裁决）。
                            // 这里是 C3 `connection-check` 的缺失继承**对齐基准**：
                            // 直接查 SecretStore（唯一读入口）而不是解析
                            // `connection_json`，因此连接检查必须同样先查 SecretStore。
                            match read_stored_secret(&state.store, &id, sk) {
                                Ok(Some(pt)) => {
                                    if let Some(lo) = logical_obj.as_mut() {
                                        lo.insert(sk.clone(), serde_json::Value::String(pt));
                                    }
                                    obj.insert(sk.clone(), serde_json::json!({"secret_set": true}));
                                }
                                Ok(None) => {}
                                Err(e) => return store_err_to_response(e),
                            }
                        }
                    }
                }
            }
            // 统一校验跑在 logical 真实值上（含 Secret regex 等规则）
            let conn_issues = desc.connection.validate_instance("connection", &logical);
            if !conn_issues.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "valid": false, "issues": conn_issues })),
                );
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
    // G7：判据必须是 **attempt 真值**而不是 `is_running()`。拆解期 `is_running()`
    // 已经是 false，但 attempt 条目仍在（正在等驱动进程退出）；此时删除配置会
    // 留下"配置没了、驱动任务还在收尾/隔离"的孤儿，且迟到状态写入可能复活端点。
    // 隔离态（FailedUnknown）同样命中——复核通过前不得删配置。
    if state.manager.has_live_attempt(&id) {
        return (
            StatusCode::CONFLICT,
            Json(json_error(
                "CONFLICT",
                "endpoint 正在运行，请先停止后再删除",
            )),
        );
    }
    // P0-D/P0-E-2：打开删除屏障。返回值是 **RAII guard**：handler 无论从哪条
    // 路径离开（等待超时、409、404、存储错误、客户端断开的取消、panic），只要
    // 没有 `commit()`，Drop 就会撤销屏障——修复前这些显式 `abort_delete` 在
    // 取消/panic 路径上根本执行不到，`deleting` 永久为 true，该端点此后所有
    // Start 都被 409 拒绝。
    //
    // 屏障只挡 Start 提交（门内 409 `ENDPOINT_DELETED`）；**Stop 的意图持久化
    // 照常进入**（P0-E-1），因此下面的等待必须配合删行前的串行器复查。
    let barrier = state.lifecycle.begin_delete(&id);
    // P0-D：**等已有的在途意图写入真正收敛**（不是清空标记）。teardown 已完成、
    // `desired=false` 仍在写库的 Stop，必须在删行之前落完：否则删行 + 同 ID 重建
    // + Start 之后，那条迟到的 `false` 会写到新实例上（评审给的五步交错）。
    // 超时 ⇒ 409 `DELETE_PERSIST_IN_PROGRESS`，撤销屏障且不删除（宁可让客户端
    // 重试，也不在没把握时做不可逆的删除）。
    //
    // P0-E-1：屏障期 Stop 会真实落盘，于是"观测为空"与"删行"之间仍可能插入一次
    // 登记 ⇒ 每轮在拿到意图串行器后**再复查一次**；复查不干净就重试（总预算仍是
    // `DELETE_PERSIST_WAIT`，不延长）。串行器一直持有到删行 + 立墓碑完成。
    let intent_lock = {
        let deadline = std::time::Instant::now() + lifecycle::DELETE_PERSIST_WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if let Err(e) = state.lifecycle.await_intent_quiescent(&id, remaining).await {
                return e.to_response();
            }
            // 屏障期间仍可能受理过 Stop（`admit_stop` 与门锁同临界区，会同步
            // begin_stop）。一旦有活 attempt 出现，同样不得删配置（P0-7 的原判据
            // 必须在新窗口里复查）。
            if state.manager.has_live_attempt(&id) {
                return (
                    StatusCode::CONFLICT,
                    Json(json_error(
                        "CONFLICT",
                        "endpoint 正在运行，请先停止后再删除",
                    )),
                );
            }
            if let Some(lock) = state.lifecycle.lock_intent_persist_if_quiescent(&id).await {
                break lock;
            }
            // 观测之后又有屏障期 Stop 登记了在途写入 ⇒ 重试（预算用尽则上面 409）。
        }
    };
    let resp = match state.store.delete_endpoint(&id) {
        Ok(true) => {
            // 清理 Snapshot 中的残留状态（latest/status）。
            // G7：必须传 `gen_floor`（该 id 已发放的最大代次）立墓碑，否则旧
            // attempt 的迟到状态写入会让已删除端点"复活"（§6.5）。
            state
                .snapshot
                .remove_endpoint(&id, state.manager.gen_floor(&id));
            // 端点已彻底删除：停止意图围栏（若曾因落盘失败写入）已无保护对象，
            // 不清会留下指向不存在端点的永久围栏项，干扰人工核验与诊断。
            let _ = state.store.clear_stop_intent_unpersisted(&id);
            // P0-D：立墓碑代次（`stop_gen + 1`）并复位缓冲意图；此时已证明没有
            // 任何在途意图写入，因此不会再有旧写入落到重建后的同 ID 记录上。
            // guard 的 `commit()` 同时把"未提交即撤销"的语义置为已完成。
            barrier.commit();
            (StatusCode::OK, Json(serde_json::json!({ "deleted": id })))
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("endpoint `{id}`"))),
        ),
        Err(e) => store_err_to_response(e),
    };
    // 串行器在删行与立墓碑之后才释放：屏障期 Stop 的落盘要么已经排在它前面完成，
    // 要么在它之后执行——那时行已删除（0 行命中）或已立墓碑代次。
    drop(intent_lock);
    resp
}

/// 启动 Endpoint（G7 两阶段提交）。
///
/// 阶段划分（设计 §1.3 修订后 + P0-B）：`admit`（同步，票据）→ 构造（长
/// await）→ `commit`（门外取得落盘串行器 → 同步临界区：代次比对 + start →
/// 临界区外落盘 true 并 await 结果）→ 应答。
///
/// 为什么不再先查 `is_running`：它既不能表达"Stop 已受理"，也不能表达"有 Start
/// 在途"，是 W1/W2/W3 的共同根因（§1.2）。冲突判定改由门 + attempt 表给出：
/// - Stop 在构造期被受理 ⇒ 409 `SUPERSEDED_BY_STOP`（契约行 1）；
/// - 旧 attempt 仍在拆解 ⇒ 409 `STOP_IN_PROGRESS`（契约行 2，不排队）；
/// - 异常终止待复核 ⇒ 409 `ATTEMPT_FAILED_UNKNOWN`；
/// - 真的已在运行 ⇒ 409 `CONFLICT`（文案不变）。
async fn start_endpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let audit = state.audit();
    let request_id = lifecycle_audit::new_request_id(lifecycle_audit::OP_START, &id);
    // 准入必须在审计之前：`admit_start` 是**内存票据**（不查运行态、不碰 DB、不起进程），
    // 它让并发的 Stop 能看见"有 Start 在途"，从而给出 `SUPERSEDED_BY_STOP`。
    //
    // 为什么顺序不能反（CI 实测教训）：先写审计等于在"Start 已进入处理"与"Start 已登记"
    // 之间插进一次 DB 写；这段窗口里到达的 Stop 看不到票据，会以 `200 was_running=false`
    // 返回，而随后的 Start 照样把端点拉起来 —— 正好复活 W1 竞态
    //（`w1_stop_during_start_construction_leaves_nothing_running` 在 windows-arm64 上暴露）。
    // 票据 RAII 释放：后续任一步失败（含审计写失败）都会作废本次尝试，不留运行副作用。
    let ticket = state.lifecycle.admit_start(&id);
    // Issue #102：**意图先落盘**（在任何运行副作用之前）。审计写不进去就不构造、不起进程
    // ——与 Control 面同一取舍：记不下来就不执行，绝不出现"驱动跑起来了但查不出谁起的"。
    if let Err(e) = audit.started(
        &request_id,
        &id,
        lifecycle_audit::OP_START,
        lifecycle_audit::ACTOR_LOCAL_API,
        serde_json::json!({
            "action": "start",
            "endpoint_id": &id,
            "source": lifecycle_audit::SRC_HTTP_START,
            "client": lifecycle_audit::client_meta(&headers),
        }),
    ) {
        tracing::error!(
            request_id = %request_id, endpoint = %id, error = %e,
            "生命周期审计意图写入失败，拒绝 Start（fail-closed）"
        );
        // 票据在此释放：不能留下"已登记但不会提交"的 Start 让后续 Stop 误判。
        drop(ticket);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json_error(
                "AUDIT_UNAVAILABLE",
                "lifecycle audit store unavailable, start refused",
            )),
        );
    }
    // C1 P0：运行期 Endpoint 只能经共享构造路径生成（REST Start、
    // Bootstrap start、Mesad 开机恢复三者同源），否则重启恢复会把
    // Secret marker 当密码交给 Driver。
    let cfg = match runtime::build_runtime_endpoint(&state, &id).await {
        Ok(cfg) => cfg,
        Err(e) => {
            // 构造失败没有任何运行副作用 ⇒ REJECTED（不是 FAILED）。
            audit.finish_or_warn(
                &request_id,
                lifecycle_audit::ST_REJECTED,
                serde_json::json!({
                    "error_code": e.code,
                    "phase": "build_runtime_endpoint",
                }),
            );
            return (e.status, e.to_json());
        }
    };
    // 审计上下文只携带父请求与 sink：门的判定逻辑完全不受它影响。
    let ctx = mesa_driver_manager::attempt_audit::AttemptContext::audited(
        request_id.clone(),
        lifecycle_audit::ACTOR_LOCAL_API,
        state.audit_sink(),
    );
    match state.lifecycle.commit_start(&ticket, cfg, ctx).await {
        Ok(()) => {
            // ACCEPTED 而不是 COMPLETED：Manager 接受只代表 attempt 已登记，
            // 设备是否真的连上由 `endpoint.runtime_epoch` 子记录证明。
            audit.finish_or_warn(
                &request_id,
                lifecycle_audit::ST_ACCEPTED,
                serde_json::json!({
                    "manager_accepted": true,
                    "state": "accepted_not_running",
                }),
            );
            (
                StatusCode::OK,
                Json(serde_json::json!({ "started": id, "request_id": request_id })),
            )
        }
        Err(e) => {
            // 4xx = 门在校验阶段拒绝（无副作用）；5xx = 执行阶段失败。
            let status = if e.status.is_client_error() {
                lifecycle_audit::ST_REJECTED
            } else {
                lifecycle_audit::ST_FAILED
            };
            audit.finish_or_warn(
                &request_id,
                status,
                serde_json::json!({"error_code": e.code, "message": &e.message}),
            );
            e.to_response()
        }
    }
}

/// 停止 Endpoint（G7 合流 + 意图前移）。
///
/// 语义（§5.2 六行）：
/// - 第一次 Stop：受理即触发**一次** teardown；
/// - teardown 中再次 Stop：**合流**等待同一次结论，不重复 teardown；
/// - 成功 ⇒ 两个请求都 200 `was_running=true`；失败 ⇒ 两个都拿到同一次明确失败；
/// - 完成后再 Stop ⇒ 200 `was_running=false`（幂等语义冻结）；
/// - 客户端断开/超时 ⇒ 只丢应答，不取消后台停止。
///
/// `desired_running=false` 在受理时（同步临界区）就落到内存意图，并在
/// `spawn_blocking` 中**与 teardown 并行**落盘（S1：DB busy 不阻止停止）。
/// 落盘失败 ⇒ 即使 teardown 成功也返回 500 `STOP_INTENT_NOT_PERSISTED`
///（§6.10.3：停止意图没落盘就返回成功，重启后会意外自动拉起）。
async fn stop_endpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let audit = state.audit();
    let request_id = lifecycle_audit::new_request_id(lifecycle_audit::OP_STOP, &id);
    // Issue #102 §7：**安全优先**。先受理 Stop（teardown 立即开始），审计绝不挡在
    // 停止前面——DB 慢/坏时"停不下来"比"记不下来"严重得多。
    let admission = state.lifecycle.admit_stop(&id);
    let stop_gen = admission.stop_gen;
    // 审计意图与 teardown/落盘并行。审计是同步 rusqlite 单行 INSERT，必须
    // spawn_blocking：否则它会占住 tokio worker，反过来拖慢 Stop（自相矛盾）。
    let audit_task = {
        let audit = audit.clone();
        let request_id = request_id.clone();
        let id_for_task = id.clone();
        let client = lifecycle_audit::client_meta(&headers);
        tokio::task::spawn_blocking(move || {
            audit.started(
                &request_id,
                &id_for_task,
                lifecycle_audit::OP_STOP,
                lifecycle_audit::ACTOR_LOCAL_API,
                serde_json::json!({
                    "action": "stop",
                    "endpoint_id": id_for_task,
                    "source": lifecycle_audit::SRC_HTTP_STOP,
                    "client": client,
                }),
            )
        })
    };
    let (stop_res, persist_res, audit_res) = tokio::join!(
        admission.handle.await_result(),
        admission.persist,
        audit_task
    );
    let audit_recorded = matches!(audit_res, Ok(Ok(())));
    if !audit_recorded {
        // 明确的、被接受的例外：停止已执行但审计缺失。必须由日志与响应同时暴露，
        // 绝不能让调用方以为审计完整。
        tracing::error!(
            request_id = %request_id, endpoint = %id,
            "Stop 审计写入失败：停止已执行但审计缺失（audit_recorded=false）"
        );
    }
    let persist_failed = match persist_res {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(e),
        // blocking 任务本身 panic/取消：同样按"意图未持久化"处理（fail-closed）。
        // 必须**归还门状态**：否则 `stop_persist_inflight` 永远非空，P0-1 的启动
        // 闸门会把该端点永久锁在 409（一次任务异常 ≠ 永久不可用）。
        Err(join) => {
            let msg = format!("persist task failed: {join}");
            let _ = state
                .lifecycle
                .finish_stop_persist(&id, stop_gen, Err(msg.clone()));
            Some(msg)
        }
    };
    if let Some(e) = persist_failed {
        tracing::error!(
            endpoint = %id, error = %e,
            "STOP intent not persisted; response is explicit failure (500)"
        );
        if audit_recorded {
            audit.finish_or_warn(
                &request_id,
                lifecycle_audit::ST_FAILED,
                serde_json::json!({
                    "error_code": "STOP_INTENT_NOT_PERSISTED",
                    "message": &e,
                    "audit_recorded": true,
                }),
            );
        }
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error(
                "STOP_INTENT_NOT_PERSISTED",
                lifecycle::STOP_INTENT_NOT_PERSISTED_MESSAGE,
            )),
        );
    }
    match stop_res {
        Ok(was) => {
            if audit_recorded {
                audit.finish_or_warn(
                    &request_id,
                    lifecycle_audit::ST_COMPLETED,
                    serde_json::json!({
                        "was_running": was,
                        // 停止意图已确认落盘（否则上面就 500 返回了）
                        "desired_running_persisted": true,
                        "audit_recorded": true,
                    }),
                );
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "stopped": id,
                    "was_running": was,
                    "request_id": request_id,
                    "audit_recorded": audit_recorded,
                })),
            )
        }
        Err(detail) => {
            if audit_recorded {
                audit.finish_or_warn(
                    &request_id,
                    lifecycle_audit::ST_FAILED,
                    serde_json::json!({
                        "error_code": "STOP_FAILED",
                        "message": &detail,
                        "audit_recorded": true,
                    }),
                );
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("STOP_FAILED", &detail)),
            )
        }
    }
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
    // PR4 门禁：generic 任务先过统一 Descriptor 校验再入库
    if let Err(r) = gate_data_tasks(&state, &ep, &body.tasks).await {
        return r;
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
    // PR4 门禁：generic 任务先过统一 Descriptor 校验再入库
    if let Err(r) = gate_data_tasks(&state, &endpoint_id, &tasks).await {
        return r;
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
// Events（PR7 v1.1 §16/§17）：历史 + 单条 + EventTask CRUD
// ---------------------------------------------------------------------------

/// EventStore 不可用时的统一回答（§22）：503 + 精确码，Data 面不受影响。
fn events_unavailable() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json_error(
            "EVENT_STORE_UNAVAILABLE",
            "events.db unavailable; data plane unaffected",
        )),
    )
}

/// StoredEvent → v1.1 §16 响应形态。`attributes_json` 损坏（理论不可达，
/// writer 写入前即合法 JSON）→ 整页 500 fail-closed，禁止静默吞行。
/// NOTE: `stream_epoch`/`batch_sequence` 是 u64，JSON number 精确承载；
/// JS 客户端超 2^53 会失精度——游标/ID 一律用 `seq`（i64 安全范围）。
fn stored_event_json(
    ev: &StoredEvent,
) -> Result<serde_json::Value, (StatusCode, Json<serde_json::Value>)> {
    let attrs: serde_json::Value = serde_json::from_str(&ev.attributes_json).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error(
                "INTERNAL",
                &format!("corrupt attributes_json: {e}"),
            )),
        )
    })?;
    let condition = match (&ev.condition_id, &ev.transition) {
        (None, None) => serde_json::Value::Null,
        (id, tr) => serde_json::json!({
            "condition_id": id,
            "transition": tr,
            "active": ev.active,
            "acknowledged": ev.acknowledged,
            "confirmed": ev.confirmed,
            "retain": ev.retain,
        }),
    };
    Ok(serde_json::json!({
        "seq": ev.seq,
        "endpoint_id": ev.endpoint_id,
        "stream_epoch": ev.stream_epoch,
        "batch_sequence": ev.batch_sequence,
        "received_at_ns": ev.received_at_ns,
        "event": {
            "event_id": ev.event_id,
            "category": ev.category,
            "kind": ev.kind,
            "source": ev.source,
            "severity": ev.severity,
            "code": ev.code,
            "message": ev.message,
            "message_locale": ev.message_locale,
            "occurred_at_ns": ev.occurred_at_ns,
            "published_at_ns": ev.published_at_ns,
            "connection_handle": ev.connection_handle,
            "condition": condition,
            "correlation_id": ev.correlation_id,
            "attributes": attrs,
        },
    }))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    endpoint_id: Option<String>,
    /// 多 endpoint 过滤（CSV，如 `?endpoint_ids=a,b`；与单值同时给出取交集）。
    endpoint_ids: Option<String>,
    /// 按设备过滤：API 层映射为该设备下全部 endpoint id（事件表不动）。
    device_id: Option<String>,
    category: Option<String>,
    kind: Option<String>,
    severity_min: Option<u16>,
    code: Option<String>,
    condition_id: Option<String>,
    active: Option<bool>,
    from_ns: Option<i64>,
    to_ns: Option<i64>,
    before_seq: Option<i64>,
    after_seq: Option<i64>,
    limit: Option<u32>,
}

async fn list_events(
    State(state): State<Arc<AppState>>,
    Query(q): Query<EventsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = state.event_services() else {
        return events_unavailable();
    };
    // M5 endpoint 范围过滤：CSV 解析 + device_id 映射 + 单值交集合并。
    // device 下无 endpoint 时直接空结果（不查 DB，避免全表扫描后客户端过滤）。
    let mut ids: Vec<String> = q
        .endpoint_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(dev) = &q.device_id {
        match state.store.list_endpoints() {
            Ok(eps) => {
                let dev_ids: Vec<String> = eps
                    .into_iter()
                    .filter(|e| &e.device_id == dev)
                    .map(|e| e.id)
                    .collect();
                if dev_ids.is_empty() {
                    return (
                        StatusCode::OK,
                        Json(serde_json::json!({ "events": [], "next_cursor": null })),
                    );
                }
                if ids.is_empty() {
                    ids = dev_ids;
                } else {
                    ids.retain(|id| dev_ids.contains(id));
                }
            }
            Err(e) => return store_err_to_response(e),
        }
    }
    if let Some(single) = &q.endpoint_id {
        if ids.is_empty() {
            ids.push(single.clone());
        } else if !ids.contains(single) {
            // 单值与多值无交集：明确空结果
            return (
                StatusCode::OK,
                Json(serde_json::json!({ "events": [], "next_cursor": null })),
            );
        } else {
            ids = vec![single.clone()];
        }
    }
    let endpoint_ids =
        if q.endpoint_ids.is_some() || q.device_id.is_some() || q.endpoint_id.is_some() {
            Some(ids)
        } else {
            None
        };
    let filter = EventFilter {
        endpoint_id: None,
        endpoint_ids,
        category: q.category,
        kind: q.kind,
        severity_min: q.severity_min,
        code: q.code,
        condition_id: q.condition_id,
        active: q.active,
        from_ns: q.from_ns,
        to_ns: q.to_ns,
        before_seq: q.before_seq,
        after_seq: q.after_seq,
        limit: q.limit,
    };
    // 阻塞式 SQLite 查询不得占 Tokio worker（与 writer 线程模型对应）
    let store = svc.store.clone();
    let res = tokio::task::spawn_blocking(move || store.query_history(&filter)).await;
    match res {
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &format!("query task failed: {e}"))),
        ),
        Ok(Err(mesa_event_store::EventStoreError::InvalidRecord(m))) => (
            StatusCode::BAD_REQUEST,
            Json(json_error("VALIDATION_ERROR", &m)),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e.to_string())),
        ),
        Ok(Ok((rows, next))) => {
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                match stored_event_json(r) {
                    Ok(v) => out.push(v),
                    Err(e) => return e,
                }
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "events": out, "next_cursor": next })),
            )
        }
    }
}

/// SSE replay 单页行数。replay 按页循环直到不满页；慢消费者追赶时每页
/// 前进，必收敛（除非写入快于查询 sustained——彼时 lag 循环继续，属于
/// 背压可见，不静默）。
const SSE_REPLAY_PAGE: u32 = 1000;

/// SSE 帧构造。 corruption 行返回 `None`（调用方结束流：客户端按最后收到
/// 的 id 重连重试；该分支理论不可达——writer 写入的 attributes_json 恒合法）。
/// fail-closed：不断发半坏帧；客户端重连后 replay 仍会撞到该行并再次结束——
/// 响亮失败，而非静默跳行。
fn sse_frame(ev: &StoredEvent) -> Option<axum::response::sse::Event> {
    match stored_event_json(ev) {
        Ok(v) => {
            let data = serde_json::to_string(&v).unwrap_or_else(|_| "{}".into());
            Some(
                axum::response::sse::Event::default()
                    .id(ev.seq.to_string())
                    .event("mesa-event")
                    .data(data),
            )
        }
        Err(_) => {
            tracing::error!(
                seq = ev.seq,
                "corrupt stored event in SSE path, ending stream"
            );
            None
        }
    }
}

type SseItem = Result<axum::response::sse::Event, std::convert::Infallible>;

/// 单页 catch-up（P0-2 流式）：查 `PAGE+1` 行，满页则留 1 行探知"还有更多"，
/// 只返回 `PAGE` 帧并报告未穷尽。调用方逐页 yield，任意时刻最多持有约
/// 一页（历史再大也不先堆完再发）。
/// `until = Some(h)`：初始 replay，查询上界字面锁死，区间恒为 `(from, h]`；
/// `until = None`：live 期追赶（gap/Lag/reconcile），追 DB 现状。
/// 返回 `(new_cursor, frames, exhausted)`；`None` = DB 失败或坏行
/// （调用方结束流，客户端按旧游标重连重试）。
async fn sse_catch_up_page(
    store: &Arc<mesa_event_store::EventStore>,
    from: i64,
    until: Option<i64>,
) -> Option<(i64, Vec<SseItem>, bool)> {
    let s = store.clone();
    let page =
        match tokio::task::spawn_blocking(move || s.replay_range(from, until, SSE_REPLAY_PAGE + 1))
            .await
        {
            Ok(Ok(p)) => p,
            // DB 失败 / 任务失败 → 结束流（客户端按旧游标重连重试）
            _ => return None,
        };
    let exhausted = page.len() <= SSE_REPLAY_PAGE as usize;
    let mut cursor = from;
    let mut frames = Vec::with_capacity(page.len().min(SSE_REPLAY_PAGE as usize));
    for ev in page.into_iter().take(SSE_REPLAY_PAGE as usize) {
        let f = sse_frame(&ev)?;
        cursor = ev.seq;
        frames.push(Ok(f));
    }
    Some((cursor, frames, exhausted))
}

#[derive(Debug, Deserialize)]
struct LiveQuery {
    after_seq: Option<i64>,
}

/// Live SSE（Checkpoint B 收紧版；原则：**Hub 是提示、DB seq 是交付权威**）：
/// ```text
/// CONNECT
///   parse cursor → hub.subscribe() → connect_high_water = DB.max_seq()
///   → HTTP 200（P0-3：boundary 在 200 前固定）
/// 有 cursor：pagewise replay (cursor, high_water]（P0-2 流式有界）
/// 无 cursor：live-only，last_sent = high_water
/// LIVE LOOP（select!）：
///   Hub seq <= last_sent → duplicate skip
///   Hub seq == last_sent+1 → fast emit
///   Hub seq > last_sent+1 → DB catch-up ASC（P0-1 多 ingress 乱序）
///   Hub Lagged → DB catch-up ASC
///   15s tick → DB reconcile（P1：abort 漏 publish 的最终兜底）
///   Hub Closed → 结束
/// ```
///
/// 无游标新连接默认 live-only（不灌历史；回填用显式 `?after_seq=`）。
/// 断开清理：流 drop 即取消，Receiver 释放，`receiver_count` 回落——无显式注销。
async fn events_live(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LiveQuery>,
    headers: axum::http::HeaderMap,
) -> Result<
    axum::response::sse::Sse<impl tokio_stream::Stream<Item = SseItem>>,
    (StatusCode, Json<serde_json::Value>),
> {
    let svc = state.event_services().ok_or_else(events_unavailable)?;
    // 非法游标 400：静默当 absent 会藏掉客户端真正想要的重放起点
    let header_cursor: Option<i64> = match headers.get("last-event-id") {
        None => None,
        Some(v) => Some(
            v.to_str()
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json_error("VALIDATION_ERROR", "invalid Last-Event-ID")),
                    )
                })?,
        ),
    };
    let effective = [q.after_seq, header_cursor].into_iter().flatten().max();

    // subscribe → high-water 都在返回 200 之前固定（P0-3）：客户端拿到 200
    // 后发生的 commit 必属于 live，不会被当成历史 baseline 吃掉。
    let hub_rx = svc.hub.subscribe();
    let store = svc.store.clone();
    let connect_high_water = match tokio::task::spawn_blocking({
        let store = store.clone();
        move || store.max_seq()
    })
    .await
    {
        Ok(Ok(m)) => m,
        _ => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json_error("INTERNAL", "events high-water unavailable")),
            ));
        }
    };
    // ⑧c 诊断：本连接发出的 DB replay 帧 / Lagged 事故 / reconcile 执行
    let diag = svc.diagnostics.clone();
    let s = async_stream::stream! {
        // 初始 replay（P0-2 + ⑧a）：逐页 yield，内存只驻一页；查询上界字面
        // 锁死在建连 high-water——区间恒为 `(cursor, high_water]`，DB 在
        // high-water 之后再写多少行也不会越界（剩下的走 live）。
        let mut last_sent: i64 = match effective {
            Some(cur) => {
                let mut cursor = cur;
                while cursor < connect_high_water {
                    let Some((cur2, frames, _)) =
                        sse_catch_up_page(&store, cursor, Some(connect_high_water)).await
                    else {
                        return;
                    };
                    diag.replay_frames_total
                        .fetch_add(frames.len() as u64, Ordering::Relaxed);
                    for f in frames {
                        yield f;
                    }
                    // 空页/DB 无进展即停（high-water 内行数固定，不会死循环）
                    if cur2 == cursor {
                        break;
                    }
                    cursor = cur2;
                }
                cursor
            }
            None => connect_high_water,
        };
        let mut rx = hub_rx;
        // reconcile 心跳：首 tick 立即消费（interval 首 tick 即时触发语义）
        let mut reconcile = tokio::time::interval(std::time::Duration::from_secs(15));
        reconcile.tick().await;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok(ev) => {
                        if ev.seq <= last_sent {
                            // duplicate（重放/Hub 重叠、乱序迟到）：跳过
                            continue;
                        }
                        if ev.seq > last_sent + 1 {
                            // P0-1：Hub 顺序 ≠ 交付顺序。多 ingress 并发下
                            // publish 可能乱序（101 先于 100），直接发会让
                            // 100 永久丢失——回 DB 按 seq ASC 补齐再发。
                            tracing::debug!(
                                got = ev.seq,
                                last_sent,
                                "SSE hub gap, catching up from DB"
                            );
                            loop {
                                let Some((cur2, frames, done)) =
                                    sse_catch_up_page(&store, last_sent, None).await
                                else {
                                    return;
                                };
                                diag.replay_frames_total
                                    .fetch_add(frames.len() as u64, Ordering::Relaxed);
                                for f in frames {
                                    yield f;
                                }
                                last_sent = cur2;
                                if done {
                                    break;
                                }
                            }
                            continue;
                        }
                        // fast-path：严格下一行，直接发
                        last_sent = ev.seq;
                        let Some(f) = sse_frame(&ev) else {
                            return;
                        };
                        yield Ok(f);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, last_sent, "SSE slow consumer, catching up from DB");
                        diag.lagged_total.fetch_add(1, Ordering::Relaxed);
                        loop {
                            let Some((cur2, frames, done)) =
                                sse_catch_up_page(&store, last_sent, None).await
                            else {
                                return;
                            };
                            diag.replay_frames_total
                                .fetch_add(frames.len() as u64, Ordering::Relaxed);
                            for f in frames {
                                yield f;
                            }
                            last_sent = cur2;
                            if done {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = reconcile.tick() => {
                    // P1：abort 兜底漏 publish（commit 落盘但 Hub 未发）无任何
                    // Hub 信号，只能靠 DB 对账发现。慢连接 15s 一次 max 循环，
                    // 代价一次索引聚合，可接受。每次 tick 执行都计数（存活证明）。
                    diag.reconcile_total.fetch_add(1, Ordering::Relaxed);
                    let check = {
                        let store = store.clone();
                        tokio::task::spawn_blocking(move || store.max_seq()).await
                    };
                    let behind = matches!(check, Ok(Ok(m)) if m > last_sent);
                    if behind {
                        tracing::debug!(last_sent, "SSE reconcile found new rows, catching up");
                        loop {
                            let Some((cur2, frames, done)) =
                                sse_catch_up_page(&store, last_sent, None).await
                            else {
                                return;
                            };
                            diag.replay_frames_total
                                .fetch_add(frames.len() as u64, Ordering::Relaxed);
                            for f in frames {
                                yield f;
                            }
                            last_sent = cur2;
                            if done {
                                break;
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(axum::response::sse::Sse::new(s).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    ))
}

// ---------------------------------------------------------------------------
// Point Live Stream（STATE STREAM，不是 EVENT STREAM；§二十冻结语义）
// ---------------------------------------------------------------------------

/// Point Live SSE 合并窗口：每个连接最多约 5 数据帧/s；同一点 200ms 内
/// N 次变化只发最终值（latest-wins）。
const POINT_LIVE_FLUSH_MS: u64 = 200;

fn point_snapshot_event(points: Vec<mesa_driver_manager::snapshot::LatestEntry>) -> SseItem {
    Ok(axum::response::sse::Event::default()
        .event("mesa-points-snapshot")
        .json_data(serde_json::json!({ "points": points }))
        .unwrap_or_else(|_| axum::response::sse::Event::default().event("mesa-points-snapshot")))
}

fn point_delta_event(points: Vec<mesa_driver_manager::snapshot::LatestEntry>) -> SseItem {
    Ok(axum::response::sse::Event::default()
        .event("mesa-points-delta")
        .json_data(serde_json::json!({ "points": points }))
        .unwrap_or_else(|_| axum::response::sse::Event::default().event("mesa-points-delta")))
}

/// `GET /api/v1/points/live`（STATE STREAM）：
/// 建连顺序是先 subscribe change notifier，再读 snapshot.latest_all()，
/// 然后首帧 mesa-points-snapshot，最后消费 notice（200ms 合并）。
/// 先 subscribe 再读 snapshot：建连窗口的变化只会重复（snapshot 已含新值
/// 加后续 delta 再发一次），绝不丢失。无 `id`/seq/replay，断线重连即新连接
/// 加 fresh snapshot，Lagged 即 Resync（不补 backlog）。
async fn points_live(
    State(state): State<Arc<AppState>>,
) -> axum::response::sse::Sse<impl tokio_stream::Stream<Item = SseItem>> {
    use mesa_driver_manager::snapshot::LatestChange;
    // ① 一定先 subscribe（一致性边界：不丢建连窗口）。
    let mut rx = state.snapshot.subscribe_latest_changes();
    // ② 再读全量（此后任何变化都有 notice 兜底）。
    let initial = state.snapshot.latest_all();
    let snapshot = state.snapshot.clone();
    let s = async_stream::stream! {
        // ③ 首帧一定是完整 snapshot（空库即 {points:[]}，合法）。
        yield point_snapshot_event(initial);
        let mut dirty: std::collections::HashSet<(String, u32)> =
            std::collections::HashSet::new();
        let mut resync = false;
        let mut flush = tokio::time::interval(std::time::Duration::from_millis(
            POINT_LIVE_FLUSH_MS,
        ));
        // 首 tick 立即消费（interval 首 tick 即就绪）：不引入 200ms 首帧延迟。
        flush.tick().await;
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Ok(change) => match &*change {
                            LatestChange::Upsert { endpoint_id, point_ids } => {
                                for id in point_ids {
                                    dirty.insert((endpoint_id.clone(), *id));
                                }
                            }
                            LatestChange::Resync => {
                                dirty.clear();
                                resync = true;
                            }
                        },
                        // 慢消费者：不补 backlog，直接转全量（state stream 优势）。
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "point live slow consumer; resync");
                            dirty.clear();
                            resync = true;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = flush.tick() => {
                    if resync {
                        yield point_snapshot_event(snapshot.latest_all());
                        resync = false;
                        dirty.clear();
                    } else if !dirty.is_empty() {
                        // delta = 变化点的完整最新行（不是 patch；字段留脏不可能）。
                        let points = snapshot.latest_for_keys(&dirty);
                        dirty.clear();
                        if !points.is_empty() {
                            yield point_delta_event(points);
                        }
                    }
                }
            }
        }
    };
    axum::response::sse::Sse::new(s).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}

/// ⑧c 事件面诊断：SSE 计数（lagged/replay/reconcile）加 store 规模。
/// P1-1 补全 ingress 全局计数、retention 累计 purge 与 hub live 订阅数。
/// 计数是进程级累计（Relaxed 原子），各连接与 attempt 共同累加；
/// 给 Lagged 分支契约测试提供程序级证明点。
async fn events_stats(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = state.event_services() else {
        return events_unavailable();
    };
    let d = &svc.diagnostics;
    let load = |v: &std::sync::atomic::AtomicU64| v.load(Ordering::Relaxed);
    let live_clients = svc.hub.receiver_count();
    let store = svc.store.clone();
    let res = tokio::task::spawn_blocking(move || store.stats()).await;
    match res {
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &format!("stats task failed: {e}"))),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e.to_string())),
        ),
        Ok(Ok(st)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "sse_lagged_total": load(&d.lagged_total),
                "sse_replay_frames_total": load(&d.replay_frames_total),
                "sse_reconcile_total": load(&d.reconcile_total),
                "ingress_batches_total": load(&d.ingress_batches_total),
                "ingress_persisted_events_total": load(&d.ingress_persisted_events_total),
                "ingress_batch_duplicates_total": load(&d.ingress_batch_duplicates_total),
                "ingress_event_duplicates_total": load(&d.ingress_event_duplicates_total),
                "ingress_gaps_total": load(&d.ingress_gaps_total),
                "ingress_regressions_total": load(&d.ingress_regressions_total),
                "ingress_collisions_total": load(&d.ingress_collisions_total),
                "ingress_invalid_total": load(&d.ingress_invalid_total),
                "ingress_store_failures_total": load(&d.ingress_store_failures_total),
                "ingress_commit_latency_last_ns": load(&d.ingress_commit_latency_last_ns),
                "ingress_commit_latency_max_ns": load(&d.ingress_commit_latency_max_ns),
                "retention_purged_total": load(&d.retention_purged_total),
                "live_clients": live_clients,
                "stored_rows": st.rows,
                "stored_size_bytes": st.size_bytes,
            })),
        ),
    }
}

async fn get_event(
    State(state): State<Arc<AppState>>,
    Path(seq): Path<i64>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = state.event_services() else {
        return events_unavailable();
    };
    let store = svc.store.clone();
    let res = tokio::task::spawn_blocking(move || store.query_by_seq(seq)).await;
    match res {
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &format!("query task failed: {e}"))),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json_error("INTERNAL", &e.to_string())),
        ),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json_error("NOT_FOUND", &format!("event seq `{seq}`"))),
        ),
        Ok(Ok(Some(row))) => match stored_event_json(&row) {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err(e) => e,
        },
    }
}

async fn list_event_tasks(
    State(state): State<Arc<AppState>>,
    Path(endpoint_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.store.list_event_tasks(&endpoint_id) {
        Ok(v) => {
            let rev = state.store.current_revision(&endpoint_id).unwrap_or(0);
            (
                StatusCode::OK,
                Json(
                    serde_json::json!({ "endpoint_id": endpoint_id, "revision": rev, "event_tasks": v }),
                ),
            )
        }
        Err(e) => store_err_to_response(e),
    }
}

async fn put_event_tasks(
    State(state): State<Arc<AppState>>,
    Path(endpoint_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    // body 可为 {event_tasks:[...]} 或直接 [...]（与 put_tasks_for_endpoint 同宽容）
    let tasks: Vec<EventTask> = if let Some(arr) = body.get("event_tasks") {
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
                "body 需为 {event_tasks:[...]} 或 [...]",
            )),
        );
    };
    // 运行中禁止修改（与 Data tasks 同语义，v1.1 §17：无 PATCH，只有全量 replace）
    if state.manager.is_running(&endpoint_id) {
        return (
            StatusCode::CONFLICT,
            Json(json_error(
                "CONFLICT",
                "endpoint 正在运行，请先停止后再修改事件任务",
            )),
        );
    }
    // PR4 门禁：mesa.events.v1 任务先过事件目录校验再入库
    if let Err(r) = gate_event_tasks(&state, &endpoint_id, &tasks).await {
        return r;
    }
    match state.store.replace_event_tasks(&endpoint_id, &tasks) {
        Ok(rev) => (
            StatusCode::OK,
            Json(serde_json::json!({ "endpoint_id": endpoint_id, "revision": rev })),
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
        // I-2 只读管理面（driver 作用域）：定义目录 + 权威身份预览。
        // 两条都**不**打开 CNC 连接、不读机床实时值、不改采集计划、无持久化副作用
        //（DC §6.3 的 R8–R11 逐条有测试断言）。POST 表示「带参数的只读计算」，
        // 与既有 /probe、/validate-connection 的先例一致。
        .route(
            "/api/v1/drivers/{id}/definitions",
            get(driver_query::definitions),
        )
        .route(
            "/api/v1/drivers/{id}/definitions/identity-preview",
            // 64 KiB body 门**只挂在本路由**（不是全局 2 MB 默认值）；外层
            // map_response 把框架级 413 改写成统一错误信封（E30）。
            post(driver_query::identity_preview)
                .layer(axum::extract::DefaultBodyLimit::max(
                    driver_query::MAX_REQUEST_BODY_BYTES,
                ))
                .layer(axum::middleware::map_response(
                    driver_query::rewrite_payload_too_large,
                )),
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
        // C3-A：Endpoint-bound 连接检查。endpoint_id 只来自 URL，driver_id 只读
        // 已保存记录；请求体形状由 handler 内 deny_unknown_fields 拒绝覆盖。
        .route(
            "/api/v1/endpoints/{endpoint_id}/connection-check",
            post(connection_check::connection_check),
        )
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
        .route("/api/v1/points/live", get(points_live))
        // P2：点展示名改名（用户元数据，Driver 无感知）
        .route(
            "/api/v1/endpoints/{endpoint_id}/points/{point_key}/display-name",
            put(update_point_display_name),
        )
        .route("/api/v1/diagnostics", get(diagnostics))
        // Devices CRUD
        .route("/api/v1/devices", get(list_devices).post(create_device))
        .route(
            "/api/v1/devices/{id}",
            get(get_device).put(update_device).delete(delete_device),
        )
        // M5 device-bootstrap（原子创建 Device + Endpoint + Tasks + Start，幂等）
        .route(
            "/api/v1/device-bootstrap",
            post(bootstrap::device_bootstrap),
        )
        // 启停
        .route("/api/v1/endpoints/{id}/start", post(start_endpoint))
        .route("/api/v1/endpoints/{id}/stop", post(stop_endpoint))
        // Tasks（全量快照）
        .route("/api/v1/tasks", get(list_tasks).post(replace_tasks))
        .route("/api/v1/tasks/{endpoint_id}", put(put_tasks_for_endpoint))
        .route("/api/v1/tasks/{endpoint_id}/{task_id}", delete(delete_task))
        // Events（PR7 v1.1 §16/§17）。
        // NOTE: `/events/live`（⑦ SSE，静态段）与 `/events/{seq}`（动态段）
        // 共存——axum 静态优先，无需调序（同 certificates/opcua 前例）。
        .route("/api/v1/events", get(list_events))
        .route("/api/v1/events/live", get(events_live))
        .route("/api/v1/events/stats", get(events_stats))
        .route("/api/v1/events/{seq}", get(get_event))
        .route(
            "/api/v1/endpoints/{id}/event-tasks",
            get(list_event_tasks).put(put_event_tasks),
        )
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

#[cfg(test)]
mod browse_json_tests {
    use super::*;
    use mesa_driver_protocol::pb::{
        BrowseDiagnosticSeverity, BrowseNode, BrowseSelectionDiagnostic,
    };

    /// 旧驱动（IPC < 1.8）不填选择契约 ⇒ JSON 三字段必须是 `0`/`""`/`[]`，
    /// 其余 7 字段逐字不变。这是「新 Core + 旧驱动行为不变」的机械判据：
    /// 消费端拿到 `selection_mode = 0` 只会走既有通用规则，不会误判成"不可选"。
    #[test]
    fn legacy_node_passes_through_with_unspecified_selection_mode() {
        let json = browse_nodes_json(vec![BrowseNode {
            id: "nsu=urn:mesa:fake:1;i=1".into(),
            label: "Counter".into(),
            kind: "node".into(),
            data_type: "STRING".into(),
            access: "read".into(),
            has_children: true,
            binding_json: r#"{"node_id":"nsu=urn:mesa:fake:1;i=1"}"#.into(),
            ..Default::default()
        }]);
        assert_eq!(json.len(), 1);
        let n = &json[0];
        assert_eq!(n["selection_mode"], 0);
        assert_eq!(n["selection_payload_json"], "");
        assert_eq!(n["diagnostics"], serde_json::json!([]));
        // 既有 7 字段逐字透传
        assert_eq!(n["id"], "nsu=urn:mesa:fake:1;i=1");
        assert_eq!(n["kind"], "node");
        assert_eq!(n["has_children"], true);
        assert_eq!(
            n["binding_json"],
            r#"{"node_id":"nsu=urn:mesa:fake:1;i=1"}"#
        );
    }

    /// 新驱动填了选择契约 ⇒ 原样透传，**Core 不改写任何内容**：
    /// 未知模式值（未来驱动）也必须原值到达消费端，由消费端 fail-closed。
    #[test]
    fn selection_contract_passes_through_verbatim() {
        let payload = r#"{"kind":"vendor_payload","parameters":{"x":1}}"#;
        let json = browse_nodes_json(vec![BrowseNode {
            id: "v://leaf".into(),
            selection_mode: 99, // 未来驱动的未知模式
            selection_payload_json: payload.into(),
            diagnostics: vec![BrowseSelectionDiagnostic {
                code: "VENDOR_CODE".into(),
                message: "为什么不可选（驱动文案）".into(),
                severity: BrowseDiagnosticSeverity::Error as i32,
            }],
            ..Default::default()
        }]);
        let n = &json[0];
        assert_eq!(
            n["selection_mode"], 99,
            "未知模式必须原值透传，不得被 Core 归一化"
        );
        assert_eq!(n["selection_payload_json"], payload);
        assert_eq!(n["diagnostics"][0]["code"], "VENDOR_CODE");
        assert_eq!(n["diagnostics"][0]["message"], "为什么不可选（驱动文案）");
        assert_eq!(n["diagnostics"][0]["severity"], 3);
    }

    /// 三字段之外的既有字段语义不得被顺手"增强"（例如把空 payload 换成
    /// binding_json）——那会让 Core 变成第二真值来源。
    ///
    /// 说明：**品牌字面量扫描**放在合同测试里（`discovery_contract.rs`）对本文件
    /// 做源码级断言；此处不做，因为断言文本自身含被扫的词，会自证失败。
    #[test]
    fn empty_payload_is_not_backfilled_from_binding_json() {
        let json = browse_nodes_json(vec![BrowseNode {
            id: "x".into(),
            kind: "folder".into(),
            has_children: true,
            binding_json: r#"{"resource_id":"variable"}"#.into(),
            ..Default::default()
        }]);
        assert_eq!(json[0]["selection_payload_json"], "");
        assert_eq!(json[0]["selection_mode"], 0);
    }
}
