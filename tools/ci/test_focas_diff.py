#!/usr/bin/env python3
"""focas_diff 自测（PR-A 工具门；无第三方依赖；CI quality 内可跑）。

运行：python3 tools/ci/test_focas_diff.py
失败即 CI FAIL——工具回归不得合入。
"""
import os
import subprocess
import sys
import tempfile

TOOL = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'focas_diff.py')
PASS = 0
FAIL = 0


def check(tag, cond, extra=''):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f'PASS {tag}')
    else:
        FAIL += 1
        print(f'FAIL {tag} {extra}')


def run(*argv):
    return subprocess.run(
        [sys.executable, TOOL, *argv], capture_output=True, text=True,
    )


def main():
    # compare 完全匹配 → exit 0 + match true（合法 GENERIC 请求）。
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    req = os.path.join(root, 'drivers/focas2/tests/fixtures/wire/spindle_load_165/type0/request_frame.bin')
    if os.path.isfile(req):
        r = run('compare', '--native', req, '--wire', req)
        check('compare match exit 0', r.returncode == 0, r.stdout[:200])
        check('compare match true', '"match": true' in r.stdout, r.stdout[:200])
    else:
        check('compare match exit 0', False, 'missing fixture')
        check('compare match true', False, 'missing fixture')

    # compare 单字节差异 → exit 1 + 首差异偏移（合法请求互比）。
    with tempfile.TemporaryDirectory() as d:
        import shutil
        a = os.path.join(d, 'a.bin')
        b = os.path.join(d, 'b.bin')
        shutil.copy(req, a)
        raw = bytearray(open(req, 'rb').read())
        raw[-1] ^= 0x01
        open(b, 'wb').write(bytes(raw))
        r = run('compare', '--native', a, '--wire', b)
        check('compare diff exit 1', r.returncode == 1, r.stdout[:200])
        check('compare diff offset last', f'"first_diff_offset": {len(raw) - 1}' in r.stdout, r.stdout[:200])

    # compare 长度不同 → exit 1（合法请求截断）。
    with tempfile.TemporaryDirectory() as d:
        import shutil
        a = os.path.join(d, 'a.bin')
        b = os.path.join(d, 'b.bin')
        shutil.copy(req, a)
        raw = open(req, 'rb').read()
        # 截断整子包（保持帧头声明长度>实际 → cut 失败）或去尾 1B。
        open(b, 'wb').write(raw[:-1])
        r = run('compare', '--native', a, '--wire', b)
        check('compare len exit 1', r.returncode == 1, r.stdout[:200])

    # compare 双损坏请求 → exit 1（不得比较成功）。
    with tempfile.TemporaryDirectory() as d:
        a = os.path.join(d, 'a.bin')
        b = os.path.join(d, 'b.bin')
        open(a, 'wb').write(b'\x00' * 20)
        open(b, 'wb').write(b'\x00' * 20)
        r = run('compare', '--native', a, '--wire', b)
        check('compare bad-bad exit 1', r.returncode == 1, r.stdout[:200])
        check('compare bad-bad no match', '"match": true' not in r.stdout, r.stdout[:200])

    # 真实 type0 fixture：自比较一致 + scan 子包闭合。
    if os.path.isfile(req):
        r = run('compare', '--native', req, '--wire', req)
        check('fixture self-compare', r.returncode == 0, r.stdout[:200])
        with tempfile.TemporaryDirectory() as d:
            # scan 单帧目录 → count=4 四槽。
            import shutil
            shutil.copy(req, os.path.join(d, 'req.bin'))
            r = run('scan', d)
            check('scan type0 count 4', '"count": 4' in r.stdout, r.stdout[:300])
    else:
        check('fixture self-compare', False, 'missing fixture')

    # scan 截断帧 → 非零退出。
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, 'bad.bin'), 'wb').write(b'\xa0\xa0\xa0\xa0\x00\x01\x21\x01\x00\x10\x00')
        r = run('scan', d)
        check('scan truncated exit 1', r.returncode == 1, r.stdout[:200])

    # scan 错误 magic → 非零退出。
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, 'bad.bin'), 'wb').write(b'\x00' * 20)
        r = run('scan', d)
        check('scan bad magic exit 1', r.returncode == 1, r.stdout[:200])

    # scan 空目录 → 非零退出。
    with tempfile.TemporaryDirectory() as d:
        r = run('scan', d)
        check('scan empty exit 1', r.returncode == 1, r.stdout[:200])

    # scan 额外尾部 → 非零退出（严格闭合）。
    with tempfile.TemporaryDirectory() as d:
        # 合法 4 槽请求 + 1 尾字节。
        import shutil
        shutil.copy(req, os.path.join(d, 'req.bin'))
        with open(os.path.join(d, 'req.bin'), 'ab') as f:
            f.write(b'\x00')
        r = run('scan', d)
        check('scan trailing exit 1', r.returncode == 1, r.stdout[:300])

    # scan 非法 count（0）→ 非零退出。
    with tempfile.TemporaryDirectory() as d:
        # magic + origin + 0x2101 + len=2 + count=0。
        open(os.path.join(d, 'bad.bin'), 'wb').write(
            b'\xa0\xa0\xa0\xa0\x00\x01\x21\x01\x00\x02\x00\x00')
        r = run('scan', d)
        check('scan bad count exit 1', r.returncode == 1, r.stdout[:300])

    # scan 非法 size（<28）→ 非零退出。
    with tempfile.TemporaryDirectory() as d:
        # count=1 + size=10（<28）+ 8B 填充。
        open(os.path.join(d, 'bad.bin'), 'wb').write(
            b'\xa0\xa0\xa0\xa0\x00\x01\x21\x01\x00\x0c\x00\x01\x00\x0a' + b'\x00' * 8)
        r = run('scan', d)
        check('scan bad size exit 1', r.returncode == 1, r.stdout[:300])

    print(f'\n{PASS} passed, {FAIL} failed')
    return 1 if FAIL else 0


if __name__ == '__main__':
    sys.exit(main())
