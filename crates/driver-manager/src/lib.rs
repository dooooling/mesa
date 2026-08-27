//! ForgeLink DriverManager（Core 侧）。
//!
//! 模块划分：
//! - [`manifest`]：driver.toml 解析与发现（§15）
//! - [`process`]：子进程 spawn / Job Object / 孤儿防护（§14.5）
//! - [`session`]：IPC 会话客户端与心跳判死（§14.3/§14.4）
//! - [`endpoint`]：Endpoint 配置闭环运行时（§6.2/§11.1）
//! - [`snapshot`]：REST 可见的进程内共享状态
//! - [`manager`]：编排入口

pub mod endpoint;
pub mod manager;
pub mod manifest;
pub mod process;
pub mod session;
pub mod snapshot;

pub use endpoint::{BuiltinEndpoint, PointIdAllocator, PointIdSource, StorePointIdSource};
pub use manager::ForgeLinkManager;
pub use snapshot::{DriverInfo, EndpointStatus, LatestEntry, Snapshot};
