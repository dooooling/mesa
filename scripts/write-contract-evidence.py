#!/usr/bin/env python3
"""生成 target/validation/contract.json 的唯一权威脚本（Suite Gate）。

机器保证：本脚本先删除旧 contract.json，再依次执行
  1) cargo build --locked --workspace  （重编全部 Driver binaries，避免旧二进制）
  2) 名单与磁盘双向对拍（contract_suites.check_manifest；幽灵/遗漏直接失败）
  3) cargo test --locked -p mesa-contract-tests --all-features（逐 suite 运行，
     真实 pass/fail 逐项解析进 contract.json；任一失败不写证据）
外部唯一入口即 `python scripts/write-contract-evidence.py`，无法通过参数跳过测试。
套件名单唯一来源 scripts/contract_suites.py。
"""
import json, pathlib, re, subprocess, sys, datetime

from contract_suites import SUITES, SUITE_SET, check_manifest

# 套件名单见 contract_suites.py（唯一来源）。

def git_sha():
    try:
        return subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        return "unknown"

def is_clean_tree():
    try:
        out = subprocess.check_output(["git", "status", "--porcelain"], text=True)
        return out.strip() == ""
    except Exception:
        return False

def run_or_exit(cmd, label):
    print(f"running: {' '.join(cmd)} ...", flush=True)
    try:
        subprocess.run(cmd, check=True)
    except subprocess.CalledProcessError as e:
        print(f"{label} FAILED (exit {e.returncode}), not writing contract.json", file=sys.stderr)
        sys.exit(e.returncode)
    except FileNotFoundError as e:
        print(f"cargo not found: {e}", file=sys.stderr)
        sys.exit(127)

def run_capture(cmd, label):
    print(f"running: {' '.join(cmd)} ...", flush=True)
    try:
        r = subprocess.run(cmd, check=True, capture_output=True, text=True)
        return r.stdout + r.stderr
    except subprocess.CalledProcessError as e:
        print(f"{label} FAILED (exit {e.returncode}), not writing contract.json", file=sys.stderr)
        print((e.stdout or "") + (e.stderr or ""), file=sys.stderr)
        sys.exit(e.returncode)
    except FileNotFoundError as e:
        print(f"cargo not found: {e}", file=sys.stderr)
        sys.exit(127)


def parse_suite_results(output):
    """解析 `cargo test --test <suite>` 输出：(passed_tests, failed_tests)。

    失败行形如 `test foo ... FAILED`；汇总行形如
    `test result: ok. 12 passed; 0 failed; ...`。
    """
    failed_names = re.findall(r"^test (\S+) \.\.\. FAILED", output, re.M)
    m = re.search(r"test result:\s+(ok|FAILED)\.\s+(\d+) passed;\s+(\d+) failed", output)
    if not m:
        return None
    return int(m.group(2)), int(m.group(3)), failed_names


def main():
    out = pathlib.Path("target/validation/contract.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    # 先删旧 Evidence，避免失败后遗留
    try:
        out.unlink()
    except FileNotFoundError:
        pass

    run_or_exit(["cargo", "build", "--locked", "--workspace"], "cargo build")

    # 名单与磁盘双向对拍（幽灵/遗漏直接失败，不写证据）
    ok, ghost, missing = check_manifest()
    if not ok:
        print(f"suite manifest mismatch: ghost={ghost} missing={missing}, not writing contract.json",
              file=sys.stderr)
        sys.exit(3)

    # 逐 suite 运行并解析真实结果（任一失败即退出，不写证据）
    suite_results = {}
    total_passed = 0
    total_failed = 0
    for suite in SUITES:
        output = run_capture(
            ["cargo", "test", "--locked", "-p", "mesa-contract-tests", "--all-features",
             "--test", suite],
            f"contract suite {suite}",
        )
        parsed = parse_suite_results(output)
        if parsed is None:
            print(f"suite {suite}: cannot parse test output, not writing contract.json", file=sys.stderr)
            sys.exit(4)
        passed, failed, failed_names = parsed
        suite_results[suite] = {"passed": passed, "failed": failed, "failed_tests": failed_names}
        total_passed += passed
        total_failed += failed

    if total_failed != 0:
        print(f"contract suites failed: {total_failed} tests failed, not writing contract.json", file=sys.stderr)
        sys.exit(5)

    sha = git_sha()
    doc = {
        "passed": len(SUITES),
        "failed": 0,
        "total": len(SUITES),
        "suites": SUITES,
        "suite_results": suite_results,
        "test_counts": {"passed_tests": total_passed, "failed_tests": total_failed},
        "git_sha": sha,
        "git_sha_short": sha[:7] if len(sha) >= 7 else sha,
        "generated_at_ns": int(datetime.datetime.now(datetime.timezone.utc).timestamp() * 1e9),
        "dirty": not is_clean_tree(),
    }
    out.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out} suites={len(SUITES)} tests={total_passed} sha={sha} dirty={doc['dirty']}")

if __name__ == "__main__":
    main()
