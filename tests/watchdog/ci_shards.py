"""Watchdog 分片准入：场景必须恰好覆盖一次，结果不得遗漏或部分通过。"""
import argparse
from collections import Counter
import json
from pathlib import Path
import re

HERE = Path(__file__).resolve().parent


def load_shards(path=HERE / "ci-shards.json"):
    shards = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(shards, dict) or set(shards) != {"1", "2"}:
        raise ValueError("分片必须与 workflow 的 1/2 两片一致")
    source = (HERE / "run-watchdog-scenarios.ps1").read_text(encoding="utf-8-sig")
    # 从真实场景表取清单，新增场景未分配时必须失败，不能只维护一份旧的 M0–M19。
    table = re.search(r"\$SCEN = \[ordered\]@\{(.*?)\n\}", source, re.S)
    if table is None:
        raise ValueError("无法识别场景定义，拒绝生成部分计划")
    defined = set(re.findall(r"^\s+(M\d+)\s*=", table[1], re.M))
    if not defined:
        raise ValueError("场景定义为空")
    assigned = []
    for cases in shards.values():
        if not isinstance(cases, list) or not cases or any(type(c) is not str for c in cases):
            raise ValueError("每片必须是非空场景列表")
        assigned.extend(cases)
    counts = Counter(assigned)
    if set(counts) != defined or any(n != 1 for n in counts.values()):
        raise ValueError("分片必须无重复、无遗漏、无未知场景地覆盖 SCEN")
    return shards


def validate_summary(summary, expected):
    rows = summary.get("rows")
    if not isinstance(rows, list) or Counter(r.get("scenario") for r in rows) != Counter(expected):
        raise ValueError("结果中的场景与本片计划不一致")
    if (type(summary.get("passed")) is not int or type(summary.get("failed")) is not int
            or summary["passed"] != len(expected) or summary["failed"] != 0):
        raise ValueError("场景未全部通过")
    for row in rows:
        total = row.get("asserts_total")
        if (row.get("result") != "PASS" or type(total) is not int or total <= 0
                or type(row.get("asserts_passed")) is not int or row["asserts_passed"] != total):
            raise ValueError(f"场景 {row.get('scenario')} 存在失败或没有实际断言")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shard", required=True, choices=("1", "2"))
    parser.add_argument("--summary", type=Path)
    args = parser.parse_args()
    cases = load_shards()[args.shard]
    if args.summary:
        validate_summary(json.loads(args.summary.read_text(encoding="utf-8-sig")), cases)
        print(f"shard {args.shard}: {len(cases)} scenarios verified")
    else:
        print(",".join(cases))


if __name__ == "__main__":
    main()
