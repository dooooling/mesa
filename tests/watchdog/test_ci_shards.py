"""对真实分片定义做故障变异，防止并行后漏跑仍得到绿灯。"""
import copy
import json
from pathlib import Path
import tempfile
import unittest

from ci_shards import load_shards, validate_summary


class ShardAdmission(unittest.TestCase):
    def test_partition_rejects_missing_duplicate_and_unknown_scenarios(self):
        original = load_shards()
        for mode in ("missing", "duplicate", "unknown"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                changed = copy.deepcopy(original)
                if mode == "missing":
                    changed["1"].pop()
                elif mode == "duplicate":
                    changed["2"].append(changed["1"][0])
                else:
                    changed["1"][0] = "M999"
                path = Path(directory) / "shards.json"
                path.write_text(json.dumps(changed), encoding="utf-8")
                with self.assertRaises(ValueError):
                    load_shards(path)

    def test_summary_rejects_partial_duplicate_and_failed_results(self):
        for cases in load_shards().values():
            summary = {"passed": len(cases), "failed": 0,
                       "rows": [{"scenario": c, "result": "PASS", "asserts_total": 1,
                                 "asserts_passed": 1} for c in cases]}
            validate_summary(summary, cases)
            for mode in ("missing", "duplicate", "failed", "empty"):
                with self.subTest(mode=mode):
                    changed = copy.deepcopy(summary)
                    if mode == "missing":
                        changed["rows"].pop()
                    elif mode == "duplicate":
                        changed["rows"].append(changed["rows"][0])
                    elif mode == "failed":
                        changed["rows"][0]["asserts_passed"] = 0
                    else:
                        changed["rows"][0].update(asserts_total=0, asserts_passed=0)
                    with self.assertRaises(ValueError):
                        validate_summary(changed, cases)


if __name__ == "__main__":
    unittest.main()
