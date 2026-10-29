#!/usr/bin/env python3
"""check_definition_meta.py 自测（纯标准库、离线、CI quality 内可跑）。

运行：python3 tools/ci/test_check_definition_meta.py
失败即 CI FAIL——校验器本身回归不得合入。

为什么必须有负向测试：这份校验器的唯一价值是「挡人」。一个只会输出 OK 的
校验器与没有校验器等价。因此 M1–M8 **每条规则**都要有「注入缺陷 → 必须 FAIL」
的用例，并且断言失败原因落在预期的规则号 / 错误码上（不是碰巧失败）。

覆盖分布（共 62 项）
--------------------
  M1 定义↔条目双向一致（10）—— 遗漏 / 孤儿 / 重复 / 非数组 / 空数组 / 非对象条目 /
                               variable_id 类型 / family 不符 / 顺序漂移(WARN) / 修复后 OK
  M2 逐条一致（6）          —— revision 不符 / 缺失 / 字符串 / 0 / 修复后 OK
  M3 引用真解析（12）       —— 指针键不存在 / L<n> 越界 / entry:<id> 查无此 id /
                               文件不存在 / 无 fragment / 值不在引用处 / 页码不在条目里 /
                               条目缺 page 列表 / source_ref 缺失 / 修复后 OK
  M4 绑定摘要（8）          —— blob 改一位 / binds_to 缺失 / 摘要格式非法 / 定义文件不存在 /
                               定义文件指错 / 无 git 时纯 Python 兜底
  M5 状态与依据（8）        —— 非法 state / 缺失 / 类型错 / hold 缺 reason / hold basis 空 /
                               basis 枚举外 / 修复后 OK
  M6 verified≠ready（6）    —— ready 无证据 / 证据为 null / basis 缺 verified /
                               candidate 判 ready / basis 与 state 不一致 / 修复后 OK
  M7 ready 证据形状（6）    —— 缺 environment / 缺 controller / 缺 cnc_software /
                               缺 verified_at / artifact_ref 不可解析 / scope 非锁定值
  M8 方式组枚举（6）        —— 改 candidate / 改 ready / basis 缺项 / 条目被删 / 修复后 OK
  另加 5 项 CLI 退出码：FAIL→1 / WARN→2 / --allow-warn→0 / sidecar 缺失→1 / 非法 JSON→1。
"""
import contextlib
import copy
import io
import json
import os
import sys
import tempfile

# Windows 控制台默认 GBK 编码：本脚本的断言标签含中文，直接 print 会抛
# UnicodeEncodeError（不是测试失败，却会让 CI 以退出码 1 结束）。
try:
    sys.stdout.reconfigure(encoding='utf-8')
except (AttributeError, OSError):  # 极老 Python 或已重定向到不可重配置的流
    pass

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, '..', '..'))
sys.path.insert(0, HERE)

import check_definition_meta as tool  # noqa: E402  （自测直接复用同一套规则实现）

PASS = 0
FAIL = 0
SIDECAR = os.path.join(REPO, tool.DEFAULT_SIDECAR)
# 每规则应有的用例数（与文件头注释一致；总 62）
EXPECTED = {'M1': 10, 'M2': 6, 'M3': 12, 'M4': 8, 'M5': 8, 'M6': 6, 'M7': 6, 'M8': 6}
SEEN = {k: 0 for k in EXPECTED}


def check(tag, cond, extra='', rule=None):
    global PASS, FAIL
    if rule:
        SEEN[rule] += 1
    if cond:
        PASS += 1
        print(f'PASS {tag}')
    else:
        FAIL += 1
        print(f'FAIL {tag} {extra}')


def base_doc():
    with open(SIDECAR, encoding='utf-8') as f:
        return json.load(f)


def run(doc):
    return tool.validate_document(copy.deepcopy(doc), repo=REPO, sidecar_path=SIDECAR)


def codes(report):
    return {(it['rule'], it['code']) for it in report.items}


def fails_of(report, rule):
    return [it for it in report.items if it['rule'] == rule and it['severity'] in ('FAIL', 'WARN')]


