"""NCK 原生寻址 REST 端到端验证（配置 → 采集 → 数据处理 → REST 展示）。

链路：模拟器（真实 TCP）+ mesad（临时 DB / 独立端口）+ REST：
  POST /endpoints → PUT /tasks/{id} → POST /start → GET /points/latest
验证：
  1. `nck_native` 原生字段任务能被保存、启动并采到 GOOD 值（DriverResolved 类型正确）；
  2. 浏览模板 → 补实例 → `identity-preview` 取权威 Key → 保存 → 采集（Web 实际路径）；
  3. 端点重启后 point_id 稳定（PointMap 由 Core 持久化）；
  4. 旧目录字段（block/variable）不会被静默接受（保存或 Configure 必须明确拒绝）。

契约基线（改动此脚本时必读）：NCK 任务**只接受** `nck.points.v1` 信封
（`{points:[{kind,point_key,parameters}]}`，`crates/core-types/src/point.rs:82`）；
旧 `mesa.resources.v1` 被驱动以 `UNSUPPORTED_BINDING` 整体拒绝（ADR 0005）。
`identity_mode=derived`（缺省）下 `point_key` 必须逐字等于地址/身份模板推导出的
规范 Key，否则 `POINT_KEY_MISMATCH` —— 因此身份 Key 只能由驱动的
`identity-preview` 入口计算，脚本不得自行拼接。

不触碰本机真实配置：使用临时 DB 与独立 HTTP 端口，不占用生产 8132。

跨平台（CI 双平台 job：`.github/workflows/_e2e.yml`，Windows + Linux 并行）
--------------------------------------------------------------------
本脚本原来只在 Windows 上跑过，接入 CI 前逐项消除了平台假设：

  * **二进制名**：Windows 带 `.exe`、Linux 不带，统一由 `bin_path()` 解析，
    不再出现 `mesad.exe` 这类硬编码扩展名。
  * **HTTP 端口**：不再硬编码 18133，改为**动态分配**（`free_port()`，排除 8132）。
    `free_port()` 是"绑定 0 端口取号 → 释放 → 由 mesad 绑定"的两步法，中间存在
    TOCTOU 窗口；因此 `start_mesad()` 在绑定失败（"address in use" 类错误）时
    **换端口重试**，最多 3 次——CI 上多个 job 并行时不会因固定端口互相踩。
  * **路径**：`--drivers-dir` 传绝对路径（不再依赖 cwd 相对解析）；
    `tempfile.mkdtemp` + `shutil.rmtree` 两端语义一致。
  * **进程终止**：`terminate()` → `wait(10)` → `kill()`，在 POSIX 上是 SIGTERM，
    在 Windows 上是 TerminateProcess；mesad 派生的驱动子进程由 Core 的
    Job Object / PR_SET_PDEATHSIG 兜底，脚本不假设信号语义。
  * **编码**：子进程输出一律按 UTF-8 解码（Rust 的 eprintln 两端都是 UTF-8）；
    stdout/stderr 只把 `errors` 放宽为 `replace`，**不改编码**——CI 侧用
    `PYTHONIOENCODING=utf-8` 保证日志是 UTF-8，本地控制台（如 cp936）也不会
    因中文断言名抛 `UnicodeEncodeError` 而假失败。
  * **子进程日志**：mesad / 模拟器的 stdout、stderr 落盘到证据目录，
    失败时打印尾部——两个平台的 CI 都能直接定位"起不来"还是"断言不过"。

用法：
  python scripts/nck-native-rest-e2e.py [--evidence-dir DIR]

前置（缺任何一项都会以 exit 2 **明确失败**，绝不静默跳过）：
  cargo build --locked --bins -p mesad -p mesa-nck-emulator -p mesa-driver-sinumerik-nck
"""
import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

# 本脚本位于 scripts/，仓库根是其父目录。
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# 平台相关：Windows 的二进制带 .exe，Linux/macOS 不带。
IS_WINDOWS = os.name == 'nt'
EXE_SUFFIX = '.exe' if IS_WINDOWS else ''

# 生产端口：任何情况下都不得占用（本地开发机上 8132 可能正跑着生产 mesad）。
PRODUCTION_PORT = 8132

