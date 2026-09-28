//! NCGuide PMC 探针：默认只读；独立写测试必须显式启用。
//! 从仓库根运行：cargo run -p mesa-driver-focas2 --example ncguide_machine_signal_probe

use mesa_driver_focas2::{FocasAddress, FocasApi, NativeFocasApi};
use std::process::ExitCode;
#[path = "support/ncguide_write.rs"]
mod ncguide_write;

const HELP: &str = "NCGuide PMC 测试（默认只读；--test-r100-bit0 显式启用翻转及恢复测试）
用法：ncguide_machine_signal_probe [--host IP] [--port PORT] [--timeout-ms MS]
默认：192.168.15.165:8193，超时 5000 ms；依次读取 R100、D0。
请从 Mesa 仓库根目录运行，以便找到 drivers/focas2/libs/win 下的 FOCAS DLL。
R100 按 WORD、D0 按 DWORD 请求；Native 接口可能降级重试较小宽度。
数值仅为当前读数，不断言 R100=0 或 D0=4。";

#[derive(Debug, PartialEq)]
struct Options {
    host: String,
    port: u16,
    timeout_ms: u64,
    test_write: bool,
    write_byte: Option<u8>,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut options = Self {
            host: "192.168.15.165".into(),
            port: 8193,
            timeout_ms: 5000,
            test_write: false,
            write_byte: None,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--test-r100-bit0" {
                options.test_write = true;
                continue;
            }
            if let Some(rest) = arg.strip_prefix("--test-r100-byte=") {
                let value: u8 = if let Some(hex) =
                    rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X"))
                {
                    u8::from_str_radix(hex, 16).map_err(|_| {
                        "--test-r100-byte 必须是 0..=255（支持 0x 前缀十六进制）".to_string()
                    })?
                } else {
                    rest.parse().map_err(|_| {
                        "--test-r100-byte 必须是 0..=255（支持 0x 前缀十六进制）".to_string()
                    })?
                };
                options.test_write = true;
                options.write_byte = Some(value);
                continue;
            }
            if arg == "--help" || arg == "-h" {
                return Ok(None);
            }
            if !matches!(arg.as_str(), "--host" | "--port" | "--timeout-ms") {
                return Err(format!("未知参数：{arg}"));
            }
            let value = args.next().ok_or_else(|| format!("{arg} 缺少值"))?;
            match arg.as_str() {
                "--host" => {
                    if value.trim().is_empty() || value.starts_with('-') || value.contains('\0') {
                        return Err("--host 必须是有效的主机名或 IP".into());
                    }
                    options.host = value;
                }
                "--port" => {
                    options.port = value
                        .parse()
                        .ok()
                        .filter(|v| *v > 0)
                        .ok_or("--port 必须在 1..=65535 范围内")?;
                }
                "--timeout-ms" => {
                    options.timeout_ms = value
                        .parse()
                        .ok()
                        .filter(|v| (1..=60000).contains(v))
                        .ok_or("--timeout-ms 必须在 1..=60000 范围内")?;
                }
                _ => unreachable!(),
            }
        }
        Ok(Some(options))
    }
}

async fn probe(api: &impl FocasApi, options: &Options) -> Result<(), String> {
    println!(
        "只读连接 {}:{}（超时 {} ms）",
        options.host, options.port, options.timeout_ms
    );
    api.connect(&options.host, options.port, options.timeout_ms)
        .await?;
    println!("连接成功。R100/R101 按 BYTE 读取并输出 bit0（W-PMC-2 最小链观测）。");
    let mut failed = false;
    // 分别读取：一个地址失败仍展示另一个地址的结果；连接成功后不提前返回，
    // 保证读取错误也会走统一 disconnect，FOCAS 调用仍由同一工作线程执行。
    // R100/R101 走 BYTE raw + 本地 mask bit0（与 W-PMC-2 ladder harness 同口径）。
    for (kind, addr) in [('R', 100), ('R', 101)] {
        let address = FocasAddress::Pmc {
            kind,
            addr,
            bit: None,
        };
        // NOTE：生产 read_batch 按 kind 定宽度（R→WORD）；此处只要 bit0，
        // 用 BYTE 语义需直调 Native byte 路径。probe 走生产口径时 R 系返回
        // WORD I32，bit0 取 `value & 1`（低字节即 R100 本体；R 为小端 WORD）。
        match api.read_batch(&[address]).await {
            Ok(values) if values.len() == 1 => {
                let (raw, bit0) = match &values[0] {
                    mesa_core_types::Value::I32(v) => (*v, (v & 1) as u8),
                    mesa_core_types::Value::U32(v) => (*v as i32, (v & 1) as u8),
                    other => {
                        failed = true;
                        eprintln!("{kind}{addr} 读取失败：非数值 {other:?}");
                        continue;
                    }
                };
                println!("{kind}{addr} raw = {raw}；{kind}{addr}.0 = {bit0}");
            }
            Ok(values) => {
                failed = true;
                eprintln!(
                    "{kind}{addr} 读取失败：应返回 1 个值，实际 {} 个",
                    values.len()
                );
            }
            Err(error) => {
                failed = true;
                eprintln!("{kind}{addr} 读取失败：{error}");
            }
        }
    }
    api.disconnect().await;
    println!("连接已关闭；全程未写入 PMC。");
    if failed {
        Err("部分 PMC 读取失败，请保留上面的错误码".into())
    } else {
        Ok(())
    }
}

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{error}\n{HELP}");
            return ExitCode::from(2);
        }
    };
    if options.test_write {
        return match ncguide_write::run_byte(
            &options.host,
            options.port,
            options.timeout_ms,
            options.write_byte,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("写测试失败：{error}");
                ExitCode::FAILURE
            }
        };
    }
    let api = NativeFocasApi::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("创建运行时失败");
    match runtime.block_on(probe(&api, &options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "测试失败：{error}\n请确认 NCGuide 已启动、PMC General 中端口一致，以及 DLL 与程序位数匹配。"
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_arguments_before_connecting() {
        for args in [
            vec!["--port", "0"],
            vec!["--port", "65536"],
            vec!["--timeout-ms", "0"],
            vec!["--timeout-ms", "60001"],
            vec!["--host"],
            vec!["--host", ""],
            vec!["--write"],
        ] {
            assert!(Options::parse(args.into_iter().map(String::from)).is_err());
        }
    }
}
