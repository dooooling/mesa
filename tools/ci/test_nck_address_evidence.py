#!/usr/bin/env python3
"""nck_address_evidence.py 自测（Workstream A；无第三方依赖、离线、CI quality 内可跑）。

运行：python3 tools/ci/test_nck_address_evidence.py
失败即 CI FAIL——校验器本身回归不得合入。

为什么必须有负向测试：这份校验器的唯一价值是「挡人」。一个只会输出 OK 的
校验器与没有校验器等价。因此六条规则**每一条**都要有「注入缺陷 → 必须 FAIL」
的用例，并且断言失败原因落在预期的规则号/错误码上（不是碰巧失败）。

覆盖：
  R1 证据出处      —— 删 sample_ref、把 sample_lines 指错行、删 golden/probe 出处、
                      手册文档缺 document_number、附加观测行号错
  R2 字段自相矛盾  —— module 与字节不符、module_hex 与 decimal 不符、decoded 与
                      raw 不符、重复 block、附加观测 module 冲突
  R3 语义越界      —— proves_module=true、自由文本「手册证明 module」、
                      手册提供 module 数值、module_from_manual 字段
  R4 三类证据混用  —— module 塞进 manual_semantic、证据类标签错、raw 塞进
                      wire_layout、缺证据类登记、module 出处类与行证据类不同
  R5 基线完整性    —— r1 blob 篡改、声称真机已验证（status/键/文本）、
                      历史观测标记被抹、删 hardware-validation、flag 非布尔
  R6 unresolved    —— 已解决却未移除、行引用幽灵 id、行漏声明必需未解决项、
                      非法状态
  另加：真实矩阵必须 OK（exit 0），WARN 场景退出码 2，FAIL 场景退出码 1。
"""
import copy
import json
import os
import sys

# Windows 控制台默认 GBK 编码：本脚本的断言标签含 `⇒` 与中文，直接 print 会抛
# UnicodeEncodeError（不是测试失败，却会让 CI 以退出码 1 结束）。这里把 stdout
# 显式切到 UTF-8，保留逐行 PASS/FAIL 报告可读性；断言逻辑不受影响。
try:
    sys.stdout.reconfigure(encoding='utf-8')
except (AttributeError, OSError):  # 极老 Python 或已重定向到不可重配置的流
    pass

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, '..', '..'))
sys.path.insert(0, HERE)

import nck_address_evidence as tool  # noqa: E402  （自测直接复用同一套规则实现）

PASS = 0
FAIL = 0
MATRIX = os.path.join(REPO, tool.DEFAULT_MATRIX)


def check(tag, cond, extra=''):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f'PASS {tag}')
    else:
        FAIL += 1
        print(f'FAIL {tag} {extra}')


def base_doc():
    with open(MATRIX, encoding='utf-8') as f:
        return json.load(f)


def run(doc):
    return tool.validate_document(copy.deepcopy(doc), repo=REPO)


def codes(report):
    return {(it['rule'], it['code']) for it in report.items}


def expect(tag, mutate, rule, code):
    """注入缺陷 → 必须 FAIL，且命中预期 (rule, code)。"""
    doc = base_doc()
    mutate(doc)
    rep = run(doc)
    hit = (rule, code) in codes(rep)
    msgs = [f"{it['rule']}/{it['code']}" for it in rep.items if it['severity'] == 'FAIL']
    check(f'{tag} → {rule}/{code} 且 FAIL', hit and rep.verdict() == 'FAIL',
          f'verdict={rep.verdict()} 实际 [{", ".join(msgs)}]')


def row(doc, block):
    for r in doc['block_observations']:
        if r['block'] == block:
            return r
    raise KeyError(block)


# ---------------------------------------------------------------------------
# 0. 真实矩阵：必须 OK
# ---------------------------------------------------------------------------

