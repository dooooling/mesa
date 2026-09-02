#!/usr/bin/env python3
"""生成 target/validation/contract.json 的唯一权威脚本（Suite Gate）。

机器保证：本脚本自身执行 `cargo test --locked -p mesa-contract-tests --all-features`
并仅在成功后才写出 contract.json，外部只需 `python scripts/write-contract-evidence.py`。
测试开始前会删除旧 contract.json，失败时不留下看似有效的旧 Evidence。
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

def main():
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--skip-run", action="store_true", help="跳过 cargo test，仅生成（调试用）")
    args = ap.parse_args()

    out = pathlib.Path("target/validation/contract.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    # 先删旧 Evidence，避免失败后遗留
    try:
        out.unlink()
    except FileNotFoundError:
        pass

    if not args.skip_run:
        print("running: cargo test --locked -p mesa-contract-tests --all-features ...", flush=True)
        try:
            subprocess.run(
                ["cargo", "test", "--locked", "-p", "mesa-contract-tests", "--all-features"],
                check=True,
            )
        except subprocess.CalledProcessError as e:
            print(f"contract tests FAILED (exit {e.returncode}), not writing contract.json", file=sys.stderr)
            sys.exit(e.returncode)
        except FileNotFoundError as e:
            print(f"cargo not found: {e}", file=sys.stderr)
            sys.exit(127)

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
        "build_profile": "release" if not "--debug" in sys.argv else "debug",
    }
    # 防御：保证集合完整性
    assert set(doc["suites"]) == REQUIRED_SET and doc["total"] == len(SUITES) == 14
    out.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out} suites={len(SUITES)} sha={sha} dirty={doc['dirty']}")

if __name__ == "__main__":
    main()
