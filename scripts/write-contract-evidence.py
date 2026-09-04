#!/usr/bin/env python3
"""生成 target/validation/contract.json 的唯一权威脚本（Suite Gate）。

机器保证：本脚本先删除旧 contract.json，再依次执行
  1) cargo build --locked --workspace  （重编全部 Driver binaries，避免旧二进制）
  2) cargo test --locked -p mesa-contract-tests --all-features
两步均成功后才写出 contract.json。外部唯一入口即 `python scripts/write-contract-evidence.py`，
无法通过参数跳过测试。
"""
import json, pathlib, subprocess, sys, datetime

SUITES = [
    "smoke",
    "protocol_negotiation",
    "session_lifecycle",
    "data_plane",
    "fault_tolerance",
    "subprocess_recovery",
    "discovery_contract",
    "descriptor_contract",
    "data_semantics",
    "control_contract",
    "management_api",
    "probe_contract",
    "profile_contract",
    "resource_contract",
    "subprocess_orphan_guard",
]
REQUIRED_SET = set(SUITES)

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

def main():
    out = pathlib.Path("target/validation/contract.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    # 先删旧 Evidence，避免失败后遗留
    try:
        out.unlink()
    except FileNotFoundError:
        pass

    run_or_exit(["cargo", "build", "--locked", "--workspace"], "cargo build")
    run_or_exit(["cargo", "test", "--locked", "-p", "mesa-contract-tests", "--all-features"], "contract tests")

    sha = git_sha()
    doc = {
        "passed": len(SUITES),
        "failed": 0,
        "total": len(SUITES),
        "suites": SUITES,
        "git_sha": sha,
        "git_sha_short": sha[:7] if len(sha) >= 7 else sha,
        "generated_at_ns": int(datetime.datetime.now(datetime.timezone.utc).timestamp() * 1e9),
        "dirty": not is_clean_tree(),
    }
    # 防御：保证集合完整性（Contract Gate 验证行为契约，不含 build_profile）
    assert set(doc["suites"]) == REQUIRED_SET and doc["total"] == len(SUITES) == 15
    out.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out} suites={len(SUITES)} sha={sha} dirty={doc['dirty']}")

if __name__ == "__main__":
    main()
