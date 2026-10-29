#!/usr/bin/env python3
"""828D 定义 sidecar 元数据（`r1.meta.json`）的机械校验（纯标准库、离线）。

设计意图
--------
`drivers/sinumerik-nck/definitions/828d/r1.meta.json` 是本仓里**唯一**声称
「42 条定义的单位 / 手册位置 / 索引规则 / 可用性判定」的机器可读数据。它会经
`definition_api.rs` 的投影进入对外 API，被前端与运维当成事实读取，因此必须有
一条**可执行**的规则集把它钉死：

- 有人删条目、加条目、改 revision → M1/M2 抓；
- 有人把页码或单位写成「看起来像真的」但查无此事 → M3 抓（**真的去解析**，
  不是只看文件在不在）；
- 有人改了 `r1.json` 却没同步 sidecar 的绑定摘要 → M4 抓；
- 有人把「语义已验证」直接升级成「真机可采集」→ M6/M7 抓；
- 有人把方式组枚举这个已知未证明项悄悄改成 ready → M8 抓。

八条规则（编号与 CI 输出一致）
------------------------------
M1 定义↔条目双向一致 —— 42 条定义与 sidecar 条目必须**双向无孤儿、无重复、无遗漏**；
                      sidecar 顶层结构（format_version / family / binds_to /
                      entries）必须齐备且类型正确。
M2 逐条一致         —— `(variable_id, revision)` 必须与 `r1.json` 的对应定义**逐条相等**；
                      revision 必须是 >=1 的整数。
M3 引用必须真解析    —— `unit` 填值 / `manual_location.page` 非 null 时，其
                      `source_ref` 必须存在**且能按 fragment 真解析**：
                      JSON Pointer 必须取到值、`L<n>` 必须在行范围内、
                      `entry:<id>` 必须命中目标 JSON 里同 id 的条目；页码还必须能在
                      被引用的交叉引用条目里查到。另外 `unit.value` 必须**真的出现在**
                      被引用的节点里（防止引用写得像真的但内容对不上）。
M4 绑定摘要          —— `binds_to.definition_blob_sha1` 必须等于实测
                      `git hash-object r1.json`（git 缺失时用纯 Python blob 兜底），
                      且 `binds_to.definition_file` 必须指向真实存在的定义文件。
M5 状态与依据        —— `state` 只能是 reference_only / hold / ready / candidate
                      四态之一；`basis` 必须是非空数组且取值都在枚举内；
                      `hold` 必须有非空 `reason_zh`。
M6 verified ≠ ready  —— `r1.json` 的 `semantics_status=verified` **不得**直接映射为
                      `ready`：ready 必须另有 `device_read_evidence`；反过来
                      `basis` 声称 `device_read_verified` 时 state 必须是 ready，
                      ready 时 basis 必须含 `device_read_verified`。
M7 ready 的证据形状  —— `state="ready"` ⇒ `device_read_evidence.environment` 的
                      `controller` / `cnc_software` / `verified_at` / `artifact_ref`
                      四字段齐全且非空，`artifact_ref` 可解析，且
                      `scope` 固定为 `"definition_level_only"`（显式声明**不保证
                      当前机床存在该对象**）。
M8 方式组枚举        —— `mode_group.op_mode` 必须存在、必须为 `hold`，且 `basis`
                      必须含 `mode_group_enumeration_unproven`。

WARN（不阻断，但必须显式可见）
-----------------------------
- 条目顺序与 `r1.json` 定义顺序不一致（顺序是评审时逐条对账的锚点）；
- `availability_rules_version` 与脚本内已知版本不一致（判定表可能已换代，
  脚本无法机械核对）。

为什么 fragment 必须「真解析」
------------------------------
历史教训：`axis.encoder.module_number` 曾挂着一条 `measurement` 来源，指向的
证据文件确实存在，但文件里**没有**对应小节。只校验「文件存在」的脚本会让这种
引用永远通过。因此本脚本对每一种 fragment 都做真解析，并且对 `unit` 追加
「值必须出现在被引用处」这一条。

用法
----
    python3 tools/ci/check_definition_meta.py                 # 校验当前 sidecar
    python3 tools/ci/check_definition_meta.py --json          # 机器可读输出
    python3 tools/ci/check_definition_meta.py --allow-warn    # WARN 也返回 0
    python3 tools/ci/check_definition_meta.py --matrix PATH --repo DIR

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

# Windows 控制台默认 GBK 编码：本脚本的结论行含中文与 `…`，直接 print 在个别
# 代码页下会抛 UnicodeEncodeError（不是校验失败，却会让 CI 以退出码 1 结束）。
# 与 `test_nck_address_evidence.py` 同一处理口径。
try:
    sys.stdout.reconfigure(encoding='utf-8')
except (AttributeError, OSError):  # 极老 Python 或已重定向到不可重配置的流
    pass

DEFAULT_SIDECAR = 'drivers/sinumerik-nck/definitions/828d/r1.meta.json'
R1_REL = 'drivers/sinumerik-nck/definitions/828d/r1.json'
# r1.json 的 git blob（CRLF 工作树 → LF blob）。本变更内必须逐字节不变。
R1_FROZEN_BLOB = '3c0abe67cbcb64b525cddec1584e6e1747e0f062'
FORMAT_VERSION = 1
AVAILABILITY_RULES_VERSION = 1

STATES = ('reference_only', 'hold', 'ready', 'candidate')
BASIS_ENUM = (
    'no_device_read',
    'identity_scope_only',
    'string_length_unproven',
    'mode_group_enumeration_unproven',
    'module_value_authority_unresolved',
    'device_read_rejected',
    'measurement_retracted',
    'device_read_verified',
)
ENV_FIELDS = ('controller', 'cnc_software', 'verified_at', 'artifact_ref')
READY_SCOPE = 'definition_level_only'
MODE_GROUP_VARIABLE = 'mode_group.op_mode'
MODE_GROUP_BASIS = 'mode_group_enumeration_unproven'


class Report:
    """校验结论收集器：每条发现都带规则号与稳定 code，便于负向测试精确断言。"""

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

    def severity(self):
        """三档结论：有 FAIL 即 FAIL，其次 WARN，INFO 不降级。"""
        worst = 'OK'
        for it in self.items:
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
    """校验上下文：仓库根、sidecar 路径、定义文档、JSON 缓存、git 能力。"""

    def __init__(self, repo, sidecar_path, r1_doc):
        self.repo = repo
        self.sidecar_path = sidecar_path
        self.r1 = r1_doc
        self._json = {}

    def abs_path(self, rel):
        return rel if os.path.isabs(rel) else os.path.join(self.repo, rel.replace('/', os.sep))

    def load_json(self, rel):
        """按仓库相对路径载入 JSON（带缓存）；失败抛 OSError / ValueError。"""
        key = os.path.normcase(rel)
        if key in self._json:
            cached = self._json[key]
            if isinstance(cached, Exception):
                raise cached
            return cached
        try:
            with open(self.abs_path(rel), encoding='utf-8') as f:
                doc = json.load(f)
        except OSError as e:
            self._json[key] = e
            raise
        except json.JSONDecodeError as e:
            wrapped = ValueError(f'{rel} 不是合法 JSON：{e}')
            self._json[key] = wrapped
            raise wrapped
        self._json[key] = doc
        return doc

    def lines_of(self, rel):
        with open(self.abs_path(rel), encoding='utf-8') as f:
            return f.read().splitlines()


# ---------------------------------------------------------------------------
# 通用小工具
# ---------------------------------------------------------------------------

def normalize_text(text):
    """把「 / 」换行标记与连续空白归一，用于「值是否真的出现在引用处」的比对。"""
    text = str(text).replace(' / ', ' ')
    return ' '.join(text.split())


def json_pointer_get(doc, pointer):
    """按 RFC 6901 取值；取不到抛 KeyError / IndexError / TypeError。"""
    if pointer == '':
        return doc
    if not pointer.startswith('/'):
        raise ValueError(f'JSON Pointer 必须以 `/` 开头：{pointer!r}')
    cur = doc
    for raw in pointer.split('/')[1:]:
        token = raw.replace('~1', '/').replace('~0', '~')
        if isinstance(cur, dict):
            if token not in cur:
                raise KeyError(token)
            cur = cur[token]
        elif isinstance(cur, list):
            if not re.fullmatch(r'\d+', token):
                raise KeyError(token)
            idx = int(token)
            if idx >= len(cur):
                raise IndexError(token)
            cur = cur[idx]
        else:
            raise KeyError(token)
    return cur


def find_entry_by_id(doc, want):
    """在任意层级的数组里找 `id` / `variable_id` 等于 want 的对象（`entry:<id>` 片段）。"""
    found = []

    def walk(node):
        if isinstance(node, dict):
            for key in ('id', 'variable_id'):
                if node.get(key) == want:
                    found.append(node)
                    return
            for v in node.values():
                walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)

    walk(doc)
    return found


def resolve_ref(ctx, ref, rep, rule, code_missing, code_file, code_fragment):
    """解析 `<仓库相对路径>#<fragment>` 并**真的取到值**。

    返回 (ok, value)。失败时已经向 rep 记录 FAIL，调用方只需看 ok。
    fragment 四类：JSON Pointer（`/a/b`）、行号（`L<n>`）、条目 id（`entry:<id>`），
    以及「无 fragment」（不允许——那只能证明文件存在，正是要挡的写法）。
    """
    if not isinstance(ref, str) or not ref.strip():
        rep.fail(rule, code_missing, f'source_ref 缺失或不是非空字符串：{ref!r}')
        return False, None
    if '#' not in ref:
        rep.fail(rule, code_fragment,
                 f'`{ref}` 没有 `#fragment`：只写文件路径无法证明内容存在（必须写指针/行号/entry id）')
        return False, None
    rel, frag = ref.split('#', 1)
    if not rel.strip():
        rep.fail(rule, code_fragment, f'`{ref}` 缺少文件路径')
        return False, None
    if not frag.strip():
        rep.fail(rule, code_fragment, f'`{ref}` 的 fragment 为空（必须写指针/行号/entry id）')
        return False, None
    if not os.path.isfile(ctx.abs_path(rel)):
        rep.fail(rule, code_file, f'`{ref}` 指向的文件不存在：{rel}')
        return False, None
    if frag.startswith('/'):
        try:
            doc = ctx.load_json(rel)
        except (OSError, ValueError) as e:
            rep.fail(rule, code_file, f'`{ref}` 无法作为 JSON 载入：{e}')
            return False, None
        try:
            return True, json_pointer_get(doc, frag)
        except (KeyError, IndexError, ValueError) as e:
            rep.fail(rule, code_fragment, f'`{ref}` 的 JSON Pointer 取不到值（{type(e).__name__}: {e}）')
            return False, None
    m = re.fullmatch(r'L(\d+)', frag)
    if m:
        want = int(m.group(1))
        try:
            lines = ctx.lines_of(rel)
        except OSError as e:
            rep.fail(rule, code_file, f'`{ref}` 无法按文本读取：{e}')
            return False, None
        if want < 1 or want > len(lines):
            rep.fail(rule, code_fragment,
                     f'`{ref}` 越界：{rel} 只有 {len(lines)} 行，引用第 {want} 行')
            return False, None
        if not lines[want - 1].strip():
            rep.fail(rule, code_fragment, f'`{ref}` 指向的第 {want} 行是空行')
            return False, None
        return True, lines[want - 1]
    if frag.startswith('entry:'):
        want = frag[len('entry:'):].strip()
        if not want:
            rep.fail(rule, code_fragment, f'`{ref}` 的 `entry:` 后缺少 id')
            return False, None
        try:
            doc = ctx.load_json(rel)
        except (OSError, ValueError) as e:
            rep.fail(rule, code_file, f'`{ref}` 无法作为 JSON 载入：{e}')
            return False, None
        hits = find_entry_by_id(doc, want)
        if not hits:
            rep.fail(rule, code_fragment, f'`{ref}` 在 {rel} 中查无 id 为 `{want}` 的条目')
            return False, None
        return True, hits[0]
    rep.fail(rule, code_fragment,
             f'`{ref}` 的 fragment `{frag}` 不是四类之一（JSON Pointer / L<n> / entry:<id>）')
    return False, None


def r1_by_id(ctx):
    """`variable_id -> definition`（r1.json 的唯一真值）。"""
    out = {}
    for d in ctx.r1.get('definitions') or []:
        if isinstance(d, dict) and isinstance(d.get('variable_id'), str):
            out[d['variable_id']] = d
    return out


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
    with open(os.path.join(repo, rel.replace('/', os.sep)), 'rb') as f:
        data = f.read()

    def blob(b):
        return hashlib.sha1(b'blob %d\0' % len(b) + b).hexdigest()

    return {'raw': blob(data), 'lf': blob(data.replace(b'\r\n', b'\n'))}


def iter_entries(doc):
    """sidecar 条目迭代器；非对象条目原样产出（由 M1 负责报错）。"""
    entries = doc.get('entries')
    if not isinstance(entries, list):
        return []
    return list(entries)


# ---------------------------------------------------------------------------
# M1：定义↔条目双向一致 + 顶层结构
# ---------------------------------------------------------------------------

def check_m1(ctx, doc, rep):
    if not isinstance(doc, dict):
        rep.fail('M1', 'document-invalid', 'sidecar 顶层必须是 JSON 对象')
        return
    if doc.get('format_version') != FORMAT_VERSION:
        rep.fail('M1', 'format-version-invalid',
                 f'format_version={doc.get("format_version")!r}，期望 {FORMAT_VERSION}')
    family = ctx.r1.get('family')
    if not isinstance(doc.get('family'), str) or not doc['family'].strip():
        rep.fail('M1', 'family-missing', 'sidecar 顶层 `family` 必须是非空字符串')
    elif doc['family'] != family:
        rep.fail('M1', 'family-mismatch',
                 f'sidecar family={doc["family"]!r} 与 r1.json family={family!r} 不一致')
    binds = doc.get('binds_to')
    if not isinstance(binds, dict):
        rep.fail('M1', 'binds-to-missing', 'sidecar 顶层 `binds_to` 必须是对象')
    else:
        for key in ('definition_file', 'definition_blob_sha1'):
            if not isinstance(binds.get(key), str) or not binds[key].strip():
                rep.fail('M1', f'binds-to-{key}-missing',
                         f'`binds_to.{key}` 必须是非空字符串')
    rules_version = doc.get('availability_rules_version')
    if not isinstance(rules_version, int) or isinstance(rules_version, bool):
        rep.fail('M1', 'rules-version-invalid',
                 f'`availability_rules_version` 必须是整数，实际 {rules_version!r}')
    elif rules_version != AVAILABILITY_RULES_VERSION:
        rep.warn('M1', 'rules-version-unknown',
                 f'`availability_rules_version`={rules_version} 与本脚本已知版本 '
                 f'{AVAILABILITY_RULES_VERSION} 不一致：判定表可能已换代，需人工确认后同步脚本')

    entries = doc.get('entries')
    if not isinstance(entries, list):
        rep.fail('M1', 'entries-invalid', 'sidecar `entries` 必须是数组')
        return

    raw = ctx.r1.get('definitions') or []
    want_ids = [d.get('variable_id') for d in raw if isinstance(d, dict)]
    seen = {}
    order = []
    for i, e in enumerate(entries):
        if not isinstance(e, dict):
            rep.fail('M1', 'entry-not-object', f'entries[{i}] 不是 JSON 对象')
            continue
        vid = e.get('variable_id')
        if not isinstance(vid, str) or not vid.strip():
            rep.fail('M1', 'entry-variable-id-invalid',
                     f'entries[{i}].variable_id 必须是非空字符串，实际 {vid!r}')
            continue
        if vid in seen:
            rep.fail('M1', 'entry-duplicate',
                     f'`{vid}` 在 sidecar 中重复出现（entries[{seen[vid]}] 与 entries[{i}]）')
            continue
        seen[vid] = i
        order.append(vid)

    known = set(want_ids)
    for vid, i in sorted(seen.items(), key=lambda kv: kv[1]):
        if vid not in known:
            rep.fail('M1', 'entry-orphan',
                     f'entries[{i}] 的 `{vid}` 在 r1.json 中不存在（孤儿条目）')
    for vid in want_ids:
        if vid not in seen:
            rep.fail('M1', 'definition-missing',
                     f'r1.json 的定义 `{vid}` 在 sidecar 中没有对应条目（遗漏）')

    # 顺序漂移：集合一致但顺序不同 ⇒ WARN（逐条对账时容易看错行）
    if order and set(order) == known and order != want_ids:
        first = next((i for i, (a, b) in enumerate(zip(order, want_ids)) if a != b), None)
        rep.warn('M1', 'entry-order-drift',
                 f'sidecar 条目顺序与 r1.json 定义顺序不一致（首个差异在第 {first} 条：'
                 f'{order[first]!r} vs {want_ids[first]!r}）')
    return


# ---------------------------------------------------------------------------
# M2：(variable_id, revision) 与 r1.json 逐条一致
# ---------------------------------------------------------------------------

def check_m2(ctx, doc, rep):
    if not isinstance(doc, dict):
        return
    r1 = r1_by_id(ctx)
    for i, e in enumerate(iter_entries(doc)):
        if not isinstance(e, dict):
            continue  # M1 已报
        vid = e.get('variable_id')
        if not isinstance(vid, str) or vid not in r1:
            continue  # M1 已报孤儿/形状
        rev = e.get('revision')
        if not isinstance(rev, int) or isinstance(rev, bool) or rev < 1:
            rep.fail('M2', 'revision-invalid',
                     f'`{vid}` 的 revision 必须是 >=1 的整数，实际 {rev!r}')
            continue
        want = r1[vid].get('revision')
        if rev != want:
            rep.fail('M2', 'revision-mismatch',
                     f'`{vid}` 的 revision={rev} 与 r1.json 的 {want} 不一致')


# ---------------------------------------------------------------------------
# M3：unit / page 的 source_ref 必须真解析
# ---------------------------------------------------------------------------

def check_unit_ref(ctx, vid, unit, rep):
    if not isinstance(unit, dict):
        rep.fail('M3', 'unit-not-object', f'`{vid}` 的 `unit` 必须是对象，实际 {type(unit).__name__}')
        return
    value = unit.get('value')
    status = unit.get('status')
    if status not in ('verified', 'unknown'):
        rep.fail('M3', 'unit-status-invalid',
                 f'`{vid}` 的 unit.status 只能是 verified / unknown，实际 {status!r}')
    if value is None:
        # 未填值 ⇒ 不要求引用（查不到就是 unknown，这是诚实状态）
        if status == 'verified':
            rep.fail('M3', 'unit-verified-without-value',
                     f'`{vid}` 的 unit.status=verified 但 value 为 null')
        return
    if not isinstance(value, str) or not value.strip():
        rep.fail('M3', 'unit-value-invalid',
                 f'`{vid}` 的 unit.value 必须是非空字符串，实际 {value!r}')
        return
    if status != 'verified':
        rep.fail('M3', 'unit-value-without-verified',
                 f'`{vid}` 的 unit.value 已填（{value!r}）但 status={status!r}，必须为 verified')
    ok, resolved = resolve_ref(ctx, unit.get('source_ref'), rep, 'M3',
                               'unit-source-ref-missing', 'unit-source-ref-file',
                               'unit-source-ref-fragment')
    if not ok:
        return
    text = resolved if isinstance(resolved, str) else json.dumps(resolved, ensure_ascii=False)
    if normalize_text(value) not in normalize_text(text):
        rep.fail('M3', 'unit-value-not-in-source',
                 f'`{vid}` 声明的单位 {value!r} 在被引用节点里查不到（引用处内容：'
                 f'{normalize_text(text)[:120]!r}…）')


def check_page_ref(ctx, vid, loc, rep):
    if not isinstance(loc, dict):
        rep.fail('M3', 'manual-location-not-object',
                 f'`{vid}` 的 `manual_location` 必须是对象，实际 {type(loc).__name__}')
        return
    for key in ('document_number', 'version', 'section', 'source_ref'):
        v = loc.get(key)
        if not isinstance(v, str) or not v.strip():
            rep.fail('M3', f'manual-location-{key}-invalid',
                     f'`{vid}` 的 manual_location.{key} 必须是非空字符串，实际 {v!r}')
    page = loc.get('page')
    if page is None:
        # 没有页码 ⇒ 仍必须给出可解析的说明性引用（否则等于没有出处）
        resolve_ref(ctx, loc.get('source_ref'), rep, 'M3',
                    'manual-location-source-ref-missing', 'manual-location-source-ref-file',
                    'manual-location-source-ref-fragment')
        return
    if not isinstance(page, int) or isinstance(page, bool) or page < 1:
        rep.fail('M3', 'manual-location-page-invalid',
                 f'`{vid}` 的 manual_location.page 必须是 >=1 的整数或 null，实际 {page!r}')
        return
    ok, resolved = resolve_ref(ctx, loc.get('source_ref'), rep, 'M3',
                               'manual-location-source-ref-missing',
                               'manual-location-source-ref-file',
                               'manual-location-source-ref-fragment')
    if not ok:
        return
    if not isinstance(resolved, dict):
        rep.fail('M3', 'page-ref-not-entry',
                 f'`{vid}` 的 source_ref 必须指向交叉引用条目对象（含 page 列表），实际 {type(resolved).__name__}')
        return
    pages = resolved.get('page')
    if not isinstance(pages, list):
        rep.fail('M3', 'page-list-missing',
                 f'`{vid}` 的 source_ref 指向的条目没有 `page` 列表，无法核对页码 {page}')
        return
    if page not in pages:
        rep.fail('M3', 'page-not-found',
                 f'`{vid}` 的页码 {page} 不在被引用条目的 page 列表 {pages} 中（疑似虚构页码）')


def check_m3(ctx, doc, rep):
    if not isinstance(doc, dict):
        return
    for e in iter_entries(doc):
        if not isinstance(e, dict):
            continue
        vid = e.get('variable_id')
        if not isinstance(vid, str):
            continue
        check_unit_ref(ctx, vid, e.get('unit'), rep)
        check_page_ref(ctx, vid, e.get('manual_location'), rep)
        rule = e.get('index_rule')
        if isinstance(rule, dict):
            resolve_ref(ctx, rule.get('source_ref'), rep, 'M3',
                        'index-rule-source-ref-missing', 'index-rule-source-ref-file',
                        'index-rule-source-ref-fragment')


# ---------------------------------------------------------------------------
# M4：binds_to 摘要 == 实测 r1.json blob
# ---------------------------------------------------------------------------

def check_m4(ctx, doc, rep, frozen_blob=R1_FROZEN_BLOB):
    if not isinstance(doc, dict):
        return
    binds = doc.get('binds_to')
    if not isinstance(binds, dict):
        rep.fail('M4', 'binds-to-missing', 'sidecar 缺少 `binds_to`，无法证明它绑定的是哪份定义')
        return
    declared = binds.get('definition_blob_sha1')
    if not isinstance(declared, str) or not re.fullmatch(r'[0-9a-f]{40}', declared):
        rep.fail('M4', 'blob-sha-invalid',
                 f'`binds_to.definition_blob_sha1` 必须是 40 位小写十六进制，实际 {declared!r}')
        declared = None
    rel = binds.get('definition_file')
    if not isinstance(rel, str) or not rel.strip():
        rep.fail('M4', 'definition-file-missing', '`binds_to.definition_file` 缺失')
        return
    # 定义文件路径：绝对路径直接用；否则依次按「sidecar 同目录」→「仓库根」→
    # 「r1.json 所在定义目录」解析。第三个候选是必要的：`definition_file` 的
    # 语义是「与 sidecar 同目录的定义文件名」，而 sidecar 被拷到临时目录做负向
    # 测试时，只有按定义目录解析才能仍然指向真正的 r1.json。
    r1_dir = os.path.dirname(ctx.abs_path(R1_REL))
    candidates = [ctx.abs_path(rel)] if os.path.isabs(rel) else [
        os.path.join(os.path.dirname(ctx.sidecar_path), rel.replace('/', os.sep)),
        ctx.abs_path(rel),
        os.path.join(r1_dir, rel.replace('/', os.sep)),
    ]
    target = next((p for p in candidates if os.path.isfile(p)), None)
    if target is None:
        rep.fail('M4', 'definition-file-not-found',
                 f'`binds_to.definition_file`={rel!r} 解析不到真实文件（试过 {candidates}）')
        return
    if os.path.normcase(os.path.abspath(target)) != os.path.normcase(ctx.abs_path(R1_REL)):
        rep.fail('M4', 'definition-file-unexpected',
                 f'`binds_to.definition_file`={rel!r} 指向 {target}，不是 r1.json（{R1_REL}）')
        return
    measured, why = git_blob_sha(ctx.repo, R1_REL)
    if measured is None:
        cands = python_blob_sha(ctx.repo, R1_REL)
        rep.info('M4', 'blob-measured-without-git', f'{why}；改用纯 Python blob 计算（raw/lf）')
        if declared is not None and declared not in (cands['raw'], cands['lf']):
            rep.fail('M4', 'blob-mismatch',
                     f'实测 r1.json blob（raw={cands["raw"]} lf={cands["lf"]}）≠ 声明 {declared}')
        if frozen_blob and frozen_blob not in (cands['raw'], cands['lf']):
            rep.fail('M4', 'r1-blob-frozen-mismatch',
                     f'实测 r1.json blob 偏离冻结值 {frozen_blob}（本变更内 r1.json 一字节不许动）')
        elif declared is not None:
            rep.info('M4', 'r1-blob-verified', f'r1.json blob 未变：{declared}')
        return
    if declared is not None and declared != measured:
        rep.fail('M4', 'blob-mismatch',
                 f'`git hash-object {R1_REL}` 实测 {measured} ≠ 声明 {declared}')
    if frozen_blob and measured != frozen_blob:
        rep.fail('M4', 'r1-blob-frozen-mismatch',
                 f'实测 r1.json blob {measured} 偏离冻结值 {frozen_blob}'
                 f'（本变更内 r1.json 一字节不许动）')
    elif declared is not None:
        rep.info('M4', 'r1-blob-verified', f'r1.json blob 未变：{measured}')


# ---------------------------------------------------------------------------
# M5：state / basis / reason_zh
# ---------------------------------------------------------------------------

def check_m5(ctx, doc, rep):
    if not isinstance(doc, dict):
        return
    for e in iter_entries(doc):
        if not isinstance(e, dict):
            continue
        vid = e.get('variable_id')
        if not isinstance(vid, str):
            continue
        avail = e.get('availability')
        if not isinstance(avail, dict):
            rep.fail('M5', 'availability-not-object',
                     f'`{vid}` 的 `availability` 必须是对象，实际 {type(avail).__name__}')
            continue
        state = avail.get('state')
        if not isinstance(state, str):
            rep.fail('M5', 'state-missing', f'`{vid}` 的 availability.state 缺失或不是字符串：{state!r}')
        elif state not in STATES:
            rep.fail('M5', 'state-invalid',
                     f'`{vid}` 的 state={state!r} 不在四态 {STATES} 内')
        basis = avail.get('basis')
        if not isinstance(basis, list) or not basis:
            rep.fail('M5', 'basis-empty',
                     f'`{vid}` 的 availability.basis 必须是非空数组，实际 {basis!r}')
        else:
            bad = [b for b in basis if b not in BASIS_ENUM]
            if bad:
                rep.fail('M5', 'basis-unknown',
                         f'`{vid}` 的 basis 含枚举外的取值 {bad}（允许值见 BASIS_ENUM）')
            if len(set(basis)) != len(basis):
                rep.fail('M5', 'basis-duplicate', f'`{vid}` 的 basis 有重复项：{basis}')
        reason = avail.get('reason_zh')
        if reason is not None and not isinstance(reason, str):
            rep.fail('M5', 'reason-zh-invalid',
                     f'`{vid}` 的 availability.reason_zh 必须是字符串，实际 {type(reason).__name__}')
        if state == 'hold':
            if not isinstance(reason, str) or not reason.strip():
                rep.fail('M5', 'hold-without-reason',
                         f'`{vid}` 的 state=hold 必须给出非空 reason_zh（说明为什么必须暂缓）')


# ---------------------------------------------------------------------------
# M6：semantics_status=verified 不得直接映射为 ready
# ---------------------------------------------------------------------------

def check_m6(ctx, doc, rep):
    if not isinstance(doc, dict):
        return
    r1 = r1_by_id(ctx)
    for e in iter_entries(doc):
        if not isinstance(e, dict):
            continue
        vid = e.get('variable_id')
        if not isinstance(vid, str):
            continue
        avail = e.get('availability')
        if not isinstance(avail, dict):
            continue
        state = avail.get('state')
        basis = avail.get('basis') if isinstance(avail.get('basis'), list) else []
        has_evidence = isinstance(e.get('device_read_evidence'), dict) and bool(e['device_read_evidence'])
        if state == 'ready':
            semantics = (r1.get(vid) or {}).get('semantics_status')
            if semantics != 'verified':
                rep.fail('M6', 'ready-without-verified-semantics',
                         f'`{vid}` 被判 ready，但 r1.json 的 semantics_status={semantics!r}（ready 要求语义已验证）')
            if not has_evidence:
                rep.fail('M6', 'ready-without-device-read-evidence',
                         f'`{vid}` 被判 ready，但没有 `device_read_evidence`：'
                         f'语义已验证 ≠ 真机已读取，缺证据不得升 ready')
            if 'device_read_verified' not in basis:
                rep.fail('M6', 'ready-basis-missing-device-read',
                         f'`{vid}` 被判 ready，但 basis 不含 `device_read_verified`')
        elif 'device_read_verified' in basis:
            rep.fail('M6', 'device-read-basis-without-ready-state',
                     f'`{vid}` 的 basis 声称 `device_read_verified`，但 state={state!r}（应为 ready）')


# ---------------------------------------------------------------------------
# M7：ready 的证据形状
# ---------------------------------------------------------------------------

def check_m7(ctx, doc, rep):
    if not isinstance(doc, dict):
        return
    for e in iter_entries(doc):
        if not isinstance(e, dict):
            continue
        vid = e.get('variable_id')
        if not isinstance(vid, str):
            continue
        avail = e.get('availability')
        if not isinstance(avail, dict) or avail.get('state') != 'ready':
            continue
        ev = e.get('device_read_evidence')
        if not isinstance(ev, dict):
            rep.fail('M7', 'ready-evidence-missing',
                     f'`{vid}` 的 state=ready 但 device_read_evidence 缺失或不是对象')
            continue
        env = ev.get('environment')
        if not isinstance(env, dict):
            rep.fail('M7', 'ready-environment-missing',
                     f'`{vid}` 的 device_read_evidence.environment 缺失或不是对象')
        else:
            for field in ENV_FIELDS:
                v = env.get(field)
                if not isinstance(v, str) or not v.strip():
                    rep.fail('M7', f'ready-environment-{field}-invalid',
                             f'`{vid}` 的 environment.{field} 必须是非空字符串，实际 {v!r}')
            artifact = env.get('artifact_ref')
            if isinstance(artifact, str) and artifact.strip():
                ref = artifact if '#' in artifact else artifact + '#L1'
                resolve_ref(ctx, ref, rep, 'M7',
                            'ready-artifact-ref-missing', 'ready-artifact-unresolvable',
                            'ready-artifact-unresolvable')
        if ev.get('scope') != READY_SCOPE:
            rep.fail('M7', 'ready-scope-invalid',
                     f'`{vid}` 的 device_read_evidence.scope={ev.get("scope")!r}，'
                     f'必须固定为 {READY_SCOPE!r}（不保证当前机床存在该对象）')


# ---------------------------------------------------------------------------
# M8：方式组枚举未证明 ⇒ mode_group.op_mode 必须 hold
# ---------------------------------------------------------------------------

def check_m8(ctx, doc, rep):
    if not isinstance(doc, dict):
        return
    hit = None
    for e in iter_entries(doc):
        if isinstance(e, dict) and e.get('variable_id') == MODE_GROUP_VARIABLE:
            hit = e
            break
    if hit is None:
        rep.fail('M8', 'mode-group-entry-missing',
                 f'sidecar 中找不到 `{MODE_GROUP_VARIABLE}` 条目（该条必须显式判定为 hold）')
        return
    avail = hit.get('availability')
    if not isinstance(avail, dict):
        rep.fail('M8', 'mode-group-availability-missing',
                 f'`{MODE_GROUP_VARIABLE}` 缺少 availability 对象')
        return
    state = avail.get('state')
    if state != 'hold':
        rep.fail('M8', 'mode-group-not-hold',
                 f'`{MODE_GROUP_VARIABLE}` 的 state={state!r}，必须为 hold'
                 f'（方式组枚举未证明，不得判 ready/candidate）')
    basis = avail.get('basis')
    if not isinstance(basis, list) or MODE_GROUP_BASIS not in basis:
        rep.fail('M8', 'mode-group-basis-missing',
                 f'`{MODE_GROUP_VARIABLE}` 的 basis 必须含 `{MODE_GROUP_BASIS}`，实际 {basis!r}')


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------

def validate_document(doc, repo='.', sidecar_path=None, frozen_blob=R1_FROZEN_BLOB):
    """对**内存中的** sidecar 文档做全部 8 条规则校验（负向测试直接调用本函数）。"""
    rep = Report()
    r1_path = os.path.join(repo, R1_REL.replace('/', os.sep))
    try:
        with open(r1_path, encoding='utf-8') as f:
            r1_doc = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        rep.fail('M1', 'r1-not-readable', f'无法读取 {R1_REL}：{e}')
        return rep
    ctx = Ctx(repo, sidecar_path or os.path.join(repo, DEFAULT_SIDECAR.replace('/', os.sep)), r1_doc)
    if not isinstance(doc, dict):
        rep.fail('M1', 'document-invalid', 'sidecar 顶层必须是 JSON 对象')
        return rep
    check_m1(ctx, doc, rep)
    check_m2(ctx, doc, rep)
    check_m3(ctx, doc, rep)
    check_m4(ctx, doc, rep, frozen_blob)
    check_m5(ctx, doc, rep)
    check_m6(ctx, doc, rep)
    check_m7(ctx, doc, rep)
    check_m8(ctx, doc, rep)
    return rep


def state_summary(doc):
    """按 state 汇总（报告与 --json 共用；不是判定，只是可读性）。"""
    summary = {s: 0 for s in STATES}
    unit_verified = 0
    page_filled = 0
    for e in iter_entries(doc):
        if not isinstance(e, dict):
            continue
        avail = e.get('availability')
        if isinstance(avail, dict) and avail.get('state') in summary:
            summary[avail['state']] += 1
        unit = e.get('unit')
        if isinstance(unit, dict) and unit.get('status') == 'verified':
            unit_verified += 1
        loc = e.get('manual_location')
        if isinstance(loc, dict) and loc.get('page') is not None:
            page_filled += 1
    return {'by_state': summary, 'unit_verified': unit_verified, 'page_filled': page_filled}


def print_report(rep, sidecar, doc):
    counts = rep.counts()
    print('828D 定义 sidecar 元数据校验（M1–M8）')
    print(f'  sidecar: {sidecar}')
    if isinstance(doc, dict):
        s = state_summary(doc)
        print(f'  条目: {len(iter_entries(doc))}    state: '
              + '  '.join(f'{k}={v}' for k, v in s['by_state'].items())
              + f'    unit verified={s["unit_verified"]}    page={s["page_filled"]}')
    print()
    if not rep.items:
        print('  无发现')
    for it in rep.items:
        print(f'{it["severity"]:<4} [{it["rule"]}] {it["code"]}: {it["message"]}')
    print()
    print(f'RESULT: {rep.verdict()}    (fail={counts["FAIL"]} warn={counts["WARN"]} info={counts["INFO"]})')


def main(argv=None):
    ap = argparse.ArgumentParser(description='校验 828D 定义 sidecar 元数据（纯标准库，离线）')
    ap.add_argument('--matrix', default=DEFAULT_SIDECAR, help=f'sidecar 路径（默认 {DEFAULT_SIDECAR}）')
    ap.add_argument('--repo', default='.', help='仓库根（默认当前目录）')
    ap.add_argument('--r1-frozen-sha', default=R1_FROZEN_BLOB,
                    help='r1.json 冻结 blob（改动需评审后同步）')
    ap.add_argument('--allow-warn', action='store_true', help='WARN 也返回 0')
    ap.add_argument('--json', action='store_true', help='以 JSON 输出结论')
    args = ap.parse_args(argv)

    repo = os.path.abspath(args.repo)
    sidecar = args.matrix if os.path.isabs(args.matrix) else os.path.join(repo, args.matrix)
    try:
        with open(sidecar, encoding='utf-8') as f:
            doc = json.load(f)
    except OSError as e:
        print(f'FAIL [M1] sidecar-not-readable: 无法读取 sidecar {sidecar}: {e}')
        print('RESULT: FAIL    (fail=1 warn=0 info=0)')
        return 1
    except json.JSONDecodeError as e:
        print(f'FAIL [M1] sidecar-not-json: sidecar 不是合法 JSON: {e}')
        print('RESULT: FAIL    (fail=1 warn=0 info=0)')
        return 1

    rep = validate_document(doc, repo=repo, sidecar_path=sidecar, frozen_blob=args.r1_frozen_sha)
    if args.json:
        print(json.dumps({
            'verdict': rep.verdict(),
            'counts': rep.counts(),
            'summary': state_summary(doc),
            'findings': rep.items,
        }, ensure_ascii=False, indent=2))
    else:
        print_report(rep, args.matrix, doc)
    verdict = rep.verdict()
    if verdict == 'FAIL':
        return 1
    if verdict == 'WARN':
        return 0 if args.allow_warn else 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