def test_real_matrix_ok():
    rep = run(base_doc())
    fails = [it for it in rep.items if it['severity'] == 'FAIL']
    check('真实矩阵无 FAIL', not fails, f'{fails}')
    check('真实矩阵结论 OK', rep.verdict() == 'OK', rep.verdict())
    infos = [it['code'] for it in rep.items if it['severity'] == 'INFO']
    check('真实矩阵记录 r1 blob 已核对', 'r1-blob-verified' in infos, infos)


# ---------------------------------------------------------------------------
# R1 证据出处
# ---------------------------------------------------------------------------

def test_r1():
    expect('R1 删 sample_ref', lambda d: row(d, 'S').pop('sample_ref'),
           'R1', 'sample-ref-missing')
    # 历史教训复现：行号每行都写得到，但指到「看起来像」的行时没有任何校验能发现，
    # 只有「该行必须逐字包含声明的字节」才能抓住。
    expect('R1 sample_lines 指向错误行', lambda d: row(d, 'S').update(sample_lines=[7]),
           'R1', 'sample-line-bytes-mismatch')
    expect('R1 删 golden_test 出处',
           lambda d: row(d, 'RP')['device_observation'].pop('golden_test'),
           'R1', 'golden_test-missing')
    expect('R1 删 probe_test 出处',
           lambda d: row(d, 'RP')['device_observation'].pop('probe_test'),
           'R1', 'probe_test-missing')
    expect('R1 手册文档缺 document_number',
           lambda d: row(d, 'SSP')['manual_semantic']['document'].pop('document_number'),
           'R1', 'manual-document-document_number-missing')
    expect('R1 手册 sha256 非法',
           lambda d: row(d, 'SSP')['manual_semantic']['document'].update(sha256='deadbeef'),
           'R1', 'manual-document-sha256-invalid')
    expect('R1 附加观测行号错',
           lambda d: row(d, 'SGA')['additional_observations'][0].update(sample_line=99),
           'R1', 'additional-observation-bytes-mismatch')
    expect('R1 删 module 值出处',
           lambda d: row(d, 'SEGA')['module_value_source'].pop('ref'),
           'R1', 'module-value-source-missing')
    expect('R1 协议布局仓内来源不存在',
           lambda d: row(d, 'S').update(
               protocol_layout={'evidence_class': 'protocol_layout',
                                'source_symbol': 'x', 'repo_source': 'src/nope.rs'}),
           'R1', 'layout-repo-source-not-found')
    expect('R1 删 claim_scope', lambda d: row(d, 'S').pop('claim_scope'),
           'R1', 'claim-scope-missing')


# ---------------------------------------------------------------------------
# R2 字段自相矛盾
# ---------------------------------------------------------------------------

def test_r2():
    expect('R2 module 与字节不符',
           lambda d: row(d, 'S').update(module_decimal=126, module_hex='0x7e'),
           'R2', 'module-contradicts-raw')
    expect('R2 module_hex 与 decimal 不符',
           lambda d: row(d, 'RP').update(module_hex='0x16'),
           'R2', 'module-hex-contradicts-decimal')
    expect('R2 decoded.column 与字节不符',
           lambda d: row(d, 'SGA')['device_observation']['decoded'].update(column=12),
           'R2', 'decoded-contradicts-raw')
    expect('R2 decoded.line_count 与字节不符',
           lambda d: row(d, 'SGA')['device_observation']['decoded'].update(line_count=2),
           'R2', 'decoded-contradicts-raw')
    expect('R2 decoded.area_no 与字节不符',
           lambda d: row(d, 'SEGA')['device_observation']['decoded'].update(area_no=2),
           'R2', 'decoded-area-no-mismatch')

    def dup_conflict(d):
        clone = copy.deepcopy(row(d, 'SSP'))
        clone['block'] = 'SPARPP'
        clone['module_decimal'] = 122
        clone['module_hex'] = '0x7a'
        clone['device_observation']['decoded']['module'] = 122
        clone['raw_request_example'] = '12 08 82 41 00 05 00 01 7a 01'
        clone['sample_lines'] = [34]
        d['block_observations'].append(clone)
        # 同一 block 再出现一次（不同 module）：必须被判定为跨行矛盾。
        again = copy.deepcopy(clone)
        again['module_decimal'] = 121
        again['module_hex'] = '0x79'
        d['block_observations'].append(again)

    expect('R2 同一 block 跨行不同 module', dup_conflict, 'R2', 'module-conflict-across-rows')

    def obs_module_conflict(d):
        # 把 S 行的附加观测指向 SEGA 的第 25 行（module 112）：行存在、字节也在，
        # 但 module 与 S 的 127 冲突 —— 这正是「看着像同一 block 其实不是」的情况。
        s = row(d, 'S')
        s['additional_observations'].append({
            'variable': 'actProgPosBKS (axis 1)',
            'sample_line': 25,
            'area': 'C',
            'area_no': 1,
            'column': 33,
            'line': 1,
            'line_count': 1,
            'raw_request': '12 08 82 41 00 21 00 01 70 01',
        })

    expect('R2 附加观测 module 与行冲突', obs_module_conflict,
           'R2', 'module-conflict-across-observations')

    def bad_layout(d):
        d['wire_layout']['format']['fields'][4]['width'] = 1

    expect('R2 布局字段宽度与总长不符', bad_layout, 'R2', 'layout-not-contiguous')


