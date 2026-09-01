//! DriverManager 编排入口：发现驱动、启动 Endpoint 运行时、统一停机。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use tokio_util::sync::CancellationToken;

use crate::endpoint::{BuiltinEndpoint, PointIdAllocator, PointIdSource, run_endpoint};
use crate::manifest::{DiscoveredDriver, scan_drivers};
use crate::profile::load_profiles;
use crate::snapshot::{DriverInfo, Snapshot};

/// Descriptor 缓存键（§4.5）：(driver_id, driver_version)
type DescriptorCacheKey = (String, String);

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct CachedDescriptor {
    descriptor: mesa_core_types::DriverDescriptor,
    fetched_at_ns: i64,
}

/// Descriptor 获取错误（§4.4），映射为 REST 503 + error.code
#[derive(Debug, Clone)]
pub struct DescriptorError {
    pub code: String,
    pub message: String,
}

impl DescriptorError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

struct RunningEntry {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

pub struct MesaManager {
    drivers: RwLock<Vec<DiscoveredDriver>>,
    snapshot: Arc<Snapshot>,
    source: Arc<dyn PointIdSource>,
    running: Mutex<HashMap<String, RunningEntry>>,
    shutdown: CancellationToken,
    descriptor_cache: RwLock<HashMap<DescriptorCacheKey, CachedDescriptor>>,
    profiles: RwLock<Vec<mesa_core_types::DeviceProfile>>,
    /// 活跃会话注册表：endpoint_id -> Session（用于 Control 面可靠转发，§22）
    active_sessions: std::sync::Arc<RwLock<HashMap<String, std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>>>>,
}

impl MesaManager {
    /// 扫描驱动目录并填充快照中的驱动清单（使用内存版 ID 源，兼容 M0/Contract Test）。
    pub fn discover(drivers_dir: &Path) -> Self {
        Self::with_source(drivers_dir, Arc::new(PointIdAllocator::default()))
    }

    /// 使用指定 [`PointIdSource`] 创建 Manager（Mesad 传入持久版）。
    pub fn with_source(drivers_dir: &Path, source: Arc<dyn PointIdSource>) -> Self {
        let drivers = scan_drivers(drivers_dir);
        let snapshot = Arc::new(Snapshot::new());
        snapshot.set_drivers(Self::driver_infos(&drivers));
        let profiles = load_profiles(drivers_dir);
        Self {
            drivers: RwLock::new(drivers),
            snapshot,
            source,
            running: Mutex::new(HashMap::new()),
            shutdown: CancellationToken::new(),
            descriptor_cache: RwLock::new(HashMap::new()),
            profiles: RwLock::new(profiles),
            active_sessions: std::sync::Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn driver_infos(drivers: &[DiscoveredDriver]) -> Vec<DriverInfo> {
        drivers
            .iter()
            .map(|d| DriverInfo {
                id: d.manifest.id.clone(),
                name: d.manifest.name.clone(),
                version: d.manifest.version.clone(),
                protocol: format!(
                    "{}.{}",
                    d.manifest.protocol_major, d.manifest.protocol_minor
                ),
                launchable: d.launchable(),
                reason: if d.platform_ok {
                    d.executable_path
                        .as_ref()
                        .map(|_| None)
                        .unwrap_or(Some("executable not found".into()))
                } else {
                    d.platform_reason.clone()
                },
            })
            .collect()
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// 重新扫描驱动目录，刷新可用驱动清单并清空 Descriptor 缓存（§4.5），同时重载 Profiles。
    pub fn rescan(&self, drivers_dir: &Path) -> Vec<DriverInfo> {
        let drivers = scan_drivers(drivers_dir);
        let infos = Self::driver_infos(&drivers);
        *self.drivers.write().unwrap() = drivers;
        self.snapshot.set_drivers(infos.clone());
        // 清空全部 Descriptor Cache（§4.5 精确失效 1）
        self.descriptor_cache.write().unwrap().clear();
        *self.profiles.write().unwrap() = load_profiles(drivers_dir);
        infos
    }

    /// 按 driver_id 查找已发现且可启动的驱动。
    pub fn find_driver(&self, driver_id: &str) -> Option<DiscoveredDriver> {
        self.drivers
            .read()
            .unwrap()
            .iter()
            .find(|d| d.manifest.id == driver_id && d.launchable())
            .cloned()
    }

    pub fn is_running(&self, endpoint_id: &str) -> bool {
        self.running.lock().unwrap().contains_key(endpoint_id)
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.running.lock().unwrap().keys().cloned().collect()
    }

    /// 启动一个 Endpoint。已在运行则返回错误。
    pub fn start_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        if self.is_running(&cfg.endpoint_id) {
            return Err(format!("endpoint `{}` already running", cfg.endpoint_id));
        }
        let disc = self
            .find_driver(&cfg.driver_id)
            .ok_or_else(|| format!("driver `{}` not found or not launchable", cfg.driver_id))?;

        let shutdown = self.shutdown.child_token();
        let cancel = shutdown.clone();
        let snapshot = Arc::clone(&self.snapshot);
        let source = Arc::clone(&self.source);
        let registry = std::sync::Arc::clone(&self.active_sessions);
        let handle = tokio::spawn(run_endpoint(
            disc,
            cfg.clone(),
            snapshot,
            source,
            shutdown,
            registry,
        ));

        self.running
            .lock()
            .unwrap()
            .insert(cfg.endpoint_id, RunningEntry { cancel, handle });
        Ok(())
    }

    /// 兼容旧名称。
    pub fn start_builtin_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        self.start_endpoint(cfg)
    }

    /// 停止指定 Endpoint。返回是否曾处于运行态。
    pub async fn stop_endpoint(&self, endpoint_id: &str) -> bool {
        let entry = self.running.lock().unwrap().remove(endpoint_id);
        let Some(entry) = entry else {
            return false;
        };
        entry.cancel.cancel();
        // 等待运行任务结束，最多 10s（涵盖子进程终止宽限）
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), entry.handle).await;
        true
    }

