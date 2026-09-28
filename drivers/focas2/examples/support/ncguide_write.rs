//! 独立实验入口：写能力仅存在于 example，不扩展生产 Driver 的控制面。
//!
//! 口径（冻结）：
//! - `W-PMC-1 Native write semantics ✅ CLOSED`（`pmc_wrpmcrng` BYTE 写路径成立）。
//! - R100.0 为 controlled ladder test signal（后续测试值只做 `0 → 1 → 0`，
//!   不再整字节随意写；`0x5A` 仅用于 BYTE 写入完整性证明，已完成使命）。
//! - R100 当前无直接 ladder bit 引用 ✅ observed；绝对未被系统使用 ❌ NOT-PROVEN。
//! - 测试信号语义：`experiment_bit0` 只翻转 bit0 并保留其他 bit
//!   （read-modify-write 非原子；仅 STOP + 可重置仿真环境）。

trait ByteIo {
    fn read(&mut self) -> Result<u8, String>;
    fn write(&mut self, value: u8) -> Result<(), String>;
}

fn experiment_byte(io: &mut impl ByteIo, target: u8) -> Result<(), String> {
    let original = io.read()?;
    println!("快照 R100 = 0x{original:02X}；测试目标 = 0x{target:02X}");
    // 写调用报错也可能已到达远端，因此从首次写起，所有普通错误均进入恢复路径。
    // 这不是原子位写；只适用于允许修改、可重置的独立仿真环境。
    let result = io.write(target).and_then(|()| {
        let actual = io.read()?;
        println!("测试回读 R100 = 0x{actual:02X}");
        if actual == target {
            Ok(())
        } else {
            Err(format!(
                "回读不匹配：期望 {target:#04X}，实际 {actual:#04X}"
            ))
        }
    });
    let restore = io.write(original);
    let verified = io.read();
    match (restore, verified) {
        (Ok(()), Ok(value)) if value == original => println!("恢复已确认：R100 = 0x{value:02X}"),
        (write, read) => {
            return Err(format!(
                "恢复未确认！原值 R100={original:#04X}；恢复写={write:?}；回读={read:?}；测试结果={result:?}。请在 PMC STATUS 核查，勿重复测试。"
            ));
        }
    }
    result
}

/// W-PMC-2 controlled ladder signal（run_byte 内嵌套定义为唯一实现；
/// 此处旧顶层 `experiment/experiment_bit0` 已删除，避免双定义）。
/// R100.0 controlled ladder test signal：只置位 bit0（`0 → 1 → 0`），保留其他 bit。
/// BYTE 层实际：原值 `0x00` → set bit0 `0x01` → readback `0x01` → restore `0x00`.
/// R100 单字节受控写：`None` 为 R100.0──R101.0 ladder 最小链验证；
/// `Some(v)` 写指定字节值（仅 BYTE 写入完整性证明用，如已完成的 `0x5A`）。
/// 始终 read original → write target → readback → restore → readback，
/// restore 放 finally 语义（失败也恢复并如实报错）。
#[cfg(all(target_os = "windows", target_pointer_width = "64"))]
#[allow(dead_code)]
pub fn run(host: &str, port: u16, timeout_ms: u64) -> Result<(), String> {
    run_byte(host, port, timeout_ms, None)
}

