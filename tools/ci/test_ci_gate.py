#!/usr/bin/env python3
"""ci-gate 严格性自测：直接执行 `.github/workflows/ci.yml` 里那段 gate python。

为什么要有这个测试
------------------
`ci-gate` 是唯一 Merge Gate（Branch Protection 只绑它）。它"看起来"在校验 job 结果，
但如果哪天有人把判定改成 `if want_run and r not in ('success','skipped')`（把"该跑却跳过"
也算通过），CI 就会在"应该跑却没跑"时放行——这类回归不会让任何 Rust 测试变红，
只能靠**对 gate 本身**的测试拦住。

判定语义（本测试锁死）：
  * planner 说该跑（flag=true）⇒ 该 job 必须 `success`；
  * planner 说不用跑（flag=false）⇒ 该 job 必须 `skipped`（"跳过了"不算失败）；
  * plan 里该 flag 不是 bool（缺失/类型错）⇒ FAIL CLOSED；
  * plan_version 不是当前版本 ⇒ FAIL CLOSED。

运行：python3 tools/ci/test_ci_gate.py（无第三方依赖；CI quality job 内执行）。
"""
import io
import json
import os
import re
import sys
from contextlib import redirect_stdout

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CI_YML = os.path.join(REPO_ROOT, ".github", "workflows", "ci.yml")
EXPECTED_PLAN_VERSION = 4

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


def load_gate_code():
    """从 ci.yml 抽出 `python3 - <<'EOF' ... EOF` 里的 gate 代码（去掉 YAML 块标量缩进）。"""
    src = open(CI_YML, encoding="utf-8").read()
    m = re.search(r"python3 - <<'EOF'\n(.*?)\n\s*EOF", src, re.S)
    if not m:
        raise SystemExit("ci.yml 里找不到 ci-gate 的 heredoc python 代码")
    lines = m.group(1).splitlines()
    indent = len(lines[0]) - len(lines[0].lstrip())
    return "\n".join(ln[indent:] if ln.startswith(" " * indent) else ln.lstrip() for ln in lines)


GATE = load_gate_code()


def base_plan(watchdog):
    plan = {
        "plan_version": EXPECTED_PLAN_VERSION,
        "quality": True,
        "canonical": True,
        "canonical_filter": "all()",
        "canonical_soak": "skip",
        "canonical_contract_bins": True,
        "platform_mode": "smoke",
        "platform_soak": "skip",
        "build_targets": {"kind": "bins"},
        "stress": False,
        "perf": False,
        "web": False,
    }
    if watchdog != "absent":
        plan["watchdog"] = watchdog
    return plan


def needs_json(plan, watchdog_result):
    return {
        "plan": {"result": "success", "outputs": {"plan": json.dumps(plan)}},
        "rust-quality": {"result": "success"},
        "rust-canonical": {"result": "success"},
        "rust-platform": {"result": "success"},
        "web": {"result": "skipped"},
        "rust-stress": {"result": "skipped"},
        "rust-perf": {"result": "skipped"},
        "watchdog": {"result": watchdog_result},
    }


def run_gate(plan, watchdog_result):
    os.environ["NEEDS_JSON"] = json.dumps(needs_json(plan, watchdog_result))
    buf = io.StringIO()
    with redirect_stdout(buf):
        try:
            exec(compile(GATE, "ci-gate", "exec"), {"__name__": "__main__"})
            return 0, buf.getvalue()
        except SystemExit as e:
            return (e.code or 0), buf.getvalue()


def main():
    # watchdog：该跑必须 success，不该跑必须 skipped，flag 缺失必须 FAIL CLOSED。
    for tag, wd, result, want in [
        ("watchdog=true + success → OK", True, "success", 0),
        ("watchdog=true + skipped → FAIL", True, "skipped", 1),
        ("watchdog=true + failure → FAIL", True, "failure", 1),
        ("watchdog=false + skipped → OK", False, "skipped", 0),
        ("watchdog=false + success → FAIL（接线错误）", False, "success", 1),
        ("plan 缺 watchdog → FAIL CLOSED", "absent", "skipped", 1),
    ]:
        got, out = run_gate(base_plan(wd), result)
        check(tag, got == want, f"exit={got} want={want}\n{out}")

    # 既有 flag 同样严格（防止改 gate 时顺手放宽别的 job）。
    for key, job in [("quality", "rust-quality"), ("canonical", "rust-canonical"),
                     ("web", "web"), ("stress", "rust-stress"), ("perf", "rust-perf")]:
        plan = base_plan(False)
        plan[key] = True
        needs = needs_json(plan, "skipped")
        needs[job] = {"result": "skipped"}
        os.environ["NEEDS_JSON"] = json.dumps(needs)
        buf = io.StringIO()
        with redirect_stdout(buf):
            try:
                exec(compile(GATE, "ci-gate", "exec"), {"__name__": "__main__"})
                got = 0
            except SystemExit as e:
                got = (e.code or 0)
        check(f"{key}=true + {job} skipped → FAIL", got == 1, buf.getvalue())

    # plan_version 必须等于当前版本。
    plan = base_plan(False)
    plan["plan_version"] = EXPECTED_PLAN_VERSION - 1
    got, out = run_gate(plan, "skipped")
    check("plan_version 旧版本 → FAIL", got == 1, out)

    # ci.yml 的 job 列表确实包含 watchdog（防止 gate 校验了、job 却没定义）。
    src = open(CI_YML, encoding="utf-8").read()
    check("ci.yml 定义了 watchdog job", re.search(r"^  watchdog:\n", src, re.M) is not None)
    check("ci-gate needs 里包含 watchdog",
          re.search(r"needs: \[plan, rust-quality, rust-canonical, rust-platform, web, "
                    r"rust-stress, rust-perf, watchdog\]", src) is not None)
    check("watchdog job 不是无条件运行",
          re.search(r"if: needs\.plan\.outputs\.watchdog == 'true'", src) is not None)

    print(f"\n{PASS} passed, {FAIL} failed")
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
