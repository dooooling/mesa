#!/usr/bin/env python3
"""CI planner 自测：plan.py 路由契约（CI v2 核心安全门）。

运行：python3 tools/ci/test_plan.py（无第三方依赖；CI quality job 内执行）。
失败即 CI FAIL——planner 回归不得合入。
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import plan
from plan import plan_for_files

PASS = 0
FAIL = 0


def check(tag, cond, extra=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"PASS {tag}")
    else:
        FAIL += 1
        print(f"FAIL {tag} {extra}")


def pkgs():
    return plan.workspace_packages()


def main():
    # plan_version 冻结（CI v5 → 5；v4 新增 watchdog 标志，v5 新增 e2e 标志）。
    check("plan_version==5", plan.PLAN_VERSION == 5)

    # docs-only → all skip。
    p = plan_for_files(["docs/a.md", "drivers/focas2/docs/x.md"], pkgs())
    check("docs-only skip", p["quality"] is False and p["canonical"] is False
          and p["platform_mode"] == "skip" and p["stress"] is False
          and p["perf"] is False and p["web"] is False
          and p["build_targets"] == {"kind": "none"}
          and p["canonical_soak"] == "skip" and p["platform_soak"] == "skip", p)

    # .gitignore → all skip。
    p = plan_for_files([".gitignore"], pkgs())
    check("gitignore skip", p["platform_mode"] == "skip" and p["web"] is False, p)

    # web-only → web only。
    p = plan_for_files(["apps/mesa-web/src/App.tsx"], pkgs())
    check("web-only", p["web"] is True and p["canonical"] is False
          and p["platform_mode"] == "skip" and p["stress"] is False, p)

    # focas2 source → rdeps(focas2), smoke, no stress/perf, no web（CI v3）。
    p = plan_for_files(["drivers/focas2/src/wire/codec.rs"], pkgs())
    check("focas2 filter", p["canonical_filter"] == "rdeps(=mesa-driver-focas2)", p)
    check("focas2 smoke", p["platform_mode"] == "smoke", p)
    check("focas2 no stress", p["stress"] is False, p)
    check("focas2 no perf", p["perf"] is False, p)
    check("focas2 web", p["web"] is False, p)

    # driver-manager → canonical+platform+stress+perf。
    p = plan_for_files(["crates/driver-manager/src/x.rs"], pkgs())
    check("manager stress", p["stress"] is True, p)
    check("manager perf", p["perf"] is True, p)

    # event-store → stress。
    p = plan_for_files(["crates/event-store/src/x.rs"], pkgs())
    check("event-store stress", p["stress"] is True, p)

    # performance-only → quality+perf。
    p = plan_for_files(["tests/performance/benches/x.rs"], pkgs())
    check("perf true", p["perf"] is True, p)

    # mesad → canonical 包含 contract tests。
    p = plan_for_files(["apps/mesad/src/main.rs"], pkgs())
    check("mesad contract", "mesa-contract-tests" in p["canonical_filter"], p)

    # tests/fixtures → mesa-contract-tests。
    p = plan_for_files(["tests/fixtures/generic-function.json"], pkgs())
    check("fixtures contract", p["canonical_filter"] == "rdeps(=mesa-contract-tests)", p)

    # Cargo.lock → FULL。
    p = plan_for_files(["Cargo.lock"], pkgs())
    check("lock FULL", p["canonical_filter"] == "all()"
          and p["platform_mode"] == "full" and p["stress"] is True
          and p["perf"] is True and p["web"] is True, p)

    # rust-toolchain.toml → FULL。
    p = plan_for_files(["rust-toolchain.toml"], pkgs())
    check("toolchain FULL", p["platform_mode"] == "full", p)

    # workflow → FULL。
    p = plan_for_files([".github/workflows/ci.yml"], pkgs())
    check("workflow FULL", p["canonical_filter"] == "all()", p)

    # tools/ci → FULL。
    p = plan_for_files(["tools/ci/plan.py"], pkgs())
    check("tools-ci FULL", p["platform_mode"] == "full", p)

    # unknown path → FULL。
    p = plan_for_files(["mystery/new.bin"], pkgs())
    check("unknown FULL", p["canonical_filter"] == "all()", p)

    # empty diff → FULL。
    p = plan_for_files([], pkgs())
    check("empty FULL", p["canonical_filter"] == "all()", p)

    # missing base/head fallback → 单 JSON FULL（B4：双 print bug 回归）。
    import json as _json
    import subprocess as _sp
    r = _sp.run([sys.executable, "-c",
                 "import os,sys; sys.path.insert(0,'tools/ci'); "
                 "os.environ.pop('CI_BASE',None); os.environ.pop('CI_HEAD',None); "
                 "import plan; plan.main()"],
                capture_output=True, text=True)
    try:
        doc = _json.loads(r.stdout.strip())
        check("fallback single JSON FULL",
              doc["canonical_filter"] == "all()" and doc["platform_mode"] == "full", r.stdout[:200])
    except Exception as e:
        check("fallback single JSON FULL", False, f"{e}: {r.stdout[:200]!r}")

    # multi-package → deterministic union。
    p = plan_for_files(
        ["drivers/focas2/src/x.rs", "drivers/s7/src/y.rs"], pkgs())
    check("multi union", p["canonical_filter"]
          == "rdeps(=mesa-driver-focas2) | rdeps(=mesa-driver-s7)", p)

    # main Rust push → 选择性集成（CI v3：不再无脑 FULL；FULL 只放 nightly）。
    p = plan_for_files(["drivers/focas2/src/x.rs"], pkgs(), is_main=True)
    check("main selective", p["canonical_filter"] == "rdeps(=mesa-driver-focas2)"
          and p["platform_mode"] == "smoke" and p["stress"] is False
          and p["perf"] is False, p)

    # FOCAS-only：filter + selective build + contract bins（graph 判定）。
    p = plan_for_files(["drivers/focas2/src/x.rs"], pkgs())
    check("focas2 filter", p["canonical_filter"] == "rdeps(=mesa-driver-focas2)", p)
    check("focas2 selective build", p["build_targets"]
          == {"kind": "packages", "packages": ["mesa-driver-focas2"]}, p)
    check("focas2 contract bins", p["canonical_contract_bins"] is True, p)
    # Rust-only 不带 web（CI v3：web 仅 apps/mesa-web 受影响才跑）。
    check("focas2 no web", p["web"] is False, p)

    # 反例（graph 级）：mesa-nck-emulator 的 rdeps 不含 contract-tests
    # → bins=false（防退化成“所有 canonical 都补 bins”；经 SPECIAL_PACKAGES
    # 映射的 tools/nck-emulator 路径因含 contract-tests 而为 true，不冲突）。
    check("nck-emulator no contract bins",
          plan.needs_contract_bins({"mesa-nck-emulator"}, pkgs()) is False,
          plan.rdeps_closure({"mesa-nck-emulator"}))

    # driver-manager 不牵连 contract-tests/mesad → selective packages。
    p = plan_for_files(["crates/driver-manager/src/x.rs"], pkgs())
    check("manager selective build", p["build_targets"]
          == {"kind": "packages", "packages": ["mesa-driver-manager"]}, p)

    # contract-tests 受影响 → 全 bins（helper + mesad + test-driver）。
    p = plan_for_files(["tests/driver-contract/tests/smoke.rs"], pkgs())
    check("contract bins build", p["build_targets"] == {"kind": "bins"}, p)

    # test-driver 改动 → stress + perf（contract/perf 基线能力）。
    p = plan_for_files(["tests/support/test-driver/src/lib.rs"], pkgs())
    check("test-driver stress", p["stress"] is True, p)
    check("test-driver perf", p["perf"] is True, p)
    check("test-driver contract bins", p["canonical_contract_bins"] is True, p)

    # soak 默认归 stress（CI v3 第三刀；canonical/platform 默认 skip）。
    p = plan_for_files(["drivers/focas2/src/x.rs"], pkgs())
    check("soak skip by default", p["canonical_soak"] == "skip"
          and p["platform_soak"] == "skip", p)

    # v3 gate schema：soak/build_targets 非法即后续 gate 可判（planner 自测锁形状）。
    p = plan_for_files(["drivers/focas2/src/x.rs"], pkgs())
    check("v3 schema kinds", p["build_targets"]["kind"] in ("none", "bins", "packages")
          and p["canonical_soak"] in ("run", "skip")
          and p["platform_soak"] in ("run", "skip"), p)

    # 18/18 workspace member 均 package 可发现（planner 自动认识）。
    check("members>=18", len(pkgs()) >= 18, str(len(pkgs())))

    # root rust-version == toolchain channel（精确 major.minor 比较；B3）。
    import tomllib
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    with open(os.path.join(root, "Cargo.toml"), "rb") as f:
        rv = tomllib.load(f)["workspace"]["package"]["rust-version"]
    with open(os.path.join(root, "rust-toolchain.toml"), encoding="utf-8") as f:
        chan = None
        for line in f:
            line = line.strip()
            if line.startswith("channel"):
                chan = line.split("=")[1].strip().strip('"').strip("'")
    def _mm(v):
        # "1.95" / "1.95.0" → (1, 95)；精确比较，禁 substring。
        parts = v.split(".")
        return (int(parts[0]), int(parts[1]))
    check("toolchain==rust-version", _mm(rv) == _mm(chan), f"{rv} vs {chan}")
    # 回归：旧 substring 检查会对 1.94 误 PASS（B3 blocker 证据）。
    check("toolchain exact (1.94 must fail)", _mm("1.94") != _mm(chan), chan)

    # 18/18 member 均 rust-version.workspace=true。
    import tomllib as tl
    with open(os.path.join(root, "Cargo.toml"), "rb") as f:
        members = tl.load(f)["workspace"]["members"]
    missing = []
    for m in members:
        with open(os.path.join(root, m, "Cargo.toml"), "rb") as f:
            d = tl.load(f)["package"]
        rv = d.get("rust-version")
        if not (isinstance(rv, dict) and rv.get("workspace") is True):
            missing.append(m)
    check("members rust-version.workspace", not missing, str(missing))

    # ── CI v4：watchdog 标志（只按 watchdog 相关路径触发，绝不无条件运行）─────────────
    # schema：每个计划都必须带 bool 类型的 watchdog（gate 会 fail-closed 校验）。
    probe_cases = [
        [],
        ["docs/a.md"],
        ["apps/mesa-web/src/App.tsx"],
        ["drivers/focas2/src/wire/codec.rs"],
        ["scripts/stop-watchdog.ps1"],
        ["tests/watchdog/stub_endpoint.py"],
        [".github/workflows/_watchdog.yml"],
        ["Cargo.lock"],
        ["mystery/new.bin"],
    ]
    shape_bad = [f for f in probe_cases
                 if not isinstance(plan_for_files(f, pkgs()).get("watchdog"), bool)]
    check("watchdog 标志恒为 bool（gate fail-closed 前提）", not shape_bad, str(shape_bad))

    # 被测脚本本体变更 → 只跑 watchdog（不牵连 Rust/web）。
    p = plan_for_files(["scripts/stop-watchdog.ps1"], pkgs())
    check("watchdog: ps1 → true", p["watchdog"] is True, p)
    check("watchdog: ps1 → 不触发 Rust",
          p["quality"] is False and p["canonical"] is False
          and p["platform_mode"] == "skip" and p["stress"] is False and p["perf"] is False,
          p)
    check("watchdog: ps1 → 不触发 web 且不构建", p["web"] is False
          and p["build_targets"] == {"kind": "none"}, p)

    # 测试资产（桩 / 场景驱动）变更 → true。
    for f in ("tests/watchdog/stub_endpoint.py",
              "tests/watchdog/run-watchdog-scenarios.ps1"):
        p = plan_for_files([f], pkgs())
        check(f"watchdog: {f} → true", p["watchdog"] is True and p["canonical"] is False, p)

    # 刻意保留 docs-only 语义：tests/watchdog/README.md 属文档 → 全部 skip（不为改文档烧 runner）。
    # 但"文档 + 代码"混合改动仍然要跑。
    p = plan_for_files(["tests/watchdog/README.md"], pkgs())
    check("watchdog: tests/watchdog/README.md → docs-only skip", p["watchdog"] is False
          and p["canonical"] is False and p["platform_mode"] == "skip", p)
    p = plan_for_files(["tests/watchdog/README.md", "tests/watchdog/stub_endpoint.py"], pkgs())
    check("watchdog: README+stub → true", p["watchdog"] is True, p)

    # watchdog workflow 自身变更 → true（同时命中 .github/ 的 FULL，fail-closed）。
    p = plan_for_files([".github/workflows/_watchdog.yml"], pkgs())
    check("watchdog: _watchdog.yml → true + FULL", p["watchdog"] is True
          and p["platform_mode"] == "full", p)

    # 无关改动 → false（"不要对每个无关 PR 无条件消耗 runner"）。
    for f in ("drivers/focas2/src/wire/codec.rs", "crates/driver-manager/src/x.rs",
              "apps/mesa-web/src/App.tsx", "docs/a.md"):
        p = plan_for_files([f], pkgs())
        check(f"watchdog: {f} → false", p["watchdog"] is False, p)

    # 混合改动（Rust + watchdog）→ 两者都跑。
    p = plan_for_files(["drivers/s7/src/y.rs", "tests/watchdog/stub_endpoint.py"], pkgs())
    check("watchdog: rust+watchdog → 两者都 true", p["watchdog"] is True
          and p["canonical"] is True, p)

    # fail-closed：未知路径 / 空 diff / Cargo.lock → watchdog 也置真。
    for f in ([], ["mystery/new.bin"], ["Cargo.lock"], ["tools/ci/plan.py"]):
        p = plan_for_files(f, pkgs())
        check(f"watchdog: fail-closed {f or '[]'} → true", p["watchdog"] is True, p)

    # ── CI v5：e2e 标志（NCK REST 端到端；只按相关路径触发，绝不无条件运行）───────────
    # schema：每个计划都必须带 bool 类型的 e2e（gate 会 fail-closed 校验）。
    e2e_probe_cases = [
        [],
        ["docs/a.md"],
        ["apps/mesa-web/src/App.tsx"],
        ["drivers/focas2/src/wire/codec.rs"],
        ["scripts/nck-native-rest-e2e.py"],
        ["tools/nck-emulator/src/main.rs"],
        ["drivers/sinumerik-nck/src/lib.rs"],
        ["crates/core-api/src/lib.rs"],
        ["crates/config-store/src/lib.rs"],
        ["apps/mesad/src/main.rs"],
        ["tests/driver-contract/tests/smoke.rs"],
        ["tests/watchdog/stub_endpoint.py"],
        ["scripts/stop-watchdog.ps1"],
        [".github/workflows/_e2e.yml"],
        ["Cargo.lock"],
        ["mystery/new.bin"],
    ]
    shape_bad = [f for f in e2e_probe_cases
                 if not isinstance(plan_for_files(f, pkgs()).get("e2e"), bool)]
    check("e2e 标志恒为 bool（gate fail-closed 前提）", not shape_bad, str(shape_bad))

    # 被测脚本本体变更 → 只跑 e2e（不牵连 Rust/web），且**不**落到 unknown→FULL。
    p = plan_for_files(["scripts/nck-native-rest-e2e.py"], pkgs())
    check("e2e: 脚本 → true", p["e2e"] is True, p)
    check("e2e: 脚本 → 不触发 Rust/web 且不构建",
          p["quality"] is False and p["canonical"] is False
          and p["platform_mode"] == "skip" and p["stress"] is False and p["perf"] is False
          and p["web"] is False and p["build_targets"] == {"kind": "none"}, p)

    # 链路真正依赖的每一处都必须触发（Rust 侧同时照常跑，两者不是二选一）。
    for f in ("tools/nck-emulator/src/main.rs",
              "drivers/sinumerik-nck/src/lib.rs",
              "crates/core-api/src/lib.rs",
              "crates/driver-manager/src/endpoint.rs",
              "crates/config-store/src/lib.rs",
              "apps/mesad/src/main.rs",
              "tests/driver-contract/tests/smoke.rs"):
        p = plan_for_files([f], pkgs())
        check(f"e2e: {f} → true", p["e2e"] is True and p["canonical"] is True, p)

    # e2e workflow 自身变更 → true（同时命中 .github/ 的 FULL，fail-closed）。
    p = plan_for_files([".github/workflows/_e2e.yml"], pkgs())
    check("e2e: _e2e.yml → true + FULL", p["e2e"] is True
          and p["platform_mode"] == "full", p)

    # 无关改动 → false（"不要对每个无关 PR 无条件消耗两个 runner"）。
    for f in ("drivers/focas2/src/wire/codec.rs", "drivers/s7/src/y.rs",
              "crates/event-store/src/x.rs", "apps/mesa-web/src/App.tsx",
              "docs/a.md", "scripts/stop-watchdog.ps1", "tests/watchdog/stub_endpoint.py"):
        p = plan_for_files([f], pkgs())
        check(f"e2e: {f} → false", p["e2e"] is False, p)

    # 混合改动（E2E 资产 + 无关 Rust）→ 两者都跑。
    p = plan_for_files(["scripts/nck-native-rest-e2e.py", "drivers/s7/src/y.rs"], pkgs())
    check("e2e: 脚本+s7 → e2e 与 canonical 都 true",
          p["e2e"] is True and p["canonical"] is True, p)

    # fail-closed：未知路径 / 空 diff / Cargo.lock / tools-ci → e2e 也置真。
    for f in ([], ["mystery/new.bin"], ["Cargo.lock"], ["tools/ci/plan.py"]):
        p = plan_for_files(f, pkgs())
        check(f"e2e: fail-closed {f or '[]'} → true", p["e2e"] is True, p)

    # watchdog 与 e2e 互不牵连（各自只按自己的路径触发）。
    p = plan_for_files(["scripts/stop-watchdog.ps1"], pkgs())
    check("e2e: watchdog-only → e2e false", p["e2e"] is False and p["watchdog"] is True, p)
    p = plan_for_files(["scripts/nck-native-rest-e2e.py"], pkgs())
    check("watchdog: e2e-only → watchdog false",
          p["watchdog"] is False and p["e2e"] is True, p)

    # e2e 相关路径里的文档变更仍按 docs-only 处理（不为改 README 烧 runner）。
    p = plan_for_files(["tests/driver-contract/README.md"], pkgs())
    check("e2e: 文档 → docs-only skip", p["e2e"] is False
          and p["platform_mode"] == "skip", p)

    print(f"\n{ PASS} passed, {FAIL} failed")
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
