/// 与 Cargo 共用构建目录，允许本地在服务运行时使用隔离产物进行验证。
/// 相对目录以 workspace 根解析；显式目录不回落默认 target，防止混用旧二进制。
pub fn target_dir() -> std::path::PathBuf {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(path) => root.join(path),
        None => root.join("target"),
    }
}
