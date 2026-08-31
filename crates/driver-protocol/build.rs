// prost 代码生成：proto 契约真值位于仓库根 proto/driver.proto，
// 生成的 Rust 类型通过 pb 模块对外暴露，本 crate 其余部分负责 framing 与转换。
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 优先使用环境变量指定的 protoc（CI 可锁版本），否则回退 vendored 二进制
    let protoc = std::env::var_os("PROTOC");
    if let Some(p) = protoc {
        unsafe {
            std::env::set_var("PROTOC", p);
        }
    } else {
        unsafe {
            std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
        }
    }

    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let proto_root = manifest.join("../../proto");

    prost_build::Config::new().compile_protos(&[proto_root.join("driver.proto")], &[proto_root])?;
    Ok(())
}