    /// 统一停机（消费 self，兼容旧调用）。
    pub async fn shutdown(self) {
        self.shutdown_all().await;
    }

    /// 统一停机（&self 版，供 Arc 持有者调用）。
    pub async fn shutdown_all(&self) {
        self.shutdown.cancel();
        let handles: Vec<tokio::task::JoinHandle<()>> = {
            let mut m = self.running.lock().unwrap();
            m.drain()
                .map(|(_, e)| {
                    e.cancel.cancel();
                    e.handle
                })
                .collect()
        };
        for h in handles {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), h).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    /// 懒加载获取 Driver Descriptor（§12）：临时 spawn → handshake → GetDescriptor → cache → shutdown
    pub async fn get_descriptor(
        &self,
        driver_id: &str,
    ) -> Result<mesa_core_types::DriverDescriptor, DescriptorError> {
        // 查找可启动驱动
        let disc = self.find_driver(driver_id).ok_or_else(|| {
            DescriptorError::new(
                "DRIVER_UNAVAILABLE",
                format!("driver `{driver_id}` not found or not launchable"),
            )
        })?;

        // 命中缓存
        let key: DescriptorCacheKey = (disc.manifest.id.clone(), disc.manifest.version.clone());
        if let Some(cached) = self.descriptor_cache.read().unwrap().get(&key) {
            return Ok(cached.descriptor.clone());
        }

        // 未命中：临时进程
        let mut proc = crate::process::DriverProcess::spawn(&disc)
            .await
            .map_err(|e| {
                DescriptorError::new("DRIVER_UNAVAILABLE", format!("spawn failed: {e}"))
            })?;

        let (mut session, _events, _) =
            crate::session::Session::connect_retry(proc.port, &proc.token)
                .await
                .map_err(|e| {
                    proc.force_kill();
                    DescriptorError::new("DRIVER_UNAVAILABLE", format!("handshake failed: {e}"))
                })?;

        let (major, minor, json) = session.get_descriptor().await.map_err(|e| match e {
            crate::session::SessionError::Timeout => {
                DescriptorError::new("DRIVER_DESCRIPTOR_TIMEOUT", "descriptor timeout 5s")
            }
            crate::session::SessionError::Handshake(msg) if msg.contains("too large") => {
                DescriptorError::new("DRIVER_DESCRIPTOR_TOO_LARGE", msg)
            }
            other => DescriptorError::new("DRIVER_UNAVAILABLE", format!("{other}")),
        })?;

        // 清理临时进程
        session.invalidate();
        proc.terminate().await;

        if json.len() > 256 * 1024 {
            return Err(DescriptorError::new(
                "DRIVER_DESCRIPTOR_TOO_LARGE",
                format!("descriptor {} bytes exceeds 256KiB", json.len()),
            ));
        }

        let desc: mesa_core_types::DriverDescriptor = serde_json::from_str(&json).map_err(|e| {
            DescriptorError::new(
                "DRIVER_DESCRIPTOR_INVALID_JSON",
                format!("invalid json: {e}"),
            )
        })?;

        // 校验契约（字段唯一、visible_if 等）
        desc.validate()
            .map_err(|e| DescriptorError::new("DRIVER_DESCRIPTOR_VALIDATION_FAILED", e))?;

        // 校验 contract 版本语义（§4.2）：此处仅透传，若 Core 不支持 Major 可返回 UNSUPPORTED
        // 暂允许任意 Major，未来 Core 可在此处做兼容性检查

        // 缓存
        self.descriptor_cache.write().unwrap().insert(
            key,
            CachedDescriptor {
                descriptor: desc.clone(),
                fetched_at_ns: mesa_core_types::now_unix_ns(),
            },
        );

        let _ = (major, minor); // 保留与 proto 协商值一致性检查余地
        Ok(desc)
    }