# ---------------------------------------------------------------------------
# R3 禁止语义越界
# ---------------------------------------------------------------------------

def test_r3():
    expect('R3 proves_module=true',
           lambda d: row(d, 'S')['manual_semantic'].update(proves_module=True),
           'R3', 'proves-module-not-false')
    expect('R3 手册提供 module 数值',
           lambda d: d['manual_mapping']['result'].update(native_module_values_provided=5),
           'R3', 'manual-provides-module-values')
    expect('R3 手册结论被改写成可证明',
           lambda d: d['manual_mapping']['result'].update(conclusion='manual_proves_module'),
           'R3', 'manual-conclusion-invalid')
    expect('R3 自由文本声称手册证明 module',
           lambda d: row(d, 'SSP').update(
               claim_scope='The manual entry name and column match proves the native module value 114.'),
           'R3', 'semantic-overreach-text')
    expect('R3 中文化语义越界文本',
           lambda d: row(d, 'SGA').update(
               claim_scope='手册条目语义一致即可证明 native module 值等于 115。'),
           'R3', 'semantic-overreach-text')
    expect('R3 结构化字段 module_from_manual',
           lambda d: row(d, 'RP').update(module_from_manual=21),
           'R3', 'semantic-overreach-key')
    # 反向保证：合法的否定表述（invariants / caveats）不得被误判为越界。
    doc = base_doc()
    doc['invariants'].append(
        'A manual entry-name match never proves a native module value; only device '
        'observation or an authoritative specification may do so.')
    rep = run(doc)
    check('R3 否定表述不误判', rep.verdict() == 'OK',
          [f"{it['rule']}/{it['code']}" for it in rep.items if it['severity'] == 'FAIL'])


# ---------------------------------------------------------------------------
# R4 三类证据不得混用
# ---------------------------------------------------------------------------

def test_r4():
    expect('R4 module 值塞进 manual_semantic',
           lambda d: row(d, 'S')['manual_semantic'].update(module_decimal=127),
           'R4', 'manual-carries-module-value')
    expect('R4 module 值塞进 protocol_layout',
           lambda d: row(d, 'S')['protocol_layout'].update(module=127),
           'R4', 'layout-carries-module-value')
    expect('R4 行证据类标签被改',
           lambda d: row(d, 'S')['manual_semantic'].update(evidence_class='device_observation'),
           'R4', 'row-manual_semantic-class-mismatch')
    expect('R4 出现未知证据类',
           lambda d: row(d, 'S')['device_observation'].update(evidence_class='observation'),
           'R4', 'unknown-evidence-class')
    expect('R4 raw 字节塞进 wire_layout',
           lambda d: d['wire_layout'].update(raw_request_example='12 08 82 41 00 0d 00 01 7f 01'),
           'R4', 'container-carries-foreign-evidence')
    expect('R4 device 上下文塞进 manual_mapping',
           lambda d: d['manual_mapping'].update(device={'model': 'SINUMERIK 828D'}),
           'R4', 'container-carries-foreign-evidence')
    expect('R4 证据类登记表缺一类',
           lambda d: d['evidence_classes'].pop('manual_semantic'),
           'R4', 'evidence-classes-invalid')
    expect('R4 module 出处类与行证据类不同',
           lambda d: row(d, 'SEGA')['module_value_source'].update(evidence_class='manual_semantic'),
           'R4', 'module-value-source-class-mismatch')
    expect('R4 行 evidence_class 不是 device_observation',
           lambda d: row(d, 'SGA').update(evidence_class='protocol_layout'),
           'R4', 'row-class-mismatch')


