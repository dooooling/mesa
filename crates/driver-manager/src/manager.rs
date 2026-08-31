//! DriverManager 编排入口：发现驱动、启动 Endpoint 运行时、统一停机。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use tokio_util::sync::CancellationToken;

use crate::endpoint::{BuiltinEndpoint, PointIdAllocator, PointIdSource, run_endpoint};
use crate::manifest::{DiscoveredDriver, scan_drivers};
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
        Self {
            drivers: RwLock::new(drivers),
            snapshot,
            source,
            running: Mutex::new(HashMap::new()),
            shutdown: CancellationToken::new(),
            descriptor_cache: RwLock::new(HashMap::new()),
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

    /// 重新扫描驱动目录，刷新可用驱动清单并清空 Descriptor 缓存（§4.5）。
    pub fn rescan(&self, drivers_dir: &Path) -> Vec<DriverInfo> {
        let drivers = scan_drivers(drivers_dir);
        let infos = Self::driver_infos(&drivers);
        *self.drivers.write().unwrap() = drivers;
        self.snapshot.set_drivers(infos.clone());
        // 清空全部 Descriptor Cache（§4.5 精确失效 1）
        self.descriptor_cache.write().unwrap().clear();
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
        let handle = tokio::spawn(run_endpoint(disc, cfg.clone(), snapshot, source, shutdown));

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
}