    /// 供 REST 列出全部驱动及其 Descriptor 状态（可选）
    pub fn descriptor_cache_snapshot(&self) -> std::collections::HashMap<String, String> {
        self.descriptor_cache
            .read()
            .unwrap()
            .iter()
            .map(|((id, ver), _)| (format!("{id}@{ver}"), "cached".into()))
            .collect()
    }

    pub fn list_profiles(&self) -> Vec<mesa_core_types::DeviceProfile> {
        self.profiles.read().unwrap().clone()
    }

    pub fn get_profile(&self, id: &str) -> Option<mesa_core_types::DeviceProfile> {
        self.profiles
            .read()
            .unwrap()
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    /// Browse（§20）：临时进程 + 分页，用于 OPC UA 等支持浏览的驱动
    pub async fn browse(
        &self,
        driver_id: &str,
        connection_json: &str,
        parent: &str,
        filter: &str,
        cursor: &str,
        limit: u32,
    ) -> Result<(Vec<mesa_driver_protocol::pb::BrowseNode>, Option<String>), DescriptorError> {
        let disc = self
            .find_driver(driver_id)
            .ok_or_else(|| DescriptorError::new("DRIVER_UNAVAILABLE", format!("driver `{driver_id}` not found")))?;
        let mut proc = crate::process::DriverProcess::spawn(&disc)
            .await
            .map_err(|e| DescriptorError::new("DRIVER_UNAVAILABLE", format!("spawn failed: {e}")))?;
        let port = proc.port;
        let token = proc.token.clone();
        let (mut session, _events, _) = match crate::session::Session::connect_retry(port, &token).await {
            Ok(v) => v,
            Err(e) => {
                proc.terminate().await;
                return Err(DescriptorError::new(
                    "DRIVER_UNAVAILABLE",
                    format!("handshake failed: {e}"),
                ));
            }
        };
        // 打开临时连接（handle 1）
        let handle = 1;
        let open_res = session
            .call(mesa_driver_protocol::pb::envelope::Body::OpenConnection(
                mesa_driver_protocol::pb::OpenConnection {
                    connection_handle: handle,
                    endpoint_id: format!("browse-{driver_id}"),
                    config_json: connection_json.to_string(),
                },
            ))
            .await
            .map_err(|e| DescriptorError::new("DRIVER_UNAVAILABLE", format!("open failed: {e}")))?;
        match open_res.body {
            Some(mesa_driver_protocol::pb::envelope::Body::OpenConnectionAck(ack)) => {
                let ok = ack.result.map(|r| r.ok).unwrap_or(false);
                if !ok {
                    session.invalidate();
                    proc.terminate().await;
                    return Err(DescriptorError::new("DRIVER_UNAVAILABLE", "open not ok"));
                }
            }
            Some(mesa_driver_protocol::pb::envelope::Body::DriverError(e)) => {
                let d = e.detail.unwrap_or_default();
                session.invalidate();
                proc.terminate().await;
                return Err(DescriptorError::new(d.code, d.message));
            }
            _ => {
                session.invalidate();
                proc.terminate().await;
                return Err(DescriptorError::new("DRIVER_UNAVAILABLE", "open unexpected"));
            }
        }
        let res = session
            .browse(handle, parent, filter, cursor, limit)
            .await
            .map_err(|e| DescriptorError::new("BROWSE_FAILED", format!("{e}")))?;
        session.invalidate();
        proc.terminate().await;
        Ok(res)
    }

    /// Control Write（§22）：经由活跃会话的可靠 Control 队列转发，永不 Latest-Wins
    pub async fn control_write(
        &self,
        endpoint_id: &str,
        target: &str,
        value: mesa_core_types::Value,
        expected: Option<mesa_core_types::Value>,
    ) -> Result<Option<mesa_core_types::Value>, DescriptorError> {
        let sess_arc = {
            let m = self.active_sessions.read().unwrap();
            m.get(endpoint_id).cloned()
        }
        .ok_or_else(|| DescriptorError::new("ENDPOINT_NOT_RUNNING", format!("endpoint `{endpoint_id}` not running")))?;
        let request_id = format!("wr-{}-{}", endpoint_id, mesa_core_types::now_unix_ns());
        let sess = sess_arc.lock().await;
        // 约定 handle 1 为 Endpoint 主连接（endpoint.rs HANDLE=1）
        sess.write(1, &request_id, target, value, expected)
            .await
            .map_err(|e| DescriptorError::new("CONTROL_FAILED", format!("{e}")))
    }

    /// Control Command（§22）：可靠转发，返回 (status, result_json, error)
    pub async fn control_command(
        &self,
        endpoint_id: &str,
        command_id: &str,
        input_json: &str,
    ) -> Result<(String, String, String), DescriptorError> {
        let sess_arc = {
            let m = self.active_sessions.read().unwrap();
            m.get(endpoint_id).cloned()
        }
        .ok_or_else(|| DescriptorError::new("ENDPOINT_NOT_RUNNING", format!("endpoint `{endpoint_id}` not running")))?;
        let request_id = format!("cmd-{}-{}", endpoint_id, mesa_core_types::now_unix_ns());
        let sess = sess_arc.lock().await;
        sess.command(1, &request_id, command_id, input_json)
            .await
            .map_err(|e| DescriptorError::new("CONTROL_FAILED", format!("{e}")))
    }

    /// 供 Endpoint 运行时注册/注销活跃会话（§22 Control 面）
    pub fn register_session(
        &self,
        endpoint_id: &str,
        sess: crate::session::Session,
    ) -> std::sync::Arc<tokio::sync::Mutex<crate::session::Session>> {
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(sess));
        self.active_sessions
            .write()
            .unwrap()
            .insert(endpoint_id.to_string(), std::sync::Arc::clone(&arc));
        arc
    }

    pub fn unregister_session(&self, endpoint_id: &str) {
        self.active_sessions.write().unwrap().remove(endpoint_id);
    }

    /// 直接通过 registry 操作（供 endpoint.rs 使用，避免 &self 借用冲突）
    pub fn registry(&self) -> std::sync::Arc<RwLock<HashMap<String, std::sync::Arc<tokio::sync::Mutex<crate::session::Session>>>>> {
        std::sync::Arc::clone(&self.active_sessions)
    }
}