# ---------------------------------------------------------------------------
# R5 基线完整性
# ---------------------------------------------------------------------------

def test_r5():
    expect('R5 r1 blob 被改',
           lambda d: d['baseline'].update(r1_blob_sha1='0' * 40),
           'R5', 'r1-blob-mismatch')
    expect('R5 r1 路径不存在',
           lambda d: d['baseline'].update(r1_path='drivers/sinumerik-nck/definitions/828d/nope.json'),
           'R5', 'r1-file-missing')
    expect('R5 main_commit 非法',
           lambda d: d['baseline'].update(main_commit='main'),
           'R5', 'baseline-main-commit-invalid')
    expect('R5 flag 非布尔',
           lambda d: d['baseline'].update(real_828d_test_in_this_change='false'),
           'R5', 'real-device-flag-invalid')
    expect('R5 行 status 声称已验证',
           lambda d: row(d, 'S').update(status='verified'),
           'R5', 'row-status-claims-verification')
    expect('R5 confidence_class 声称已验证',
           lambda d: row(d, 'RP').update(confidence_class='hardware_verified'),
           'R5', 'row-confidence-claims-verification')
    expect('R5 布尔键声称真机已验证',
           lambda d: row(d, 'SSP').update(hardware_verified=True),
           'R5', 'claimed-hardware-verification')
    expect('R5 文本声称本变更已验证真机',
           lambda d: row(d, 'SGA').update(
               claim_scope='Verified on the real 828D in this change, see probe.'),
           'R5', 'claimed-hardware-verification-text')
    expect('R5 历史观测标记被抹',
           lambda d: row(d, 'SEGA')['device_observation'].update(observation_is_historical=False),
           'R5', 'observation-not-marked-historical')

    def drop_hw_unresolved(d):
        d['unresolved'] = [u for u in d['unresolved'] if u['id'] != 'hardware-validation']
        for r in d['block_observations']:
            r['unresolved'] = [x for x in r['unresolved'] if x != 'hardware-validation']

    expect('R5 删掉 hardware-validation 未解决项', drop_hw_unresolved,
           'R5', 'hardware-validation-unresolved-removed')


# ---------------------------------------------------------------------------
# R6 unresolved 必须真实存在
# ---------------------------------------------------------------------------

