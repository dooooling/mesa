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
    # plan_version 冻结（CI v3 → 3）。
    check("plan_version==3", plan.PLAN_VERSION == 3)

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

    # focas2 source → rdeps(focas2), smoke, no stress/perf, web true。
    p = plan_for_files(["drivers/focas2/src/wire/codec.rs"], pkgs())
    check("focas2 filter", p["canonical_filter"] == "rdeps(=mesa-driver-focas2)", p)
    check("focas2 smoke", p["platform_mode"] == "smoke", p)
    check("focas2 no stress", p["stress"] is False, p)
    check("focas2 no perf", p["perf"] is False, p)
    check("focas2 web", p["web"] is True, p)

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

    # focas2 selective build：只构建受影响包（CI v3 第一刀）。
    p = plan_for_files(["drivers/focas2/src/x.rs"], pkgs())
    check("focas2 selective build", p["build_targets"]
          == {"kind": "packages", "packages": ["mesa-driver-focas2"]}, p)

    # driver-manager 不牵连 contract-tests/mesad → selective packages。
    p = plan_for_files(["crates/driver-manager/src/x.rs"], pkgs())
    check("manager selective build", p["build_targets"]
          == {"kind": "packages", "packages": ["mesa-driver-manager"]}, p)

    # contract-tests 受影响 → 全 bins（helper/mesad/sim 需要）。
    p = plan_for_files(["tests/driver-contract/tests/smoke.rs"], pkgs())
    check("contract bins build", p["build_targets"] == {"kind": "bins"}, p)

    # soak 默认归 stress（CI v3 第三刀；canonical/platform 默认 skip）。
    p = plan_for_files(["drivers/focas2/src/x.rs"], pkgs())
    check("soak skip by default", p["canonical_soak"] == "skip"
          and p["platform_soak"] == "skip", p)

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

    print(f"\n{ PASS} passed, {FAIL} failed")
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
