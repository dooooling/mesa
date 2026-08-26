//! DriverManager 编排入口：发现驱动、启动 Endpoint 运行时、统一停机。

use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::endpoint::{run_endpoint, BuiltinEndpoint, PointIdAllocator};
use crate::manifest::{scan_drivers, DiscoveredDriver};
use crate::snapshot::{DriverInfo, Snapshot};

pub struct ForgeLinkManager {
    drivers: Vec<DiscoveredDriver>,
    snapshot: Arc<Snapshot>,
    allocator: Arc<PointIdAllocator>,
    shutdown: CancellationToken,
}

impl ForgeLinkManager {
    /// 扫描驱动目录并填充快照中的驱动清单。
    pub fn discover(drivers_dir: &Path) -> Self {
        let drivers = scan_drivers(drivers_dir);
        let infos: Vec<DriverInfo> = drivers
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
            .collect();
        let snapshot = Arc::new(Snapshot::new());
        snapshot.set_drivers(infos);
        Self {
            drivers,
            snapshot,
            allocator: Arc::new(PointIdAllocator::default()),
            shutdown: CancellationToken::new(),
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// 启动一个内置配置的 Endpoint。M0 仅支持每驱动单实例（handle=1）。
    pub fn start_builtin_endpoint(&self, cfg: BuiltinEndpoint) -> Result<(), String> {
        let disc = self
            .drivers
            .iter()
            .find(|d| d.manifest.id == cfg.driver_id)
            .filter(|d| d.launchable())
            .ok_or_else(|| format!("driver `{}` not found or not launchable", cfg.driver_id))?
            .clone();

        let shutdown = self.shutdown.child_token();
        let snapshot = Arc::clone(&self.snapshot);
        let allocator = Arc::clone(&self.allocator);
        tokio::spawn(run_endpoint(disc, cfg, snapshot, allocator, shutdown));
        Ok(())
    }

    /// 统一停机：取消全部 Endpoint 任务（各任务自行优雅终止子进程）。
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        // 给运行时一个收尾窗口；子进程终止有各自的宽限与强杀兜底
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}