def test_r6():
    def resolve_module_authority(d):
        # 假装「找到权威规格」但不移除 unresolved 条目：过期必须被抓。
        row(d, 'S')['module_value_source'].update(
            evidence_class='authoritative_spec', authoritative_spec='some spec 1.0')

    expect('R6 已解决却仍列在 unresolved', resolve_module_authority,
           'R6', 'unresolved-stale')

    def pages_filled(d):
        # 补全了所有 page ⇒ manual-entry-page-traceability 已解决，条目必须移除。
        for r in d['block_observations']:
            r['manual_semantic']['page'] = 'p.42'
    expect('R6 页码补齐后条目过期', pages_filled, 'R6', 'unresolved-stale')

    def pending_review_cleared(d):
        res = d['manual_mapping']['result']
        res['pending_review'] = []
        res['exact_semantic_matches'] = res['required_entries']
    expect('R6 手册待复核清空后条目过期', pending_review_cleared, 'R6', 'unresolved-stale')

    def phantom(d):
        row(d, 'S')['unresolved'].append('module-value-solved-by-nobody')
    expect('R6 行引用幽灵 unresolved id', phantom, 'R6', 'row-unresolved-phantom')

    def incomplete(d):
        row(d, 'S')['unresolved'] = [x for x in row(d, 'S')['unresolved']
                                     if x != 'area-no-mapping']
    expect('R6 行漏声明 area-no-mapping', incomplete, 'R6', 'row-unresolved-incomplete')

    def bad_status(d):
        d['unresolved'][0]['status'] = 'resolved'
    expect('R6 unresolved 状态非法', bad_status, 'R6', 'unresolved-status-invalid')

    def no_question(d):
        d['unresolved'][0].pop('question')
    expect('R6 unresolved 缺 question', no_question, 'R6', 'unresolved-question-missing')

    def duplicate(d):
        d['unresolved'].append(copy.deepcopy(d['unresolved'][0]))
    expect('R6 unresolved id 重复', duplicate, 'R6', 'unresolved-duplicate')


# ---------------------------------------------------------------------------
# 退出码 / CLI 行为
# ---------------------------------------------------------------------------

def test_exit_codes():
    import tempfile

    # FAIL → 1
    doc = base_doc()
    row(doc, 'S').pop('sample_ref')
    with tempfile.TemporaryDirectory() as tmp:
        p = os.path.join(tmp, 'bad.json')
        with open(p, 'w', encoding='utf-8') as f:
            json.dump(doc, f)
        rc = tool.main(['--matrix', p, '--repo', REPO])
        check('CLI FAIL 退出码 1', rc == 1, rc)

    # WARN → 2（--allow-warn 时 0）
    # 删掉 device-software-version 登记但保留空 SW 版本 ⇒ 仅 WARN。
    doc = base_doc()
    doc['unresolved'] = [u for u in doc['unresolved'] if u['id'] != 'device-software-version']
    for r in doc['block_observations']:
        r['unresolved'] = [x for x in r['unresolved'] if x != 'device-software-version']
    rep = run(doc)
    check('删 SW 版本登记 ⇒ WARN', rep.verdict() == 'WARN',
          [f"{it['rule']}/{it['code']}" for it in rep.items if it['severity'] != 'INFO'])
    with tempfile.TemporaryDirectory() as tmp:
        p = os.path.join(tmp, 'warn.json')
        with open(p, 'w', encoding='utf-8') as f:
            json.dump(doc, f)
        rc = tool.main(['--matrix', p, '--repo', REPO])
        check('CLI WARN 退出码 2', rc == 2, rc)
        rc = tool.main(['--matrix', p, '--repo', REPO, '--allow-warn'])
        check('CLI WARN + --allow-warn 退出码 0', rc == 0, rc)

    # 真实矩阵 → 0
    rc = tool.main(['--repo', REPO])
    check('CLI 真实矩阵退出码 0', rc == 0, rc)

    # 矩阵文件不存在 → 1（不得静默通过）
    rc = tool.main(['--matrix', 'docs/evidence/nope.json', '--repo', REPO])
    check('CLI 矩阵缺失退出码 1', rc == 1, rc)

    # 矩阵不是合法 JSON → 1
    with tempfile.TemporaryDirectory() as tmp:
        p = os.path.join(tmp, 'broken.json')
        with open(p, 'w', encoding='utf-8') as f:
            f.write('{"schema_version": 2,')
        rc = tool.main(['--matrix', p, '--repo', REPO])
        check('CLI 非法 JSON 退出码 1', rc == 1, rc)


def main():
    test_real_matrix_ok()
    test_r1()
    test_r2()
    test_r3()
    test_r4()
    test_r5()
    test_r6()
    test_exit_codes()
    print()
    print(f'PASS={PASS} FAIL={FAIL}')
    if FAIL:
        print('RESULT: FAIL')
        return 1
    print('RESULT: OK')
    return 0


if __name__ == '__main__':
    sys.exit(main())
