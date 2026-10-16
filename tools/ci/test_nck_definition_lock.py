#!/usr/bin/env python3
"""nck_definition_lock.py 自测（N10；无第三方依赖；CI quality 内可跑）。

运行：python3 tools/ci/test_nck_definition_lock.py
失败即 CI FAIL——工具回归不得合入。

覆盖 §6.6 T6 的三个场景 + 当前树自洽 + canonical 摘要字面量：
  - 场景①：改定义内容 + **同步**改 Lock ⇒ 非零退出（普通 CI 挡不住的那一类）；
  - 场景②：仅**新增** `(variable_id, revision)` ⇒ 退出 0；
  - 场景③：**删除**已发布 revision ⇒ 非零退出；
  - 自洽：改定义但不改 Lock ⇒ 非零退出；
  - 字面量：canonical JSON 的键序/分隔符/非 ASCII 处理必须与冻结字符串逐字相同。
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(HERE, 'nck_definition_lock.py')
sys.path.insert(0, HERE)

import nck_definition_lock as tool  # noqa: E402  （自测直接复用 canonical JSON 规则）

PASS = 0
FAIL = 0

LOCK_REL = 'drivers/sinumerik-nck/definitions/828d/DIGESTS.lock'
R1_REL = 'drivers/sinumerik-nck/definitions/828d/r1.json'
R2_REL = 'drivers/sinumerik-nck/definitions/828d/r2.json'


def check(tag, cond, extra=''):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f'PASS {tag}')
    else:
        FAIL += 1
        print(f'FAIL {tag} {extra}')


def def1(label_zh='状态'):
    return {
        'variable_id': 'channel.program.status',
        'revision': 1,
        'family': '828d',
        'scope': 'collect',
        'semantics_status': 'verified',
        'label': {'zh': label_zh, 'en': 'status'},
        'source': [{
            'kind': 'frozen_vector',
            'reference': 'drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json',
        }],
        'targets': ['channel'],
        'target_required': ['channel'],
        'arguments': [],
        'addressing': {
            'area': 'C',
            'area_no': {'source': 'target', 'kind': 'channel'},
            'module': {'source': 'fixed', 'value': 127},
            'column': 13,
            'line': {'source': 'fixed', 'value': 1},
        },
        'decode': 'U16_LE',
        'transport_size': 9,
        'count': 1,
        'unit_mode': 'current',
        'identity_template': 'channel.program.status@channel={channel}',
        'identity_keys': [{'path': 'channel', 'required': True}],
        'golden_ref': [{'area': 'C', 'block': 'S', 'variable': 'progStatus', 'entry': 'nc_variable'}],
    }


def def2():
    return {
        'variable_id': 'channel.feed.override',
        'revision': 1,
        'family': '828d',
        'scope': 'collect',
        'semantics_status': 'verified',
        'label': {'zh': '进给倍率', 'en': 'feed override'},
        'source': [{
            'kind': 'frozen_vector',
            'reference': 'drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json',
        }],
        'targets': ['channel'],
        'target_required': ['channel'],
        'arguments': [],
        'addressing': {
            'area': 'C',
            'area_no': {'source': 'target', 'kind': 'channel'},
            'module': {'source': 'fixed', 'value': 127},
            'column': 3,
            'line': {'source': 'fixed', 'value': 1},
        },
        'decode': 'F64_LE',
        'transport_size': 9,
        'count': 1,
        'unit_mode': 'current',
        'identity_template': 'channel.feed.override@channel={channel}',
        'identity_keys': [{'path': 'channel', 'required': True}],
        'golden_ref': [{'area': 'C', 'block': 'S', 'variable': 'feedRateIpoOvr', 'entry': 'nc_variable'}],
    }


def lock_of(defs, file_rel=R1_REL):
    return {
        'format_version': 1,
        'family': '828d',
        'entries': [
            {
                'variable_id': d['variable_id'],
                'revision': d['revision'],
                'file': file_rel,
                'digest': tool.definition_digest(d),
            }
            for d in defs
        ],
    }


def write_json(path, obj):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w', encoding='utf-8') as fh:
        json.dump(obj, fh, ensure_ascii=False, indent=2, sort_keys=True)


def commit(root, msg):
    env = dict(os.environ)
    env.update({
        'GIT_AUTHOR_NAME': 't', 'GIT_AUTHOR_EMAIL': 't@example.com',
        'GIT_COMMITTER_NAME': 't', 'GIT_COMMITTER_EMAIL': 't@example.com',
    })
    subprocess.run(['git', 'add', '-A'], cwd=root, check=True, capture_output=True)
    subprocess.run(['git', 'commit', '-m', msg], cwd=root, check=True,
                   capture_output=True, env=env)


def head(root):
    return subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=root, check=True,
                          capture_output=True, text=True).stdout.strip()


def new_repo(defs=None, lock=None):
    """建立临时 git 仓库：基线提交 = r1.json + DIGESTS.lock。"""
    root = tempfile.mkdtemp(prefix='nck-lock-')
    subprocess.run(['git', 'init', '-q'], cwd=root, check=True, capture_output=True)
    defs = defs if defs is not None else [def1(), def2()]
    write_json(os.path.join(root, R1_REL.replace('/', os.sep)),
               {'format_version': 1, 'family': '828d', 'revision': 1, 'definitions': defs})
    write_json(os.path.join(root, LOCK_REL.replace('/', os.sep)),
               lock if lock is not None else lock_of(defs))
    commit(root, 'baseline')
    return root, head(root)


def run(*argv):
    return subprocess.run([sys.executable, TOOL, *argv], capture_output=True, text=True)


def main():
    # ---- 字面量：canonical JSON 必须与冻结字符串逐字相同 -------------------
    expect = (
        '{"addressing":{"area":"C","area_no":{"kind":"channel","source":"target"},'
        '"column":13,"line":{"source":"fixed","value":1},"module":{"source":"fixed","value":127}},'
        '"arguments":[],"count":1,"decode":"U16_LE","family":"828d",'
        '"golden_ref":[{"area":"C","block":"S","entry":"nc_variable","variable":"progStatus"}],'
        '"identity_keys":[{"path":"channel","required":true}],'
        '"identity_template":"channel.program.status@channel={channel}",'
        '"label":{"en":"status","zh":"状态"},"revision":1,"scope":"collect",'
        '"semantics_status":"verified","source":[{"kind":"frozen_vector","reference":'
        '"drivers/sinumerik-nck/tests/fixtures/legacy-828d-wire.json"}],'
        '"target_required":["channel"],"targets":["channel"],"transport_size":9,'
        '"unit_mode":"current","variable_id":"channel.program.status"}'
    )
    got = tool.canonical_json(def1())
    check('canonical JSON literal', got == expect, f'\n got={got}\nwant={expect}')
    check('digest is lowercase hex sha256',
          tool.definition_digest(def1()) == hashlib.sha256(expect.encode('utf-8')).hexdigest())
    check('digest ignores its own field',
          tool.definition_digest({**def1(), 'digest': 'deadbeef'}) == tool.definition_digest(def1()))

    # ---- 场景①：改定义内容 + 同步改 Lock ⇒ 仍必须失败 ---------------------
    root, base = new_repo()
    try:
        write_json(os.path.join(root, R1_REL.replace('/', os.sep)),
                   {'format_version': 1, 'family': '828d', 'revision': 1,
                    'definitions': [def1('状态（被原地修改）'), def2()]})
        write_json(os.path.join(root, LOCK_REL.replace('/', os.sep)),
                   lock_of([def1('状态（被原地修改）'), def2()]))
        commit(root, 'modify definition and sync lock')
        r = run('--base', base, '--repo', root)
        check('T6-1 modified+synced lock exit 1', r.returncode == 1, r.stdout[:400])
        check('T6-1 report is not ok', '"ok": false' in r.stdout, r.stdout[:400])
    finally:
        shutil.rmtree(root, ignore_errors=True)

    # ---- 场景②：仅新增 revision ⇒ 通过 -----------------------------------
    root, base = new_repo()
    try:
        write_json(os.path.join(root, R2_REL.replace('/', os.sep)),
                   {'format_version': 1, 'family': '828d', 'revision': 1,
                    'definitions': [{**def1(), 'variable_id': 'channel.feed.actual', 'column': 1}]})
        write_json(os.path.join(root, LOCK_REL.replace('/', os.sep)),
                   lock_of([def1(), def2(), {**def1(), 'variable_id': 'channel.feed.actual',
                                             'column': 1}], file_rel=R1_REL))
        # 上面把新定义也登记在 r1.json 的 file 路径下不合语义，改为独立条目：
        lock = json.load(open(os.path.join(root, LOCK_REL.replace('/', os.sep)), encoding='utf-8'))
        lock['entries'][-1]['file'] = R2_REL
        write_json(os.path.join(root, LOCK_REL.replace('/', os.sep)), lock)
        commit(root, 'add revision')
        r = run('--base', base, '--repo', root)
        check('T6-2 added revision exit 0', r.returncode == 0, r.stdout[:400])
        check('T6-2 reports addition',
              'channel.feed.actual@1' in r.stdout, r.stdout[:400])
    finally:
        shutil.rmtree(root, ignore_errors=True)

    # ---- 场景③：删除已发布 revision ⇒ 失败 -------------------------------
    root, base = new_repo()
    try:
        write_json(os.path.join(root, R1_REL.replace('/', os.sep)),
                   {'format_version': 1, 'family': '828d', 'revision': 1, 'definitions': [def1()]})
        write_json(os.path.join(root, LOCK_REL.replace('/', os.sep)), lock_of([def1()]))
        commit(root, 'delete published revision')
        r = run('--base', base, '--repo', root)
        check('T6-3 deleted revision exit 1', r.returncode == 1, r.stdout[:400])
        check('T6-3 names the deletion',
              '删除已发布 revision' in r.stdout, r.stdout[:400])
    finally:
        shutil.rmtree(root, ignore_errors=True)

    # ---- 自洽：改定义但不改 Lock ⇒ 失败（同一提交内自检） -----------------
    root, _base = new_repo()
    try:
        write_json(os.path.join(root, R1_REL.replace('/', os.sep)),
                   {'format_version': 1, 'family': '828d', 'revision': 1,
                    'definitions': [def1('改名但未同步 Lock'), def2()]})
        r = run('--base', head(root), '--repo', root)
        check('self-check stale lock exit 1', r.returncode == 1, r.stdout[:400])
        check('self-check names mismatch', 'Lock 与定义不一致' in r.stdout, r.stdout[:400])
    finally:
        shutil.rmtree(root, ignore_errors=True)

    # ---- 现网：当前树自洽（真实 Lock + 真实定义目录） ---------------------
    real = os.path.dirname(os.path.dirname(HERE))
    if os.path.isfile(os.path.join(real, LOCK_REL.replace('/', os.sep))):
        r = run('--repo', real)
        check('current tree self-consistent', r.returncode == 0, r.stdout[:600])
        r = run('--base', 'main', '--repo', real)
        check('current tree vs main baseline exit 0', r.returncode == 0, r.stdout[:600])
    else:
        check('current tree self-consistent', False, 'DIGESTS.lock 不存在')

    print(f'\n{PASS} passed, {FAIL} failed')
    return 1 if FAIL else 0


if __name__ == '__main__':
    sys.exit(main())