def expect(tag, mutate, rule, code, rule_no=None):
    """注入缺陷 → 必须 FAIL，且命中预期 (rule, code)。"""
    doc = base_doc()
    mutate(doc)
    rep = run(doc)
    hit = (rule, code) in codes(rep)
    msgs = [f"{it['rule']}/{it['code']}" for it in rep.items if it['severity'] == 'FAIL']
    check(f'{tag} → {rule}/{code} 且 FAIL', hit and rep.verdict() == 'FAIL',
          f'verdict={rep.verdict()} 实际 [{", ".join(msgs)}]', rule=rule_no or rule)


def entry(doc, vid):
    for e in doc['entries']:
        if e.get('variable_id') == vid:
            return e
    raise KeyError(vid)


def ready_evidence():
    """合法的 device_read_evidence（M6/M7 的正向夹具）。"""
    return {
        'environment': {
            'controller': 'SINUMERIK 828D',
            'cnc_software': 'CNC software, version 5.25',
            'verified_at': '2026-10-13',
            'artifact_ref': 'docs/evidence/nck-live-identity-probe.txt#L52',
        },
        'scope': 'definition_level_only',
    }


def make_ready(doc, vid, basis=('device_read_verified',), evidence=True):
    e = entry(doc, vid)
    e['availability'] = {'state': 'ready', 'basis': list(basis), 'reason_zh': '自测夹具：刻意判 ready'}
    if evidence is True:
        e['device_read_evidence'] = ready_evidence()
    elif evidence is not None:
        e['device_read_evidence'] = evidence
    return e


# ---------------------------------------------------------------------------
# 0. 真实 sidecar：每条规则都必须干净
# ---------------------------------------------------------------------------

def test_real_sidecar():
    doc = base_doc()
    rep = run(doc)
    for rule in EXPECTED:
        check(f'真实 sidecar 在 {rule} 上无 FAIL/WARN', not fails_of(rep, rule),
              f'{fails_of(rep, rule)}')
    infos = [it['code'] for it in rep.items if it['severity'] == 'INFO']
    check('真实 sidecar 记录 r1 blob 已核对', 'r1-blob-verified' in infos, infos)
    check('真实 sidecar 结论 OK', rep.verdict() == 'OK', rep.verdict())


# ---------------------------------------------------------------------------
# M1 定义↔条目双向一致
# ---------------------------------------------------------------------------

def test_m1():
    expect('M1 删一条（遗漏）', lambda d: d['entries'].pop(0), 'M1', 'definition-missing')
    expect('M1 加一条孤儿',
           lambda d: d['entries'].append({'variable_id': 'ghost.variable', 'revision': 1,
                                          'unit': {'value': None, 'source_ref': None, 'status': 'unknown'},
                                          'manual_location': {}, 'index_rule': {}, 'availability': {}}),
           'M1', 'entry-orphan')
    expect('M1 重复条目', lambda d: d['entries'].append(copy.deepcopy(d['entries'][0])),
           'M1', 'entry-duplicate')
    expect('M1 entries 不是数组', lambda d: d.update(entries={}), 'M1', 'entries-invalid')
    expect('M1 entries 为空数组', lambda d: d.update(entries=[]), 'M1', 'definition-missing')
    expect('M1 条目不是对象', lambda d: d['entries'].insert(0, 'not-an-object'), 'M1', 'entry-not-object')
    expect('M1 variable_id 类型错误', lambda d: d['entries'][0].update(variable_id=123),
           'M1', 'entry-variable-id-invalid')
    expect('M1 family 与 r1.json 不符', lambda d: d.update(family='840d-sl'), 'M1', 'family-mismatch')

    # 顺序漂移：WARN（不是 FAIL），且「换回来」必须变 OK —— 这是 M1 的「修好后变 OK」。
    doc = base_doc()
    doc['entries'][0], doc['entries'][1] = doc['entries'][1], doc['entries'][0]
    rep = run(doc)
    warn = ('M1', 'entry-order-drift') in codes(rep)
    check('M1 条目顺序漂移 → WARN 而非 FAIL',
          warn and rep.verdict() == 'WARN', f'verdict={rep.verdict()} items={codes(rep)}', rule='M1')
    doc['entries'][0], doc['entries'][1] = doc['entries'][1], doc['entries'][0]
    rep2 = run(doc)
    check('M1 顺序恢复后 M1 无发现', not fails_of(rep2, 'M1') and rep2.verdict() == 'OK',
          f'verdict={rep2.verdict()} items={fails_of(rep2, "M1")}', rule='M1')


# ---------------------------------------------------------------------------
# M2 逐条一致
# ---------------------------------------------------------------------------

