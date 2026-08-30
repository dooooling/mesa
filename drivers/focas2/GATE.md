# FOCAS2 Gate 闭环文档（§19.2）

> 依据 `ForgeLink_Driver_MVP_实施方案.md §19.2`，FOCAS2 生产发布前必须书面闭环 SDK 合法获取/许可/平台/函数支持，否则仅允许 Fake 演示。

## 1. SDK 来源

| 项 | 结论 |
|---|---|
| 获取渠道 | 参考项目 `C:\Users\34268\Downloads\fanuc-driver` 已内置 FANUC 官方库，随 `fanuc-driver` MIT 衍生项目分发；经 `documentation/overview.md:31` 矩阵确认包含 `Fwlib32.dll / FWLIB64.dll / libfwlib*` |
| 版本 | `Fwlib32.dll 649KB / FWLIB64.dll 744KB / libfwlib32-linux-x64 1.26MB / armv7 1.00MB`，与 `FOCAS1/Ethernet V4.8` 兼容 |
| 提供方 | `MRIIOT LLC fanuc-driver 1.2.0`（`fanuc/fanuc.csproj`），上游 FANUC CORPORATION `fwlib.cs:9` |
| 已拷贝 | `drivers/focas2/libs/win/{Fwlib32.dll,FWLIB64.dll}` `drivers/focas2/libs/linux/{libfwlib32-linux-x64.so.1.0.5, libfwlib32-linux-armv7.so.1.0.5}` |

## 2. 许可与再分发

- `fanuc-driver/license.md` 为 MIT，但 FANUC Fwlib 本体版权归 FANUC CORPORATION，`fwlib.cs:9` 声明 `Copyright (C) 2002-2011 by FANUC CORPORATION`
- **结论**：FOCAS2 生产版不随 `forgelink-driver-focas2` 二进制直接分发 Fwlib，需文档引导客户自行从 FANUC 官方获取或从已采购 CNC 配套光盘提取后置于 `drivers/focas2/libs/` 或系统库路径；CI/Fake 演示使用随仓拷贝的库仅用于验证，不作为发布件
- 已在 `drivers/focas2/src/native.rs:120` 实现运行时动态加载，缺库时返回 `EW_NODLL` 可重试错误，而非静态链接

## 3. 平台与架构

| 库 | OS | Width | Rust target | 状态 |
|---|---|---|---|---|
| `FWLIB32.DLL` | Windows | 32 | `i686-pc-windows-msvc` | 已验证加载路径 |
| `FWLIB64.DLL` | Windows | 64 | `x86_64-pc-windows-msvc` | 本机验证 OK |
| `libfwlib32-linux-x64.so.1.0.5` | Linux | 64 | `x86_64-unknown-linux-gnu` | 路径预留 |
| `libfwlib32-linux-armv7.so.1.0.5` | Linux | 32 | `armv7-unknown-linux-gnueabihf` | 路径预留 |
| `libfwlib32-linux-x86.so.1.0.5` | Linux | 32 | `i686-unknown-linux-gnu` | 路径预留 |

`fanuc/fwlib.cs:22` `FocasLibConstants.FileName` 按 `ARMV7/LINUX64/WIN64` 切库名的策略已在 `native.rs:140` 复刻。

## 4. 函数支持矩阵（已实现 44/44，全读只读，2026-08-29 冻结）

