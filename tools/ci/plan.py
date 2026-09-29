#!/usr/bin/env python3
"""Mesa CI v2 planner：变更路径 → 执行计划（CI 唯一调度真值）。

输入：变更文件列表（git diff --name-only），输出 JSON 计划：
{
  "plan_version": 2,
  "quality": true,            # rust-quality（fmt/clippy）
  "canonical": true,          # rust-canonical（Linux x64 完整语义）
  "canonical_filter": ...,    # nextest filterset：all() 或 rdeps(=pkg) 联合
  "platform_mode": "smoke",   # skip / smoke / full
  "stress": false,            # rust-stress
  "perf": false,              # rust-perf
  "web": true,                # web build/test
  "reason": "...",            # 人类可读路由原因
}

fail-closed：任何无法识别的非文档路径 → FULL；空 diff → FULL；
diff 失败 → FULL。
"""
from __future__ import annotations

import os
import sys
import tomllib

PLAN_VERSION = 2

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
STRESS_PACKAGES = {
    "mesa-core-types",
    "mesa-driver-protocol",
    "mesa-driver-sdk",
    "mesa-driver-manager",
    "mesa-config-store",
    "mesa-event-store",
    "mesa-core-api",
    "mesad",
    "mesa-driver-simulator",
}

# performance-sensitive packages（改动触发 perf）。
PERF_PACKAGES = {
    "mesa-core-types",
    "mesa-driver-protocol",
    "mesa-driver-sdk",
    "mesa-driver-manager",
    "mesa-config-store",
    "mesa-core-api",
    "mesa-driver-simulator",
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


def is_docs_only(path: str) -> bool:
    pl = path.lower()
    return path == ".gitignore" or path == "docs/" or path.startswith("docs/") or pl.endswith(".md")


def full_plan(reason: str) -> dict:
    return {
        "plan_version": PLAN_VERSION,
        "quality": True,
        "canonical": True,
        "canonical_filter": "all()",
        "platform_mode": "full",
        "stress": True,
        "perf": True,
        "web": True,
        "reason": reason,
    }


def empty_plan(reason: str) -> dict:
    """docs-only：全部 skip（CI-1 语义延续）。"""
    return {
        "plan_version": PLAN_VERSION,
        "quality": False,
        "canonical": False,
        "canonical_filter": "none()",
        "platform_mode": "skip",
        "stress": False,
        "perf": False,
        "web": False,
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
            "platform_mode": "skip",
            "stress": False,
            "perf": False,
            "web": True,
            "reason": "web-only → web only",
        }

    # 强制 FULL 路径。
    for p in files:
        for fp in FULL_PATHS:
            if p == fp.rstrip("/") or p.startswith(fp):
                return full_plan(f"full-ci path: {p}")

    # 路径 → packages。
    pkgs: set[str] = set()
    for p in files:
        matched = False
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

    plan = {
        "plan_version": PLAN_VERSION,
        "quality": True,
        "canonical": True,
        "canonical_filter": filt,
        "platform_mode": "smoke",
        "stress": stress,
        "perf": perf,
        "web": True,
        "reason": "; ".join(reasons),
    }
    if is_main and any(
        not is_docs_only(p) and not (p == "apps/mesa-web/" or p.startswith("apps/mesa-web/"))
        for p in files
    ):
        # main Rust/code change → 全集成安全网。
        if plan["canonical"] or plan["platform_mode"] != "skip" or stress or perf:
            plan["canonical_filter"] = "all()"
            plan["platform_mode"] = "full"
            plan["stress"] = True
            plan["perf"] = True
            plan["web"] = True
            plan["reason"] += "; main push → FULL integration"
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
