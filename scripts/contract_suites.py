#!/usr/bin/env python3
"""合同测试套件唯一名单（证据链单一定来源）。

suite = tests/driver-contract/tests/<name>.rs（integration test target，文件名即套件名）。
write-contract-evidence.py 与 generate-release-validation.py 都从这里 import，
杜绝两边各写一份再分叉。

增删 suite 必须显式评审本文件（与 stats 键冻结同理）。
"""
import pathlib

# 基线（§21 + V2.1 扩展；DeviceProfile 整链已于 f81e7ce 删除，不再有 profile_contract）。
BASELINE = [
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
    "resource_contract",
    "subprocess_orphan_guard",
]
# 事件面（PR10 Event Plane V1，含 hardening gates）。
EVENT = [
    "event_hardening_smoke",
    "event_identity",
    "event_lifecycle",
    "event_persistence",
    "event_pressure",
    "event_retention_pressure",
    "event_runtime",
    "event_scheduler",
    "event_soak",
    "event_sse",
    "event_store_faults",
    "opcua_event_e2e",
]
# 生命周期与恢复。
LIFECYCLE = [
    "mesad_restart",
    "stop_lifecycle",
]

SUITES = BASELINE + EVENT + LIFECYCLE
SUITE_SET = set(SUITES)


def actual_suite_files(repo_root=None):
    """磁盘实际套件文件（stem 集合；common/ 等目录除外）。"""
    root = pathlib.Path(repo_root or pathlib.Path(__file__).resolve().parent.parent)
    d = root / "tests" / "driver-contract" / "tests"
    return {p.stem for p in d.glob("*.rs")}


def check_manifest(repo_root=None):
    """名单与磁盘双向对拍。返回 (ok, 幽灵, 遗漏)。"""
    actual = actual_suite_files(repo_root)
    ghost = sorted(SUITE_SET - actual)
    missing = sorted(actual - SUITE_SET)
    return (not ghost and not missing, ghost, missing)