READY_PREFIX = 'MESA_NCK_EMULATOR_READY='

# 任务信封 / 点位 kind 常量（与 Core/驱动的公开契约同名，避免脚本里出现"魔法字符串"）。
NCK_POINTS_KIND = 'nck.points.v1'      # Task Binding Kind（point.rs:82）
POINT_KIND_VARIABLE = 'nck_variable'   # 语义点位（定义目录 + target/arguments）
POINT_KIND_NATIVE = 'nck_native'       # 原生 12 字段点位

# 原生地址 C/1/module18/column42/line1 的规范身份（`identity_mode=derived` 要求
# point_key 逐字等于它，否则驱动以 `POINT_KEY_MISMATCH` 拒绝）。
NATIVE_KEY = 'native.C.1.18.42.1'

# 采集等待预算（CI runner 比开发机慢：从 20s 放宽到 30s；断言本身不放宽）。
POINT_WAIT_S = 30
MESAD_READY_S = 60
EMULATOR_READY_S = 20


class E2EFailure(Exception):
    """前置条件或链路失败（不是断言失败）：用于在 finally 里统一出证据。"""


def bin_path(name):
    """target/debug/<name>[.exe]（不硬编码平台扩展名）。"""
    return os.path.join(ROOT, 'target', 'debug', name + EXE_SUFFIX)


EMU = bin_path('mesa-nck-emulator')
MESAD = bin_path('mesad')
DRIVER = bin_path('mesa-driver-sinumerik-nck')
DRIVERS_DIR = os.path.join(ROOT, 'drivers')


def _ensure_stdio():
    """让控制台在非 UTF-8 代码页上也不会因中文输出崩掉（不改编码，只放宽 errors）。

    CI 侧用 `PYTHONIOENCODING=utf-8` 让日志是真正的 UTF-8；这里只是兜底：
    Windows runner 的控制台代码页可能是 cp1252，直接 print 中文会抛
    UnicodeEncodeError，那会被误判成"测试失败"。
    """
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors='replace')
        except (AttributeError, ValueError, OSError):
            pass


def req(url, method='GET', body=None, timeout=15):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(url, data=data, method=method,
                               headers={'content-type': 'application/json'})
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            raw = resp.read().decode()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        try:
            return e.code, json.loads(raw)
        except json.JSONDecodeError:
            return e.code, raw


def wait_http(base, seconds=MESAD_READY_S, proc=None):
    """等 REST 就绪；给了 proc 时，进程一旦退出立即返回（端口冲突可快速重试）。"""
    for _ in range(seconds * 2):
        if proc is not None and proc.poll() is not None:
            return False
        try:
            code, _ = req(base + '/api/v1/drivers', timeout=3)
            if code == 200:
                return True
        except OSError:
            pass
        time.sleep(0.5)
    return False


def poll_until(fn, timeout_s=POINT_WAIT_S, interval_s=0.5):
    """轮询到 fn() 返回真值；超时返回 None。"""
    deadline = time.time() + timeout_s
    while True:
        value = fn()
        if value:
            return value
        if time.time() >= deadline:
            return None
        time.sleep(interval_s)


def free_port():
    """取一个当前空闲的回环端口（绑定 0 端口 → 立即释放）。

    注意这是"取号"而非"预占"：真正绑定发生在 mesad 进程内，中间有 TOCTOU 窗口，
    所以 `start_mesad()` 必须能在绑定失败时换端口重试。
    """
    for _ in range(20):
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(('127.0.0.1', 0))
            port = s.getsockname()[1]
        if port != PRODUCTION_PORT:
            return port
    raise E2EFailure('无法分配到空闲的回环端口（连续 20 次都拿到 8132）')


def _newest_source(src_dir):
    """递归求源码目录内最新 mtime（返回 (mtime, path)）。"""
    newest, newest_path = 0.0, ''
    for dirpath, _dirnames, filenames in os.walk(src_dir):
        for name in filenames:
            if not name.endswith('.rs'):
                continue
            p = os.path.join(dirpath, name)
            m = os.path.getmtime(p)
            if m > newest:
                newest, newest_path = m, p
    return newest, newest_path