def test_m2():
    expect('M2 revision 与 r1.json 不符',
           lambda d: d['entries'][0].update(revision=2), 'M2', 'revision-mismatch')
    expect('M2 revision 缺失', lambda d: d['entries'][0].pop('revision'), 'M2', 'revision-invalid')
    expect('M2 revision 是字符串', lambda d: d['entries'][0].update(revision='1'),
           'M2', 'revision-invalid')
    expect('M2 revision 为 0', lambda d: d['entries'][0].update(revision=0), 'M2', 'revision-invalid')
    expect('M2 revision 为布尔', lambda d: d['entries'][0].update(revision=True),
           'M2', 'revision-invalid')

    doc = base_doc()
    doc['entries'][0]['revision'] = 9
    run(doc)
    doc['entries'][0]['revision'] = 1
    rep = run(doc)
    check('M2 revision 改回后 M2 无发现', not fails_of(rep, 'M2'), fails_of(rep, 'M2'), rule='M2')


# ---------------------------------------------------------------------------
# M3 unit / page 的引用必须真解析
# ---------------------------------------------------------------------------

XREF = 'docs/reference/828d-manual-cross-reference.json'
TXT = 'docs/evidence/nck-live-identity-probe.txt'


def test_m3():
    def unit_of(d, vid):
        return entry(d, vid)['unit']

    expect('M3 unit 填值但 source_ref 缺失',
           lambda d: unit_of(d, 'channel.feed.override').update(source_ref=None),
           'M3', 'unit-source-ref-missing')
    expect('M3 unit source_ref 指向不存在的文件',
           lambda d: unit_of(d, 'channel.feed.override').update(source_ref='docs/reference/nope.json#/x'),
           'M3', 'unit-source-ref-file')
    expect('M3 unit source_ref 没有 fragment',
           lambda d: unit_of(d, 'channel.feed.override').update(source_ref=XREF),
           'M3', 'unit-source-ref-fragment')
    expect('M3 unit JSON Pointer 指向不存在的键',
           lambda d: unit_of(d, 'channel.feed.override').update(source_ref=XREF + '#/entries/8/no_such_key'),
           'M3', 'unit-source-ref-fragment')
    expect('M3 unit L<n> 越界',
           lambda d: unit_of(d, 'channel.feed.override').update(source_ref=TXT + '#L99999'),
           'M3', 'unit-source-ref-fragment')
    expect('M3 unit entry:<id> 查无此 id',
           lambda d: unit_of(d, 'channel.feed.override').update(source_ref=XREF + '#entry:no-such-id'),
           'M3', 'unit-source-ref-fragment')
    # 「引用写得像真的但内容对不上」：指针能解析，但节点里没有声明的单位
    expect('M3 单位值不在被引用节点里',
           lambda d: unit_of(d, 'channel.feed.override').update(
               value='mm/min', source_ref=XREF + '#/entries/8/cross_reference_202503/manual_stated_address'),
           'M3', 'unit-value-not-in-source')

    expect('M3 页码不在交叉引用条目里',
           lambda d: entry(d, 'channel.feed.actual')['manual_location'].update(page=9999),
           'M3', 'page-not-found')
    expect('M3 page 非 null 但 source_ref 缺失',
           lambda d: entry(d, 'channel.feed.actual')['manual_location'].update(source_ref=None),
           'M3', 'manual-location-source-ref-missing')
    expect('M3 引用的条目没有 page 列表',
           lambda d: entry(d, 'channel.feed.actual')['manual_location'].update(
               source_ref=XREF + '#/entries/8/cross_reference_202503/manual_stated_address'),
           'M3', 'page-list-missing')
    expect('M3 manual_location 不是对象',
           lambda d: entry(d, 'channel.feed.actual').update(manual_location='5.3.16'),
           'M3', 'manual-location-not-object')

    # 修好后变 OK：换成完全合法的 unit + page 引用
    doc = base_doc()
    e = entry(doc, 'channel.feed.actual')
    e['unit'] = {'value': 'mm/min, inch/min, user defined',
                 'source_ref': XREF + '#/entries/8/cross_reference_202503/quoted_excerpt',
                 'status': 'verified'}
    e['manual_location']['page'] = 272
    e['manual_location']['source_ref'] = XREF + '#/entries/8/cross_reference_202503'
    rep = run(doc)
    check('M3 合法引用后 M3 无发现', not fails_of(rep, 'M3'), fails_of(rep, 'M3'), rule='M3')


