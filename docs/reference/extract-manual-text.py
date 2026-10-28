#!/usr/bin/env python3
"""把手册 PDF 逐页抽成纯文本缓存，供后续只读检索使用（不联网、不改 r1.json）。

为什么单独一个脚本：四份手册共 4602 页，逐页 pypdf 抽取耗时数分钟；
检索方（`grep-manual-terms.py`）需要反复按多个关键词扫，重复抽取不可接受。
缓存写到 `docs/reference/.cache_manual_<id>.txt`（`.gitignore` 已按
`docs/reference/.cache_*` 排除，不入库）。

页边界用 `\x0c`（换页符）分隔：第 N 段即 PDF 第 N 页，与手册页脚
页码偏移为 0（见 sinumerik-828d-manuals.json 的 page_offset_note），
因此"PDF 页号 = 印刷页号"，检索结果可直接写成页码。

用法：
    python3 docs/reference/extract-manual-text.py            # 抽取缺失/过期的缓存
    python3 docs/reference/extract-manual-text.py --force    # 全部重抽
"""
from __future__ import annotations

import argparse
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
MANUALS = os.path.join(HERE, 'manuals')

DOCS = [
    ('828d-sysvar-lists-0723', '828D_sysvar_lists_man_0723_zh-CHS.pdf'),
    ('828d-md-lists-0723', '828D_md_lists_man_0723_zh-CHS.pdf'),
    ('sinamics-s120-para-lists-0723', '828D_S120_para_lists_man_0723_zh-CHS.pdf'),
    ('nc-variables-032025', '828D_ncvar_lists_man_0325_en-US.pdf'),
]

SEP = '\x0c'


def cache_path(doc_id: str) -> str:
    return os.path.join(HERE, f'.cache_manual_{doc_id}.txt')


def extract(pdf_path: str, out_path: str) -> int:
    from pypdf import PdfReader

    reader = PdfReader(pdf_path)
    pages = []
    for i, page in enumerate(reader.pages, start=1):
        try:
            text = page.extract_text() or ''
        except Exception as exc:  # 单页抽取失败不得中断整册
            text = f'<<EXTRACT_ERROR p{i}: {exc}>>'
        pages.append(text)
        if i % 100 == 0:
            print(f'  ... {i}/{len(reader.pages)} 页', flush=True)
    with open(out_path, 'w', encoding='utf-8', newline='\n') as f:
        f.write(SEP.join(pages))
    return len(pages)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description='逐页抽取手册文本到本地缓存')
    ap.add_argument('--force', action='store_true', help='忽略已有缓存，全部重抽')
    args = ap.parse_args(argv)

    missing = []
    for doc_id, name in DOCS:
        pdf = os.path.join(MANUALS, name)
        if not os.path.isfile(pdf):
            missing.append(name)
            continue
        out = cache_path(doc_id)
        if os.path.isfile(out) and not args.force:
            print(f'skip {doc_id}（缓存已存在）')
            continue
        print(f'extract {doc_id} <- {name}', flush=True)
        n = extract(pdf, out)
        print(f'  done {doc_id}: {n} 页 -> {os.path.basename(out)}', flush=True)

    if missing:
        print(f'缺少手册文件：{missing}', file=sys.stderr)
    return 0


if __name__ == '__main__':
    sys.exit(main())