def check_binaries():
    """三个二进制必须存在，且**不能比源码旧**。

    切分支后忘记重编会拉起**另一份源码**构建的驱动：原生字段被判成非法（400）、
    identity/浏览直接 503，报错与真实缺陷高度相似。AGENTS.md 已就此警告过
    （"旧二进制让故障注入静默失效"），这里把它变成启动前的硬检查。

    局限（NOTE）：只比对各二进制**自身 crate** 的 src，不覆盖其依赖 crate；
    依赖改动而未重编的情况仍需靠 `cargo build` 的常规流程保证。
    """
    build_hint = ('cargo build --locked --bins -p mesad -p mesa-nck-emulator '
                  '-p mesa-driver-sinumerik-nck')
    specs = [
        (EMU, os.path.join(ROOT, 'tools', 'nck-emulator', 'src')),
        (DRIVER, os.path.join(ROOT, 'drivers', 'sinumerik-nck', 'src')),
        (MESAD, os.path.join(ROOT, 'apps', 'mesad', 'src')),
    ]
    for exe, src_dir in specs:
        rel = os.path.relpath(exe, ROOT)
        if not os.path.isfile(exe):
            return False, '缺少 %s（先 %s）' % (rel, build_hint)
        if not os.path.isdir(src_dir):
            return False, '缺少源码目录 %s（仓库不完整）' % os.path.relpath(src_dir, ROOT)
        newest, newest_path = _newest_source(src_dir)
        if newest > os.path.getmtime(exe):
            return False, ('%s 比 %s 旧；先 %s（否则跑的是另一份源码的二进制）'
                           % (rel, os.path.relpath(newest_path, ROOT), build_hint))
    return True, 'ok'


def _tail(path, limit=2000):
    try:
        with open(path, encoding='utf-8', errors='replace') as f:
            return f.read()[-limit:]
    except OSError:
        return ''


def start_emulator(stderr_path):
    """启动真实 TCP 模拟器，返回 (proc, host, port)。

    模拟器自报 `MESA_NCK_EMULATOR_READY=<addr>`（`--port 0` 由 OS 原子分配端口，
    避免"预占端口"的 TOCTOU）。用一个守护线程持续抽 stderr：既能把 READY 行
    交回主线程，也能把后续输出写进证据文件（否则管道写满会阻塞子进程）。
    """
    proc = subprocess.Popen(
        [EMU, '--port', '0', '--scenario', 'happy', '--element-size', '8'],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        text=True, encoding='utf-8', errors='replace',
    )
    ready = {}
    done = threading.Event()

    def pump():
        try:
            with open(stderr_path, 'w', encoding='utf-8') as log:
                for line in proc.stderr:
                    log.write(line)
                    log.flush()
                    if 'value' not in ready and line.startswith(READY_PREFIX):
                        ready['value'] = line.strip()[len(READY_PREFIX):]
                        done.set()
        finally:
            done.set()

    threading.Thread(target=pump, name='emulator-stderr', daemon=True).start()
    deadline = time.time() + EMULATOR_READY_S
    while time.time() < deadline and 'value' not in ready:
        if done.is_set():
            break
        time.sleep(0.05)
    if 'value' not in ready:
        _terminate(proc)
        raise E2EFailure('模拟器未自报 READY（%s）；stderr 尾部：\n%s'
                         % (os.path.relpath(stderr_path, ROOT), _tail(stderr_path)))
    host, port = ready['value'].rsplit(':', 1)
    return proc, host, int(port)