# ---------------------------------------------------------------------------
# M4 绑定摘要
# ---------------------------------------------------------------------------

def test_m4():
    expect('M4 blob 改一位',
           lambda d: d['binds_to'].update(definition_blob_sha1='0' + tool.R1_FROZEN_BLOB[1:]),
           'M4', 'blob-mismatch')
    expect('M4 binds_to 缺失', lambda d: d.pop('binds_to'), 'M4', 'binds-to-missing')
    expect('M4 binds_to 不是对象', lambda d: d.update(binds_to='r1.json'), 'M4', 'binds-to-missing')
    expect('M4 definition_blob_sha1 缺失',
           lambda d: d['binds_to'].pop('definition_blob_sha1'), 'M4', 'blob-sha-invalid')
    expect('M4 blob 大写',
           lambda d: d['binds_to'].update(definition_blob_sha1=tool.R1_FROZEN_BLOB.upper()),
           'M4', 'blob-sha-invalid')
    expect('M4 definition_file 指向不存在的文件',
           lambda d: d['binds_to'].update(definition_file='r404.json'),
           'M4', 'definition-file-not-found')
    expect('M4 definition_file 指向 DIGESTS.lock',
           lambda d: d['binds_to'].update(definition_file='DIGESTS.lock'),
           'M4', 'definition-file-unexpected')

    # 无 git 时的纯 Python blob 兜底：合法数据仍 OK，篡改仍被抓
    real = tool.git_blob_sha
    tool.git_blob_sha = lambda repo, rel: (None, '自测注入：git 不可用')
    try:
        rep = run(base_doc())
        ok = rep.verdict() == 'OK' and ('M4', 'blob-measured-without-git') in codes(rep)
        doc = base_doc()
        doc['binds_to']['definition_blob_sha1'] = 'f' * 40
        rep2 = run(doc)
        caught = ('M4', 'blob-mismatch') in codes(rep2) and rep2.verdict() == 'FAIL'
    finally:
        tool.git_blob_sha = real
    check('M4 无 git 时纯 Python blob 兜底：合法 OK 且篡改仍 FAIL', ok and caught,
          f'ok={ok} caught={caught}', rule='M4')


# ---------------------------------------------------------------------------
# M5 state / basis / reason_zh
# ---------------------------------------------------------------------------

def test_m5():
    expect('M5 state 非法取值',
           lambda d: entry(d, 'channel.feed.actual')['availability'].update(state='verified'),
           'M5', 'state-invalid')
    expect('M5 state 缺失',
           lambda d: entry(d, 'channel.feed.actual')['availability'].pop('state'),
           'M5', 'state-missing')
    expect('M5 state 类型错误',
           lambda d: entry(d, 'channel.feed.actual')['availability'].update(state=1),
           'M5', 'state-missing')
    expect('M5 hold 缺 reason_zh',
           lambda d: entry(d, 'channel.program.block')['availability'].pop('reason_zh'),
           'M5', 'hold-without-reason')
    expect('M5 hold reason_zh 为空白',
           lambda d: entry(d, 'channel.program.block')['availability'].update(reason_zh='   '),
           'M5', 'hold-without-reason')
    expect('M5 hold basis 为空数组',
           lambda d: entry(d, 'channel.program.block')['availability'].update(basis=[]),
           'M5', 'basis-empty')
    expect('M5 basis 含枚举外的值',
           lambda d: entry(d, 'channel.feed.actual')['availability'].update(basis=['looks_fine']),
           'M5', 'basis-unknown')

    doc = base_doc()
    a = entry(doc, 'channel.program.block')['availability']
    a.pop('reason_zh')
    a['basis'] = []
    run(doc)
    a['reason_zh'] = '手册只给 TYPE_STRING，长度未证实'
    a['basis'] = ['string_length_unproven', 'no_device_read']
    rep = run(doc)
    check('M5 补齐 reason_zh/basis 后 M5 无发现', not fails_of(rep, 'M5'), fails_of(rep, 'M5'), rule='M5')


# ---------------------------------------------------------------------------
# M6 semantics_status=verified 不得直接映射为 ready
# ---------------------------------------------------------------------------

