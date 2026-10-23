#!/usr/bin/env python3
"""生成 target/validation/contract.json 的唯一权威脚本（Suite Gate）。

机器保证：本脚本先删除旧 contract.json，再依次执行
  1) cargo build --locked --workspace  （重编全部 Driver binaries，避免旧二进制）
  2) 名单与磁盘双向对拍（contract_suites.check_manifest；幽灵/遗漏直接失败）
  3) 按 scripts/contract_suites.py 的名单逐 suite 调 `cargo nextest run --profile ci`
     （与 CI 门禁同一个运行器、同一份 .config/nextest.toml），机器可读结果逐 suite
     归集进 contract.json；任一失败不写证据
外部唯一入口即 `python scripts/write-contract-evidence.py`，无法通过参数跳过测试。
套件名单唯一来源 scripts/contract_suites.py。

为什么必须用 nextest 而不是 `cargo test`（历史缺陷，勿回退）：
`cargo test --test <suite>` 在套件内同进程并行，`stop_lifecycle`（断言进程级全局
LIVE_CHILDREN == 0）与 `reconnect_backoff`（依赖进程级共享 LogBuf）这类进程级断言
在并行下会假失败；nextest 每个测试独立进程 + `[test-groups]` 调度（serial-process=1 /
process=2）才能稳定通过。证据必须由与准入相同的运行器产出，否则证据与门禁不同源。

为什么逐 suite 跑、而不是一次跑完 30 个（实测理由，勿随意改回）：
一次性把 30 个 suite 放进同一个 nextest 进程时，`mesa-driver-ports` 这个**全局**端口
租约目录会被多个 suite 的 driver 进程同时写入，`subprocess_orphan_guard` 的
c3-08 用例会把它看到的新租约误判成自己的（该用例自己的 NOTE 已承认这一归因在跨套件
并发下不成立，见 tests/driver-contract/tests/subprocess_orphan_guard.rs:827-830），
从而在 15s 有界等待上超时。实测：一次性跑 30 套件时该用例失败（new_claims 里混入了
其它套件的 5 个端口），单独跑 10/10 通过。逐 suite 跑消除跨套件全局态竞争，同时
完整保留"每测试独立进程 + test-groups"这两个真正修好 stop_lifecycle / reconnect_backoff
的性质，因此证据既不比门禁弱，也不会随机器负载随机变红。
"""
import json, os, pathlib, re, subprocess, sys, datetime

from contract_suites import SUITES, SUITE_SET, check_manifest

# 套件名单见 contract_suites.py（唯一来源）。

# ---- 与 CI 同源的运行器契约 -------------------------------------------------
# CI 安装版本：.github/actions/rust-ci-setup/action.yml:74 `tool: cargo-nextest@0.9.146`
# CI 主流水线：.github/workflows/_rust-canonical.yml:86-96 `cargo nextest run --profile ci`
# 版本不一致意味着"证据不是门禁跑出来的"，必须 fail-closed（exit 6），不得静默降级。
NEXTEST_PINNED_VERSION = "0.9.146"
NEXTEST_PROFILE = "ci"          # 读仓库 .config/nextest.toml 的 [profile.ci]，不自造 profile
NEXTEST_PACKAGE = "mesa-contract-tests"

# 机器可读结果格式。0.9.146 的 `--message-format` 只支持 human / libtest-json /
# libtest-json-plus；JUnit XML 只能通过 [profile.ci.junit] 配置启用，而 .config/nextest.toml
# 属禁改文件，故选用 libtest-json-plus：它比 libtest-json 多带 nextest 子对象
# （含 test_binary / kind），可按 suite 精确归集，且用 --message-format-version 固定 schema。
NEXTEST_MESSAGE_FORMAT = "libtest-json-plus"
NEXTEST_MESSAGE_FORMAT_VERSION = "0.1"

# 该格式在 0.9.146 属实验特性，必须显式开启；不开启时 nextest 直接报错（不会产出假结果）。
NEXTEST_JSON_ENV = "NEXTEST_EXPERIMENTAL_LIBTEST_JSON"