def start_mesad(tmp_dir, stdout_path, stderr_path):
    """启动 mesad（临时 DB / 动态 HTTP 端口），返回 (proc, base, port)。

    端口是"取号 → 由 mesad 绑定"，存在 TOCTOU：若绑定失败（"address in use"），
    换一个端口重试（最多 3 次）。其它启动错误立即失败并带日志尾部。
    """
    out = open(stdout_path, 'w', encoding='utf-8')
    err = open(stderr_path, 'w', encoding='utf-8')
    last = ''
    try:
        for attempt in range(1, 4):
            port = free_port()
            base = 'http://127.0.0.1:%d' % port
            proc = subprocess.Popen(
                [MESAD, '--db', os.path.join(tmp_dir, 'e2e.db'),
                 '--events-db', os.path.join(tmp_dir, 'e2e-events.db'),
                 '--http-port', str(port), '--drivers-dir', DRIVERS_DIR],
                cwd=ROOT, stdout=out, stderr=err,
            )
            if wait_http(base, proc=proc):
                return proc, base, port
            last = _tail(stderr_path)
            _terminate(proc)
            conflict = ('in use' in last.lower() or 'socket address' in last.lower()
                        or '10048' in last)
            print('mesad 启动失败（第 %d 次，端口 %d）%s'
                  % (attempt, port, '：端口冲突，换端口重试' if conflict else ''))
            if not conflict:
                raise E2EFailure('mesad 未就绪（端口 %d）；stderr 尾部：\n%s' % (port, last))
        raise E2EFailure('mesad 连续 3 次端口绑定失败；stderr 尾部：\n%s' % last)
    finally:
        # 子进程已持有自己的句柄；这里关掉父进程的 Python 文件对象即可。
        out.close()
        err.close()


def _terminate(proc, grace_s=10):
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=grace_s)
    except subprocess.TimeoutExpired:
        proc.kill()
        try:
            proc.wait(timeout=grace_s)
        except subprocess.TimeoutExpired:
            pass


def _point_key(p):
    return p.get('point_key') or p.get('key')