def test_m6():
    expect('M6 verified 判 ready 但无 device_read_evidence',
           lambda d: make_ready(d, 'channel.feed.actual', evidence=None),
           'M6', 'ready-without-device-read-evidence')
    expect('M6 verified 判 ready 但证据为 null',
           lambda d: make_ready(d, 'channel.feed.actual', evidence=None).update(device_read_evidence=None),
           'M6', 'ready-without-device-read-evidence')
    expect('M6 verified 判 ready 但 basis 缺 device_read_verified',
           lambda d: make_ready(d, 'channel.feed.actual', basis=('no_device_read',)),
           'M6', 'ready-basis-missing-device-read')
    expect('M6 candidate 语义判 ready（r0033）',
           lambda d: make_ready(d, 'drive.parameter.r0033'),
           'M6', 'ready-without-verified-semantics')

    # basis 声称已验证读取但 state 不是 ready ⇒ 自相矛盾
    doc = base_doc()
    entry(doc, 'channel.feed.actual')['availability']['basis'] = ['device_read_verified']
    rep = run(doc)
    caught = ('M6', 'device-read-basis-without-ready-state') in codes(rep)
    check('M6 basis 声称 device_read_verified 但 state=candidate → FAIL', caught, codes(rep), rule='M6')

    # 修好后变 OK：verified 语义 + 完整证据 + 正确 basis（M6 与 M7 同时干净）
    doc = base_doc()
    make_ready(doc, 'channel.feed.actual')
    rep = run(doc)
    check('M6/M7 verified 语义 + 完整真机证据 ⇒ ready 通过',
          not fails_of(rep, 'M6') and not fails_of(rep, 'M7'),
          fails_of(rep, 'M6') + fails_of(rep, 'M7'), rule='M6')


# ---------------------------------------------------------------------------
# M7 ready 的证据形状
# ---------------------------------------------------------------------------

def test_m7():
    expect('M7 ready 缺 environment',
           lambda d: make_ready(d, 'channel.feed.actual').pop('device_read_evidence'),
           'M7', 'ready-evidence-missing')
    expect('M7 environment 缺 controller',
           lambda d: make_ready(d, 'channel.feed.actual')['device_read_evidence']['environment'].pop('controller'),
           'M7', 'ready-environment-controller-invalid')
    expect('M7 environment 缺 cnc_software',
           lambda d: make_ready(d, 'channel.feed.actual')['device_read_evidence']['environment'].pop('cnc_software'),
           'M7', 'ready-environment-cnc_software-invalid')
    expect('M7 environment 缺 verified_at',
           lambda d: make_ready(d, 'channel.feed.actual')['device_read_evidence']['environment'].pop('verified_at'),
           'M7', 'ready-environment-verified_at-invalid')
    expect('M7 artifact_ref 指向不存在的文件',
           lambda d: make_ready(d, 'channel.feed.actual')['device_read_evidence']['environment'].update(
               artifact_ref='docs/evidence/nope.txt#L1'),
           'M7', 'ready-artifact-unresolvable')
    expect('M7 scope 非 definition_level_only',
           lambda d: make_ready(d, 'channel.feed.actual')['device_read_evidence'].update(
               scope='machine_exists'),
           'M7', 'ready-scope-invalid')


# ---------------------------------------------------------------------------
# M8 方式组枚举未证明 ⇒ op_mode 必须 hold
# ---------------------------------------------------------------------------

def test_m8():
    expect('M8 op_mode 改为 candidate',
           lambda d: entry(d, 'mode_group.op_mode')['availability'].update(state='candidate'),
           'M8', 'mode-group-not-hold')
    expect('M8 op_mode 改为 ready（带完整证据）',
           lambda d: make_ready(d, 'mode_group.op_mode'),
           'M8', 'mode-group-not-hold')
    expect('M8 op_mode basis 去掉 mode_group_enumeration_unproven',
           lambda d: entry(d, 'mode_group.op_mode')['availability'].update(basis=['no_device_read']),
           'M8', 'mode-group-basis-missing')
    expect('M8 op_mode 的 availability 不是对象',
           lambda d: entry(d, 'mode_group.op_mode').update(availability='hold'),
           'M8', 'mode-group-availability-missing')
    expect('M8 sidecar 里删掉 op_mode 条目',
           lambda d: d['entries'].remove(entry(d, 'mode_group.op_mode')),
           'M8', 'mode-group-entry-missing')

    doc = base_doc()
    entry(doc, 'mode_group.op_mode')['availability'] = {'state': 'candidate', 'basis': ['no_device_read'],
                                                        'reason_zh': '自测夹具'}
    run(doc)
    entry(doc, 'mode_group.op_mode')['availability'] = {
        'state': 'hold',
        'basis': ['mode_group_enumeration_unproven', 'no_device_read'],
        'reason_zh': '方式组（B 区）枚举未证明，暂缓',
    }
    rep = run(doc)
    check('M8 恢复 hold + basis 后 M8 无发现', not fails_of(rep, 'M8'), fails_of(rep, 'M8'), rule='M8')