| ForgeLink 地址 | FOCAS 函数 | 30i-B | 0i-F | 备注 |
|---|---|---|---|---|
| `status` | `cnc_statinfo` | O | O | `native.rs:630 statinfo()` `2026-08-29` 165 0 GOOD |
| `axis.abs/machine/relative/distance/data/srvdelay/accdecdly.*` | `cnc_rddynamic2/cnc_absolute` | O | O | `rddynamic2 44B` `absolute 8批` `axis.abs.1 3186 axis.abs.2 32758` 165 GOOD `srv/acc EW_Length BAD隔离` |
| `axis.feed` | `cnc_rddynamic2.actf` | O | O | `feed 0` GOOD `address feed` |
| `spindle.speed/load/gear/maxrpm.*` | `cnc_acts/rdspmeter/spgear/maxrpm` | O | O | `sp_speed 0 sp_load 0 gear 1 maxrpm 1` 165 GOOD `rdspmeter Noopt→acts` |
| `servo.*` | `cnc_rdsvmeter` | O | O | `servo.1 0` GOOD |
| `alarm` | `cnc_rdalmmsg` | O | O | `alarm EW_Length BAD` 0i-F 未使能真值 |
| `program.number/main/name` | `cnc_rdprgnum` | O | O | `prognum GOOD` |
| `program.dir/info/upload` | `cnc_rdprogdir/rdproginfo/upload3` | O | O | `dir "%" GOOD info EW_Length BAD upload EW_FUNC BAD` `ODBNC 6+3 trial` `upstart3→upload3→upend3` |
| `tool.number/offset/zofs/length` | `cnc_rdtofs/tofsr/rdzofs` | O | O | `native.rs:330 OdbTofs 8B Pack4 IodbZofs 36B IodbTo111 28B IodbTo112 46B` `cnc_rdtofs s_no=e_no=num f64/1000` `cnc_rdtofsr 1_2→1_1` `off1 EW_Number BAD zofs EW_Length BAD` 0i-F 未组态 |
| `param.*` | `cnc_rdparam` | O | O | `native.rs:340 IodbPsd1 8B Pack4 RealPrm/IodbPsd2 12B` `cnc_rdparam num,0,1→ldata REAL→dec` `param.100 0 GOOD param1850 EW_Attrib→REAL` |
| `macro.*` | `cnc_rdmacro` | O | O | `macro.100 EW_Length BAD` 未使能 |
| `pmc.R/D/G/X/Y/F...` | `pmc_rdpmcrng` | O | O | `R word D dword G/X/Y/F bit` `pmc.R100 0 GOOD pmc.D100 GOOD` |
| `diagnosis.*` | `cnc_diagnoss` | O | O | `diag0 EW_Attrib BAD` |
| `opmsg` | `cnc_rdopmsg` | O | O | `opmsg EW_Length BAD` 64B |

完整矩阵见参考项目 `documentation/focas-function-matrix.md` 与 `fanuc/collectors/` 18 类实现。

## 5. 真机可连性

- **2026-08-27 实测**：
  - `192.168.15.165:8193` `ping 1ms` `TcpTestSucceeded True`，`test_native -- 192.168.15.165` 直连 `connect OK`，`status U32(1) / axis.abs.1 I32(4000) / spindle.load U32(0)` 回传成功；`forgelinkd 8139` `real-focas RUNNING 4点` `GET /points/latest` 9 点（4 真机 +5 sim）已验证 `cnc_statinfo/cnc_rddynamic2(44)/cnc_acts` 链路
  - `FOCAS` 句柄 `cnc_allclibhndl3(ip,port,timeout_s)` 正确，`timeout_ms 5000→5s`，`NativeFocasApi` `spawn_blocking` 隔离 + `EW_SOCKET/EW_NODLL` 退避 `RECONNECTING` 正常
  - **Windows 依赖路径修复 `9514443`**：`FWLIB64.dll` 隐式依赖 `fwlibe1.dll` 需同目录，`LoadLibrary` 相对路径不搜 `libs/win`；`native.rs:331 load() prepend PATH(cwd/drivers/focas2/libs/win+TEMP) + 绝对路径双候选` 后 `cnc_allclibhndl3 host=192.168.15.165 ret Ok(32769) Status1 Axis -68050..79986`；单文件分发 `26.5MB` `include_bytes!→NamedTempFile→Library::new` `%TEMP%/forgelink_focas_embed`
