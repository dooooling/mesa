//! DriverManager 编排入口：发现驱动、启动 Endpoint 运行时、统一停机。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use tokio_util::sync::CancellationToken;

use crate::endpoint::{run_endpoint, BuiltinEndpoint, PointIdAllocator, PointIdSource};
use crate::manifest::{scan_drivers, DiscoveredDriver};
use crate::snapshot::{DriverInfo, Snapshot};

struct RunningEntry {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

pub struct ForgeLinkManager {
    drivers: RwLock<Vec<DiscoveredDriver>>,
    snapshot: Arc<Snapshot>,
    source: Arc<dyn PointIdSource>,
    running: Mutex<HashMap<String, RunningEntry>>,
    shutdown: CancellationToken,
}

impl ForgeLinkManager {
    /// 扫描驱动目录并填充快照中的驱动清单（使用内存版 ID 源，兼容 M0/Contract Test）。
    pub fn discover(drivers_dir: &Path) -> Self {
        Self::with_source(drivers_dir, Arc::new(PointIdAllocator::default()))
    }

    /// 使用指定 [`PointIdSource`] 创建 Manager（forgelinkd 传入持久版）。
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
        }
    }

    fn driver_infos(drivers: &[DiscoveredDriver]) -> Vec<DriverInfo> {
        drivers
            .iter()
            .map(|d| DriverInfo {
                id: d.manifest.id.clone(),
                name: d.manifest.name.clone(),
                version: d.manifest.version.clone(),
                protocol: format!("{}.{}", d.manifest.protocol_major, d.manifest.protocol_minor),
                launchable: d.launchable(),
                reason: if d.platform_ok {
                    d.executable_path.as_ref().map(|_| None).unwrap_or(Some("executable not found".into()))
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

    /// 重新扫描驱动目录，刷新可用驱动清单。
    pub fn rescan(&self, drivers_dir: &Path) -> Vec<DriverInfo> {
        let drivers = scan_drivers(drivers_dir);
        let infos = Self::driver_infos(&drivers);
        *self.drivers.write().unwrap() = drivers;
        self.snapshot.set_drivers(infos.clone());
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

        self.running.lock().unwrap().insert(
            cfg.endpoint_id,
            RunningEntry { cancel, handle },
        );
        Ok(())
    }

    /// 兼容旧名称。
    pub fn start_builtin_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        self.start_endpoint(cfg)
    }

    /// 停止指定 Endpoint。返回是否曾处于运行态。
    pub async fn stop_endpoint(&self, endpoint_id: &str) -> bool {
        let entry = self.running.lock().unwrap().remove(endpoint_id);
        let Some(entry) = entry else { return false; };
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
            m.drain().map(|(_, e)| { e.cancel.cancel(); e.handle }).collect()
        };
        for h in handles {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), h).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
