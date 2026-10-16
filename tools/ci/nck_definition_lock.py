#!/usr/bin/env python3
"""NCK 定义目录 DIGESTS.lock 的**基线分支**校验（PR-B 附加 2 / §6.6 T6）。

为什么必须对照基线：普通 CI 只做「重算当前定义 vs 当前 Lock」，挡不住开发者
**同时**修改定义文件与 Lock（两者一起改即自洽通过）。本工具以 PR base（默认
`main`）的 `DIGESTS.lock` 为基线做三条断言：

  1. 基线中已存在的 `(variable_id, revision)`：其 digest 与定义内容**逐字不变**
     （变化即失败，**不得**靠同步更新 Lock 通过）；
  2. 新分支中**新增**的 `(variable_id, revision)`：允许（只允许新增 revision）；
  3. 基线中存在、新分支中**缺失**的 `(variable_id, revision)`：失败
     （不得删除已发布 revision）。

另有第 4 组断言（自检）：当前 Lock 的每个 digest 必须等于当前定义文件按
canonical JSON 重算的 SHA-256——没有这一层，「改了定义但不改 Lock」会被漏掉。

离线、无第三方依赖。

用法：
    python3 tools/ci/nck_definition_lock.py --base <git-rev> [--repo DIR] [--lock PATH]
    python3 tools/ci/nck_definition_lock.py            # 仅做当前树自检（无基线）
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys

# 相对**仓库根**的固定位置（§3.1 C5：定义目录与 src/ 平级、按 revision 分文件）。
DEFINITIONS_DIR = 'drivers/sinumerik-nck/definitions'
DEFAULT_LOCK = 'drivers/sinumerik-nck/definitions/828d/DIGESTS.lock'


# ---------------------------------------------------------------------------
# canonical JSON + 摘要（与 Rust 侧 definition.rs::compute_digest 同构）
# ---------------------------------------------------------------------------

def canonical_json(obj) -> str:
    """canonical JSON（§3.2 五条规则）。

    - 顶层与嵌套对象键按**字节序升序**（`sort_keys=True`；键名全 ASCII，
      与 Rust 的字节序排序一致）；
    - 数组**保序**；
    - `digest` 字段本身不参与；
    - 数字沿用 JSON 文本（不在本层做浮点归一；定义数据只用 -200.0/0.001 这类
      可被两侧最短往返格式化一致表达的常量）；
    - 缺省可选字段不出现（定义文件本身不写 `null`，本层不补键）。
    """
    if isinstance(obj, dict):
        obj = {k: v for k, v in obj.items() if k != 'digest'}
    return json.dumps(obj, sort_keys=True, separators=(',', ':'), ensure_ascii=False)


def definition_digest(defn) -> str:
    """单条定义的 lowercase hex SHA-256(canonical JSON)。"""
    return hashlib.sha256(canonical_json(defn).encode('utf-8')).hexdigest()


# ---------------------------------------------------------------------------
# 读取
# ---------------------------------------------------------------------------

def repo_root(repo: str | None) -> str:
    out = subprocess.run(['git', 'rev-parse', '--show-toplevel'],
                         cwd=repo, capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit(f'不在 git 仓库中（--repo={repo}）：{out.stderr.strip()}')
    return out.stdout.strip()


def git_show(root: str, rev: str, rel_path: str) -> str | None:
    """读取基线版本的文件内容；基线不存在该路径时返回 None。"""
    out = subprocess.run(['git', 'show', f'{rev}:{rel_path}'],
                         cwd=root, capture_output=True, text=True)
    if out.returncode != 0:
        return None
    return out.stdout


def load_definitions(root: str) -> dict:
    """扫描定义目录下所有 `*.json`（排除 Lock），返回 {(id, rev): (digest, file)}。"""
    found = {}
    base = os.path.join(root, DEFINITIONS_DIR)
    for dirpath, _dirnames, filenames in os.walk(base):
        for name in sorted(filenames):
            if not name.endswith('.json'):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, root).replace(os.sep, '/')
            with open(path, encoding='utf-8') as fh:
                doc = json.load(fh)
            for defn in doc.get('definitions', []):
                key = (defn['variable_id'], int(defn['revision']))
                if key in found:
                    raise SystemExit(f'定义重复：{key} 同时出现在 {found[key][1]} 与 {rel}')
                found[key] = (definition_digest(defn), rel)
    return found


def parse_lock(text: str, where: str) -> dict:
    doc = json.loads(text)
    entries = {}
    for e in doc.get('entries', []):
        key = (e['variable_id'], int(e['revision']))
        if key in entries:
            raise SystemExit(f'{where} 中条目重复：{key}')
        entries[key] = {'digest': e['digest'], 'file': e.get('file', '')}
    return entries


# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------

def compare(baseline: dict, current: dict, computed: dict) -> list:
    """返回问题清单；空列表 = 通过。"""
    problems = []

    # 断言 1：基线已发布项不可变（内容与 Lock 两个角度都比一次）。
    for key, base_entry in sorted(baseline.items()):
        cur_entry = current.get(key)
        if cur_entry is None:
            problems.append(
                f'[删除已发布 revision] {key[0]}@{key[1]}：基线中存在，新分支缺失（不得删除）')
            continue
        if cur_entry['digest'] != base_entry['digest']:
            problems.append(
                f'[已发布定义被修改] {key[0]}@{key[1]}：baseline={base_entry["digest"]} '
                f'current={cur_entry["digest"]}（同步更新 Lock 也不允许）')
        if cur_entry.get('file') != base_entry.get('file'):
            problems.append(
                f'[发行文件路径变化] {key[0]}@{key[1]}：{base_entry.get("file")} → {cur_entry.get("file")}')
        got = computed.get(key)
        if got is not None and got[0] != base_entry['digest']:
            problems.append(
                f'[定义内容被修改] {key[0]}@{key[1]}：实算={got[0]} baseline={base_entry["digest"]}'
                f'（{got[1]}）')

    # 断言 2：新增项允许（无问题），但仍然必须通过自检（见下）。
    # 断言 3：当前 Lock 与当前定义内容自洽 + 没有「只在 Lock 里、没有定义文件」的幽灵条目。
    for key, cur_entry in sorted(current.items()):
        got = computed.get(key)
        if got is None:
            problems.append(
                f'[幽灵条目] {key[0]}@{key[1]}：Lock 有条目但定义目录中找不到该定义')
            continue
        if got[0] != cur_entry['digest']:
            problems.append(
                f'[Lock 与定义不一致] {key[0]}@{key[1]}：lock={cur_entry["digest"]} '
                f'实算={got[0]}（{got[1]}）')

    # 断言 4：定义目录里存在、但 Lock 未登记（新发布必须同时登记摘要）。
    for key, (digest, rel) in sorted(computed.items()):
        if key not in current:
            problems.append(
                f'[Lock 缺条目] {key[0]}@{key[1]}（{rel}, digest={digest}）：'
                f'新增/修改定义必须同步登记 DIGESTS.lock')
    return problems


def main() -> int:
    ap = argparse.ArgumentParser(description='NCK 定义目录 DIGESTS.lock 基线校验')
    ap.add_argument('--base', default='', help='基线 git rev（PR base；空则跳过基线比对）')
    ap.add_argument('--repo', default=None, help='仓库目录（默认当前目录所属仓库）')
    ap.add_argument('--lock', default=DEFAULT_LOCK, help='相对仓库根的 Lock 路径')
    args = ap.parse_args()

    root = repo_root(args.repo)
    lock_abs = os.path.join(root, args.lock.replace('/', os.sep))
    if not os.path.isfile(lock_abs):
        print(json.dumps({'ok': False, 'reason': f'Lock 不存在：{args.lock}'}, ensure_ascii=False))
        return 1
    with open(lock_abs, encoding='utf-8') as fh:
        current = parse_lock(fh.read(), args.lock)

    baseline = {}
    if args.base:
        text = git_show(root, args.base, args.lock)
        if text is None:
            print(f'NOTE: 基线 {args.base} 中不存在 {args.lock}（首次发布）→ 只允许新增')
        else:
            baseline = parse_lock(text, f'{args.base}:{args.lock}')
    else:
        print('NOTE: 未提供 --base（非 PR 触发？）→ 跳过基线比对，只做当前树自检')

    computed = load_definitions(root)
    problems = compare(baseline, current, computed)

    report = {
        'ok': not problems,
        'base': args.base or None,
        'lock': args.lock,
        'baseline_entries': len(baseline),
        'current_entries': len(current),
        'computed_definitions': len(computed),
        'added': sorted(f'{k[0]}@{k[1]}' for k in set(current) - set(baseline)),
        'problems': problems,
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))
    if problems:
        print(f'\nFAIL: {len(problems)} 项（已发布 revision 不可原地修改，只允许新增）')
        return 1
    print('\nPASS: 已发布 revision 逐字不变，新增项自洽')
    return 0


if __name__ == '__main__':
    sys.exit(main())
