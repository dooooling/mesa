//! Profile 加载器（V2.1 §10, §44）：扫描 drivers/<driver>/profiles/*.json

use std::path::Path;

use mesa_core_types::DeviceProfile;

/// 扫描 `drivers_dir` 下所有 `profiles/*.json`，返回校验通过的 Profile 列表。
pub fn load_profiles(drivers_dir: &Path) -> Vec<DeviceProfile> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(drivers_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let driver_dir = entry.path();
        if !driver_dir.is_dir() {
            continue;
        }
        let profiles_dir = driver_dir.join("profiles");
        if !profiles_dir.is_dir() {
            continue;
        }
        let Ok(profile_entries) = std::fs::read_dir(&profiles_dir) else {
            continue;
        };
        for pe in profile_entries.flatten() {
            let path = pe.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                tracing::warn!(path=%path.display(), "profile read failed");
                continue;
            };
            match serde_json::from_str::<DeviceProfile>(&content) {
                Ok(p) => match p.validate() {
                    Ok(()) => out.push(p),
                    Err(e) => {
                        tracing::warn!(path=%path.display(), error=%e, "profile validate failed")
                    }
                },
                Err(e) => tracing::warn!(path=%path.display(), error=%e, "profile parse failed"),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn load_profiles_finds_at_least_simulator() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../drivers");
        let profiles = load_profiles(&dir);
        assert!(profiles.iter().any(|p| p.id == "simulator-basic"));
        assert!(profiles.iter().any(|p| p.id == "fanuc-0i-f-plus"));
    }
}
