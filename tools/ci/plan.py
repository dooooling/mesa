#!/usr/bin/env python3
"""Mesa CI v2 planner：变更路径 → 执行计划（CI 唯一调度真值）。

输入：变更文件列表（git diff --name-only），输出 JSON 计划：
{
  "plan_version": 5,
  "quality": true,            # rust-quality（fmt/clippy）
  "canonical": true,          # rust-canonical（Linux x64 完整语义）
  "canonical_filter": ...,    # nextest filterset：all() 或 rdeps(=pkg) 联合
  "canonical_soak": "skip",   # run / skip（长 soak 归属 stress）
  "platform_mode": "smoke",   # skip / smoke / full
  "platform_soak": "skip",    # run / skip（main full 才跑长 soak）
  "build_targets": ...,       # selective cargo 构建目标（见下）
  "stress": false,            # rust-stress
  "perf": false,              # rust-perf
  "web": true,                # web build/test
  "watchdog": false,          # stop-watchdog 离线场景（Windows + Linux，M0–M18）
  "e2e": false,               # NCK REST 端到端（Windows + Linux，真实 mesad + 模拟器）
  "reason": "...",            # 人类可读路由原因
}

watchdog（CI v4）：
- 只有 **watchdog 相关路径**变更才置真（scripts/stop-watchdog.ps1、tests/watchdog/**、
  .github/workflows/_watchdog.yml）；**绝不**无条件运行——它要起 Windows + Linux 两个 runner
  跑完整离线套件（单 job 25 分钟预算），与本仓库绝大多数改动无关。
- FULL（未知路径/空 diff/diff 失败/`.github/**`、`tools/ci/**` 等 fail-closed 路径）时为真：
  "无法判断影响面"时宁可多跑一次，也不要漏跑。

e2e（CI v5）：
- 只有 **E2E 相关路径**变更才置真（scripts/nck-native-rest-e2e.py、tools/nck-emulator/**、
  drivers/sinumerik-nck/**、crates/{core-api,driver-manager,config-store}/**、apps/mesad/**、
  tests/driver-contract/**、.github/workflows/_e2e.yml）；同样**绝不**无条件运行——
  它是"重"测试：真实 mesad 进程 + 真实 TCP 模拟器 + 临时 DB，双平台各 20 分钟预算。
- 触发面按"这条链路真正依赖的东西"选：NCK 驱动的地址编译/身份、Core 的任务门禁与
  REST、DriverManager 的启停/配置重放、ConfigStore 的持久化、mesad 的装配，
  以及承载同一批断言的 contract suites。改这些而不跑 E2E 等于让回归静默溜过去。
- FULL 时同样为真（fail-closed）。

build_targets（selective compile，CI v3）：
- {"kind": "bins"}：cargo build --workspace --bins（FULL/未知影响用）
- {"kind": "packages", "packages": [...]}：cargo build -p ...（driver-only 用）
- {"kind": "none"}：不单独构建（docs-only/web-only 用）

fail-closed：任何无法识别的非文档路径 → FULL；空 diff → FULL；
diff 失败 → FULL。
"""
from __future__ import annotations

import os
import sys
import tomllib

PLAN_VERSION = 5

# --- watchdog 相关路径（CI v4：只有这些路径变更才跑 watchdog 套件） ---
# 说明：
#   * scripts/stop-watchdog.ps1 —— 被测脚本本体；
#   * tests/watchdog/           —— 假端点桩 + 场景驱动 + 断言（含 fixtures）；
#   * .github/workflows/_watchdog.yml —— 该 job 自身（改了它必须验证它能跑）。
# 刻意**不**包含 .github/workflows/ci.yml：它属于 FULL_PATHS（任何改动都会触发全量 rust job），
# 而 watchdog 的接线是否正确由 ci-gate 在每个 PR 上校验（watchdog=false 时必须 skipped）。
WATCHDOG_PATHS = (
    "scripts/stop-watchdog.ps1",
    "tests/watchdog/",
)
WATCHDOG_CI_FILES = (
    ".github/workflows/_watchdog.yml",
)


