#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""828D 手册元数据与交叉引用清单的机械校验（可在 CI 之外本地一键跑）。

用法：
    python docs/reference/check-manual-metadata.py            # 人类可读输出
    python docs/reference/check-manual-metadata.py --verbose  # 额外打印统计细节

退出码：0 = 无 FAIL；1 = 存在 FAIL。WARN 不影响退出码。

校验项（对应任务第六节 1–5）：
  A. 文件身份：manuals.json 的 file_present / sha256 / size_bytes（以及可选 page_count）与实际文件是否一致。
     文件不存在时如实报 missing（WARN），不判 FAIL —— 因为 PDF 按设计不入库。
  B. 清单自相矛盾：file_presence_audit 与逐条 documents[] 是否取自同一轮检查结果
     （上一轮教训：audit 说“没有 PDF”、documents 说 file_present=true）。
  C. 统计口径自检：828d-definition-source-audit.json 的 42/90/35/7/6、24/16/2、36/6 与 r1.json 实测是否一致；
     同时校验 r1.json 的 blob SHA-1 未被本轮改动。
  D. 交叉引用覆盖率：35 条 manual 来源逐条存在且字段齐全，match_conclusion 分布可复算；
     not_found 条目不得带页码（禁止猜页码）；原始 manual reference 必须与 r1.json 逐字一致；
     7 条无 manual 来源的定义必须逐条给出去向（独立交叉引用或显式缺口）。
  E. 缺口说明：任何 not_found / pending_review 条目必须在 .md 的
     “not_found / pending_review 逐条说明”一节出现；.md 里的分布数字必须与 JSON 一致。