# ---------------------------------------------------------------------------
# CLI 退出码（5 项）
# ---------------------------------------------------------------------------

def _run_cli(argv):
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        rc = tool.main(argv)
    return rc, buf.getvalue()


def test_cli():
    tmp = tempfile.mkdtemp(prefix='mesa-meta-selftest-')
    base = base_doc()

    # 1) FAIL → 1
    bad = copy.deepcopy(base)
    bad['entries'].pop(0)
    p_fail = os.path.join(tmp, 'fail.json')
    with open(p_fail, 'w', encoding='utf-8') as f:
        json.dump(bad, f, ensure_ascii=False)
    rc, _ = _run_cli(['--matrix', p_fail, '--repo', REPO])
    check('CLI FAIL 场景退出码 = 1', rc == 1, f'rc={rc}')

    # 2) WARN → 2，3) --allow-warn → 0（同一份顺序漂移夹具）
    warn = copy.deepcopy(base)
    warn['entries'][0], warn['entries'][1] = warn['entries'][1], warn['entries'][0]
    p_warn = os.path.join(tmp, 'warn.json')
    with open(p_warn, 'w', encoding='utf-8') as f:
        json.dump(warn, f, ensure_ascii=False)
    rc_warn, _ = _run_cli(['--matrix', p_warn, '--repo', REPO])
    check('CLI WARN 场景退出码 = 2', rc_warn == 2, f'rc={rc_warn}')
    rc_allow, _ = _run_cli(['--matrix', p_warn, '--repo', REPO, '--allow-warn'])
    check('CLI --allow-warn 时 WARN 退出码 = 0', rc_allow == 0, f'rc={rc_allow}')

    # 4) sidecar 缺失 → 1
    rc_missing, _ = _run_cli(['--matrix', os.path.join(tmp, 'no-such-sidecar.json'), '--repo', REPO])
    check('CLI sidecar 缺失退出码 = 1', rc_missing == 1, f'rc={rc_missing}')

    # 5) 非法 JSON → 1
    p_bad = os.path.join(tmp, 'bad.json')
    with open(p_bad, 'w', encoding='utf-8') as f:
        f.write('{ not json')
    rc_bad, _ = _run_cli(['--matrix', p_bad, '--repo', REPO])
    check('CLI 非法 JSON 退出码 = 1', rc_bad == 1, f'rc={rc_bad}')


def main():
    print('=== check_definition_meta.py 自测（M1–M8 + CLI）===')
    print()
    print('--- 真实 sidecar 基线 ---')
    test_real_sidecar()
    for name, fn in (('M1', test_m1), ('M2', test_m2), ('M3', test_m3), ('M4', test_m4),
                     ('M5', test_m5), ('M6', test_m6), ('M7', test_m7), ('M8', test_m8)):
        print(f'--- {name} ---')
        fn()
    print('--- CLI 退出码 ---')
    cli_before = PASS + FAIL
    test_cli()
    cli_count = PASS + FAIL - cli_before

    print()
    # 基线 1 项（真实 sidecar 结论 OK）不属于任何单条规则的分布，单独计。
    rule_total = sum(SEEN.values())
    ok_dist = all(SEEN[k] == v for k, v in EXPECTED.items())
    print(f'规则用例分布：{SEEN}（期望 {EXPECTED}）→ {"一致" if ok_dist else "不一致"}')
    print(f'规则用例合计 {rule_total}（M1–M8 共 62） + CLI {cli_count}（期望 5）')
    print(f'PASS={PASS} FAIL={FAIL}')
    if FAIL == 0 and ok_dist and rule_total == 62 and cli_count == 5:
        print('RESULT: OK')
        return 0
    print('RESULT: FAIL')
    return 1


if __name__ == '__main__':
    sys.exit(main())