def needs_watchdog(files: list[str]) -> bool:
    """watchdog 离线套件是否必须运行（只看与它真正相关的路径）。"""
    for p in files:
        if p in WATCHDOG_CI_FILES:
            return True
        for prefix in WATCHDOG_PATHS:
            if p == prefix.rstrip("/") or p.startswith(prefix):
                return True
    return False


# --- E2E（NCK REST 端到端）相关路径（CI v5） ---
# 说明（为什么是这些）：
#   * scripts/nck-native-rest-e2e.py —— 被测脚本本体（**非** workspace member，
#     因此还要进 E2E_ONLY_PATHS，否则会落到 unknown path → FULL）；
#   * tools/nck-emulator/            —— 真实 TCP 模拟器（对端行为）；
#   * drivers/sinumerik-nck/         —— 地址编译 / 身份 / 定义目录 / presets；
#   * crates/core-api/               —— REST 路由、任务门禁、point_key 唯一性；
#   * crates/driver-manager/         —— 端点启停、配置重放、PointMap 应用；
#   * crates/config-store/           —— 任务/端点持久化（重启后 point_id 稳定性）；
#   * apps/mesad/                    —— 进程装配（DB / 端口 / drivers-dir）；
#   * tests/driver-contract/         —— 同一批 REST/端点/数据面断言的 contract suites。
# 刻意**不**包含 .github/workflows/ci.yml：它属 FULL_PATHS，接线是否正确由 ci-gate
# 在每个 PR 上校验（e2e=false 时必须 skipped）。
E2E_PATHS = (
    "scripts/nck-native-rest-e2e.py",
    "tools/nck-emulator/",
    "drivers/sinumerik-nck/",
    "crates/core-api/",
    "crates/driver-manager/",
    "crates/config-store/",
    "apps/mesad/",
    "tests/driver-contract/",
)
E2E_CI_FILES = (".github/workflows/_e2e.yml",)

# E2E 相关、但**不是** workspace member、也没有 SPECIAL_PACKAGES 映射的路径。
# 它们必须被显式认作"已知的非 Rust 路径"，否则会 fall through 到 unknown → FULL，
# 让改一行 E2E 脚本也触发全量 Rust 集成（与"不要无条件消耗 runner"相反）。
E2E_ONLY_PATHS = ("scripts/nck-native-rest-e2e.py",)


def needs_e2e(files: list[str]) -> bool:
    """NCK REST 端到端是否必须运行（只看与它真正相关的路径）。"""
    for p in files:
        if p in E2E_CI_FILES:
            return True
        for prefix in E2E_PATHS:
            if p == prefix.rstrip("/") or p.startswith(prefix):
                return True
    return False


def needs_e2e_only(files: list[str]) -> bool:
    """该路径是否属于"已知的 E2E-only 非 Rust 路径"（用于避免 unknown→FULL）。"""
    for p in files:
        for prefix in E2E_ONLY_PATHS:
            if p == prefix.rstrip("/") or p.startswith(prefix):
                return True
    return False

# --- workspace members（从 Cargo.toml 自动发现，不手写） ---

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def workspace_packages() -> dict[str, str]:
    """member dir → package name（读每个 member/Cargo.toml）。"""
    with open(os.path.join(REPO_ROOT, "Cargo.toml"), "rb") as f:
        members = tomllib.load(f)["workspace"]["members"]
    out: dict[str, str] = {}
    for m in members:
        with open(os.path.join(REPO_ROOT, m, "Cargo.toml"), "rb") as f:
            out[m.rstrip("/")] = tomllib.load(f)["package"]["name"]
    return out


# --- 静态路由表（路径前缀 → package；特殊路径单独处理） ---

# runtime-sensitive packages（改动触发 stress）。
# test-driver 改动同样触发（contract/perf 基线能力）。
STRESS_PACKAGES = {
    "mesa-core-types",
    "mesa-driver-protocol",
    "mesa-driver-sdk",
    "mesa-driver-manager",
    "mesa-config-store",
    "mesa-event-store",
    "mesa-core-api",
    "mesad",
    "mesa-test-driver",
}

# performance-sensitive packages（改动触发 perf）。
PERF_PACKAGES = {
    "mesa-core-types",
    "mesa-driver-protocol",
    "mesa-driver-sdk",
    "mesa-driver-manager",
    "mesa-config-store",
    "mesa-core-api",
    "mesa-test-driver",
    "mesa-performance-tests",
}