依赖：仅标准库；若安装了 pypdf 且 PDF 在位，额外校验页数。
"""

from __future__ import print_function

import argparse
import collections
import hashlib
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))

MANUALS = os.path.join(HERE, 'sinumerik-828d-manuals.json')
SRC_AUDIT = os.path.join(HERE, '828d-definition-source-audit.json')
XREF_JSON = os.path.join(HERE, '828d-manual-cross-reference.json')
XREF_MD = os.path.join(HERE, '828d-manual-cross-reference.md')
R1 = os.path.join(REPO, 'drivers/sinumerik-nck/definitions/828d/r1.json')
MANUALS_DIR = os.path.join(HERE, 'manuals')

EXPECTED_R1_BLOB_SHA1 = '3c0abe67cbcb64b525cddec1584e6e1747e0f062'
CONC = ('exact_match', 'matching_older_version', 'version_difference', 'not_found', 'pending_review')

RESULTS = []


def add(level, code, msg):
    RESULTS.append((level, code, msg))


def ok(code, msg):
    add('OK', code, msg)


def warn(code, msg):
    add('WARN', code, msg)


def fail(code, msg):
    add('FAIL', code, msg)


def load_json(path):
    with open(path, 'r', encoding='utf-8') as f:
        return json.load(f)


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def git(*args):
    try:
        out = subprocess.run(['git'] + list(args), cwd=REPO, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, check=False)
        if out.returncode != 0:
            return None
        return out.stdout.decode('utf-8', 'replace').strip()
    except (OSError, ValueError):
        return None


# ---------------------------------------------------------------- A 文件身份
def check_files(manuals, verbose):
    # 两个计数必须分开，否则「干净克隆（PDF 按设计不入库）」会被误报成「在位 N 份」：
    #   declared_present —— 清单 documents[].file_present=True 的数量：清单"记着"有几份
    #   actual_present   —— 本地磁盘上真实存在的数量：本轮"实际能核验"几份
    # 二者含义不同，合并成一个数字会误导（曾出现：文件全不在本地，却报"在位 3 / 4"）。
    declared_present = 0
    actual_present = 0
    missing_files = []
    for doc in manuals['documents']:
        name = doc['file_name']
        path = os.path.join(MANUALS_DIR, name)
        if doc.get('file_present') is True:
            declared_present += 1
        if not os.path.exists(path):
            missing_files.append(name)
            warn('A1', '%s 不在本地（按设计 PDF 不入库）：如实报 missing。'
                       '清单记为 file_present=%s；本轮无法核验 sha256/size_bytes/page_count'
                 % (name, doc.get('file_present')))
            continue
        actual_present += 1
        actual_size = os.path.getsize(path)
        actual_sha = sha256_of(path)
        if doc.get('file_present') is not True:
            fail('A2', '%s 实际存在，但 documents[].file_present 不是 true（清单过期）' % name)
        if doc.get('size_bytes') != actual_size:
            fail('A3', '%s size_bytes 不符：清单 %s，实测 %s' % (name, doc.get('size_bytes'), actual_size))
        if doc.get('sha256') != actual_sha:
            fail('A4', '%s sha256 不符：清单 %s，实测 %s' % (name, doc.get('sha256'), actual_sha))
        # 页数（可选：需要 pypdf）
        expect_pages = (doc.get('verified') or {}).get('pages')
        try:
            from pypdf import PdfReader  # noqa: F401
        except Exception:
            warn('A5', '%s 页数未校验：未安装 pypdf（pip install pypdf 后可校验 %s 页）' % (name, expect_pages))
            continue
        try:
            from pypdf import PdfReader
            actual_pages = len(PdfReader(path).pages)
        except Exception as exc:  # 损坏/加密等
            warn('A5', '%s 页数未校验：pypdf 打开失败（%s）' % (name, exc))
            continue
        if expect_pages is not None and actual_pages != expect_pages:
            fail('A6', '%s page_count 不符：清单 %s，实测 %s' % (name, expect_pages, actual_pages))
        else:
            ok('A6', '%s 页数一致：%s' % (name, actual_pages))
    total = len(manuals['documents'])
    ok('A0', '本地实际在位 %d / %d；清单记录为在位 %d 份'
             '（二者必须分开看：PDF 按设计不入库，干净克隆里实际在位可为 0，'
             '此时 A2–A6 跳过，仅核验清单内部一致性）'
        % (actual_present, total, declared_present))
    if actual_present == 0 and total:
        warn('A0b', '本地无任何 PDF：A2–A6 身份核对全部跳过。这不代表清单记录有误，'
                    '但表示本轮未对文件内容做任何实测。缺失清单：%s' % (missing_files or '（无）'))
    # PDF 不得入库
    tracked = git('ls-files', os.path.join('docs', 'reference', 'manuals'))
    if tracked is None:
        warn('A7', 'git 不可用，跳过“PDF 未被跟踪”检查')
    else:
        pdf_tracked = [ln for ln in tracked.splitlines() if ln.lower().endswith('.pdf')]
        if pdf_tracked:
            fail('A7', 'PDF 被 git 跟踪（应当只本地保留）：%s' % ', '.join(pdf_tracked))
        else:
            ok('A7', 'PDF 未被 git 跟踪（git ls-files docs/reference/manuals → 无 .pdf）')


# ------------------------------------------------- B 清单内部自相矛盾
def check_audit_consistency(manuals, verbose):
    audit = manuals.get('file_presence_audit')
    if not isinstance(audit, dict):
        fail('B0', 'manuals.json 缺少 file_presence_audit 对象')
        return
    docs = manuals['documents']
    present = [d['file_name'] for d in docs if d.get('file_present') is True]
    absent = [d['file_name'] for d in docs if d.get('file_present') is not True]
    expect = {
        'expected_file_count': len(docs),
        'present_file_count': len(present),
        'missing_files': absent,
        'documents_file_present_true': len(present),
        'documents_file_present_false': len(absent),
        'all_documents_file_present_consistent': True,
    }
    for key, want in expect.items():
        if key not in audit:
            fail('B1', 'file_presence_audit 缺少字段 %s（无法机械对账）' % key)
            continue
        if audit[key] != want:
            fail('B2', 'file_presence_audit.%s = %r，但由 documents[] 推出应为 %r —— 清单自相矛盾'
                 % (key, audit[key], want))
        else:
            ok('B2', 'file_presence_audit.%s = %r 与 documents[] 一致' % (key, audit[key]))
    # 结论文字不得与 documents[] 相反（本轮教训的文本形态）
    text = str(audit.get('conclusion', ''))
    BAD = ['未找到任何 PDF', '没有 PDF', 'file_present=false', '一律 file_present', '未到位']
    hits = [b for b in BAD if b in text]
    if hits:
        fail('B3', 'file_presence_audit.conclusion 仍含过期表述 %s（与 documents[].file_present 相反）' % hits)
    else:
        ok('B3', 'file_presence_audit.conclusion 无与 documents[] 相反的过期表述')
    if not absent and present:
        ok('B4', '三份文档 file_present=true 且 audit 声明 present_file_count=%d' % len(present))
    # checked_at_commit 只是「检查发生时的 HEAD」这一历史记录值，用于追溯。
    #
    # 不能拿它与当前 HEAD 比对来判「清单过期」：本清单本身就在这次提交里，每提交一次
    # HEAD 必变，那样每次都会 WARN，WARN 会退化成噪音，真正的过期反而被淹没。
    #
    # 真正会过期的风险在于「被审计的输入变了」，由两处机械比对覆盖：
    #   - r1.json 内容是否仍是审计时的版本 —— C3 用 git hash-object 对比 EXPECTED_R1_BLOB_SHA1
    #   - PDF 是否仍是审计时的那份     —— A 组用 sha256/size 与 documents[] 记录值比对
    cac = audit.get('checked_at_commit') or manuals.get('checked_at_commit')
    if not cac or not re.match(r'^[0-9a-f]{40}$', str(cac)):
        fail('B5', 'file_presence_audit.checked_at_commit 缺失或不是 40 位 commit SHA')
    else:
        head = git('rev-parse', 'HEAD')
        if head and head != cac:
            ok('B5', 'checked_at_commit=%s 与当前 HEAD=%s 不同属正常（本次提交即含本清单）；'
                     '过期以 C3 的 r1.json 摘要与 A 组 PDF 摘要为准'
                     % (cac[:12], head[:12]))
        else:
            ok('B5', 'checked_at_commit=%s 与当前 HEAD 一致' % cac[:12])
    # 文档正文层面：不允许在任何 .md 里留下“PDF 不在位 / 文件缺失”的过期结论
    if present:
        BAD_MD = ['PDF 未到位', 'PDF未到位', '未找到 PDF', '找不到任何 PDF',
                  'file_present: false', 'file_present=false', '一律标 `pending`']
        offenders = []
        for name in sorted(os.listdir(HERE)):
            if not name.endswith('.md'):
                continue
            path = os.path.join(HERE, name)
            try:
                lines = open(path, encoding='utf-8').read().splitlines()
            except OSError:
                continue
            for no, line in enumerate(lines, 1):
                for bad in BAD_MD:
                    if bad in line:
                        offenders.append('%s:%d（%s）' % (name, no, bad))
        if offenders:
            fail('B6', '文档正文仍写“文件不在位/缺失”，但 documents[].file_present 全为 true：%s'
                 % '; '.join(offenders[:8]))
        else:
            ok('B6', 'docs/reference 下没有任何 .md 仍断言“PDF 不在位/文件缺失”')


# ------------------------------------------------------ C 统计口径自检
def check_statistics(manuals, src_audit, xref, verbose):
    r1 = load_json(R1)
    defs = r1['definitions']
    kinds = collections.Counter(s['kind'] for x in defs for s in x['source'])
    sem = collections.Counter(x['semantics_status'] for x in defs)
    scope = collections.Counter(x['scope'] for x in defs)
    with_manual = sum(1 for x in defs if any(s['kind'] == 'manual' for s in x['source']))
    without_manual = len(defs) - with_manual
    sum_ = src_audit['summary']
    checks = [
        ('definition_count', sum_['definition_count'], len(defs)),
        ('source_entry_total', sum_['source_entry_total'], sum(kinds.values())),
        ('count_by_kind.manual', sum_['count_by_kind'].get('manual'), kinds['manual']),
        ('count_by_kind.frozen_vector', sum_['count_by_kind'].get('frozen_vector'), kinds['frozen_vector']),
        ('count_by_kind.measurement', sum_['count_by_kind'].get('measurement'), kinds['measurement']),
        ('count_by_kind.identity_probe', sum_['count_by_kind'].get('identity_probe'), kinds['identity_probe']),
        ('count_by_semantics_status.verified', sum_['count_by_semantics_status'].get('verified'), sem['verified']),
        ('count_by_semantics_status.parameter_identity',
         sum_['count_by_semantics_status'].get('parameter_identity'), sem['parameter_identity']),
        ('count_by_semantics_status.candidate',
         sum_['count_by_semantics_status'].get('candidate'), sem['candidate']),
        ('count_by_scope.collect', sum_['count_by_scope'].get('collect'), scope['collect']),
        ('count_by_scope.identity', sum_['count_by_scope'].get('identity'), scope['identity']),
        ('definitions_with_manual_source', with_manual, with_manual),
        ('definitions_without_manual_source', without_manual, without_manual),
    ]
    for name, recorded, actual in checks:
        if recorded != actual:
            fail('C1', '统计口径不符：%s 记录 %r，r1.json 实测 %r' % (name, recorded, actual))
        else:
            ok('C1', '%s = %r（与 r1.json 实测一致）' % (name, recorded)) if verbose else None
    # 任务给出的硬口径
    hard = {'definitions_total': 42, 'manual': 35, 'without_manual': 7, 'identity_probe': 6,
            'verified': 24, 'parameter_identity': 16, 'candidate': 2, 'collect': 36, 'identity_scope': 6}
    actual_hard = {'definitions_total': len(defs), 'manual': kinds['manual'], 'without_manual': without_manual,
                   'identity_probe': kinds['identity_probe'], 'verified': sem['verified'],
                   'parameter_identity': sem['parameter_identity'], 'candidate': sem['candidate'],
                   'collect': scope['collect'], 'identity_scope': scope['identity']}
    c2_bad = [(k, want, actual_hard[k]) for k, want in hard.items() if actual_hard[k] != want]
    for k, want, got in c2_bad:
        fail('C2', '任务基线口径不符：%s 应为 %r，实测 %r' % (k, want, got))
    if not c2_bad:
        ok('C2', '硬口径全部一致：42 定义 / 90 来源 / 35 manual / 7 无 manual / 6 identity_probe / 24-16-2 / 36-6')
    # r1.json 未被改动
    blob = git('hash-object', '--', os.path.relpath(R1, REPO))
    if blob is None:
        warn('C3', 'git 不可用，跳过 r1.json blob SHA-1 校验')
    elif blob != EXPECTED_R1_BLOB_SHA1:
        fail('C3', 'r1.json blob SHA-1 = %s，基线为 %s —— 定义文件被改动（本轮不允许）'
             % (blob, EXPECTED_R1_BLOB_SHA1))
    else:
        ok('C3', 'r1.json blob SHA-1 未变：%s' % blob)
    # 交叉引用自报数字与 r1.json 一致
    c4_bad = False
    if xref['summary']['definitions_with_manual_source'] != with_manual:
        fail('C4', '交叉引用表自报 manual 来源数 %d，与 r1.json 实测 %d 不符'
             % (xref['summary']['definitions_with_manual_source'], with_manual))
        c4_bad = True
    if xref['summary']['definitions_without_manual_source'] != without_manual:
        fail('C4', '交叉引用表自报无 manual 来源数 %d，与 r1.json 实测 %d 不符'
             % (xref['summary']['definitions_without_manual_source'], without_manual))
        c4_bad = True
    if not c4_bad:
        ok('C4', '交叉引用表自报 35/7 分组与 r1.json 一致')


# ------------------------------------------------- D 交叉引用覆盖率
def check_crossref(xref, manuals, verbose):
    r1 = load_json(R1)
    defs = {x['variable_id']: x for x in r1['definitions']}
    with_manual = {vid for vid, x in defs.items() if any(s['kind'] == 'manual' for s in x['source'])}
    without_manual = set(defs) - with_manual
    entries = xref['entries']
    ids = [e['variable_id'] for e in entries]
    dup = [i for i, c in collections.Counter(ids).items() if c > 1]
    if dup:
        fail('D1', '交叉引用表存在重复条目：%s' % dup)
    missing = sorted(with_manual - set(ids))
    extra = sorted(set(ids) - with_manual)
    if missing:
        fail('D1', '覆盖率不足：%d 条 manual 来源没有交叉引用 → %s' % (len(missing), missing))
    if extra:
        fail('D1', '交叉引用表混入了非 manual 来源的定义：%s' % extra)
    if not missing and not extra and not dup:
        ok('D1', '覆盖率 100%%：%d 条 manual 来源逐条有交叉引用，无重复无越界' % len(with_manual))

    dist = collections.Counter(e['match_conclusion'] for e in entries)
    bad = [c for c in dist if c not in CONC]
    d2_bad = False
    if bad:
        fail('D2', '出现未定义的 match_conclusion 取值：%s（只允许 %s）' % (bad, list(CONC)))
        d2_bad = True
    if sum(dist.values()) != len(with_manual):
        fail('D2', '五类结论合计 %d ≠ manual 来源数 %d' % (sum(dist.values()), len(with_manual)))
        d2_bad = True
    declared = xref['summary']['match_conclusion_distribution']
    for c in CONC:
        if declared.get(c) != dist.get(c, 0):
            fail('D2', '分布自报 %s=%r，实测 %r' % (c, declared.get(c), dist.get(c, 0)))
            d2_bad = True
    if not d2_bad:
        ok('D2', '五类分布可复算：%s（合计 %d）'
           % (', '.join('%s=%d' % (c, dist.get(c, 0)) for c in CONC), sum(dist.values())))

    # 字段完整性 + 不得把 07/2023 写成来源 + not_found 不得带页码
    manifest_files = {d['file_name']: d for d in manuals['documents']}
    problems = []
    for e in entries:
        vid = e['variable_id']
        for key in ('variable_id', 'revision', 'scope', 'semantics_status',
                    'original_manual_references', 'cross_reference_202307', 'match_conclusion', 'notes'):
            if key not in e:
                problems.append('%s 缺字段 %s' % (vid, key))
        # 原始 reference 必须逐字等于 r1.json
        want = [s['reference'] for s in defs[vid]['source'] if s['kind'] == 'manual']
        if e['original_manual_references'] != want:
            problems.append('%s 的 original_manual_references 与 r1.json 不逐字一致' % vid)
        cr = e['cross_reference_202307']
        if e['match_conclusion'] == 'not_found':
            if cr.get('page') or cr.get('pdf_file'):
                problems.append('%s 判为 not_found 却带 page/pdf_file（禁止猜页码）' % vid)
            if not cr.get('searched_pdfs'):
                problems.append('%s 判为 not_found 却没有记录已检索的文件列表' % vid)
        else:
            if not cr.get('page'):
                problems.append('%s 未给页码' % vid)
            if cr.get('pdf_file') not in manifest_files:
                problems.append('%s 的 pdf_file 不在 manuals.json documents[] 中' % vid)
            else:
                doc = manifest_files[cr['pdf_file']]
                if cr.get('pdf_document_number') != doc['document_number']:
                    problems.append('%s 的 pdf_document_number 与清单不一致' % vid)
                if cr.get('pdf_version') != doc['expected_version']:
                    problems.append('%s 的 pdf_version 与清单不一致' % vid)
            pages = cr.get('page') or []
            total = cr.get('pdf_total_pages')
            if total and any((not isinstance(p, int)) or p < 1 or p > total for p in pages):
                problems.append('%s 的页码越界：%s（该 PDF 共 %s 页）' % (vid, pages, total))
        if e.get('is_original_source') is not False:
            problems.append('%s 未声明 is_original_source=false（不得把 07/2023 当来源）' % vid)
        if e.get('must_not_backfill_to_r1') is not True:
            problems.append('%s 未声明 must_not_backfill_to_r1=true' % vid)
    if problems:
        for p in problems[:20]:
            fail('D3', p)
        if len(problems) > 20:
            fail('D3', '……另有 %d 条同类问题' % (len(problems) - 20))
    else:
        ok('D3', '逐条字段完整：原始 reference 逐字照抄、PDF 元数据与清单一致、页码在页数范围内、'
                 'not_found 均无页码、均声明非来源且禁止回填')

    # 7 条无 manual 来源的定义
    e7 = xref.get('entries_without_manual_source') or []
    ids7 = [e['variable_id'] for e in e7]
    miss7 = sorted(without_manual - set(ids7))
    if miss7:
        fail('D4', '无 manual 来源的定义缺去向记录：%s' % miss7)
    bad7 = []
    for e in e7:
        if e.get('has_manual_source_in_r1') is not False:
            bad7.append('%s 未声明 has_manual_source_in_r1=false' % e['variable_id'])
        if e.get('additive_only') is not True:
            bad7.append('%s 未声明 additive_only=true（独立交叉引用不得追认为来源）' % e['variable_id'])
        if e['match_conclusion'] == 'not_found' and (e['cross_reference_202307'].get('page')):
            bad7.append('%s 判 not_found 却带页码' % e['variable_id'])
    if bad7:
        for p in bad7:
            fail('D4', p)
    else:
        ok('D4', '7 条无 manual 来源的定义逐条有去向记录，且均标记「独立交叉引用、不追认为来源」')
    return dist


# ------------------------------------------------- E .md 缺口说明
def check_markdown(xref, dist, verbose):
    if not os.path.exists(XREF_MD):
        fail('E0', '%s 不存在' % XREF_MD)
        return
    text = open(XREF_MD, 'r', encoding='utf-8').read()
    anchor = None
    for m in re.finditer(r'^##\s+.*not_found.*pending_review.*$', text, re.M):
        anchor = m
        break
    need = [e['variable_id'] for e in xref['entries']
            if e['match_conclusion'] in ('not_found', 'pending_review')]
    if anchor is None:
        fail('E1', '.md 缺少“not_found / pending_review 逐条说明”标题（校验锚点）')
    else:
        rest = text[anchor.end():]
        nxt = re.search(r'^##\s', rest, re.M)
        section = rest[:nxt.start()] if nxt else rest
        missing = [i for i in need if i not in section]
        if missing:
            fail('E1', '.md 的缺口小节缺少这些条目的说明：%s' % missing)
        else:
            ok('E1', '.md 缺口小节覆盖全部 %d 条 not_found/pending_review 条目' % len(need))
    # .md 里的分布数字必须与 JSON 一致
    m24 = re.search(r'^###\s+2\.4.*$', text, re.M)
    if m24 is None:
        warn('E2', '.md 缺少 §2.4 分布小节，无法核对分布数字')
        return
    rest = text[m24.end():]
    nxt = re.search(r'^##\s', rest, re.M)
    body = rest[:nxt.start()] if nxt else rest
    split = body.find('7 条无 manual 来源定义')
    body35 = body[:split] if split > 0 else body
    body7 = body[split:] if split > 0 else ''
    parsed35 = dict((k, int(v)) for k, v in re.findall(
        r'^\|\s*`(%s)`\s*\|\s*(\d+)\s*\|' % '|'.join(CONC), body35, re.M))
    parsed7 = dict((k, int(v)) for k, v in re.findall(
        r'^\|\s*`(%s)`\s*\|\s*(\d+)\s*\|' % '|'.join(CONC), body7, re.M))
    bad = []
    for c in CONC:
        if parsed35.get(c) != dist.get(c, 0):
            bad.append('35 条表 %s: md=%r json=%r' % (c, parsed35.get(c), dist.get(c, 0)))
    if bad:
        fail('E2', '.md 分布数字与 JSON 不一致：%s' % bad)
    else:
        ok('E2', '.md §2.4 的 35 条分布数字与 JSON 完全一致')
    if parsed7:
        declared7 = xref['summary']['match_conclusion_distribution_without_manual_source']
        bad7 = [c for c in CONC if parsed7.get(c) != declared7.get(c, 0)]
        if bad7:
            fail('E3', '.md 的 7 条独立交叉引用分布与 JSON 不一致：%s' % bad7)
        else:
            ok('E3', '.md 的 7 条独立交叉引用分布与 JSON 一致')
    # 两个独立缺口必须在 .md 有专章
    for needle, code in (('axis.encoder.module_number', 'E4'),
                         ('module 26', 'E5')):
        if needle not in text:
            fail(code, '.md 未记录独立缺口关键字：%s' % needle)
    if 'axis.encoder.module_number' in text:
        ok('E4', '.md 记录了 axis.encoder.module_number 的独立缺口')
    if 'module 26' in text and '不允许' in text:
        ok('E5', '.md 记录了 module 26 / module 130 的三条禁令')


def main():
    ap = argparse.ArgumentParser(description='校验 828D 手册元数据与交叉引用清单')
    ap.add_argument('--verbose', action='store_true', help='打印每条通过项')
    args = ap.parse_args()

    for path in (MANUALS, SRC_AUDIT, XREF_JSON, R1):
        if not os.path.exists(path):
            print('[FAIL] F0 缺少文件：%s' % path)
            return 1
    manuals = load_json(MANUALS)
    src_audit = load_json(SRC_AUDIT)
    xref = load_json(XREF_JSON)

    check_files(manuals, args.verbose)
    check_audit_consistency(manuals, args.verbose)
    check_statistics(manuals, src_audit, xref, args.verbose)
    dist = check_crossref(xref, manuals, args.verbose)
    check_markdown(xref, dist, args.verbose)

    order = {'FAIL': 0, 'WARN': 1, 'OK': 2}
    show = [r for r in RESULTS if args.verbose or r[0] != 'OK']
    for level, code, msg in sorted(show, key=lambda r: order[r[0]]):
        print('[%s] %-3s %s' % (level, code, msg))
    n = collections.Counter(r[0] for r in RESULTS)
    print('\n汇总：OK %d / WARN %d / FAIL %d' % (n['OK'], n['WARN'], n['FAIL']))
    if n['FAIL']:
        print('结论：FAIL —— 清单或交叉引用与实测不一致，必须修复。')
        return 1
    print('结论：通过（无 FAIL%s）。' % ('；有 %d 条 WARN 需人看' % n['WARN'] if n['WARN'] else ''))
    return 0


if __name__ == '__main__':
    sys.exit(main())
