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

- **当前**：无 FANUC 真机在网，`NativeFocasApi` 以 `Fake` 为默认，`use_native=true` 时若库缺失或网络不可达返回 `EW_NODLL/EW_SOCKET` 由 `DriverManager` 按 §11 退避重连，进入 `RECONNECTING` 而非 `FAILED`
- **后续**：落实至少一台 `0i-F` 或 `30i-B` Ethernet 真机，验证 `cnc_allclibhndl3(ip,port,timeout)` 建连、超时、断线重连；`FakeFocasApi` 仅用于 CI，最终兼容性以真机为准 `§20`

## 6. 发布 Gate

- [x] SDK 来源与版本留痕
- [x] 再分发策略明确（不随二进制分发）
- [x] 平台库清单与加载路径验证
- [x] 函数矩阵与 Phase B 已实现/预留标注
- [ ] 真机联调（待客户提供 CNC）

> 本文档即 Gate 凭证，随 `61f1111` 之后提交入仓，未完成真机项前 FOCAS2 仅以 Fake 模式对外演示。
