#!/usr/bin/env python3
"""
生成 release-validation.json 并按 schemas/release-validation.schema.json 校验（§25）。
用法:
  python scripts/generate-release-validation.py [--out release-validation.json] [--validate]
  严格发布: python scripts/generate-release-validation.py --validate --strict --out target/validation/release-validation.json
环境自动采集: OS/CPU/cores/RAM/rust_version/build_profile/git_sha
聚合来源: target/validation/contract.json performance.json soak.json real-device.json
"""
import argparse, json, platform, subprocess, sys, pathlib, datetime

def sh(cmd):
    try:
        return subprocess.check_output(cmd, shell=True, text=True).strip()
    except Exception:
        return "unknown"

def git_short():
    return sh("git rev-parse --short HEAD")

def git_full():
    return sh("git rev-parse HEAD")

def is_clean_tree():
    try:
        out = subprocess.check_output(["git", "status", "--porcelain"], text=True)
        return out.strip() == ""
    except Exception:
        return False

def sha_matches(evidence_sha, head_full):
    # 正式 Release 要求 40 位完整 SHA 精确相等
    if not evidence_sha or not head_full or head_full == "unknown":
        return False
    return evidence_sha == head_full

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="release-validation.json")
    ap.add_argument("--validate", action="store_true")
    ap.add_argument("--strict", action="store_true", help="release 严格模式：要求 contract/perf/soak/real_device 全量门禁")
    ap.add_argument("--contract-json", default=None)
    ap.add_argument("--perf-json", default=None)
    ap.add_argument("--soak-json", default=None)
    ap.add_argument("--real-device-json", default=None)
    args = ap.parse_args()

    git_sha = git_full()
    git_sha_short = git_short()
    rust_ver = sh("rustc --version")
    clean = is_clean_tree()
    def load_json(p):
        try:
            return json.loads(pathlib.Path(p).read_text(encoding="utf-8"))
        except Exception:
            return None
    contract = load_json(args.contract_json) if args.contract_json else load_json("target/validation/contract.json")
    perf_raw = load_json(args.perf_json) if args.perf_json else load_json("target/validation/performance.json")
    soak_raw = load_json(args.soak_json) if args.soak_json else load_json("target/validation/soak.json")
    real_dev_raw = load_json(args.real_device_json) if args.real_device_json else load_json("target/validation/real-device.json")

    # 兼容 real-device 的两种形态：数组 或 {git_sha, entries/matrix/real_device_matrix}
    def unwrap_real_device(v):
        if v is None:
            return None, None
        if isinstance(v, list):
            return v, None
        if isinstance(v, dict):
            # 尝试提取数组
            for k in ["entries", "matrix", "real_device_matrix", "real_device"]:
                if k in v and isinstance(v[k], list):
                    return v[k], v
            # 若字典本身即为单条？视为无效
            # 若字典带有 git_sha 但顶层即为对象而非数组，返回空
            if "git_sha" in v and "driver" not in v:
                return [], v
        return v if isinstance(v, list) else [], None

    rd_list, rd_wrapper = unwrap_real_device(real_dev_raw)
    # 保留原始 wrapper 以便 strict 校验 git_sha
    real_dev_for_doc = rd_list if rd_list is not None else []

    # build_profile 以 performance.json 的真实值（cfg debug_assertions）为准，避免聚合器猜测
    env_build = "debug"
    if isinstance(perf_raw, dict) and isinstance(perf_raw.get("build_profile"), str):
        env_build = perf_raw.get("build_profile")
    elif isinstance(soak_raw, dict) and isinstance(soak_raw.get("build_profile"), str):
        env_build = soak_raw.get("build_profile")
    # contract/performance/soak 保留原始对象以便校验 git_sha
    doc = {
        "schema_version": "1.0.0",
        "git_sha": git_sha,
        "git_sha_short": git_sha_short,
        "generated_at_ns": int(datetime.datetime.now(datetime.timezone.utc).timestamp() * 1e9),
        "dirty": not clean,
        "environment": {
            "os": f"{platform.system()} {platform.release()}",
            "cpu": platform.processor() or "unknown",
            "cores": __import__("os").cpu_count() or 1,
            "ram": "unknown",
            "rust_version": rust_ver,
            "build_profile": env_build
        },
        "contract_tests": contract if contract else {"passed": 0, "failed": 0, "total": 0, "suites": []},
        "performance": perf_raw if perf_raw else {"throughput_updates_per_sec": 0, "ipc_p95_ms": 0, "ipc_p99_ms": 0, "configure_1k_ms": 0, "configure_10k_ms": 0, "configure_50k_ms": 0, "rss_delta_mib": 0},
        "soak": soak_raw if soak_raw else {"duration_hours": 0, "rss_growth_percent": 0, "leak_detected": False},
        "real_device_matrix": real_dev_for_doc
    }
    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out} git_sha={git_sha}")

    if args.validate:
        schema = pathlib.Path("schemas/release-validation.schema.json")
        if not schema.exists():
            print("schema not found", file=sys.stderr); sys.exit(2)
        data = json.loads(out.read_text(encoding="utf-8"))
        for k in ["schema_version","git_sha","environment","contract_tests"]:
            if k not in data:
                print(f"missing {k}", file=sys.stderr); sys.exit(1)
        if args.strict:
            # 工作树必须干净，否则证据与代码不一致
            if not clean:
                print("strict: 工作树不干净 (git status --porcelain 非空)，请提交后再生成 Release Evidence", file=sys.stderr); sys.exit(1)
            if data.get("dirty"):
                print("strict: release-validation dirty==true", file=sys.stderr); sys.exit(1)
            perf = data.get("performance", {}) if isinstance(data.get("performance"), dict) else {}
            soak_d = data.get("soak", {}) if isinstance(data.get("soak"), dict) else {}
            rd = data.get("real_device_matrix", [])
            ct = data.get("contract_tests", {}) if isinstance(data.get("contract_tests"), dict) else {}
            # --- Contract 硬条件 ---
            if ct.get("failed", 0) != 0 or ct.get("passed", 0) == 0 or ct.get("passed", 0) != ct.get("total", 0):
                print(f"strict: contract_tests 未全通过 {ct}", file=sys.stderr); sys.exit(1)
            suites = ct.get("suites", [])
            if not isinstance(suites, list) or not suites or not all(isinstance(s, str) for s in suites):
                print(f"strict: contract_tests.suites 需为 string[] 当前 {suites}", file=sys.stderr); sys.exit(1)
            required_suites = {"smoke","protocol_negotiation","session_lifecycle","data_plane","fault_tolerance","subprocess_recovery","discovery_contract","descriptor_contract","data_semantics","control_contract","management_api","profile_contract","resource_contract","subprocess_orphan_guard"}
            if set(suites) != required_suites:
                print(f"strict: contract_tests.suites 需为完整 14 suites {sorted(required_suites)} 当前 {sorted(suites)}", file=sys.stderr); sys.exit(1)
            if ct.get("total") != len(suites) or ct.get("total") != 14 or ct.get("passed") != 14:
                print(f"strict: contract_tests total/passed 需 ==14 当前 {ct}", file=sys.stderr); sys.exit(1)
            # --- Evidence 与 commit 绑定（40位全量，兼容短 7 位）---
            def check_sha(name, obj):
                if not isinstance(obj, dict):
                    print(f"strict: {name} 需为 object 且携带 git_sha", file=sys.stderr); sys.exit(1)
                sha = obj.get("git_sha")
                if not sha or not isinstance(sha, str):
                    print(f"strict: {name}.git_sha 缺失，旧结果不能冒充新版本", file=sys.stderr); sys.exit(1)
                if not sha_matches(sha, git_sha):
                    print(f"strict: {name}.git_sha {sha} != HEAD {git_sha}（旧 evidence）", file=sys.stderr); sys.exit(1)
                if obj.get("dirty"):
                    print(f"strict: {name} dirty==true，证据来自脏工作树", file=sys.stderr); sys.exit(1)
            # contract 必须绑定
            check_sha("contract_tests", ct)
            # performance 必须绑定且为 soak 且 duration >=3600 且 release build
            check_sha("performance", perf)
            if perf.get("build_profile") != "release":
                print(f"strict: performance.build_profile 需为 release 当前 {perf.get('build_profile')}", file=sys.stderr); sys.exit(1)
            mode = perf.get("mode")
            if mode != "soak":
                print(f"strict: performance.mode 需为 soak 当前 {mode}", file=sys.stderr); sys.exit(1)
            dur = perf.get("duration_seconds", 0)
            if not isinstance(dur, (int, float)) or dur < 3600:
                print(f"strict: performance.duration_seconds {dur} <3600（需 1h soak）", file=sys.stderr); sys.exit(1)
            # soak 必须绑定且 duration >=1h 且 release build
            check_sha("soak", soak_d)
            if soak_d.get("build_profile") != "release":
                print(f"strict: soak.build_profile 需为 release 当前 {soak_d.get('build_profile')}", file=sys.stderr); sys.exit(1)
            if soak_d.get("duration_hours", 0) < 1.0:
                print(f"strict: soak duration {soak_d.get('duration_hours')}h <1.0h", file=sys.stderr); sys.exit(1)
            if soak_d.get("duration_seconds", dur) < 3600:
                print(f"strict: soak.duration_seconds {soak_d.get('duration_seconds')} <3600", file=sys.stderr); sys.exit(1)
            if soak_d.get("mode") and soak_d.get("mode") != "soak":
                print(f"strict: soak.mode {soak_d.get('mode')} != soak", file=sys.stderr); sys.exit(1)
            # real-device 也需绑定（若 wrapper 存在）
            if rd_wrapper is not None:
                rsha = rd_wrapper.get("git_sha")
                if not rsha or not sha_matches(rsha, git_sha):
                    print(f"strict: real_device.git_sha {rsha} != HEAD {git_sha}", file=sys.stderr); sys.exit(1)
                if rd_wrapper.get("dirty"):
                    print("strict: real_device dirty==true", file=sys.stderr); sys.exit(1)
            elif isinstance(real_dev_raw, dict) and real_dev_raw.get("git_sha"):
                if not sha_matches(real_dev_raw.get("git_sha"), git_sha):
                    print(f"strict: real_device git_sha mismatch", file=sys.stderr); sys.exit(1)
                if real_dev_raw.get("dirty"):
                    print("strict: real_device dirty==true", file=sys.stderr); sys.exit(1)
            elif isinstance(real_dev_raw, list):
                # 旧数组形态无绑定，直接失败，避免旧证据冒充
                print("strict: real_device 证据未绑定 git_sha（需 object 含 git_sha + entries）", file=sys.stderr); sys.exit(1)

            # Performance 硬条件：50K / 20ms / 50ms
            if perf.get("throughput_updates_per_sec", 0) < 50000:
                print(f"strict: throughput {perf.get('throughput_updates_per_sec')} < 50000", file=sys.stderr); sys.exit(1)
            if perf.get("ipc_p95_ms", 0) > 20 or perf.get("ipc_p99_ms", 0) > 50:
                print(f"strict: ipc p95 {perf.get('ipc_p95_ms')}ms >20 or p99 {perf.get('ipc_p99_ms')}ms >50", file=sys.stderr); sys.exit(1)
            # Soak 硬条件
            if soak_d.get("rss_growth_percent", 100) > 10:
                print(f"strict: rss_growth {soak_d.get('rss_growth_percent')}% >10%", file=sys.stderr); sys.exit(1)
            if soak_d.get("leak_detected", True):
                print("strict: leak_detected true", file=sys.stderr); sys.exit(1)
            if not rd:
                print("strict: real_device_matrix 为空，需填真机矩阵", file=sys.stderr); sys.exit(1)
            # Real device 硬条件：s7/focas2/opcua 必须有 passed
            required = {"s7", "focas2", "opcua"}
            passed_drivers = {x.get("driver") for x in rd if isinstance(x, dict) and x.get("status") == "passed"}
            missing = required - passed_drivers
            if missing:
                print(f"strict: real_device_matrix 缺失 required passed {sorted(missing)} 当前 passed {sorted(passed_drivers)}", file=sys.stderr); sys.exit(1)
            print("validate PASS (strict)")
        else:
            print("validate PASS (required fields)")

if __name__ == "__main__":
    main()
