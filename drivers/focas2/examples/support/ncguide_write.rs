//! 独立实验入口：写能力仅存在于 example，不扩展生产 Driver 的控制面。
//!
//! 口径（冻结）：
//!
//! - `W-PMC-1 Native write semantics ✅ CLOSED`（`pmc_wrpmcrng` BYTE 写路径成立）。
//! - R100.0 为 controlled ladder test signal（后续测试值只做 `0 → 1 → 0`，
//!   不再整字节随意写；`0x5A` 仅用于 BYTE 写入完整性证明，已完成使命）。
//! - R100 当前无直接 ladder bit 引用 ✅ observed；绝对未被系统使用 ❌ NOT-PROVEN。
//! - 测试信号语义：ladder RMW 只置位 bit0 并保留其他 bit
//!   （read-modify-write 非原子；仅 STOP + 可重置仿真环境）；
//!   首次写之前必须 `R100.0==0 && R101.0==0`（fail-closed，不写 PMC）。
//! - 真实实现唯一入口为 [`ladder_sequence`]（Windows FFI 与单测 Fake 共用；
//!   单测直接覆盖，PR #69 B1）；`experiment_byte` 仅服务整字节完整性路径。

/// R100 BYTE IO（non-test 下仅 Windows 编译；单测下全平台编译。
/// cfg 门保证 Linux 非测试 example 构建不残留死代码，CI `-D warnings` 下通过）。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
trait ByteIo {
    fn read(&mut self) -> Result<u8, String>;
    fn write(&mut self, value: u8) -> Result<(), String>;
}

/// W-PMC-2 最小链 IO（R100 读写 + R101 只读；R101 永不写入）。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
trait LadderIo {
    fn read_r100(&mut self) -> Result<u8, String>;
    fn write_r100(&mut self, value: u8) -> Result<(), String>;
    fn read_r101(&mut self) -> Result<u8, String>;
}

/// bit0 取值（纯函数；ladder 与单测共用）。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
fn bit0(byte: u8) -> u8 {
    byte & 1
}

/// RMW 目标：只置位 bit0，保留其他 bit（纯函数）。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
fn ladder_target(original: u8) -> u8 {
    original | 0x01
}

/// B2 fail-closed（纯函数）：首次写之前必须 `R100.0==0 && R101.0==0`；
/// 不满足即 Err，且调用方不得执行任何 PMC 写（见 [`ladder_sequence`]）。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
fn check_before_zero(r100: u8, r101: u8) -> Result<(), String> {
    if bit0(r100) != 0 || bit0(r101) != 0 {
        return Err(format!(
            "ladder 起点必须 R100.0==0 且 R101.0==0（实际 R100=0x{r100:02X} R101=0x{r101:02X}），未写入 PMC"
        ));
    }
    Ok(())
}

/// 置位后 R100 回读校验（纯函数）：bit0==1 且其他 bit 与 original 一致。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
fn check_set_readback(original: u8, r100: u8) -> Result<(), String> {
    if bit0(r100) != 1 || (r100 & !0x01) != (original & !0x01) {
        return Err(format!(
            "R100 回读不符：期望 bit0=1 且其他 bit 与 0x{original:02X} 一致，实际 0x{r100:02X}",
        ));
    }
    Ok(())
}

/// 传播校验（纯函数）：R100.0=1 后 R101.0 必须 ==1。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
fn check_propagated(r101: u8) -> Result<(), String> {
    if bit0(r101) != 1 {
        return Err(format!(
            "R100.0=1 期望 R101.0=1（ladder propagation），实际 R101.0={}",
            bit0(r101),
        ));
    }
    Ok(())
}

/// restore 校验（纯函数）：恢复写 Ok + R100 回读 == original；
/// original bit0==0 时还要求 R101.0 回 0。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
fn check_restored(
    original: u8,
    restore: &Result<(), String>,
    v100: &Result<u8, String>,
    v101: &Result<u8, String>,
    result: &Result<(), String>,
) -> Result<(), String> {
    match (restore, v100, v101) {
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
            Ok(())
        }
        (write, read100, read101) => Err(format!(
            "恢复未确认！原值 R100={original:#04X}；恢复写={write:?}；R100 回读={read100:?}；R101 回读={read101:?}；测试结果={result:?}。请在 PMC STATUS 核查，勿重复测试。"
        )),
    }
}

