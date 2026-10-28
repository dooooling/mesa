#!/usr/bin/env python3
"""在手册文本缓存里穷尽检索关键词，输出「文件 + 页码 + 上下文」（只读、离线）。

为什么需要它：`r1.json` 的字符串长度与 `(C, SPARPP)` 条目依据必须能逐处指认
页码，而 pypdf 逐页文本散落在 `.cache_manual_*.txt`（换页符 `\\x0c` 分页，
第 N 段 = PDF 第 N 页 = 印刷第 N 页，偏移 0）。人工在 4602 页里找不可复核，
故把检索做成可重复执行的脚本，结论只来自它的输出。

用法：
    python3 docs/reference/grep-manual-terms.py --terms progName SPARPP
    python3 docs/reference/grep-manual-terms.py --preset sparp
    python3 docs/reference/grep-manual-terms.py --preset md30220
    python3 docs/reference/grep-manual-terms.py --preset strings --json out.json

输出：每条命中一行 `doc_id p<页号> :: 上下文`；`--json` 另存结构化结果，
便于把页码写进审计文档而不靠手抄。**不联网、不写 r1.json。**
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SEP = '\x0c'

DOCS = [
    ('828d-sysvar-lists-0723', '828D_sysvar_lists_man_0723_zh-CHS.pdf'),
    ('828d-md-lists-0723', '828D_md_lists_man_0723_zh-CHS.pdf'),
    ('sinamics-s120-para-lists-0723', '828D_S120_para_lists_man_0723_zh-CHS.pdf'),
    ('nc-variables-032025', '828D_ncvar_lists_man_0325_en-US.pdf'),
]

PRESETS = {
    # 任务 1：progName / SPARPP / SPARPI / SPARPF 全部出现处
    'sparp': ['progName', 'SPARPP', 'SPARPI', 'SPARPF', 'SPARP'],
    # 任务 4：编码器驱动号（机床数据编号 + 名称）
    'md30220': ['30220', 'ENC_MODULE_NR', '编码器驱动号', '编码器驱动编号',
                'Encoder drive number', 'encoder drive', 'Encoder module'],
    # 任务 2：字符串长度依据
    'strings': ['actBlock', 'progName', 'TYPE_STRING', 'String length', '字符串长度',
                'CHAR[', 'OCTET STRING'],
}


def cache_path(doc_id: str) -> str:
    return os.path.join(HERE, f'.cache_manual_{doc_id}.txt')


def pages_of(doc_id: str):
    path = cache_path(doc_id)
    if not os.path.isfile(path):
        return None
    with open(path, encoding='utf-8') as f:
        return f.read().split(SEP)


def context_of(text: str, start: int, end: int, width: int = 90) -> str:
    """把命中处的上下文压成一行（换行/多余空白归一）。"""
    lo = max(0, start - width)
    hi = min(len(text), end + width)
    snippet = text[lo:hi]
    snippet = re.sub(r'\s+', ' ', snippet).strip()
    return snippet


def search(doc_id: str, terms, flags=re.IGNORECASE):
    pages = pages_of(doc_id)
    if pages is None:
        print(f'!! 缺缓存 {doc_id}，先跑 extract-manual-text.py', file=sys.stderr)
        return []
    hits = []
    for term in terms:
        pat = re.compile(re.escape(term), flags)
        for no, text in enumerate(pages, start=1):
            for m in pat.finditer(text):
                hits.append({
                    'doc_id': doc_id,
                    'term': term,
                    'page': no,
                    'context': context_of(text, m.start(), m.end()),
                })
    return hits


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description='手册缓存关键词检索（只读）')
    ap.add_argument('--terms', nargs='*', default=None, help='直接给关键词')
    ap.add_argument('--preset', default=None, choices=sorted(PRESETS), help='用预置关键词组')
    ap.add_argument('--case-sensitive', action='store_true', help='大小写敏感（默认不敏感）')
    ap.add_argument('--json', default=None, help='把结构化结果写到该路径')
    args = ap.parse_args(argv)

    terms = args.terms if args.terms else (PRESETS[args.preset] if args.preset else None)
    if not terms:
        ap.error('需要 --terms 或 --preset')

    flags = 0 if args.case_sensitive else re.IGNORECASE
    all_hits = []
    summary = {}
    for doc_id, _name in DOCS:
        hits = search(doc_id, terms, flags)
        all_hits.extend(hits)
        for term in terms:
            pages = sorted({h['page'] for h in hits if h['term'] == term})
            summary.setdefault(term, {})[doc_id] = pages
        print(f'--- {doc_id}: {len(hits)} 处命中 ---')
        for h in hits:
            print(f'{h["doc_id"]} p{h["page"]} [{h["term"]}] :: {h["context"]}')

    print('\n=== 汇总（每条词 × 每份文档 → 页码列表）===')
    for term in terms:
        total = sum(len(v) for v in summary.get(term, {}).values())
        print(f'[{term}] 命中页数合计 {total}')
        for doc_id, _n in DOCS:
            pages = summary.get(term, {}).get(doc_id, [])
            shown = ','.join(str(p) for p in pages[:40])
            more = '' if len(pages) <= 40 else f' …(+{len(pages) - 40})'
            print(f'    {doc_id}: {len(pages)} 页 {shown}{more}')

    if args.json:
        with open(args.json, 'w', encoding='utf-8', newline='\n') as f:
            json.dump({'terms': terms, 'summary': summary, 'hits': all_hits},
                      f, ensure_ascii=False, indent=2)
        print(f'\n结构化结果已写入 {args.json}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