# 单个测试的终止事件中，除 ok / ignored 之外一律视为失败（含超时、泄漏、崩溃等）。
TEST_FAILURE_EVENTS = {
    "failed", "timeout", "leak", "segv", "sigsegv", "abort", "exec-failed", "error",
}

# suite 名会拼进 nextest filterset 表达式，必须是安全标识符（防注入选出错误集合）。
SUITE_NAME_RE = re.compile(r"^[A-Za-z0-9_]+$")


def git_sha():
    try:
        return subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        return "unknown"

def is_clean_tree():
    try:
        # 未跟踪的非忽略源文件也属于工作区；target 等构建产物由 .gitignore 排除。
        out = subprocess.check_output(
            ["git", "status", "--porcelain", "--untracked-files=normal"], text=True
        )
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


def nextest_version():
    """返回本机 cargo-nextest 版本号（如 '0.9.146'）；取不到则返回 None。"""
    try:
        out = subprocess.check_output(["cargo", "nextest", "--version"], text=True,
                                      stderr=subprocess.STDOUT)
    except FileNotFoundError:
        print("cargo-nextest not found: 证据必须由与 CI 相同的运行器产出", file=sys.stderr)
        sys.exit(127)
    except subprocess.CalledProcessError as e:
        print(f"cargo nextest --version failed (exit {e.returncode})", file=sys.stderr)
        return None
    m = re.search(r"^cargo-nextest\s+(\S+)", out, re.M)
    return m.group(1) if m else None


def suite_command(suite):
    """与 CI 同源的 nextest 命令：--profile ci + 固定 message-format schema。

    `-E 'binary(=<suite>)'` 用精确匹配：`binary()` 的裸名匹配若写成子串形式，
    `smoke` 会连带选中 `event_hardening_smoke`，导致统计串味。
    """
    return [
        "cargo", "nextest", "run",
        "--locked",
        "--profile", NEXTEST_PROFILE,
        "-p", NEXTEST_PACKAGE,
        "--all-features",
        # profile.ci 已设 fail-fast=false；显式再给一次，避免 profile 变更后静默提前退出。
        "--no-fail-fast",
        "-E", f"binary(={suite})",
        "--message-format", NEXTEST_MESSAGE_FORMAT,
        "--message-format-version", NEXTEST_MESSAGE_FORMAT_VERSION,
        # 进度条只在交互终端有意义，脚本里关掉以免污染日志。
        "--show-progress", "none",
    ]


def binary_from_test_name(name):
    """libtest-json 测试名形如 `mesa-contract-tests::smoke$handshake_roundtrip`
    → 返回 suite 名 `smoke`（$ 前为 `<crate>::<test_binary>`）。"""
    head, _, _ = name.partition("$")
    _, _, binary = head.partition("::")
    return binary


def parse_suite_jsonl(text, suite):
    """解析单个 suite 的 nextest libtest-json-plus 结果（JSON Lines）。

    返回 (rec, failed_tests, stray, seen, foreign)：
      rec          : {test_count, passed, failed, ignored, filtered_out, status}
      failed_tests : 失败的测试全名（含超时/崩溃等非 ok 终止事件）
      stray        : 无法解析的非空 stdout 行（非空即视为解析失败，交由调用方 fail-closed）
      seen         : 是否真的观察到该 suite 的结果事件（false = 没跑/被过滤，不得当通过）
      foreign      : 结果里出现的非本 suite 的 binary（filterset 泄漏，不得静默丢弃）

    只认机器可读事件，不做任何 `cargo test` 文本格式解析。
    """
    rec = {"test_count": 0, "passed": 0, "failed": 0, "ignored": 0,
           "filtered_out": 0, "status": "ok"}
    failed_tests, stray, seen, foreign = [], [], False, set()
    for raw in text.splitlines():
        line = raw.strip()
        if not line:
            continue
        if not line.startswith("{"):
            stray.append(raw)
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            stray.append(raw)
            continue
        kind = msg.get("type")
        event = msg.get("event")
        if kind == "suite":
            nx = msg.get("nextest") or {}
            binary = nx.get("test_binary")
            if not binary:
                # 缺 nextest 子对象说明拿到的不是 libtest-json-plus，无法按 suite 归集。
                stray.append(raw)
                continue
            if nx.get("kind") != "test" or binary != suite:
                foreign.add(binary)
                continue
            seen = True
            if event == "started":
                rec["test_count"] = max(rec["test_count"], int(msg.get("test_count") or 0))
            elif event in ("ok", "failed"):
                # 同一个 binary 可能分多个调度阶段上报（如 event_soak 的
                # threads-required="num-test-threads" 覆盖）：真正执行的用例按阶段累加，
                # 而 ignored / filtered_out 是每阶段重复上报的常量，取 max 以免重复计数。
                rec["passed"] += int(msg.get("passed") or 0)
                rec["failed"] += int(msg.get("failed") or 0)
                rec["ignored"] = max(rec["ignored"], int(msg.get("ignored") or 0))
                rec["filtered_out"] = max(rec["filtered_out"], int(msg.get("filtered_out") or 0))
                if event != "ok":
                    rec["status"] = "failed"
        elif kind == "test":
            if event in TEST_FAILURE_EVENTS:
                name = msg.get("name") or ""
                if binary_from_test_name(name) != suite:
                    foreign.add(binary_from_test_name(name))
                    continue
                failed_tests.append(name)
    return rec, failed_tests, stray, seen, foreign