/// R100 单字节受控写：`None` 为 R100.0 controlled signal（只置位 bit0，保留其他 bit）；
/// `Some(v)` 写指定字节值（仅 BYTE 写入完整性证明用，如已完成的 `0x5A`）。
/// 始终 read original → write target → readback → restore → readback，
/// restore 放 finally 语义（失败也恢复并如实报错）。
#[cfg(all(target_os = "windows", target_pointer_width = "64"))]
pub fn run_byte(host: &str, port: u16, timeout_ms: u64, target: Option<u8>) -> Result<(), String> {
    use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library};
    use std::ffi::{CString, c_char};
    // Windows FOCAS 使用 WINAPI；long 为 32 位。所有调用同步留在当前线程。
    type Open = unsafe extern "system" fn(*const c_char, u16, i32, *mut u16) -> i16;
    type Close = unsafe extern "system" fn(u16) -> i16;
    type Read = unsafe extern "system" fn(u16, i16, i16, u16, u16, u16, *mut Block) -> i16;
    type Write = unsafe extern "system" fn(u16, i16, *mut Block) -> i16;
    #[repr(C)]
    struct Block {
        area: i16,
        width: i16,
        start: u16,
        end: u16,
        bytes: [u8; 8],
    }
    impl Block {
        #[allow(dead_code)]
        fn new(value: u8) -> Self {
            Self::for_addr(100, value)
        }

        fn for_addr(addr: u16, value: u8) -> Self {
            Self {
                area: 5,
                width: 0,
                start: addr,
                end: addr,
                bytes: [value, 0, 0, 0, 0, 0, 0, 0],
            }
        }
    }
    fn check(name: &str, rc: i16) -> Result<(), String> {
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("{name}: FOCAS rc={rc}"))
        }
    }
    struct Session {
        handle: u16,
        read: Read,
        write: Write,
        close: Close,
    }

    /// W-PMC-2 controlled ladder signal：`R100.0 ──( R101.0 )` 最小链验证。
    /// 全程：R100 BYTE 读 + mask bit0 输出 R101.0；写仍只碰 R100.0（R101 由 ladder 驱动）。
    /// 流程（不假设 original）：before 双端快照 → RMW 置位 bit0 → 回读 R100
    /// （bit0==1 且其他 bit 与 original 一致）→ 等 ≥1 scan（100ms）→ 读 R101
    /// （bit0==1）→ restore 完整 original BYTE（禁硬编码 0x00）→ 等 ≥1 scan →
    /// 双端 readback（R100==original；original bit0==0 则 R101.0==0）。
    /// restore 放 finally 语义（失败也恢复并如实报错）。
    fn experiment_ladder(session: &mut Session) -> Result<(), String> {
        fn bit0(byte: u8) -> u8 {
            byte & 1
        }
        // before：双端快照（不假设 original）。
        let original = session.read()?;
        let r101_before = session.read_byte(101)?;
        println!(
            "before: R100 raw = 0x{original:02X} / bit0 = {}；R101 raw = 0x{r101_before:02X} / bit0 = {}",
            bit0(original),
            bit0(r101_before),
        );
        // 写调用报错也可能已到达远端，因此从首次写起，所有普通错误均进入恢复路径。
        let result = (|| {
            // RMW 只置位 R100.0（保留其他 bit）。
            let set = original | 0x01;
            session.write(set)?;
            let r100 = session.read()?;
            println!(
                "after set write rc=0；R100 raw = 0x{r100:02X} / bit0 = {}",
                bit0(r100),
            );
            // R100 回读：bit0==1 且其他 bit 与 original 一致。
            if bit0(r100) != 1 || (r100 & !0x01) != (original & !0x01) {
                return Err(format!(
                    "R100 回读不符：期望 bit0=1 且其他 bit 与 0x{original:02X} 一致，实际 0x{r100:02X}",
                ));
            }
            // 等 ≥1 PMC scan（ladder 消费信号；100ms 短等待）。
            std::thread::sleep(std::time::Duration::from_millis(100));
            let r101 = session.read_byte(101)?;
            println!("after set: R101 raw = 0x{r101:02X} / bit0 = {}", bit0(r101),);
            if bit0(r101) != 1 {
                return Err(format!(
                    "R100.0=1 期望 R101.0=1（ladder propagation），实际 R101.0={}",
                    bit0(r101),
                ));
            }
            Ok(())
        })();
        // restore：写回完整 original BYTE（禁硬编码 0x00）+ finally 双端 readback。
        let restore = session.write(original);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let v100 = session.read();
        let v101 = session.read_byte(101);
        match (&restore, &v100, &v101) {
            (Ok(()), Ok(a), Ok(b)) if *a == original => {
                let expect_r101 = if bit0(original) == 0 {
                    "0"
                } else {
                    "1（跟随 original bit0）"
                };
                println!(
                    "restore: write rc=0；R100 raw = 0x{a:02X} / bit0 = {}；R101 raw = 0x{b:02X} / bit0 = {}（期望 R101.0={expect_r101}）",
                    bit0(*a),
                    bit0(*b),
                );
                if bit0(original) == 0 && bit0(*b) != 0 {
                    return Err(format!(
                        "restore 后 R101.0 应回 0，实际 R101.0={}（R101=0x{b:02X}）",
                        bit0(*b),
                    ));
                }
            }
            (write, read100, read101) => {
                return Err(format!(
                    "恢复未确认！原值 R100={original:#04X}；恢复写={write:?}；R100 回读={read100:?}；R101 回读={read101:?}；测试结果={result:?}。请在 PMC STATUS 核查，勿重复测试。"
                ));
            }
        }
        result
    }
    impl Session {
        fn read_byte(&mut self, addr: u16) -> Result<u8, String> {
            let mut block = Block::for_addr(addr, 0);
            // 协议长度为 8 字节头 + 1 字节负载，不使用 Rust 结构体大小。
            let rc = unsafe { (self.read)(self.handle, 5, 0, addr, addr, 9, &mut block) };
            let value = block.bytes[0];
            println!(
                "pmc_rdpmcrng R{addr} BYTE start={addr} end={addr} length=9 rc={rc} value=0x{value:02X}"
            );
            check("pmc_rdpmcrng", rc)?;
            Ok(value)
        }
    }
    impl ByteIo for Session {
        fn read(&mut self) -> Result<u8, String> {
            self.read_byte(100)
        }
        fn write(&mut self, value: u8) -> Result<(), String> {
            let mut block = Block::for_addr(100, value);
            let rc = unsafe { (self.write)(self.handle, 9, &mut block) };
            println!(
                "pmc_wrpmcrng R100 BYTE start=100 end=100 length=9 rc={rc} value=0x{value:02X}"
            );
            check("pmc_wrpmcrng", rc)
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            let rc = unsafe { (self.close)(self.handle) };
            if rc != 0 {
                eprintln!("释放句柄失败：FOCAS rc={rc}");
            }
        }
    }
    let path =
        std::fs::canonicalize("drivers/focas2/libs/win/FWLIB64.dll").map_err(|e| e.to_string())?;
    // FOCAS 在连接时还会动态加载机型 DLL，LoadLibraryEx 的搜索标志不覆盖
    // 后续加载。此入口在创建任何运行时/线程前执行，安全设置进程 PATH。
    let mut paths = vec![path.parent().expect("DLL 父目录").to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let search_path = std::env::join_paths(paths).map_err(|e| e.to_string())?;
    unsafe {
        std::env::set_var("PATH", search_path);
    }
    // 库的生命周期覆盖句柄。
    let library = unsafe { Library::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH) }
        .map_err(|e| e.to_string())?;
    let (open, close, read, write) = unsafe {
        (
            *library
                .get::<Open>(b"cnc_allclibhndl3\0")
                .map_err(|e| e.to_string())?,
            *library
                .get::<Close>(b"cnc_freelibhndl\0")
                .map_err(|e| e.to_string())?,
            *library
                .get::<Read>(b"pmc_rdpmcrng\0")
                .map_err(|e| e.to_string())?,
            *library
                .get::<Write>(b"pmc_wrpmcrng\0")
                .map_err(|e| e.to_string())?,
        )
    };
    let host_c = CString::new(host).map_err(|e| e.to_string())?;
    let mut handle = 0;
    println!("受控写测试 {host}:{port}，仅 R100.0，BYTE length=9");
    check("cnc_allclibhndl3", unsafe {
        open(
            host_c.as_ptr(),
            port,
            timeout_ms.div_ceil(1000) as i32,
            &mut handle,
        )
    })?;
    let mut session = Session {
        handle,
        read,
        write,
        close,
    };
    match target {
        Some(value) => experiment_byte(&mut session, value),
        None => experiment_ladder(&mut session),
    }
}