/// W-PMC-2 真实 ladder 安全实现（Windows FFI 与单测 Fake 共用唯一实现；
/// 单测直接覆盖本函数，PR #69 B1）。
/// 流程：before 双端快照 → B2 fail-closed → RMW 置位 → 回读校验 →
/// scan 等待 → 传播校验 → finally restore → 双端 readback 校验。
/// `sleep_ms` 由调用方注入（现场 100ms；单测 no-op）。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
fn ladder_sequence(io: &mut impl LadderIo, sleep_ms: &dyn Fn(u64)) -> Result<(), String> {
    // before：双端快照 + B2 fail-closed（首次写之前，不写 PMC）。
    let original = io.read_r100()?;
    let r101_before = io.read_r101()?;
    println!(
        "before: R100 raw = 0x{original:02X} / bit0 = {}；R101 raw = 0x{r101_before:02X} / bit0 = {}",
        bit0(original),
        bit0(r101_before),
    );
    check_before_zero(original, r101_before)?;
    // 写调用报错也可能已到达远端，因此从首次写起，所有普通错误均进入恢复路径。
    let result = (|| {
        let set = ladder_target(original);
        io.write_r100(set)?;
        let r100 = io.read_r100()?;
        println!(
            "after set write rc=0；R100 raw = 0x{r100:02X} / bit0 = {}",
            bit0(r100),
        );
        check_set_readback(original, r100)?;
        // 等 ≥1 PMC scan（ladder 消费信号）。
        sleep_ms(100);
        let r101 = io.read_r101()?;
        println!("after set: R101 raw = 0x{r101:02X} / bit0 = {}", bit0(r101),);
        check_propagated(r101)
    })();
    // finally restore（失败也恢复）+ 双端 readback。
    let restore = io.write_r100(original);
    sleep_ms(100);
    let v100 = io.read_r100();
    let v101 = io.read_r101();
    check_restored(original, &restore, &v100, &v101, &result)?;
    result
}

#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
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

    /// W-PMC-2 真实 ladder 入口：`Session` 适配顶层 [`ladder_sequence`]
    /// （唯一实现；单测直接覆盖同函数，PR #69 B1）。
    fn experiment_ladder(session: &mut Session) -> Result<(), String> {
        ladder_sequence(&mut SessionLadder(session), &|ms| {
            std::thread::sleep(std::time::Duration::from_millis(ms))
        })
    }
    /// `Session` 的 [`LadderIo`] 适配（顶层 impl，避免函数内 non-local impl）。
    struct SessionLadder<'a>(&'a mut Session);
    impl LadderIo for SessionLadder<'_> {
        fn read_r100(&mut self) -> Result<u8, String> {
            self.0.read_byte(100)
        }
        fn write_r100(&mut self, value: u8) -> Result<(), String> {
            ByteIo::write(self.0, value)
        }
        fn read_r101(&mut self) -> Result<u8, String> {
            self.0.read_byte(101)
        }
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

/// W-PMC-2 controlled ladder signal（run_byte 内嵌套定义见 run_byte；此处为冗余旧定义，删除）。
#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
#[allow(dead_code)]
fn experiment_ladder_outer_unused() {}

#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
#[allow(dead_code)]
fn experiment(_io: &mut impl ByteIo) -> Result<(), String> {
    Err("已由 experiment_ladder 替代（R100.0──R101.0 最小链）".into())
}

#[cfg(any(test, all(target_os = "windows", target_pointer_width = "64")))]
#[allow(dead_code)]
fn experiment_bit0(_io: &mut impl ByteIo, _original: u8) -> Result<(), String> {
    Err("已由 experiment_ladder 替代（R100.0──R101.0 最小链）".into())
}