# runtime-sensitive contract suites（自身修改触发 stress）。
STRESS_SUITES = {
    "event_runtime",
    "event_soak",
    "event_pressure",
    "event_store_faults",
    "event_persistence",
    "event_retention_pressure",
    "stop_lifecycle",
    "management_api",
    "session_lifecycle",
    "subprocess_recovery",
    "subprocess_orphan_guard",
    "mesad_restart",
    "fault_tolerance",
}

# 特殊路径 → 额外 packages（Cargo dependency 表达不了的运行时依赖）。
SPECIAL_PACKAGES: dict[str, list[str]] = {
    # mesad_restart 从 target/debug/mesad 启动 mesad 二进制。
    "apps/mesad": ["mesa-contract-tests"],
    # nck-emulator 改动影响 sinumerik 驱动与 contract tests。
    "tools/nck-emulator": ["mesa-driver-sinumerik-nck", "mesa-contract-tests"],
    # 根 fixtures 供 contract tests 消费。
    "tests/fixtures": ["mesa-contract-tests"],
}

# 强制 FULL 的路径前缀/精确名（fail-closed，不做 dependency-filter）。
FULL_PATHS = (
    ".github/",
    "tools/ci/",
    ".cargo/",
    ".config/",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
)


# --- workspace rdeps（Cargo metadata 反向依赖闭包；不解析 filter 字符串） ---

_RDEPS_CACHE: dict[str, set[str]] | None = None


def reverse_deps() -> dict[str, set[str]]:
    """package → 被哪些 packages 直接依赖（workspace 内，含 dev-deps）。

    从 Cargo.toml 的 [dependencies]/[dev-dependencies]/[build-dependencies]
    path 依赖解析；workspace = true 的 path 依赖回查 member 名。
    缓存一次（planner 进程内多次调用共用）。
    """
    global _RDEPS_CACHE
    if _RDEPS_CACHE is not None:
        return _RDEPS_CACHE
    members = workspace_packages()  # dir → name
    # 正向：path → package 名（归一化，对大小写/分隔符不敏感比对用）。
    def _norm(p: str) -> str:
        return os.path.normpath(p).replace("\\", "/").lower()

    norm_to_name = {_norm(m): name for m, name in members.items()}
    # package name → 直接依赖的 workspace package names。
    deps: dict[str, set[str]] = {name: set() for name in members.values()}
    for member_dir, pkg_name in members.items():
        with open(os.path.join(REPO_ROOT, member_dir, "Cargo.toml"), "rb") as f:
            doc = tomllib.load(f)
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            table = doc.get(section, {})
            if not isinstance(table, dict):
                continue
            for dep_name, spec in table.items():
                target: str | None = None
                if isinstance(spec, dict):
                    if "path" not in spec and "workspace" not in spec:
                        continue
                    if "path" in spec and "workspace" not in spec:
                        # 纯 path 依赖：归一化后回查 member。
                        p = os.path.normpath(os.path.join(member_dir, spec["path"]))
                        target = norm_to_name.get(_norm(p))
                    else:
                        # workspace = true（含 workspace.path 双写）：
                        # dep 名即 package 名。
                        target = dep_name
                # 字符串 spec 无 path（如 serde = "1"），跳过。
                if target is None:
                    continue
                if target in deps:
                    deps[pkg_name].add(target)
    # 反转：dep → dependents（dep 名按 TOML key；workspace members 的
    # package 名与 key 一致时直接命中——path 依赖的 key 即 package 名）。
    rdeps: dict[str, set[str]] = {name: set() for name in members.values()}
    for pkg, ds in deps.items():
        for d in ds:
            if d in rdeps:
                rdeps[d].add(pkg)
            # 非 workspace 外部依赖（如 serde/tokio）不在图中，跳过。
    _RDEPS_CACHE = rdeps
    return rdeps


def rdeps_closure(pkgs: set[str]) -> set[str]:
    """pkgs 的反向依赖传递闭包（含自身）。"""
    graph = reverse_deps()
    seen = set(pkgs)
    stack = list(pkgs)
    while stack:
        cur = stack.pop()
        for dep in graph.get(cur, ()):
            if dep not in seen:
                seen.add(dep)
                stack.append(dep)
    return seen