/// W-PMC-2 controlled ladder signal（run 内嵌套定义见 run_byte；此处为冗余旧定义，删除）。
#[allow(dead_code)]
fn experiment_ladder_outer_unused() {}

#[allow(dead_code)]
fn experiment(_io: &mut impl ByteIo) -> Result<(), String> {
    Err("已由 experiment_ladder 替代（R100.0──R101.0 最小链）".into())
}

#[allow(dead_code)]
fn experiment_bit0(_io: &mut impl ByteIo, _original: u8) -> Result<(), String> {
    Err("已由 experiment_ladder 替代（R100.0──R101.0 最小链）".into())
}

#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
pub fn run(_: &str, _: u16, _: u64) -> Result<(), String> {
    Err("写测试当前仅支持 Windows 64 位与仓库 FWLIB64.dll".into())
}

#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
pub fn run_byte(_: &str, _: u16, _: u64, _: Option<u8>) -> Result<(), String> {
    Err("写测试当前仅支持 Windows 64 位与仓库 FWLIB64.dll".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        value: u8,
        writes: Vec<u8>,
        fail_first: bool,
        fail_restore: bool,
    }
    impl ByteIo for Fake {
        fn read(&mut self) -> Result<u8, String> {
            Ok(self.value)
        }
        fn write(&mut self, value: u8) -> Result<(), String> {
            self.writes.push(value);
            if self.fail_restore && self.writes.len() == 2 {
                return Err("恢复失败".into());
            }
            self.value = value;
            if self.fail_first && self.writes.len() == 1 {
                Err("写已生效但响应丢失".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn preserves_other_bits_and_restores_even_after_write_error() {
        for original in [0, 1, 0xAA, 0xFF] {
            for fail_first in [false, true] {
                let mut io = Fake {
                    value: original,
                    writes: vec![],
                    fail_first,
                    fail_restore: false,
                };
                assert_eq!(experiment(&mut io).is_err(), fail_first);
                // R100.0 controlled signal：只置位 bit0，保留其他 bit。
                assert_eq!(io.writes, [original | 0x01, original]);
                assert_eq!(io.value, original);
            }
        }
    }
    #[test]
    fn restoration_failure_is_never_reported_as_success() {
        let mut io = Fake {
            value: 0,
            writes: vec![],
            fail_first: false,
            fail_restore: true,
        };
        assert!(experiment(&mut io).unwrap_err().contains("恢复未确认"));
    }
}