#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
#[allow(dead_code)]
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
    /// 单测 Fake：R100 可写 + R101 由 ladder 语义跟随（`follow=true` 时
    /// R101.0 跟随 R100.0，模拟 `R100.0──(R101.0)`；`follow=false` 时 R101
    /// 恒 0，模拟 ladder 未导通；与现场 `experiment_ladder` 同语义）。
    struct Fake {
        r100: u8,
        r101_follow: bool,
        writes: Vec<u8>,
        fail_first: bool,
        fail_restore: bool,
    }
    impl Fake {
        fn r101(&self) -> u8 {
            if self.r101_follow {
                self.r100 & 0x01
            } else {
                0
            }
        }
    }
    impl LadderIo for Fake {
        fn read_r100(&mut self) -> Result<u8, String> {
            Ok(self.r100)
        }
        fn write_r100(&mut self, value: u8) -> Result<(), String> {
            self.writes.push(value);
            if self.fail_restore && self.writes.len() == 2 {
                return Err("恢复失败".into());
            }
            self.r100 = value;
            if self.fail_first && self.writes.len() == 1 {
                Err("写已生效但响应丢失".into())
            } else {
                Ok(())
            }
        }
        fn read_r101(&mut self) -> Result<u8, String> {
            Ok(self.r101())
        }
    }
    // 旧整字节路径 Fake（仅 `experiment_byte` 用；与 ladder 无关）。
    struct ByteFake {
        value: u8,
    }
    impl ByteIo for ByteFake {
        fn read(&mut self) -> Result<u8, String> {
            Ok(self.value)
        }
        fn write(&mut self, value: u8) -> Result<(), String> {
            self.value = value;
            Ok(())
        }
    }
    fn no_sleep(_: u64) {}
    /// B1：真实 ladder 安全实现直测——`0→1→0` 闭环 + RMW + restore。
    /// before 非零即 fail-closed 且不写 PMC（false PASS 防护）。
    #[test]
    fn ladder_closed_loop_and_before_fail_closed() {
        // 正常闭环：before 0/0 → set → R101.0==1 → restore → 双端 0。
        let mut io = Fake {
            r100: 0,
            r101_follow: true,
            writes: vec![],
            fail_first: false,
            fail_restore: false,
        };
        assert!(ladder_sequence(&mut io, &no_sleep).is_ok());
        assert_eq!(io.writes, [0x01, 0x00], "RMW 置位 + restore 原值");
        assert_eq!((io.r100, io.r101()), (0, 0), "双端恢复 0");
        // B2 起点门：original bit0==1（0xAB：bit0=1 且高位保留场景）
        // → fail-closed（不写 PMC）。注意 0xAA bit0==0，不触发起点门；
        // 高位保留由下正常路径覆盖（0xA0→0xA1→0xA0）。
        let mut io = Fake {
            r100: 0xAB,
            r101_follow: false,
            writes: vec![],
            fail_first: false,
            fail_restore: false,
        };
        // before R100.0==1（0xAB bit0=1）→ fail-closed（不写 PMC）。
        assert!(ladder_sequence(&mut io, &no_sleep).is_err());
        assert!(io.writes.is_empty(), "fail-closed 不得写 PMC");
        // 高位保留正常路径：original=0xA0（bit0=0）→ 置位 0xA1 → restore 0xA0。
        let mut io = Fake {
            r100: 0xA0,
            r101_follow: true,
            writes: vec![],
            fail_first: false,
            fail_restore: false,
        };
        assert!(ladder_sequence(&mut io, &no_sleep).is_ok());
        assert_eq!(io.writes, [0xA1, 0xA0], "高位保留 RMW");
        assert_eq!(io.r100, 0xA0);
        // before R101.0==1 → fail-closed（不写 PMC）。
        struct StuckHigh(Fake);
        impl LadderIo for StuckHigh {
            fn read_r100(&mut self) -> Result<u8, String> {
                Ok(0)
            }
            fn write_r100(&mut self, value: u8) -> Result<(), String> {
                self.0.writes.push(value);
                Ok(())
            }
            fn read_r101(&mut self) -> Result<u8, String> {
                Ok(0x01)
            }
        }
        let mut io = StuckHigh(Fake {
            r100: 0,
            r101_follow: false,
            writes: vec![],
            fail_first: false,
            fail_restore: false,
        });
        assert!(ladder_sequence(&mut io, &no_sleep).is_err());
        assert!(io.0.writes.is_empty(), "R101 残留时不得写 PMC");
        // ladder 未导通：set 后 R101.0==0 → Err，但 restore 仍执行。
        let mut io = Fake {
            r100: 0,
            r101_follow: false,
            writes: vec![],
            fail_first: false,
            fail_restore: false,
        };
        assert!(ladder_sequence(&mut io, &no_sleep).is_err());
        assert_eq!(io.writes, [0x01, 0x00], "失败也 restore");
        assert_eq!(io.r100, 0);
        // 写失败也 restore（fail_first：首写 Err 但可能已生效）。
        let mut io = Fake {
            r100: 0,
            r101_follow: true,
            writes: vec![],
            fail_first: true,
            fail_restore: false,
        };
        assert!(ladder_sequence(&mut io, &no_sleep).is_err());
        assert_eq!(io.writes, [0x01, 0x00]);
        assert_eq!(io.r100, 0);
    }
    #[test]
    fn restoration_failure_is_never_reported_as_success() {
        let mut io = Fake {
            r100: 0,
            r101_follow: true,
            writes: vec![],
            fail_first: false,
            fail_restore: true,
        };
        assert!(
            ladder_sequence(&mut io, &no_sleep)
                .unwrap_err()
                .contains("恢复未确认")
        );
    }
    #[test]
    fn byte_path_still_roundtrips() {
        let mut io = ByteFake { value: 0x5A };
        assert!(experiment_byte(&mut io, 0x5A).is_ok());
        assert_eq!(io.value, 0x5A);
    }
}