def needs_contract_bins(pkgs: set[str], packages: dict[str, str]) -> bool:
    """canonical 是否需要 contract runtime bins。

    rdeps(pkgs) 含 mesa-contract-tests 即 true——filter 字符串只是
    rdeps 的渲染，不作为判断依据（filter 写法变了也不漏）。
    """
    _ = packages  # 签名保留 packages 以便未来扩展；当前用 workspace graph。
    return "mesa-contract-tests" in rdeps_closure(pkgs)


def is_docs_only(path: str) -> bool:
    pl = path.lower()
    return path == ".gitignore" or path == "docs/" or path.startswith("docs/") or pl.endswith(".md")


def full_plan(reason: str) -> dict:
    return {
        "plan_version": PLAN_VERSION,
        "quality": True,
        "canonical": True,
        "canonical_filter": "all()",
        "canonical_soak": "run",
        "canonical_contract_bins": True,
        "platform_mode": "full",
        "platform_soak": "run",
        "build_targets": {"kind": "bins"},
        "stress": True,
        "perf": True,
        "web": True,
        # fail-closed：影响面无法判断时宁可多跑一次 watchdog 离线套件。
        "watchdog": True,
        # 同理：E2E 也是 fail-closed 置真（未知路径/空 diff 都可能悄悄改坏这条链路）。
        "e2e": True,
        "reason": reason,
    }


def empty_plan(reason: str) -> dict:
    """docs-only：全部 skip（CI-1 语义延续）。"""
    return {
        "plan_version": PLAN_VERSION,
        "quality": False,
        "canonical": False,
        "canonical_filter": "none()",
        "canonical_soak": "skip",
        "canonical_contract_bins": False,
        "platform_mode": "skip",
        "platform_soak": "skip",
        "build_targets": {"kind": "none"},
        "stress": False,
        "perf": False,
        "web": False,
        "watchdog": False,
        "e2e": False,
        "reason": reason,
    }


