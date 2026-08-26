//! Driver Manifest 解析与发现（方案 §15）。
//!
//! 约定：每个 Driver 以独立目录发布（`driver.toml` + 可执行文件）；
//! DriverManager 启动/Rescan 时扫描并做静态校验，运行时版本协商交给 Hello/Welcome。

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// `driver.toml` 的最小 Schema。
#[derive(Debug, Clone, Deserialize)]
pub struct DriverManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable: String,
    pub protocol_major: u32,
    pub protocol_minor: u32,
    #[serde(default)]
    pub sdk: Option<String>,
    /// 可选平台约束；声明后必须与宿主匹配，否则 PLATFORM_MISMATCH 拒绝启动。
    #[serde(default)]
    pub os: Option<Vec<String>>,
    #[serde(default)]
    pub arch: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredDriver {
    pub manifest: DriverManifest,
    pub manifest_dir: PathBuf,
    /// 解析到的可执行文件路径；None 表示未找到二进制。
    pub executable_path: Option<PathBuf>,
    pub platform_ok: bool,
    pub platform_reason: Option<String>,
    pub protocol_ok: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Parse(#[from] toml::de::Error),
}

impl DiscoveredDriver {
    /// 是否满足启动前置条件（可执行文件存在 + 平台匹配 + 协议 Major 兼容）。
    pub fn launchable(&self) -> bool {
        self.executable_path.is_some() && self.platform_ok && self.protocol_ok
    }
}

/// 扫描 `root` 下的一级子目录，收集合法 Manifest。重复 `id` 后者跳过并告警，
/// 单个目录解析失败不影响其余驱动的发现。
pub fn scan_drivers(root: &Path) -> Vec<DiscoveredDriver> {
    let mut out = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(root = %root.display(), err = %e, "drivers dir unreadable");
            return out;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("driver.toml");
        let raw = match std::fs::read_to_string(&manifest_path) {
            Ok(r) => r,
            Err(_) => continue, // 无 Manifest 的目录视为普通目录，静默跳过
        };
        let manifest: DriverManifest = match toml::from_str(&raw) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(dir = %path.display(), err = %e, "invalid driver.toml");
                continue;
            }
        };

        // 静态校验（§15）：id 唯一、SemVer 格式、协议 Major 预检查
        if !seen_ids.insert(manifest.id.clone()) {
            tracing::warn!(id = %manifest.id, "duplicate driver id, skipped");
            continue;
        }
        if !is_semver_like(&manifest.version) {
            tracing::warn!(id = %manifest.id, v = %manifest.version, "version is not x.y.z, skipped");
            continue;
        }
        let protocol_ok = manifest.protocol_major == forgelink_driver_protocol::PROTOCOL_MAJOR;
        if !protocol_ok {
            tracing::warn!(
                id = %manifest.id,
                declared = manifest.protocol_major,
                expected = forgelink_driver_protocol::PROTOCOL_MAJOR,
                "protocol major mismatch"
            );
        }

        let (platform_ok, platform_reason) = check_platform(&manifest);
        let executable_path = resolve_executable(&path, &manifest.executable);

        out.push(DiscoveredDriver {
            manifest,
            manifest_dir: path,
            executable_path,
            platform_ok,
            platform_reason,
            protocol_ok,
        });
    }
    out
}

fn check_platform(m: &DriverManifest) -> (bool, Option<String>) {
    let host_os = std::env::consts::OS; // "windows" | "linux"
    let host_arch = std::env::consts::ARCH; // "x86_64"
    if let Some(os_list) = &m.os {
        if !os_list.iter().any(|o| o.eq_ignore_ascii_case(host_os)) {
            return (false, Some(format!("PLATFORM_MISMATCH: os {host_os} not in {os_list:?}")));
        }
    }
    if let Some(arch_list) = &m.arch {
        if !arch_list.iter().any(|a| a.eq_ignore_ascii_case(host_arch)) {
            return (false, Some(format!("PLATFORM_MISMATCH: arch {host_arch} not in {arch_list:?}")));
        }
    }
    (true, None)
}

/// 可执行文件解析顺序：
/// 1. Manifest 同目录（部署形态）；
/// 2. 仓库 target/{debug,release}/（开发形态：cargo build 后无需拷贝）。
///
/// TODO: 打包阶段引入安装布局约定后收敛为单一规则（§25）。
fn resolve_executable(dir: &Path, exe: &str) -> Option<PathBuf> {
    for name in [exe.to_string(), format!("{exe}.exe")] {
        let p = dir.join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    // 向上查找 target 目录（最多 5 层），适配 workspace 根在上级的情况
    let mut cur = dir.parent();
    for _ in 0..5 {
        let Some(p) = cur else { break };
        for profile in ["debug", "release"] {
            let cand = p.join("target").join(profile).join(exe);
            if cand.is_file() {
                return Some(cand);
            }
            let cand_exe = p.join("target").join(profile).join(format!("{exe}.exe"));
            if cand_exe.is_file() {
                return Some(cand_exe);
            }
        }
        cur = p.parent();
    }
    None
}

/// 轻量 SemVer 形状校验（x.y.z 全数字）。完整 SemVer 语义校验留给发布工具链。
fn is_semver_like(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("driver.toml");
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn scan_finds_valid_and_skips_invalid() {
        let root = std::env::temp_dir().join(format!("fl-scan-{}", now_ns()));
        let good = root.join("good");
        write_manifest(
            &good,
            &format!(
                r#"
id = "sim"
name = "Sim"
version = "0.1.0"
executable = "sim-driver"
protocol_major = {}
protocol_minor = 0
os = ["{}"]
arch = ["{}"]
"#,
                forgelink_driver_protocol::PROTOCOL_MAJOR,
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        );
        // 放一个假二进制保证 launchable
        std::fs::write(good.join("sim-driver.exe"), b"MZ").ok();
        std::fs::write(good.join("sim-driver"), b"ELF").ok();

        let bad_toml = root.join("badtoml");
        write_manifest(&bad_toml, "not = valid ===");

        let wrong_proto = root.join("wrongproto");
        std::fs::create_dir_all(&wrong_proto).unwrap();
        std::fs::write(
            wrong_proto.join("driver.toml"),
            "id=\"wp\"\nname=\"W\"\nversion=\"1.0.0\"\nexecutable=\"w\"\nprotocol_major=99\nprotocol_minor=0\n",
        )
        .unwrap();

        let found = scan_drivers(&root);
        assert_eq!(found.len(), 2, "good + wrongproto, badtoml skipped");
        let sim = found.iter().find(|d| d.manifest.id == "sim").unwrap();
        assert!(sim.launchable(), "sim must be launchable: {:?}", sim.platform_reason);
        let wp = found.iter().find(|d| d.manifest.id == "wp").unwrap();
        assert!(!wp.protocol_ok);

        std::fs::remove_dir_all(&root).ok();
    }

    fn now_ns() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn semver_shape_check() {
        assert!(is_semver_like("0.1.0"));
        assert!(is_semver_like("10.20.30"));
        assert!(!is_semver_like("1.2"));
        assert!(!is_semver_like("a.b.c"));
        assert!(!is_semver_like("1.2.x"));
    }
}
