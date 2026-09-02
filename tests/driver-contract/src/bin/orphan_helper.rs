//! 辅助进程：模拟 Core，拉起 Driver 并持有 Job/Session，待外层测试杀死本进程以验证孤儿防护。
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn sim_exe() -> PathBuf {
    let target = repo_root().join("target");
    for profile in ["debug", "release"] {
        for name in ["mesa-driver-simulator.exe", "mesa-driver-simulator"] {
            let p = target.join(profile).join(name);
            if p.is_file() {
                return p;
            }
        }
    }
    panic!("simulator binary not built");
}

#[tokio::main]
async fn main() {
    // 发现 simulator（手工构造，避免 scan 依赖）
    let exe = sim_exe();
    let disc = mesa_driver_manager::manifest::DiscoveredDriver {
        manifest: mesa_driver_manager::manifest::DriverManifest {
            id: "simulator".into(),
            name: "Mesa Simulator".into(),
            version: "0.0.0".into(),
            executable: exe.file_name().unwrap().to_string_lossy().to_string(),
            protocol_major: mesa_driver_protocol::PROTOCOL_MAJOR,
            protocol_minor: mesa_driver_protocol::PROTOCOL_MINOR,
            sdk: None,
            os: None,
            arch: None,
        },
        manifest_dir: PathBuf::new(),
        executable_path: Some(exe),
        platform_ok: true,
        platform_reason: None,
        protocol_ok: true,
    };
    let proc = mesa_driver_manager::process::DriverProcess::spawn(&disc)
        .await
        .expect("spawn driver from helper");
    println!("DRIVER_PID={}", proc.pid);
    println!("HELPER_PID={}", std::process::id());
    // 保持进程存活并持有 _job / stdin，不退出；外层测试会杀死本进程
    // 故意泄漏 proc 以避免 Drop 关闭 stdin / Job；外层杀死 helper 才是测试目标
    std::mem::forget(proc);
    // 阻塞至被杀死
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
