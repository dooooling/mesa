# 本地西门子参考手册（不提交 Git）

本目录用于存放本次 828D 变量库建设所依据的三份西门子官方手册。**PDF 文件本身不进入 Git**，以避免未经许可重新分发西门子文档；仓库只追踪手册清单（`../sinumerik-828d-manuals.json`）、文件 SHA-256 与由其派生的变量来源审计结果。

## 期望文件名

| 文件名 | 文档号 | 版本 |
|---|---|---|
| `828D_sysvar_lists_man_0723_zh-CHS.pdf` | `6FC5397-5FP40-0RA6` | 07/2023 |
| `828D_md_lists_man_0723_zh-CHS.pdf` | `6FC5397-4DP40-6RA6` | 07/2023 |
| `828D_S120_para_lists_man_0723_zh-CHS.pdf` | `A5E53014702F AA` | 07/2023 |

## 放置步骤

```powershell
New-Item -ItemType Directory -Force docs/reference/manuals
# 把三份 PDF 复制到本目录（文件名大小写、扩展名以实际为准）
Get-FileHash docs/reference/manuals/*.pdf -Algorithm SHA256
```

把得到的 SHA-256 填入 `../sinumerik-828d-manuals.json` 对应条目的 `sha256`，并把 `file_present` 置为 `true`、`status` 改为已核对。

> **状态（Phase 0 阶段 A，commit `c394584`）**：上述步骤已完成——三份 PDF 均在位，`file_present` 全为 `true`，
> SHA-256 / size_bytes / 页数 / 首页文档号版本均已实测填入；逐条交叉引用见 `../828d-manual-cross-reference.md`。
> 清单是否过期可用 `python ../check-manual-metadata.py` 机械检查（OK / WARN / FAIL 三档）。

## 硬性要求

- **不得**根据文档号、文件名或版本推算 SHA-256，只能对实际文件计算。
- **不得**用其他版本手册的章节/页码冒充 07/2023 版本。
- 现有 42 条定义的 manual 来源为 **828D NC variables 03/2025**（16 条）与 **S120/S150 参数手册 11/2023**（19 条），**与本目录的 07/2023 版本不等同**，不得互相追认。
- 文件已到位；07/2023 的章节页码只登记在交叉引用文档里，**不得**回填 r1.json（现有定义的 manual 来源是 03/2025 与 11/2023 版）。

## 为什么不用 `tests/fixtures/manuals/` 或 `target/`

这三份是**开发参考资料**，不是测试夹具（放 `tests/fixtures/` 会暗示它们参与自动化测试断言）；放 `target/` 会在清理构建目录时被误删。