def run(base, emu_host, emu_port, results):
    """链路主体（成功路径恒为 13 项断言；前置失败时只记录到失败点为止）。"""
    ep = 'e2e-nck'
    dev = 'e2e-dev'
    code, body = req(base + '/api/v1/devices', 'POST', {'id': dev, 'name': 'E2E Device'})
    print('POST /devices ->', code)
    if code not in (200, 201):
        raise E2EFailure('建设备失败: %s' % (body,))
    code, body = req(base + '/api/v1/endpoints', 'POST', {
        'id': ep, 'name': 'E2E NCK', 'device_id': dev,
        'driver_id': 'sinumerik-nck',
        'connection': {
            'host': emu_host, 'port': emu_port, 'family': '828d',
            'local_tsap': 256, 'remote_tsap': 258,
            'timeout_ms': 3000, 'max_read_items': 8,
        },
    })
    print('POST /endpoints ->', code)
    if code not in (200, 201):
        raise E2EFailure('建端点失败: %s' % (body,))

    # ---- 任务 A：`nck_native` 原生字段点位 ----
    # 现行契约（ADR 0005 / PR-B）：NCK 任务**只接受** `nck.points.v1` 信封
    # （`{points:[{kind,point_key,parameters}]}`）；旧 `mesa.resources.v1` 被
    # 驱动以 `UNSUPPORTED_BINDING` 整体拒绝，不做隐式迁移。
    # `identity_mode=derived`（缺省）下 point_key 必须逐字等于地址推导出的
    # `native.<area>.<area_no>.<module>.<column>.<line>`，否则 `POINT_KEY_MISMATCH`。
    native_params = {
        'area': 'C', 'area_no': 1, 'module': 18, 'column': 42,
        'line': 1, 'count': 1, 'unit_mode': 'current',
        'decode': 'F64_BE', 'transport_size': '0x09',
    }
    native_point = {
        'kind': POINT_KIND_NATIVE,
        'point_key': NATIVE_KEY,
        'parameters': native_params,
    }
    task = {
        'id': 't1',
        'schedule': {'mode': 'poll', 'interval_ms': 200},
        'binding': {
            'kind': NCK_POINTS_KIND,
            'config': {'points': [native_point]},
        },
    }
    code, body = req(base + '/api/v1/tasks/' + ep, 'PUT', [task])
    print('PUT /tasks (原生字段) ->', code)
    results.append(('保存原生字段任务', code in (200, 201)))

    # 往返一致性：读回的任务参数必须与提交的**逐字节一致**，
    # 不允许任何隐藏回填或静默重解释（§7.3 第 5 条）。
    code, body = req(base + '/api/v1/tasks?endpoint=' + ep)
    stored = None
    for t in (body or {}).get('tasks', []):
        if t.get('id') == 't1':
            stored = t
    if stored is None:
        print('FAIL: 读回任务为空', body)
        results.append(('任务往返可读回', False))
        raise E2EFailure('读回任务为空: %s' % (body,))
    stored_point = stored['binding']['config']['points'][0]
    results.append(('任务往返参数逐字节一致（无静默重解释）',
                    stored_point == native_point))
    print('读回点位是否一致:', stored_point == native_point)

    code, body = req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')
    print('POST /start ->', code)
    results.append(('启动端点', code in (200, 202)))

    # 等待采到 GOOD
    def first_point():
        _, latest = req(base + '/api/v1/points/latest')
        pts = (latest or {}).get('points') or (latest or {}).get('items') or []
        for p in pts:
            if _point_key(p) == NATIVE_KEY:
                return p
        return None

    got = poll_until(first_point)
    print('latest point:', json.dumps(got, ensure_ascii=False) if got else None)
    quality = (got or {}).get('quality')
    results.append(('原生任务采到 GOOD', str(quality).lower() in ('good', '0', '1') or quality == 'Good'))
    if got:
        dtype = got.get('data_type') or got.get('value_type')
        print('data_type:', dtype, 'value:', got.get('value'))

    # 3) 模板路径：经 REST Browse 取模板 → 只预填固定字段 → 使用者补实例/行
    #    → 保存 → 采集。这是 Web 实际走的路径（与上面手写参数不同）。
    req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')
    code, body = req(base + '/api/v1/endpoints/%s/browse' % ep, 'POST',
                     {'parent': '', 'limit': 10})
    root_ids = [n.get('id') for n in (body or {}).get('nodes', [])]
    print('browse 根 ->', code, root_ids)
    results.append(('Browse 根为模板+身份两类入口',
                    code == 200 and 'nck://presets' in root_ids
                    and 'nck://identity' in root_ids))

    # filter 精确取一个 8 字节模板（模拟器 --element-size 8）。
    code, body = req(base + '/api/v1/endpoints/%s/browse' % ep, 'POST',
                     {'parent': 'nck://presets', 'filter': 'rpa', 'limit': 10})
    # 浏览叶的 Point Kind 是 `nck_variable`（语义点位）；旧 `variable` 资源叶已废弃。
    nodes = [n for n in (body or {}).get('nodes', []) if n.get('kind') == POINT_KIND_VARIABLE]
    print('browse 模板(filter=rpa) ->', code, '命中', len(nodes))
    if not nodes:
        print('FAIL: 未取到模板叶', body)
        results.append(('取到模板叶', False))
        raise E2EFailure('未取到模板叶: %s' % (body,))
    leaf = nodes[0]
    print('leaf label:', leaf.get('label'))
    binding = json.loads(leaf['binding_json'])
    bparams = binding['parameters']
    identity = binding.get('identity') or {}
    # 模板只预填**固定**字段：实例字段（target/arguments）必须留空、预览 Key 必须为 null，
    # 否则等于替使用者猜了通道号/R 参数序号。
    results.append(('模板只预填固定字段（实例字段未预填）',
                    binding.get('kind') == POINT_KIND_VARIABLE
                    and bparams.get('target') == {} and bparams.get('arguments') == {}
                    and identity.get('preview_point_key') is None
                    and bool(identity.get('keys'))))
    results.append(('模板标注 candidate 且列出待确认项',
                    'candidate' in (leaf.get('label') or '')
                    and '待确认' in (leaf.get('label') or '')))

    # 使用者补实例：target.channel = 通道号、arguments.index = R 参数序号。
    # 身份 Key 的规范编码（类型标签）**只由驱动算**——客户端不得自己拼，
    # 因此这里走 Web 实际使用的权威入口 `identity-preview`。
    bparams['target'] = {'channel': 1}
    bparams['arguments'] = {'index': 0}
    code, body = req(base + '/api/v1/drivers/sinumerik-nck/definitions/identity-preview',
                     'POST',
                     {'family': '828d', 'kind': POINT_KIND_VARIABLE, 'parameters': bparams})
    preview = body if isinstance(body, dict) else {}
    preview_key = preview.get('point_key')
    print('identity-preview ->', code,
          json.dumps(body, ensure_ascii=False)[:300] if body else None)
    results.append(('身份预览返回权威 Key（derived / collect）',
                    code == 200 and isinstance(preview_key, str) and bool(preview_key)
                    and preview_key.startswith(str(bparams.get('variable_id')) + '#')
                    and 'index=' in preview_key and 'channel=' in preview_key
                    and preview.get('identity_mode') == 'derived'
                    and preview.get('scope') == 'collect'))

    preset_point = {
        'kind': POINT_KIND_VARIABLE,
        'point_key': preview_key or '',
        'parameters': bparams,
    }
    task2 = {
        'id': 't2',
        'schedule': {'mode': 'poll', 'interval_ms': 200},
        'binding': {'kind': NCK_POINTS_KIND, 'config': {'points': [preset_point]}},
    }
    code, body = req(base + '/api/v1/tasks/' + ep, 'PUT', [task2])
    print('PUT /tasks (模板+确认) ->', code)
    results.append(('模板补必填后可保存', code in (200, 201)))
    req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')

    def preset_point_latest():
        _, latest = req(base + '/api/v1/points/latest')
        for p in ((latest or {}).get('points') or []):
            if _point_key(p) == preview_key:
                return p
        return None

    got2 = poll_until(preset_point_latest)
    print('preset point:', json.dumps(got2, ensure_ascii=False) if got2 else None)
    results.append(('模板任务采到 GOOD', (got2 or {}).get('quality') == 'GOOD'))

    # 端点重启 + PointMap 稳定性：先把两条任务一起提交，让两个点**同时存在**；
    # 点身份（point_id）由 Core 分配并持久化，重启后必须保持不变且继续 GOOD。
    # 注意：运行中的端点改任务必须先 Stop（否则 409 CONFLICT）。
    req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')
    code, body = req(base + '/api/v1/tasks/' + ep, 'PUT', [task, task2])
    print('PUT /tasks (两条任务) ->', code)
    results.append(('两条任务可同时保存', code in (200, 201)))
    req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')

    def both_good():
        _, latest = req(base + '/api/v1/points/latest')
        pts = (latest or {}).get('points') or []
        if len(pts) >= 2 and all(p.get('quality') == 'GOOD' for p in pts):
            return {p.get('key'): p.get('point_id') for p in pts}
        return None

    ids_before = poll_until(both_good) or {}
    req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')
    code, body = req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')
    time.sleep(3)
    ids_after = poll_until(both_good) or {}
    print('重启前:', ids_before)
    print('重启后:', ids_after)
    results.append(('端点重启后两个点都在且 point_id 不变（PointMap 稳定）',
                    len(ids_before) >= 2 and ids_after == ids_before))

    # 5) 负例：旧目录字段（block/variable）绝不能被静默接受。
    #
    # 现行契约（P9 护栏 / ADR 0005）：Core **不解释** `parameters`，只校验信封形状，
    # 所以保存**可以**通过；权威拒绝点在驱动 Configure（`UNKNOWN_FIELD`）。
    # 两条路径都断言同一件事——旧字段最终**跑不起来**。
    req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')
    legacy_params = {
        'area': 'C', 'area_no': 1, 'block': 'S', 'variable': 'progStatus',
        'line': 1, 'count': 1, 'unit_mode': 'current',
    }
    legacy_task = {
        'id': 't-legacy',
        'schedule': {'mode': 'poll', 'interval_ms': 200},
        'binding': {'kind': NCK_POINTS_KIND, 'config': {'points': [{
            'kind': POINT_KIND_NATIVE,
            'point_key': 'native.C.1.18.42.9',
            'parameters': legacy_params,
        }]}},
    }
    code, body = req(base + '/api/v1/tasks/' + ep, 'PUT', [legacy_task])
    msg = json.dumps(body, ensure_ascii=False) if body else ''
    print('PUT /tasks (旧字段) ->', code, msg[:300])
    if code >= 400:
        results.append(('旧字段在保存时被拒', True))
    else:
        # 保存通过是契约要求（Core 不解释 parameters）；拒绝必须由驱动在 Configure 给出。
        code, body = req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')
        time.sleep(3)
        _, st = req(base + '/api/v1/endpoints')
        detail = ''
        for e in (st or {}).get('endpoints', []):
            if e.get('id') == ep:
                detail = str((e.get('runtime') or {}).get('detail', ''))
        print('start ->', code, '| runtime detail:', detail[:300])
        results.append(('旧字段在 Configure 被明确拒绝（不得静默跑起来）',
                        'block' in detail or 'UNKNOWN_FIELD' in detail))

    req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')


