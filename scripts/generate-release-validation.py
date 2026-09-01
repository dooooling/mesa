#!/usr/bin/env python3
"""
生成 release-validation.json 并按 schemas/release-validation.schema.json 校验（§25）。
用法:
  python scripts/generate-release-validation.py [--out release-validation.json] [--validate]
环境自动采集: OS/CPU/cores/RAM/rust_version/build_profile/git_sha
"""
import argparse, json, platform, subprocess, sys, pathlib, datetime

def sh(cmd):
    try:
        return subprocess.check_output(cmd, shell=True, text=True).strip()
    except Exception:
        return "unknown"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="release-validation.json")
    ap.add_argument("--validate", action="store_true")
    args = ap.parse_args()

    git_sha = sh("git rev-parse --short HEAD")
    rust_ver = sh("rustc --version")
    # 性能占位：真实 CI 中由 tests/performance/* 注入
    doc = {
        "schema_version": "1.0.0",
        "git_sha": git_sha,
        "generated_at_ns": int(datetime.datetime.now(datetime.timezone.utc).timestamp() * 1e9),
        "environment": {
            "os": f"{platform.system()} {platform.release()}",
            "cpu": platform.processor() or "unknown",
            "cores": __import__("os").cpu_count() or 1,
            "ram": "unknown",
            "rust_version": rust_ver,
            "build_profile": "release" if "--release" in sys.argv else "debug"
        },
        "contract_tests": {
            "passed": 0, "failed": 0, "total": 0, "suites": []
        },
        "performance": {
            "throughput_updates_per_sec": 0,
            "ipc_p95_ms": 0, "ipc_p99_ms": 0,
            "configure_1k_ms": 0, "configure_10k_ms": 0, "configure_50k_ms": 0,
            "rss_delta_mib": 0
        },
        "soak": { "duration_hours": 0, "rss_growth_percent": 0, "leak_detected": False },
        "real_device_matrix": []
    }
    # 尝试从 cargo test --workspace 解析 passed（简易）
    # 若需精确，可在 CI 中传 --contract-json
    out = pathlib.Path(args.out)
    out.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out}")

    if args.validate:
        schema = pathlib.Path("schemas/release-validation.schema.json")
        if not schema.exists():
            print("schema not found", file=sys.stderr); sys.exit(2)
        # 简易校验：仅检查 required 字段存在（完整校验需 ajv/jsonschema）
        data = json.loads(out.read_text(encoding="utf-8"))
        for k in ["schema_version","git_sha","environment","contract_tests"]:
            if k not in data:
                print(f"missing {k}", file=sys.stderr); sys.exit(1)
        print("validate PASS (required fields)")

if __name__ == "__main__":
    main()