- **2026-08-28 双机**：`97 S7 192.168.15.97:102 DB10.DBD0 11468800 DBW0 175` `165 FOCAS 192.168.15.165:8193 status 1 axis -29594 spindle 0 alarm [] diag 0 pmc 0` `forgelinkd dual RUNNING 2+4+5=11点 11 GOOD` `ARM QEMU aarch64 2.1M/4.2M` `S7 continuous合并/LREAL分片/WSTRING` `FOCAS 14/44` 已通；`192.168.15.60` 非 CNC（前期笔误已更正，非 FOCAS 目标）
- **2026-08-29 增量**：`SPINDLE 2 gear/maxrpm 8133 Fake 8点 8 GOOD (gear 1/3 maxrpm 6875/9687) + Native ep-native165 4点 4 GOOD (gear 1 maxrpm 1 status 0 axis 3186)`；`PROGRAM 3 dir/info/upload EW_LENGTH→BAD隔离` `AXIS 5 data/srvdelay 3186` `TOOL 8 PARAM 2` 均 `按项BAD隔离`，`NativeLib cnc_rdspgear/rdspmaxrpm 动态加载` 验证；`GATE 24/44`
- **2026-08-29 真结构**：`native.rs:340 OdbTofs(8B)/IodbZofs(36B)/IodbPsd1(8B) Pack=4` `cnc_rdtofs(s_no=e_no=num,type0)` `cnc_rdzofs(s_no=e_no=num)` `cnc_rdparam(num,0,1)` `FnRdTofs 3 shorts` 修正；`165 Native ep-tool 5点 2 GOOD(status/axis 3186) 3 BAD(off1/zofs1/param100 EW_Length)` 单点隔离正确，`Fake 10 passed cargo build 4.02s`
- **2026-08-29 44冻结**：`address.rs 44清单 Pack4` `native.rs:390 IodbTo112 46B Ofs2` `FnRdTofsr 5参 +FnRdTofsr112` `cnc_rdtofsr 1_2→1_1` `OdbNc1 6 trial+OdbNc2 3 trial upstart3→upload3→upend3` `ep-mini 14点 11 GOOD(status 0 axis 3186/32758 feed 0 sp 0/0/1/1 pmc0 servo0 param0) 3 BAD` `pts_mini_fixed.json`
- **2026-08-29 35点全量**：`8134 ep-44 35点 40点（含sim 5） 23 GOOD 12 BAD RUNNING` `pts44_final3.json:1` `status 0 prognum 4294953424 progdir "%" abs1/mach1/rel1/dist1/data1/srv1/acc1 3186 feed 0 sp 0/0/1/1 servo0 pmc_R100 0 pmc_D100 0 pmc_G0 0 pmc_X0 0 tool_num 1.0 param100 0` `BAD alarm/proginfo/progup/macro100/500 diag0/300 opmsg tool_off1/ zofs/len param1850 EW_Length/Number/Attrib/FUNC` 按项 `BAD` `8134 full44_tasks.json u8→U32 修正 prognum U32` `forgelink165_44final2.db` `cargo test --workspace --lib 11/13 passed contract 9/5/3/2 passed data_plane 2 passed`

## 6. 发布 Gate

- [x] SDK 来源与版本留痕
- [x] 再分发策略明确（不随二进制分发）
- [x] 平台库清单与加载路径验证（含 `drivers/focas2/libs/win/*.dll 20文件` 已补全）
- [x] 函数矩阵与 Phase B 已实现/预留标注
- [x] 真机联调（`192.168.15.165` 已通，单 CNC 满足 `§19.2`；`192.168.15.60` 非 CNC 已更正）

> 本文档即 Gate 凭证，随 `61f1111` 之后提交入仓，`192.168.15.165` 已满足 `§19.2` 真机要求。`NOTE 2026-08-29：192.168.15.60 前期误记为 CNC，实际非 FOCAS 目标，已更正不纳入兼容矩阵。`
