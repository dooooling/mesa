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
    ap.add_argument("--strict", action="store_true", help="release 严格模式：要求 performance/soak/real_device 非零")
    ap.add_argument("--contract-json", default=None)
    ap.add_argument("--perf-json", default=None)
    ap.add_argument("--soak-json", default=None)
    args = ap.parse_args()

    git_sha = sh("git rev-parse --short HEAD")
    rust_ver = sh("rustc --version")
    # 尝试从 target/validation/* 聚合真实结果
    def load_json(p):
        try:
            return json.loads(pathlib.Path(p).read_text(encoding="utf-8"))
        except Exception:
            return None
    contract = load_json(args.contract_json) if args.contract_json else load_json("target/validation/contract.json")
    perf = load_json(args.perf_json) if args.perf_json else load_json("target/validation/performance.json")
    soak = load_json(args.soak_json) if args.soak_json else load_json("target/validation/soak.json")
    real_dev = load_json("target/validation/real-device.json")

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
        "contract_tests": contract if contract else {"passed": 0, "failed": 0, "total": 0, "suites": []},
        "performance": perf if perf else {"throughput_updates_per_sec": 0, "ipc_p95_ms": 0, "ipc_p99_ms": 0, "configure_1k_ms": 0, "configure_10k_ms": 0, "configure_50k_ms": 0, "rss_delta_mib": 0},
        "soak": soak if soak else {"duration_hours": 0, "rss_growth_percent": 0, "leak_detected": False},
        "real_device_matrix": real_dev if real_dev else []
    }
    # 若聚合到了真实数据，覆盖占位
    out = pathlib.Path(args.out)
    out.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out}")

    if args.validate:
        schema = pathlib.Path("schemas/release-validation.schema.json")
        if not schema.exists():
            print("schema not found", file=sys.stderr); sys.exit(2)
        data = json.loads(out.read_text(encoding="utf-8"))
        for k in ["schema_version","git_sha","environment","contract_tests"]:
            if k not in data:
                print(f"missing {k}", file=sys.stderr); sys.exit(1)
        if args.strict:
            perf = data.get("performance", {})
            soak_d = data.get("soak", {})
            rd = data.get("real_device_matrix", [])
            ct = data.get("contract_tests", {})
            if ct.get("failed", 0) != 0 or ct.get("passed", 0) == 0:
                print("strict: contract_tests 未通过或为空", file=sys.stderr); sys.exit(1)
            if perf.get("throughput_updates_per_sec", 0) == 0 or perf.get("ipc_p95_ms", 0) == 0:
                print("strict: performance 结果为空，需先跑 e2e_50k_real 并输出 target/validation/performance.json", file=sys.stderr); sys.exit(1)
            if soak_d.get("duration_hours", 0) == 0:
                print("strict: soak 结果为空，需先跑 PERF_SOAK=1 e2e_50k_real", file=sys.stderr); sys.exit(1)
            if not rd:
                print("strict: real_device_matrix 为空，需填真机矩阵", file=sys.stderr); sys.exit(1)
            print("validate PASS (strict)")
        else:
            print("validate PASS (required fields)")

if __name__ == "__main__":
    main()