def write_evidence(evidence_dir, results, exit_code, started_ts):
    """落盘 summary.json（artifact 上传用；失败也写，便于定位）。"""
    if not evidence_dir:
        return
    try:
        os.makedirs(evidence_dir, exist_ok=True)
        passed = sum(1 for _, ok in results if ok)
        doc = {
            'script': 'scripts/nck-native-rest-e2e.py',
            'platform': sys.platform,
            'python': sys.version.split()[0],
            'started_at': time.strftime('%Y-%m-%dT%H:%M:%S%z',
                                        time.localtime(started_ts)),
            'duration_s': round(time.time() - started_ts, 3),
            'exit_code': exit_code,
            'total': len(results),
            'passed': passed,
            'failed': len(results) - passed,
            'results': [{'name': name, 'passed': bool(ok)} for name, ok in results],
        }
        path = os.path.join(evidence_dir, 'summary.json')
        with open(path, 'w', encoding='utf-8') as f:
            json.dump(doc, f, ensure_ascii=False, indent=2)
        print('evidence:', os.path.relpath(path, ROOT))
    except OSError as e:  # 证据写失败不得掩盖真实测试结论
        print('WARN: 写证据失败: %s' % e)


def main(argv=None):
    _ensure_stdio()
    ap = argparse.ArgumentParser(description='NCK 原生寻址 REST 端到端验证')
    ap.add_argument('--evidence-dir', default=os.environ.get('MESA_E2E_EVIDENCE_DIR'),
                    help='证据目录（summary.json + 子进程日志；缺省不落盘）')
    args = ap.parse_args(argv)
    evidence = os.path.abspath(args.evidence_dir) if args.evidence_dir else None
    if evidence:
        os.makedirs(evidence, exist_ok=True)
    started_ts = time.time()

    fresh, why = check_binaries()
    if not fresh:
        print('FAIL(前置): ' + why)
        write_evidence(evidence, [('二进制存在且不比源码旧', False)], 2, started_ts)
        return 2

    tmp = tempfile.mkdtemp(prefix='nck-e2e-')
    emu = mesad = None
    results = []
    code = 1
    mesad_out = os.path.join(evidence or tmp, 'mesad.stdout.log')
    mesad_err = os.path.join(evidence or tmp, 'mesad.stderr.log')
    emu_err = os.path.join(evidence or tmp, 'emulator.stderr.log')
    try:
        emu, emu_host, emu_port = start_emulator(emu_err)
        print('emulator ready: %s:%d' % (emu_host, emu_port))

        mesad, base, _port = start_mesad(tmp, mesad_out, mesad_err)
        print('mesad ready on', base)

        run(base, emu_host, emu_port, results)
        code = 0 if results and all(ok for _, ok in results) else 1
    except E2EFailure as e:
        print('FAIL: %s' % e)
        code = 1
    finally:
        _terminate(mesad)
        _terminate(emu)
        print('\n=== 结果 ===')
        for name, passed in results:
            print(('PASS  ' if passed else 'FAIL  ') + name)
        if not results:
            print('FAIL  （链路未跑到断言阶段；见上方 FAIL 与子进程日志）')
        print('\n%d/%d PASS' % (sum(1 for _, ok in results if ok), len(results)))
        if code != 0:
            for label, path in (('mesad.stderr', mesad_err),
                                ('mesad.stdout', mesad_out),
                                ('emulator.stderr', emu_err)):
                tail = _tail(path)
                if tail.strip():
                    print('\n--- %s (tail) ---\n%s' % (label, tail))
        write_evidence(evidence, results, code, started_ts)
        shutil.rmtree(tmp, ignore_errors=True)
    return code


if __name__ == '__main__':
    sys.exit(main())