def plan_for_files(files: list[str], packages: dict[str, str], is_main: bool = False) -> dict:
    """核心路由（纯函数，可单测；is_main=true 时 Rust/code change 升级全集成）。"""
    if not files:
        return full_plan("empty diff → FULL (fail-closed)")
    if all(is_docs_only(p) for p in files):
        return empty_plan("docs-only → all skip")

    # web-only：只有 apps/mesa-web 下文件。
    if all(p == "apps/mesa-web/" or p.startswith("apps/mesa-web/") for p in files):
        return {
            "plan_version": PLAN_VERSION,
            "quality": False,
            "canonical": False,
            "canonical_filter": "none()",
            "canonical_soak": "skip",
            "canonical_contract_bins": False,
            "platform_mode": "skip",
            "platform_soak": "skip",
            "build_targets": {"kind": "none"},
            "stress": False,
            "perf": False,
            "web": True,
            "watchdog": False,
            "e2e": False,
            "reason": "web-only → web only",
        }

    # 强制 FULL 路径。
    for p in files:
        for fp in FULL_PATHS:
            if p == fp.rstrip("/") or p.startswith(fp):
                return full_plan(f"full-ci path: {p}")

    # 路径 → packages。
    watchdog_changed = needs_watchdog(files)
    e2e_changed = needs_e2e(files)
    pkgs: set[str] = set()
    for p in files:
        matched = False
        # watchdog / E2E 相关路径里的**已知非 Rust 路径**：只置对应标志，不牵连 Rust job。
        # （不加这一条的话它们会落到下面的 unknown path → FULL，等于让每个测试资产小改动
        #  都跑一遍全量 Rust 集成，与"不要无条件消耗 runner"的要求相反。）
        if needs_watchdog([p]) or needs_e2e_only([p]):
            matched = True
        for prefix, extra in SPECIAL_PACKAGES.items():
            if p == prefix or p.startswith(prefix + "/"):
                pkgs.update(extra)
                matched = True
        for member, name in packages.items():
            if p == member or p.startswith(member + "/"):
                # member 内 .md 视为文档（不触发该 package）。
                if p.lower().endswith(".md"):
                    continue
                pkgs.add(name)
                matched = True
        if not matched and not is_docs_only(p):
            return full_plan(f"unknown path: {p} (fail-closed)")

    if not pkgs:
        if watchdog_changed or e2e_changed:
            # 只有测试资产变更：只跑对应的重套件，Rust/web 全部 skip。
            parts = []
            if watchdog_changed:
                parts.append("watchdog")
            if e2e_changed:
                parts.append("e2e")
            return {
                "plan_version": PLAN_VERSION,
                "quality": False,
                "canonical": False,
                "canonical_filter": "none()",
                "canonical_soak": "skip",
                "canonical_contract_bins": False,
                "platform_mode": "skip",
                "platform_soak": "skip",
                "build_targets": {"kind": "none"},
                "stress": False,
                "perf": False,
                "web": False,
                "watchdog": watchdog_changed,
                "e2e": e2e_changed,
                "reason": "%s-only → %s suite only"
                % ("+".join(parts), "+".join(parts)),
            }
        # 全是 member 内 .md。
        return empty_plan("member docs-only → all skip")

    # canonical filter：deterministic 联合 rdeps。
    filt = " | ".join(f"rdeps(={p})" for p in sorted(pkgs))
    reasons = [f"rust package change: {p}" for p in sorted(pkgs)]

    # contract suite 自身修改 → stress。
    stress = bool(pkgs & STRESS_PACKAGES)
    for p in files:
        base = os.path.basename(p)
        if base.endswith(".rs"):
            stem = base[:-3]
            if stem in STRESS_SUITES:
                stress = True
                reasons.append(f"runtime-sensitive suite: {stem}")
                break

    perf = bool(pkgs & PERF_PACKAGES)

    # selective build：受影响 packages + 其 rdeps 需要的 bins。
    # driver-only（如 focas2）只构建该驱动与其测试，不全 workspace bins。
    # contract-tests/mesad 受影响时才需要全 bins（helper + mesad + test-driver）。
    if pkgs & {"mesa-contract-tests", "mesad"}:
        build_targets: dict = {"kind": "bins"}
    else:
        build_targets = {"kind": "packages", "packages": sorted(pkgs)}

    # contract runtime bins：canonical filter 的 rdeps 若含 mesa-contract-tests，
    # 则 canonical 必须先补 mesad + test-driver + contract --bins。
    # 不解析 filter 字符串：直接用 workspace dependency graph 求 rdeps 闭包。
    contract_bins = needs_contract_bins(pkgs, packages)

    # web：只有 apps/mesa-web 受影响才跑（Rust-only 不带 web）。
    web = any(
        p == "apps/mesa-web/" or p.startswith("apps/mesa-web/") for p in files
    )

    plan = {
        "plan_version": PLAN_VERSION,
        "quality": True,
        "canonical": True,
        "canonical_filter": filt,
        "canonical_soak": "skip",
        "canonical_contract_bins": contract_bins,
        "platform_mode": "smoke",
        "platform_soak": "skip",
        "build_targets": build_targets,
        "stress": stress,
        "perf": perf,
        "web": web,
        "watchdog": watchdog_changed,
        "e2e": e2e_changed,
        "reason": "; ".join(reasons),
    }
    if is_main and any(
        not is_docs_only(p) and not (p == "apps/mesa-web/" or p.startswith("apps/mesa-web/"))
        for p in files
    ):
        # main Rust/code change → 选择性集成（CI v3：不再无脑 FULL）。
        # main 跑 planner 同一计划（selective），FULL 只放 nightly/manual。
        pass
    return plan


def changed_files(base: str, head: str) -> list[str] | None:
    import subprocess

    try:
        out = subprocess.run(
            ["git", "diff", "--name-only", f"{base}...{head}"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.splitlines()
        return [line for line in out if line.strip()]
    except subprocess.CalledProcessError:
        return None


def main() -> int:
    base = os.environ.get("CI_BASE", "")
    head = os.environ.get("CI_HEAD", "")
    is_main = os.environ.get("CI_IS_MAIN", "") == "1"
    if not base or not head:
        import json

        print(json.dumps(full_plan("missing base/head → FULL (fail-closed)")))
        return 0
    import json

    files = changed_files(base, head)
    if files is None:
        print(json.dumps(full_plan("diff failed → FULL (fail-closed)")))
        return 0
    print(json.dumps(plan_for_files(files, workspace_packages(), is_main)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
