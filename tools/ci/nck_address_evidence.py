#!/usr/bin/env python3
"""NCK 原生地址证据矩阵的机械校验（Workstream A / 离线、纯标准库）。

设计意图
--------
`docs/evidence/nck-native-address-evidence.json` 是本仓里**唯一**声称
「NCK 各 block 的 native `module` 字节等于多少」的机器可读证据。它能被读、
被引用、被当成实现依据，因此必须有一条**可执行**的规则集把它钉死：
只要有人删出处、改字节、把手册语义说成 module 证明、把三类证据混成一份，
本脚本就必须 FAIL。人眼评审挡不住的东西（尤其跨行自相矛盾）交给机器。

六条规则（编号与 CI 输出一致）
------------------------------
R1 缺证据出处      —— block 行必须有 sample_ref + sample_lines（且行号必须**真的**
                     含有它声明的请求字节）、module_value_source.ref、手册文档
                     （number/version/sha256）、协议布局来源符号与仓内文件、
                     探针/测试 provenance、设备上下文。任缺即 FAIL。
R2 字段自相矛盾    —— 同一 block 出现多次且 module 不同、module_decimal 与
                     module_hex 不一致、decoded 与按 `wire_layout` 重新解码的
                     raw bytes 不一致、附加观测的 module 与行 module 不一致，
                     全部 FAIL。解码**只**用矩阵自己声明的布局，所以布局与
                     字节一旦不匹配也会被抓到。
R3 禁止语义越界    —— 任何形如「手册语义匹配可证明 native module 值」的表述
                     （自由文本 + 结构化字段）必须 FAIL；`proves_module` 必须
                     显式为 false；手册不得提供任何 module 数值
                     （`native_module_values_provided` 必须为 0）。
R4 三类证据不得混用 —— protocol-layout / manual-semantic / device-observation 必须
                     各自独立成容器与字段，且不得互相顶替（例如把 module 值
                     放进 `manual_semantic`、把 raw bytes 放进 `wire_layout`、
                     或让某行的 module 出处类与行证据类不同）。未知的
                     `evidence_class` 值同样 FAIL。
R5 基线完整性      —— `baseline.r1_blob_sha1` 必须等于实测 `git hash-object
                     r1.json`，且等于本脚本冻结字面量（r1.json 在本变更内
                     一字节不许动）；`real_828d_test_in_this_change` 为 false 时
                     **任何行**都不得声称本变更已在真机验证（含 status /
                     confidence_class / 自由文本），且每行必须标注观测是历史
                     转录（`observation_is_historical`）。
R6 unresolved 真实存在 —— `unresolved` 里的每一项若已被矩阵里的证据解决，必须
                     从 unresolved 移除（防止「已解决但仍挂着」的过期条目）；
                     行里引用的 unresolved id 必须真实存在（防止幽灵 id）。

为什么不用 PowerShell 处理：`Group-Object` / `Select-Object -Unique` 对嵌套
JSON 只能看到顶层对象，历史上有过「看着去重了其实没比」的教训；本脚本用
Python 一次性载入整棵结构做逐字段比较。

用法
----
    python3 tools/ci/nck_address_evidence.py                 # 校验当前矩阵
    python3 tools/ci/nck_address_evidence.py --json          # 机器可读输出
    python3 tools/ci/nck_address_evidence.py --allow-warn    # WARN 也返回 0
    python3 tools/ci/nck_address_evidence.py --matrix PATH --repo DIR

退出码：OK=0，FAIL=1，WARN=2（`--allow-warn` 时 WARN=0）。
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

DEFAULT_MATRIX = 'docs/evidence/nck-native-address-evidence.json'
R1_REL = 'drivers/sinumerik-nck/definitions/828d/r1.json'
# r1.json 的 git blob（CRLF 工作树 → LF blob）。本变更内必须逐字节不变。
R1_FROZEN_BLOB = '3c0abe67cbcb64b525cddec1584e6e1747e0f062'
SCHEMA_VERSION = 2

CANONICAL_CLASSES = ('protocol_layout', 'manual_semantic', 'device_observation')
# 顶层每个证据类一个专属容器：容器名 → 该类唯一合法标签。
CLASS_CONTAINERS = {
    'wire_layout': 'protocol_layout',
    'manual_mapping': 'manual_semantic',
    'block_observations': 'device_observation',
}
# 只要没有权威规格来源，这两条 unresolved 就必须仍然存在，且被每行声明。
REQUIRED_ROW_UNRESOLVED = ('module-value-authority', 'area-no-mapping')


# R3：禁止「手册语义 ⇒ native module 值」的表述（含中英）。命中后再用否定词过滤。
# 全部大小写不敏感：句首大写的 "Manual ... proves module" 与全小写同等有害。
OVERREACH_PATTERNS = [
    re.compile(r'manual[^.;\n]{0,120}?(?:prove|proves|proven|proving|establish|establishes|'
               r'established|confirm|confirms|confirmed|implies|therefore)[^.;\n]{0,60}?module',
               re.I),
    re.compile(r'module[^.;\n]{0,60}?(?:from|because of|taken from|derived from|per)[^.;\n]{0,40}?'
               r'(?:manual|semantic match|semantic name)', re.I),
    re.compile(r'semantic (?:match|name|equivalence)[^.;\n]{0,60}?(?:prove|establish|confirm)', re.I),
    re.compile(r'(?:手册|文档)[^。；;\n]{0,40}?(?:证明|可证|推定|即可确定|足以确定)'),
    re.compile(r'语义[^。；;\n]{0,30}?(?:证明|可证|推定|即可确定)'),
]
NEGATION_TOKENS = (
    'not ', "n't", 'never', 'cannot', 'can not', 'no ', 'without', 'does not', 'do not',
    'is not', 'are not', '不得', '不能', '不可', '不应', '不是', '并非', '无', '未', '不提供', '不说明',
)
# R5：`real_828d_test_in_this_change=false` 时禁止出现的「本变更已真机验证」声明。
VERIFIED_KEYS = re.compile(
    r'^(?:verified_in_this_change|verified_on_real_device|hardware_verified|'
    r'hardware_verified_in_this_change|verified_this_pr)$', re.I)
VERIFIED_TEXT = [
    re.compile(r'verified (?:on|against) (?:the )?real (?:device|828d|machine)[^.;\n]{0,40}'
               r'(?:in this change|this pr|this branch|during this change)', re.I),
    re.compile(r'本变更[^。；;\n]{0,20}?(?:已验证真机|已实测真机|真机已验证)'),
]
# R3：禁止的结构化字段名（手册给出 module 值的直说）。
OVERREACH_KEYS = re.compile(
    r'^(?:module_from_manual|manual_module|manual_module_value|semantic_module_proof|'
    r'proves_native_module|module_by_manual_semantic)$', re.I)
MODULE_VALUE_KEYS = ('module', 'module_decimal', 'module_hex', 'module_value', 'module_bytes')
DEVICE_ONLY_KEYS = ('raw_request', 'raw_request_example', 'sample_ref', 'sample_lines',
                    'decoded', 'response_status', 'device', 'probe_test', 'additional_observations')


class Report:
    """校验结论收集器：每条发现都带规则号，便于负向测试精确断言。"""

    def __init__(self):
        self.items = []

    def add(self, rule, severity, code, message):
        self.items.append({'rule': rule, 'severity': severity, 'code': code, 'message': message})

    def fail(self, rule, code, message):
        self.add(rule, 'FAIL', code, message)

    def warn(self, rule, code, message):
        self.add(rule, 'WARN', code, message)

    def info(self, rule, code, message):
        self.add(rule, 'INFO', code, message)

    def severity(self, rule=None):
        """三档结论：只要有一条 FAIL 就是 FAIL，其次是 WARN，INFO 不降级。"""
        worst = 'OK'
        for it in self.items:
            if rule is not None and it['rule'] != rule:
                continue
            if it['severity'] == 'FAIL':
                return 'FAIL'
            if it['severity'] == 'WARN':
                worst = 'WARN'
        return worst

    def verdict(self):
        return self.severity()

    def counts(self):
        c = {'FAIL': 0, 'WARN': 0, 'INFO': 0}
        for it in self.items:
            c[it['severity']] += 1
        return c


class Ctx:
    """校验上下文：仓库根、文件缓存、git 能力。"""

    def __init__(self, repo, matrix_path):
        self.repo = repo
        self.matrix_path = matrix_path
        self._lines = {}
        self.doc = None
        self.unresolved_ids = set()

    def path(self, rel):
        """仓内相对路径（允许 `file:line` 形式，行号部分忽略）。"""
        rel = rel.split('::')[0]
        rel = rel.split(':')[0] if re.search(r':\d+$', rel) else rel
        return os.path.join(self.repo, rel.replace('/', os.sep))

    def file_exists(self, rel):
        rel = rel.split('::')[0]
        rel = re.sub(r':\d+$', '', rel)
        return os.path.isfile(os.path.join(self.repo, rel.replace('/', os.sep)))

    def lines_of(self, rel):
        if rel not in self._lines:
            with open(self.path(rel), encoding='utf-8') as f:
                self._lines[rel] = f.read().splitlines()
        return self._lines[rel]

    def line_contains(self, rel, line_no, raw):
        """第 line_no 行（1 基）是否逐字包含 `raw`（空白与大小写归一）。"""
        try:
            lines = self.lines_of(rel)
        except OSError as e:
            return False, f'无法读取 {rel}: {e}'
        if line_no < 1 or line_no > len(lines):
            return False, f'{rel} 只有 {len(lines)} 行，引用第 {line_no} 行不存在'
        want = ' '.join(raw.lower().split())
        have = ' '.join(lines[line_no - 1].lower().split())
        if want not in have:
            return False, f'{rel}:{line_no} 不含声明的请求字节 `{want}`，实际 `{lines[line_no - 1].strip()}`'
        return True, ''


# ---------------------------------------------------------------------------
# 通用小工具
# ---------------------------------------------------------------------------

def walk(obj, path='$'):
    """深度遍历 (路径, 键, 值)；键为 None 表示根。"""
    if isinstance(obj, dict):
        for k, v in obj.items():
            yield path, k, v
            yield from walk(v, f'{path}.{k}')
    elif isinstance(obj, list):
        for i, v in enumerate(obj):
            yield f'{path}[{i}]', None, v
            yield from walk(v, f'{path}[{i}]')


def all_strings(doc):
    """(路径, 字符串) 序列，用于自由文本扫描（键名也扫，防止键名本身构成 claim）。"""
    for path, key, value in walk(doc):
        if key is not None:
            yield f'{path}.{key}', key
        if isinstance(value, str):
            yield path, value


def negated(text):
    low = text.lower()
    return any(tok in low for tok in NEGATION_TOKENS)


def parse_hex_bytes(raw):
    """'12 08 82 ...' → [0x12, ...]；非法返回 None。"""
    if not isinstance(raw, str):
        return None
    parts = raw.split()
    out = []
    for p in parts:
        if not re.fullmatch(r'[0-9a-fA-F]{2}', p):
            return None
        out.append(int(p, 16))
    return out


def parse_layout(wire_layout, rep):
    """把 `wire_layout.format` 解析成 {name: field}；非法布局直接报错（R2）。"""
    fmt = (wire_layout or {}).get('format') or {}
    total = fmt.get('bytes')
    fields = fmt.get('fields') or []
    if not isinstance(total, int) or total <= 0:
        rep.fail('R2', 'layout-bytes-missing', 'wire_layout.format.bytes 缺失或非法')
        return None
    table = {}
    spans = []
    for f in fields:
        name = f.get('name')
        off = f.get('offset')
        width = f.get('width')
        if not name or not isinstance(off, int) or not isinstance(width, int) or width <= 0:
            rep.fail('R2', 'layout-field-invalid', f'wire_layout 字段非法: {f}')
            return None
        table[name] = f
        spans.append((off, off + width, name))
    spans.sort()
    cursor = 0
    for start, end, name in spans:
        if start != cursor:
            rep.fail('R2', 'layout-not-contiguous', f'字段 `{name}` 起点 {start} 与上一个字段末端 {cursor} 不连续')
            return None
        cursor = end
    if cursor != total:
        rep.fail('R2', 'layout-length-mismatch', f'字段总宽 {cursor} ≠ format.bytes {total}')
        return None
    return {'bytes': total, 'fields': table, 'order': [n for _, _, n in spans]}


def decode_by_layout(raw_bytes, layout):
    """按矩阵**自己声明**的布局解码 raw bytes（R2 的机械依据）。"""
    out = {}
    for name, f in layout['fields'].items():
        off, width = f['offset'], f['width']
        chunk = raw_bytes[off:off + width]
        if f.get('endian') == 'little':
            value = int.from_bytes(chunk, 'little')
        else:
            value = int.from_bytes(chunk, 'big')
        out[name] = value
    return out


def field_expected_int(field):
    """布局里声明的 observed_value / observed_values 转成 int 列表（没有则 None）。"""
    if 'observed_value' in field:
        v = field['observed_value']
        if isinstance(v, str):
            try:
                return [int(v, 16)]
            except ValueError:
                return None
        if isinstance(v, int):
            return [v]
    if 'observed_values' in field:
        vals = field['observed_values']
        out = []
        for v in vals:
            try:
                out.append(int(v, 16) if isinstance(v, str) else int(v))
            except (TypeError, ValueError):
                return None
        return out
    return None


# ---------------------------------------------------------------------------
# R1：证据出处
# ---------------------------------------------------------------------------

def check_r1(ctx, rep):
    doc = ctx.doc
    rows = doc.get('block_observations')
    if not isinstance(rows, list) or not rows:
        rep.fail('R1', 'block-observations-empty', 'block_observations 缺失或为空')
        return
    for i, row in enumerate(rows):
        tag = row.get('block') or f'#{i}'
        # 1.1 采样出处
        sample_ref = row.get('sample_ref')
        if not isinstance(sample_ref, str) or not sample_ref.strip():
            rep.fail('R1', 'sample-ref-missing', f'block {tag} 缺 sample_ref（采样来源）')
        elif not ctx.file_exists(sample_ref):
            rep.fail('R1', 'sample-ref-not-found', f'block {tag} 的 sample_ref `{sample_ref}` 不存在')
        raw = row.get('raw_request_example')
        lines = row.get('sample_lines')
        if not isinstance(lines, list) or not lines:
            rep.fail('R1', 'sample-lines-missing', f'block {tag} 缺 sample_lines（采样行号）')
        elif isinstance(sample_ref, str) and sample_ref.strip():
            for ln in lines:
                if not isinstance(ln, int) or isinstance(ln, bool) or ln < 1:
                    rep.fail('R1', 'sample-line-invalid', f'block {tag} 的 sample_lines 含非法行号 {ln!r}')
                    continue
                ok, why = ctx.line_contains(sample_ref, ln, raw or '')
                if not ok:
                    rep.fail('R1', 'sample-line-bytes-mismatch', f'block {tag}：{why}')
        # 1.2 附加观测出处
        for j, obs in enumerate(row.get('additional_observations') or []):
            ln = obs.get('sample_line')
            obs_raw = obs.get('raw_request')
            if not isinstance(sample_ref, str) or not isinstance(ln, int) or not isinstance(obs_raw, str):
                rep.fail('R1', 'additional-observation-incomplete',
                         f'block {tag} 的第 {j} 条附加观测缺 sample_line/raw_request')
                continue
            ok, why = ctx.line_contains(sample_ref, ln, obs_raw)
            if not ok:
                rep.fail('R1', 'additional-observation-bytes-mismatch', f'block {tag}：{why}')
        # 1.3 module 值自身的出处
        mvs = row.get('module_value_source') or {}
        if not isinstance(mvs.get('ref'), str) or not mvs['ref'].strip():
            rep.fail('R1', 'module-value-source-missing', f'block {tag} 的 module 值缺 module_value_source.ref')
        # 1.4 手册文档出处
        ms = row.get('manual_semantic') or {}
        manual_doc = ms.get('document') or {}
        for key in ('title', 'document_number', 'version', 'sha256'):
            val = manual_doc.get(key)
            if not isinstance(val, str) or not val.strip():
                rep.fail('R1', f'manual-document-{key}-missing',
                         f'block {tag} 缺手册文档字段 {key}')
        sha = manual_doc.get('sha256')
        if isinstance(sha, str) and sha and not re.fullmatch(r'[0-9a-f]{64}', sha):
            rep.fail('R1', 'manual-document-sha256-invalid', f'block {tag} 的 sha256 不是 64 位十六进制')
        # 1.5 协议布局出处
        pl = row.get('protocol_layout') or {}
        if not isinstance(pl.get('source_symbol'), str) or not pl['source_symbol'].strip():
            rep.fail('R1', 'layout-source-symbol-missing', f'block {tag} 缺 protocol_layout.source_symbol')
        repo_source = pl.get('repo_source')
        if not isinstance(repo_source, str) or not repo_source.strip():
            rep.fail('R1', 'layout-repo-source-missing', f'block {tag} 缺 protocol_layout.repo_source')
        elif not ctx.file_exists(repo_source):
            rep.fail('R1', 'layout-repo-source-not-found',
                     f'block {tag} 的 repo_source `{repo_source}` 文件不存在')
        # 1.6 测试 provenance + 设备上下文
        dobs = row.get('device_observation') or {}
        for key in ('probe_test', 'golden_test'):
            val = dobs.get(key)
            if not isinstance(val, str) or not val.strip():
                rep.fail('R1', f'{key}-missing', f'block {tag} 缺 {key}（测试 provenance）')
            elif not ctx.file_exists(val):
                rep.fail('R1', f'{key}-not-found', f'block {tag} 的 {key} `{val}` 文件不存在')
        device = dobs.get('device') or {}
        if not isinstance(device.get('model'), str) or not device['model'].strip():
            rep.fail('R1', 'device-model-missing', f'block {tag} 缺 device.model（设备上下文）')
        if device.get('host') in (None, '') or device.get('port') in (None, ''):
            rep.fail('R1', 'device-endpoint-missing', f'block {tag} 缺 device.host/port（设备上下文）')
        if device.get('software_version') in (None, ''):
            if 'device-software-version' in ctx.unresolved_ids:
                rep.info('R1', 'device-software-version-unresolved',
                         f'block {tag}：SW 版本为空，已由 unresolved device-software-version 记录')
            else:
                rep.warn('R1', 'device-software-version-empty',
                         f'block {tag}：device.software_version 为空且未在 unresolved 中说明')
        # 1.7 声明边界
        if not isinstance(row.get('claim_scope'), str) or not row['claim_scope'].strip():
            rep.fail('R1', 'claim-scope-missing', f'block {tag} 缺 claim_scope（声明该行证据的适用范围）')


# ---------------------------------------------------------------------------
# R2：字段自相矛盾
# ---------------------------------------------------------------------------

def check_r2(ctx, rep):
    doc = ctx.doc
    layout = parse_layout(doc.get('wire_layout'), rep)
    rows = doc.get('block_observations') or []
    if layout is None:
        return
    seen = {}
    for i, row in enumerate(rows):
        tag = row.get('block') or f'#{i}'
        module = row.get('module_decimal')
        if not isinstance(module, int) or isinstance(module, bool) or not (0 <= module <= 255):
            rep.fail('R2', 'module-decimal-invalid', f'block {tag} 的 module_decimal 非法: {module!r}')
            module = None
        hexv = row.get('module_hex')
        if not isinstance(hexv, str) or not re.fullmatch(r'0x[0-9a-f]{2}', hexv):
            rep.fail('R2', 'module-hex-invalid', f'block {tag} 的 module_hex 非法: {hexv!r}')
        elif module is not None and int(hexv, 16) != module:
            rep.fail('R2', 'module-hex-contradicts-decimal',
                     f'block {tag}：module_hex {hexv} ≠ module_decimal {module}')
        if isinstance(row.get('block'), str):
            if row['block'] in seen:
                if seen[row['block']] != module:
                    rep.fail('R2', 'module-conflict-across-rows',
                             f'block {row["block"]} 在不同行给出不同 module：{seen[row["block"]]} vs {module}')
                else:
                    rep.fail('R2', 'duplicate-block-row', f'block {row["block"]} 出现了不止一行')
            else:
                seen[row['block']] = module

        raw_bytes = parse_hex_bytes(row.get('raw_request_example'))
        if raw_bytes is None:
            rep.fail('R2', 'raw-bytes-unparsable', f'block {tag} 的 raw_request_example 不是十六进制字节串')
            continue
        if len(raw_bytes) != layout['bytes']:
            rep.fail('R2', 'raw-bytes-length-mismatch',
                     f'block {tag}：请求 {len(raw_bytes)} 字节 ≠ 布局 {layout["bytes"]} 字节')
            continue
        decoded_actual = decode_by_layout(raw_bytes, layout)
        # 2.1 固定头（布局声明的 observed_value）
        for name in ('spec_type', 'spec_length', 'syntax_id'):
            field = layout['fields'].get(name)
            if field is None:
                continue
            allowed = field_expected_int(field)
            if allowed and decoded_actual[name] not in allowed:
                rep.fail('R2', 'raw-header-contradicts-layout',
                         f'block {tag}：字节中 {name}=0x{decoded_actual[name]:02x} 不在布局声明 '
                         f'{[hex(v) for v in allowed]} 内')
        # 2.2 area_unit 分解（布局里声明的 area_letters 为唯一字母表）
        au_field = layout['fields'].get('area_unit') or {}
        letters = au_field.get('area_letters') or {}
        area_unit = decoded_actual.get('area_unit')
        decoded_decl = (row.get('device_observation') or {}).get('decoded') or {}
        if isinstance(area_unit, int):
            index = area_unit >> 5
            unit = area_unit & 0x1F
            index_letters = {v: k for k, v in letters.items() if isinstance(v, int)}
            expect_letter = index_letters.get(index)
            if decoded_decl.get('area_index') != index:
                rep.fail('R2', 'decoded-area-index-mismatch',
                         f'block {tag}：decoded.area_index={decoded_decl.get("area_index")} ≠ 字节解出 {index}')
            if decoded_decl.get('area_no') != unit:
                rep.fail('R2', 'decoded-area-no-mismatch',
                         f'block {tag}：decoded.area_no={decoded_decl.get("area_no")} ≠ 字节解出 {unit}')
            if expect_letter is not None and decoded_decl.get('area') != expect_letter:
                rep.fail('R2', 'decoded-area-letter-mismatch',
                         f'block {tag}：decoded.area={decoded_decl.get("area")!r} ≠ 布局字母表 {expect_letter!r}')
        # 2.3 其余解码字段逐个比对
        for name in layout['order']:
            if name in ('area_unit',):
                continue
            if name not in decoded_decl:
                rep.fail('R2', 'decoded-field-missing', f'block {tag}：decoded 缺字段 {name}')
                continue
            if decoded_decl[name] != decoded_actual[name]:
                rep.fail('R2', 'decoded-contradicts-raw',
                         f'block {tag}：decoded.{name}={decoded_decl[name]!r} ≠ 字节解出 {decoded_actual[name]!r}')
        if module is not None and decoded_actual.get('module') != module:
            rep.fail('R2', 'module-contradicts-raw',
                     f'block {tag}：module_decimal={module} ≠ 字节解出 {decoded_actual.get("module")}')
        # 2.4 附加观测：同 block 必须同 module，且字段与字节一致
        for j, obs in enumerate(row.get('additional_observations') or []):
            obs_raw = parse_hex_bytes(obs.get('raw_request'))
            if obs_raw is None or len(obs_raw) != layout['bytes']:
                rep.fail('R2', 'additional-observation-unparsable',
                         f'block {tag} 第 {j} 条附加观测的 raw_request 不可解析或长度不符')
                continue
            dec = decode_by_layout(obs_raw, layout)
            if module is not None and dec.get('module') != module:
                rep.fail('R2', 'module-conflict-across-observations',
                         f'block {tag} 第 {j} 条附加观测的 module=0x{dec.get("module", 0):02x} ≠ 行 module {module}')
            for key, wire_key in (('area_no', None), ('column', 'column'), ('line', 'line'),
                                  ('line_count', 'line_count')):
                if key not in obs:
                    rep.fail('R2', 'additional-observation-field-missing',
                             f'block {tag} 第 {j} 条附加观测缺 {key}')
                    continue
                actual = dec.get(wire_key) if wire_key else (obs_raw[3] & 0x1F)
                if obs[key] != actual:
                    rep.fail('R2', 'additional-observation-contradicts-raw',
                             f'block {tag} 第 {j} 条附加观测 {key}={obs[key]!r} ≠ 字节解出 {actual!r}')


# ---------------------------------------------------------------------------
# R3：禁止语义越界
# ---------------------------------------------------------------------------

def check_r3(ctx, rep):
    doc = ctx.doc
    # 3.1 自由文本
    for path, text in all_strings(doc):
        if not isinstance(text, str) or not text:
            continue
        for pat in OVERREACH_PATTERNS:
            for m in pat.finditer(text):
                span = text[max(0, m.start() - 40):m.end()]
                if negated(m.group(0)) or negated(span[:40]):
                    continue
                rep.fail('R3', 'semantic-overreach-text',
                         f'{path} 出现「手册/语义匹配 ⇒ native module 值」的表述：...{text[max(0, m.start()-20):m.end()+20]!r}')
                break
    # 3.2 结构化字段
    for path, key, value in walk(doc):
        if key is None:
            continue
        if OVERREACH_KEYS.match(key):
            rep.fail('R3', 'semantic-overreach-key', f'{path}.{key} 是禁止的「手册给出 module 值」字段')
        if key == 'proves_module' and value is not False:
            rep.fail('R3', 'proves-module-not-false',
                     f'{path}.proves_module={value!r}，必须显式为 false')
    # 3.3 手册不得提供任何 module 数值
    result = ((doc.get('manual_mapping') or {}).get('result') or {})
    for key in ('native_module_values_provided', 'complete_five_field_native_addresses_provided'):
        if result.get(key) != 0:
            rep.fail('R3', 'manual-provides-module-values',
                     f'manual_mapping.result.{key}={result.get(key)!r}，手册不可能提供 module 值，必须为 0')
    if result.get('conclusion') != 'manual_semantic_only':
        rep.fail('R3', 'manual-conclusion-invalid',
                 f'manual_mapping.result.conclusion={result.get("conclusion")!r}，必须为 manual_semantic_only')
    # 3.4 每行都必须显式声明手册不构成 module 证明
    for i, row in enumerate(doc.get('block_observations') or []):
        ms = row.get('manual_semantic') or {}
        if 'proves_module' not in ms:
            rep.fail('R3', 'proves-module-missing',
                     f'block {row.get("block", i)} 的 manual_semantic 缺 proves_module: false')


# ---------------------------------------------------------------------------
# R4：三类证据不得混用
# ---------------------------------------------------------------------------

def check_r4(ctx, rep):
    doc = ctx.doc
    classes = doc.get('evidence_classes')
    if not isinstance(classes, dict):
        rep.fail('R4', 'evidence-classes-missing', '缺 evidence_classes（三类证据的登记表）')
    else:
        if tuple(sorted(classes.keys())) != tuple(sorted(CANONICAL_CLASSES)):
            rep.fail('R4', 'evidence-classes-invalid',
                     f'evidence_classes 必须恰好是 {list(CANONICAL_CLASSES)}，实际 {sorted(classes.keys())}')
        for name in CANONICAL_CLASSES:
            entry = classes.get(name) or {}
            if not isinstance(entry.get('definition'), str) or not entry['definition'].strip():
                rep.fail('R4', 'evidence-class-definition-missing', f'证据类 {name} 缺 definition')
            srcs = entry.get('sources')
            if not isinstance(srcs, list) or not srcs:
                rep.fail('R4', 'evidence-class-sources-missing', f'证据类 {name} 缺 sources')

    # 4.1 三大容器必须存在且标签自洽
    for container, expected in CLASS_CONTAINERS.items():
        if container not in doc:
            rep.fail('R4', 'evidence-container-missing', f'缺证据容器 {container}')
            continue
        if container != 'block_observations':
            tag = doc[container].get('evidence_class')
            if tag != expected:
                rep.fail('R4', 'evidence-container-class-mismatch',
                         f'{container} 的 evidence_class={tag!r}，应为 {expected!r}')

    # 4.2 任何 evidence_class 值都必须在登记表内
    for path, key, value in walk(doc):
        if key == 'evidence_class' and isinstance(value, str) and value not in CANONICAL_CLASSES:
            rep.fail('R4', 'unknown-evidence-class', f'{path}.evidence_class={value!r} 不是合法证据类')

    # 4.3 每行三类字段齐备、标签自洽
    for i, row in enumerate(doc.get('block_observations') or []):
        tag = row.get('block', f'#{i}')
        row_class = row.get('evidence_class')
        if row_class != 'device_observation':
            rep.fail('R4', 'row-class-mismatch',
                     f'block {tag} 的 evidence_class={row_class!r}，block 行只能承载 device_observation')
        for sub, expected in (('protocol_layout', 'protocol_layout'),
                              ('manual_semantic', 'manual_semantic'),
                              ('device_observation', 'device_observation')):
            obj = row.get(sub)
            if not isinstance(obj, dict):
                rep.fail('R4', f'row-{sub}-missing', f'block {tag} 缺 {sub} 证据字段')
                continue
            if obj.get('evidence_class') != expected:
                rep.fail('R4', f'row-{sub}-class-mismatch',
                         f'block {tag} 的 {sub}.evidence_class={obj.get("evidence_class")!r}，应为 {expected!r}')
        # module 值的出处必须与行证据类一致（不得让别类顶替）
        mvs = row.get('module_value_source') or {}
        if mvs.get('evidence_class') != row_class:
            rep.fail('R4', 'module-value-source-class-mismatch',
                     f'block {tag}：module_value_source.evidence_class={mvs.get("evidence_class")!r} '
                     f'与行证据类 {row_class!r} 不一致（module 值只能由本行同类证据承载）')
        # 4.4 禁止互相顶替
        for key in MODULE_VALUE_KEYS:
            if key in (row.get('manual_semantic') or {}):
                rep.fail('R4', 'manual-carries-module-value',
                         f'block {tag}：manual_semantic 里不得出现 module 值字段 `{key}`')
            if key in (row.get('protocol_layout') or {}):
                rep.fail('R4', 'layout-carries-module-value',
                         f'block {tag}：protocol_layout 里不得出现 module 值字段 `{key}`'
                         f'（布局只说明字段位置）')
    # 4.5 其它容器不得夹带别类证据
    for container in ('wire_layout', 'manual_mapping'):
        obj = doc.get(container) or {}
        for key in DEVICE_ONLY_KEYS:
            if key in obj:
                rep.fail('R4', 'container-carries-foreign-evidence',
                         f'{container} 里不得出现只属于 device_observation 的字段 `{key}`')
        for key in MODULE_VALUE_KEYS:
            if key in obj:
                rep.fail('R4', 'container-carries-module-value',
                         f'{container} 里不得出现 module 值字段 `{key}`')


# ---------------------------------------------------------------------------
# R5：基线完整性
# ---------------------------------------------------------------------------

def git_blob_sha(repo, rel):
    """优先用 `git hash-object`（唯一与提交内容一致的度量）；失败返回 (None, 原因)。"""
    try:
        out = subprocess.run(['git', 'hash-object', '--', rel], cwd=repo,
                             capture_output=True, text=True, check=False)
    except OSError as e:  # git 不可用（纯净离线环境）
        return None, f'git 不可用: {e}'
    if out.returncode != 0:
        return None, f'git hash-object 失败: {out.stderr.strip()}'
    return out.stdout.strip(), ''


def python_blob_sha(repo, rel):
    """git 不可用时的等价实现：blob 头 + 内容；同时给出 CRLF 工作树下的两种候选。"""
    with open(os.path.join(repo, rel), 'rb') as f:
        data = f.read()
    def blob(b):
        return hashlib.sha1(b'blob %d\0' % len(b) + b).hexdigest()
    return {'raw': blob(data), 'lf': blob(data.replace(b'\r\n', b'\n'))}


def check_r5(ctx, rep, frozen_blob=R1_FROZEN_BLOB):
    doc = ctx.doc
    baseline = doc.get('baseline') or {}
    if not isinstance(baseline.get('main_commit'), str) or \
            not re.fullmatch(r'[0-9a-f]{40}', baseline.get('main_commit', '')):
        rep.fail('R5', 'baseline-main-commit-invalid', 'baseline.main_commit 必须是 40 位提交哈希')
    r1_path = baseline.get('r1_path')
    if not isinstance(r1_path, str) or not r1_path.strip():
        rep.fail('R5', 'baseline-r1-path-missing', 'baseline.r1_path 缺失（无法核对 r1.json）')
        r1_path = R1_REL
    declared = baseline.get('r1_blob_sha1')
    if not isinstance(declared, str) or not re.fullmatch(r'[0-9a-f]{40}', declared or ''):
        rep.fail('R5', 'baseline-r1-blob-invalid', 'baseline.r1_blob_sha1 不是 40 位十六进制')
    if not os.path.isfile(os.path.join(ctx.repo, r1_path.replace('/', os.sep))):
        rep.fail('R5', 'r1-file-missing', f'r1.json 不存在：{r1_path}')
    else:
        measured, why = git_blob_sha(ctx.repo, r1_path)
        if measured is None:
            cands = python_blob_sha(ctx.repo, r1_path)
            rep.info('R5', 'blob-measured-without-git', f'{why}；改用纯 Python blob 计算（raw/lf）')
            if declared not in (cands['raw'], cands['lf']):
                rep.fail('R5', 'r1-blob-mismatch',
                         f'实测 r1.json blob（raw={cands["raw"]} lf={cands["lf"]}）≠ 声明 {declared}')
            elif frozen_blob and frozen_blob not in (cands['raw'], cands['lf']):
                rep.fail('R5', 'r1-blob-frozen-mismatch',
                         f'实测 r1.json blob 偏离冻结值 {frozen_blob}')
            else:
                rep.info('R5', 'r1-blob-verified', f'r1.json blob 未变：{declared}')
        else:
            if declared != measured:
                rep.fail('R5', 'r1-blob-mismatch',
                         f'`git hash-object {r1_path}` 实测 {measured} ≠ 声明 {declared}')
            if frozen_blob and measured != frozen_blob:
                rep.fail('R5', 'r1-blob-frozen-mismatch',
                         f'`git hash-object {r1_path}` = {measured}，已偏离冻结值 {frozen_blob}'
                         f'（r1.json 在本变更内必须逐字节不变）')
            if declared == measured and (not frozen_blob or measured == frozen_blob):
                rep.info('R5', 'r1-blob-verified', f'r1.json blob 未变：{measured}')
    # 5.2 真机声明门
    flag = baseline.get('real_828d_test_in_this_change')
    if not isinstance(flag, bool):
        rep.fail('R5', 'real-device-flag-invalid',
                 f'baseline.real_828d_test_in_this_change 必须是布尔值，实际 {flag!r}')
        flag = False
    rows = doc.get('block_observations') or []
    if not flag:
        for path, key, value in walk(doc):
            if key is not None and VERIFIED_KEYS.match(key) and value is not False and value not in (None, ''):
                rep.fail('R5', 'claimed-hardware-verification',
                         f'{path}.{key}={value!r}：本变更未跑真机，不得声称真机已验证')
        for path, text in all_strings(doc):
            for pat in VERIFIED_TEXT:
                if pat.search(text):
                    rep.fail('R5', 'claimed-hardware-verification-text',
                             f'{path} 声称本变更已在真机验证：{text[:120]!r}')
        for i, row in enumerate(rows):
            tag = row.get('block', f'#{i}')
            if row.get('status') in ('verified', 'verified_in_change', 'hardware_verified'):
                rep.fail('R5', 'row-status-claims-verification',
                         f'block {tag} 的 status={row.get("status")!r}：本变更未跑真机，只能是 observed')
            cc = row.get('confidence_class')
            if cc in ('hardware_verified', 'verified', 'verified_in_change'):
                rep.fail('R5', 'row-confidence-claims-verification',
                         f'block {tag} 的 confidence_class={cc!r}：本变更未跑真机')
            dobs = row.get('device_observation') or {}
            if dobs.get('observation_is_historical') is not True:
                rep.fail('R5', 'observation-not-marked-historical',
                         f'block {tag}：本变更未跑真机，device_observation.observation_is_historical 必须为 true')
        if 'hardware-validation' not in ctx.unresolved_ids:
            rep.fail('R5', 'hardware-validation-unresolved-removed',
                     '未跑真机时必须保留 unresolved hardware-validation')


# ---------------------------------------------------------------------------
# R6：unresolved 必须真实存在
# ---------------------------------------------------------------------------

def resolved_predicates(doc):
    """unresolved id → 「已被本矩阵证据解决」的判据（True 即必须移除该条目）。

    判据只看**结构化证据**，不看措辞：所以「自称已解决」不会让条目合法地留着，
    而「确实补上了证据」则会立刻让过期条目 FAIL。
    """
    rows = doc.get('block_observations') or []
    manual = (doc.get('manual_mapping') or {}).get('result') or {}
    baseline = doc.get('baseline') or {}
    wire = doc.get('wire_layout') or {}

    def any_row(pred):
        return any(pred(r) for r in rows if isinstance(r, dict))

    return {
        # 出现权威规格来源（而不是设备观测）即视为已解决
        'module-value-authority': lambda: manual.get('native_module_values_provided') not in (0, None) or any_row(
            lambda r: ((r.get('module_value_source') or {}).get('evidence_class') == 'authoritative_spec')
            or bool((r.get('module_value_source') or {}).get('authoritative_spec'))),
        'area-no-mapping': lambda: bool(wire.get('area_no_rule')) or any_row(
            lambda r: ((r.get('area_no_source') or {}).get('evidence_class') == 'authoritative_spec')),
        'channel.program.name.main': lambda: (not manual.get('pending_review')) and (
            manual.get('exact_semantic_matches') == manual.get('required_entries')),
        'string-lengths': lambda: bool((doc.get('string_lengths') or {}).get('authority')),
        'drive-parameter-manual-112023': lambda: any(
            '11/2023' in str(s) for s in (doc.get('resolved_sources') or [])),
        'axis.encoder.module_number': lambda: bool(
            (doc.get('probe_outputs') or {}).get('axis.encoder.module_number')),
        'hardware-validation': lambda: baseline.get('real_828d_test_in_this_change') is True or (
            bool(rows) and all(r.get('status') == 'verified' for r in rows)),
        'manual-entry-page-traceability': lambda: bool(rows) and all(
            (r.get('manual_semantic') or {}).get('page') not in (None, '') for r in rows),
        'device-software-version': lambda: bool(rows) and all(
            ((r.get('device_observation') or {}).get('device') or {}).get('software_version') not in (None, '')
            for r in rows),
    }


def check_r6(ctx, rep):
    doc = ctx.doc
    entries = doc.get('unresolved')
    if not isinstance(entries, list):
        rep.fail('R6', 'unresolved-missing', 'unresolved 缺失')
        return
    ids = []
    for i, entry in enumerate(entries):
        if not isinstance(entry, dict):
            rep.fail('R6', 'unresolved-entry-invalid', f'unresolved[{i}] 不是对象')
            continue
        eid = entry.get('id')
        if not isinstance(eid, str) or not eid.strip():
            rep.fail('R6', 'unresolved-id-missing', f'unresolved[{i}] 缺 id')
            continue
        ids.append(eid)
        if not isinstance(entry.get('question'), str) or not entry['question'].strip():
            rep.fail('R6', 'unresolved-question-missing', f'unresolved `{eid}` 缺 question')
        if entry.get('status') not in ('open', 'requires_explicit_authorization'):
            rep.fail('R6', 'unresolved-status-invalid',
                     f'unresolved `{eid}` 的 status={entry.get("status")!r}（只允许 open / '
                     f'requires_explicit_authorization）')
    dup = {x for x in ids if ids.count(x) > 1}
    for x in sorted(dup):
        rep.fail('R6', 'unresolved-duplicate', f'unresolved id `{x}` 重复')
    ctx.unresolved_ids = set(ids)

    # 6.1 行引用的 unresolved 必须真实存在
    for i, row in enumerate(doc.get('block_observations') or []):
        tag = row.get('block', f'#{i}')
        listed = row.get('unresolved')
        if not isinstance(listed, list):
            rep.fail('R6', 'row-unresolved-missing', f'block {tag} 缺 unresolved 列表')
            continue
        for ref in listed:
            if ref not in ctx.unresolved_ids:
                rep.fail('R6', 'row-unresolved-phantom',
                         f'block {tag} 引用了不存在的 unresolved id `{ref}`')
        for required in REQUIRED_ROW_UNRESOLVED:
            if required in ctx.unresolved_ids and required not in listed:
                rep.fail('R6', 'row-unresolved-incomplete',
                         f'block {tag} 未声明仍未解决的 `{required}`')

    # 6.2 过期条目：证据已解决却仍列在 unresolved
    preds = resolved_predicates(doc)
    for eid in ids:
        pred = preds.get(eid)
        if pred is None:
            rep.warn('R6', 'unresolved-without-predicate',
                     f'unresolved `{eid}` 没有登记解决判据，无法机械检测过期（请在 resolved_predicates 补一条）')
            continue
        try:
            done = bool(pred())
        except Exception as exc:  # 判据本身不得让校验崩掉
            rep.warn('R6', 'unresolved-predicate-error', f'unresolved `{eid}` 判据异常: {exc}')
            continue
        if done:
            rep.fail('R6', 'unresolved-stale',
                     f'unresolved `{eid}` 已被矩阵证据解决，必须从 unresolved 移除（防过期）')


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------

RULES = (
    ('R1', '证据出处完整性', None),   # R1 依赖 unresolved_ids，单独编排
    ('R2', '字段自相矛盾', check_r2),
    ('R3', '禁止语义越界', check_r3),
    ('R4', '三类证据分离', check_r4),
    ('R5', '基线完整性', None),
    ('R6', 'unresolved 真实性', None),
)


def validate_document(doc, repo='.', frozen_blob=R1_FROZEN_BLOB, matrix_path=DEFAULT_MATRIX):
    """对**内存中的**矩阵文档做全部 6 条规则校验（负向测试直接调用本函数）。"""
    rep = Report()
    ctx = Ctx(repo, matrix_path)
    ctx.doc = doc
    if not isinstance(doc, dict):
        rep.fail('R1', 'document-invalid', '矩阵顶层必须是对象')
        return rep
    if doc.get('schema_version') != SCHEMA_VERSION:
        rep.fail('R1', 'schema-version-invalid',
                 f'schema_version={doc.get("schema_version")!r}，期望 {SCHEMA_VERSION}')
    # unresolved 先行：R1/R5 需要知道哪些缺口已被显式登记
    check_r6(ctx, rep)
    check_r1(ctx, rep)
    check_r2(ctx, rep)
    check_r3(ctx, rep)
    check_r4(ctx, rep)
    check_r5(ctx, rep, frozen_blob)
    return rep


def print_report(rep, matrix_path, baseline):
    counts = rep.counts()
    print('NCK 原生地址证据矩阵校验（6 条规则）')
    print(f'  矩阵: {matrix_path}')
    if baseline:
        print(f'  基线: main={baseline.get("main_commit")} '
              f'r1_blob={baseline.get("r1_blob_sha1")} '
              f'real_828d_test_in_this_change={baseline.get("real_828d_test_in_this_change")}')
    print()
    if not rep.items:
        print('  无发现')
    for it in rep.items:
        print(f'{it["severity"]:<4} [{it["rule"]}] {it["code"]}: {it["message"]}')
    print()
    print(f'RESULT: {rep.verdict()}    (fail={counts["FAIL"]} warn={counts["WARN"]} info={counts["INFO"]})')


def main(argv=None):
    ap = argparse.ArgumentParser(description='校验 NCK 原生地址证据矩阵（纯标准库，离线）')
    ap.add_argument('--matrix', default=DEFAULT_MATRIX, help=f'矩阵路径（默认 {DEFAULT_MATRIX}）')
    ap.add_argument('--repo', default='.', help='仓库根（默认当前目录）')
    ap.add_argument('--r1-frozen-sha', default=R1_FROZEN_BLOB,
                    help='r1.json 冻结 blob（改动需评审后同步）')
    ap.add_argument('--allow-warn', action='store_true', help='WARN 也返回 0')
    ap.add_argument('--json', action='store_true', help='以 JSON 输出结论')
    args = ap.parse_args(argv)

    repo = os.path.abspath(args.repo)
    matrix_path = args.matrix if os.path.isabs(args.matrix) else os.path.join(repo, args.matrix)
    try:
        with open(matrix_path, encoding='utf-8') as f:
            doc = json.load(f)
    except OSError as e:
        print(f'FAIL [R1] matrix-not-readable: 无法读取矩阵 {matrix_path}: {e}')
        print('RESULT: FAIL    (fail=1 warn=0 info=0)')
        return 1
    except json.JSONDecodeError as e:
        print(f'FAIL [R1] matrix-not-json: 矩阵不是合法 JSON: {e}')
        print('RESULT: FAIL    (fail=1 warn=0 info=0)')
        return 1

    rep = validate_document(doc, repo=repo, frozen_blob=args.r1_frozen_sha, matrix_path=args.matrix)
    if args.json:
        print(json.dumps({'verdict': rep.verdict(), 'counts': rep.counts(), 'findings': rep.items},
                         ensure_ascii=False, indent=2))
    else:
        print_report(rep, args.matrix, doc.get('baseline') or {})
    verdict = rep.verdict()
    if verdict == 'FAIL':
        return 1
    if verdict == 'WARN':
        return 0 if args.allow_warn else 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