def run_suite(suite, log_dir):
    """运行单个 suite，把 JSONL 与人类可读诊断追加进完整运行日志。"""
    cmd = suite_command(suite)
    print(f"running: {' '.join(cmd)} ...", flush=True)
    env = dict(os.environ)
    env[NEXTEST_JSON_ENV] = "1"
    try:
        # Rust/nextest 输出为 UTF-8；Windows 默认 GBK 会让读管道线程在中文诊断处崩溃。
        # 汇总与测试名称使用 ASCII，异常日志字节替换不改变 pass/fail 的判定。
        r = subprocess.run(cmd, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", env=env)
    except FileNotFoundError as e:
        print(f"cargo not found: {e}", file=sys.stderr)
        sys.exit(127)
    # 完整运行日志（评审可复核；JSONL 是唯一解析来源，stderr 是人读诊断）。
    with open(log_dir / "nextest-run.jsonl", "a", encoding="utf-8") as f:
        f.write(r.stdout)
    with open(log_dir / "nextest-run.stderr.log", "a", encoding="utf-8") as f:
        f.write(f"\n===== suite {suite} (exit {r.returncode}) =====\n")
        f.write(r.stderr)
    return r.returncode, r.stdout, r.stderr


def main():
    out = pathlib.Path("target/validation/contract.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    # 先删旧 Evidence，避免失败后遗留
    try:
        out.unlink()
    except FileNotFoundError:
        pass
    # 每轮重新生成完整运行日志，避免与上一轮结果混淆。
    for name in ("nextest-run.jsonl", "nextest-run.stderr.log"):
        try:
            (out.parent / name).unlink()
        except FileNotFoundError:
            pass

    # 运行器同源性：版本必须与 CI pin 完全一致，否则证据不是门禁跑出来的。
    version = nextest_version()
    if version != NEXTEST_PINNED_VERSION:
        print(f"cargo-nextest version mismatch: found {version!r}, CI pins "
              f"{NEXTEST_PINNED_VERSION!r} (.github/actions/rust-ci-setup/action.yml:74), "
              f"not writing contract.json", file=sys.stderr)
        sys.exit(6)
    print(f"runner: cargo-nextest {version} (profile={NEXTEST_PROFILE})", flush=True)

    run_or_exit(["cargo", "build", "--locked", "--workspace"], "cargo build")

    # 名单与磁盘双向对拍（幽灵/遗漏直接失败，不写证据）
    ok, ghost, missing = check_manifest()
    if not ok:
        print(f"suite manifest mismatch: ghost={ghost} missing={missing}, not writing contract.json",
              file=sys.stderr)
        sys.exit(3)

    bad_names = [s for s in SUITES if not SUITE_NAME_RE.fullmatch(s)]
    if bad_names:
        print(f"invalid suite names {bad_names} (filterset 不安全), not writing contract.json",
              file=sys.stderr)
        sys.exit(3)

    # 逐 suite 运行并解析真实结果（任一失败即退出，不写证据）
    suite_results = {}
    total_passed = 0
    total_failed = 0
    any_suite_failure = False
    for suite in SUITES:
        rc, stdout, stderr = run_suite(suite, out.parent)
        rec, failed_tests, stray, seen, foreign = parse_suite_jsonl(stdout, suite)

        if stray:
            # 非空 stdout 行无法解析 ⇒ 结果不可信，绝不写证据。
            preview = "\n".join(stray[:5])
            print(f"suite {suite}: cannot parse nextest output ({len(stray)} stray stdout lines, "
                  f"exit {rc}):\n{preview}\nnot writing contract.json", file=sys.stderr)
            sys.exit(4)
        if foreign:
            print(f"suite {suite}: results contain other binaries {sorted(foreign)} "
                  f"(filterset 未生效), not writing contract.json", file=sys.stderr)
            sys.exit(4)
        if not seen:
            # 没有任何该 suite 的结果事件 = 没真正跑（被过滤/名字不匹配），不是"通过"。
            print(f"suite {suite}: no result events (exit {rc}), not writing contract.json",
                  file=sys.stderr)
            sys.exit(4)
        if rec["test_count"] == 0 or (rec["passed"] + rec["failed"] + rec["ignored"]) == 0:
            print(f"suite {suite}: ran 0 tests (test_count={rec['test_count']}), "
                  f"not writing contract.json", file=sys.stderr)
            sys.exit(4)
        if rec["filtered_out"]:
            print(f"suite {suite}: {rec['filtered_out']} tests filtered out, "
                  f"not writing contract.json", file=sys.stderr)
            sys.exit(4)
        if rec["ignored"]:
            # 已知：event_soak 的 150s extended soak 标了 #[ignore]（manual/nightly 档）。
            # 记录进证据以便复核，但不视为失败。
            print(f"suite {suite}: {rec['ignored']} test(s) ignored (not executed)", file=sys.stderr)

        suite_results[suite] = {
            "passed": rec["passed"],
            "failed": rec["failed"],
            "ignored": rec["ignored"],
            "tests": rec["passed"] + rec["failed"],
            "failed_tests": failed_tests,
        }
        total_passed += rec["passed"]
        total_failed += rec["failed"]
        if rc != 0 or rec["status"] != "ok" or rec["failed"] != 0 or failed_tests:
            any_suite_failure = True
            # 保留失败套件完整诊断；最终准入仍失败，不能只留下被汇总掩盖的失败计数。
            failure_log = out.parent / f"contract-{suite}-failed.log"
            lines = [f"# suite {suite}: passed={rec['passed']} failed={rec['failed']} "
                     f"ignored={rec['ignored']} exit={rc}"]
            lines += [f"FAILED {n}" for n in failed_tests]
            lines += ["--- nextest stderr ---", stderr]
            failure_log.write_text("\n".join(lines) + "\n", encoding="utf-8")
            print(f"suite {suite} failed, continuing to collect all suites "
                  f"(no evidence will be written)", file=sys.stderr)
        else:
            print(f"suite {suite}: passed={rec['passed']} failed=0 "
                  f"ignored={rec['ignored']}", flush=True)

    if any_suite_failure or total_failed != 0:
        print(f"contract suites failed: {total_failed} tests failed, not writing contract.json",
              file=sys.stderr)
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
        # 运行器溯源：证明证据由与 CI 同一版本/同一 profile 的 nextest 产出。
        "runner": {
            "tool": "cargo-nextest",
            "version": version,
            "profile": NEXTEST_PROFILE,
            "config": ".config/nextest.toml",
            "message_format": f"{NEXTEST_MESSAGE_FORMAT}/{NEXTEST_MESSAGE_FORMAT_VERSION}",
            "invocation": "per-suite: cargo nextest run --locked --profile ci "
                          f"-p {NEXTEST_PACKAGE} --all-features -E 'binary(=<suite>)'",
            "command_template": suite_command("<suite>"),
        },
    }
    out.write_text(json.dumps(doc, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"wrote {out} suites={len(SUITES)} tests={total_passed} sha={sha} dirty={doc['dirty']}")

if __name__ == "__main__":
    main()
