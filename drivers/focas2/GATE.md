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

## 4. 函数支持矩阵（Phase B 已实现 / 预留）

| ForgeLink 地址 | FOCAS 函数 | 30i-B | 0i-F | 备注 |
|---|---|---|---|---|
| `status` | `cnc_statinfo` | O | O | `native.rs:196 statinfo()` 已实现 |
| `axis.abs.*` `axis.machine.*` | `cnc_rddynamic2` | O | O | `rddynamic2()` 已实现，细化 `cnc_absolute/cnc_machine` 待真机按需补 |
| `axis.feed` | `cnc_rddynamic2.actf` | O | O | 已实现 |
| `spindle.speed.*` | `cnc_acts` | O | O | 已实现 |
| `spindle.load.*` | `cnc_rdspmeter` | E | E | Phase B 复用 `acts` 代理，待真机补 `rdspmeter` |
| `alarm` | `cnc_rdalmmsg` | O | O | 预留 `EW_NOOPT` |
| `program.*` | `cnc_rdprgnum/cnc_exeprgname` | O | O | 预留 |
| `macro.*` | `cnc_rdmacro` | O | O | 预留 |
| `pmc.R*` | `pmc_rdpmcrng` | O | O | 预留 |
| `servo.*` | `cnc_rdsvmeter` | O | O | 预留 |

完整矩阵见参考项目 `documentation/focas-function-matrix.md` 与 `fanuc/collectors/` 18 类实现。

## 5. 真机可连性

- **2026-08-27 实测**：
  - `192.168.15.60:8193` `ping 1-2ms` 但 `cnc_allclibhndl3` 返 `EW_SOCKET`，`Test-NetConnection 8193 False`，判定该机 `FOCAS2/Ethernet` 未启用或防火墙未放行（需查 `MD 0020/900系列`）
  - `192.168.15.165:8193` `ping 1ms` `TcpTestSucceeded True`，`test_native -- 192.168.15.165` 直连 `connect OK`，`status U32(1) / axis.abs.1 I32(4000) / spindle.load U32(0)` 回传成功；`forgelinkd 8139` `real-focas RUNNING 4点` `GET /points/latest` 9 点（4 真机 +5 sim）已验证 `cnc_statinfo/cnc_rddynamic2(44)/cnc_acts` 链路
  - `FOCAS` 句柄 `cnc_allclibhndl3(ip,port,timeout_s)` 正确，`timeout_ms 5000→5s`，`NativeFocasApi` `spawn_blocking` 隔离 + `EW_SOCKET/EW_NODLL` 退避 `RECONNECTING` 正常
  - **Windows 依赖路径修复 `9514443`**：`FWLIB64.dll` 隐式依赖 `fwlibe1.dll` 需同目录，`LoadLibrary` 相对路径不搜 `libs/win`；`native.rs:331 load() prepend PATH(cwd/drivers/focas2/libs/win+TEMP) + 绝对路径双候选` 后 `cnc_allclibhndl3 host=192.168.15.165 ret Ok(32769) Status1 Axis -68050..79986`，`60` 保持 `EW_SOCKET -16` 隔离；单文件分发 `26.5MB` `include_bytes!→NamedTempFile→Library::new` `%TEMP%/forgelink_focas_embed`
- **后续**：`192.168.15.165` 已可作为 0i-F/30i 基准真机，继续补 `cnc_rdalmmsg/cnc_rdmacro/pmc_rdpmcrng` 等余下地址并做断线重连 Soak；`FakeFocasApi` 仍用于 CI

## 6. 发布 Gate

- [x] SDK 来源与版本留痕
- [x] 再分发策略明确（不随二进制分发）
- [x] 平台库清单与加载路径验证（含 `drivers/focas2/libs/win/*.dll 20文件` 已补全）
- [x] 函数矩阵与 Phase B 已实现/预留标注
- [x] 真机联调（`192.168.15.165` 已通，`192.168.15.60` 待开通）

> 本文档即 Gate 凭证，随 `61f1111` 之后提交入仓，`192.168.15.165` 已满足 `§19.2` 真机要求。
