"""NCK 原生寻址 REST 端到端验证（PR-C 集成回归）。

链路：模拟器（真实 TCP）+ mesad（临时 DB / 独立端口）+ REST：
  POST /endpoints → PUT /tasks/{id} → POST /start → GET /points/latest
验证：
  1. 原生字段任务能被保存、启动并采到 GOOD 值（DriverResolved 类型正确）；
  2. 旧目录字段（block/variable/access_path）被**明确拒绝**，不会被静默接受。

不触碰本机真实配置：使用临时 DB 与独立 HTTP 端口。
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

# 本脚本位于 scripts/，仓库根是其父目录。
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EMU = os.path.join(ROOT, 'target', 'debug', 'mesa-nck-emulator.exe')
MESAD = os.path.join(ROOT, 'target', 'debug', 'mesad.exe')
HTTP_PORT = 18133
READY_PREFIX = 'MESA_NCK_EMULATOR_READY='


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


def wait_http(base, seconds=40):
    for _ in range(seconds * 2):
        try:
            code, _ = req(base + '/api/v1/drivers', timeout=3)
            if code == 200:
                return True
        except OSError:
            pass
        time.sleep(0.5)
    return False


def driver_binary_is_fresh():
    """驱动二进制必须比其源码新。

    切分支后忘记重编会拉起**另一份源码**构建的驱动：原生字段被判成非法（400）、
    identity/浏览直接 503，报错与真实缺陷高度相似。AGENTS.md 已就此警告过
    （“旧二进制让故障注入静默失效”），这里把它变成启动前的硬检查。
    """
    exe = os.path.join(ROOT, 'target', 'debug', 'mesa-driver-sinumerik-nck.exe')
    if not os.path.isfile(exe):
        return False, '缺少 %s（先 cargo build --workspace）' % exe
    exe_mtime = os.path.getmtime(exe)
    src_dir = os.path.join(ROOT, 'drivers', 'sinumerik-nck', 'src')
    newest = 0.0
    for name in os.listdir(src_dir):
        if name.endswith('.rs'):
            newest = max(newest, os.path.getmtime(os.path.join(src_dir, name)))
    if newest > exe_mtime:
        return False, ('%s 比 drivers/sinumerik-nck/src 旧；先 cargo build --workspace'
                       '（否则跑的是另一份源码的驱动）' % os.path.basename(exe))
    return True, 'ok'


def main():
    fresh, why = driver_binary_is_fresh()
    if not fresh:
        print('FAIL(前置): ' + why)
        return 2
    tmp = tempfile.mkdtemp(prefix='nck-e2e-')
    emu = mesad = None
    results = []
    try:
        # 1) 模拟器（真实 TCP，自报 READY 地址）
        emu = subprocess.Popen(
            [EMU, '--port', '0', '--scenario', 'happy', '--element-size', '8'],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
        )
        ready = None
        deadline = time.time() + 15
        while time.time() < deadline:
            line = emu.stderr.readline()
            if not line:
                break
            if line.startswith(READY_PREFIX):
                ready = line.strip()[len(READY_PREFIX):]
                break
        if not ready:
            print('FAIL: 模拟器未自报 READY')
            return 1
        emu_host, emu_port = ready.rsplit(':', 1)
        emu_port = int(emu_port)
        print('emulator ready:', ready)

        # 2) mesad（临时 DB / 独立端口）
        mesad = subprocess.Popen(
            [MESAD, '--db', os.path.join(tmp, 'e2e.db'),
             '--events-db', os.path.join(tmp, 'e2e-events.db'),
             '--http-port', str(HTTP_PORT), '--drivers-dir', 'drivers'],
            cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        base = 'http://127.0.0.1:%d' % HTTP_PORT
        if not wait_http(base):
            print('FAIL: mesad 未就绪')
            return 1
        print('mesad ready on', base)

        ep = 'e2e-nck'
        dev = 'e2e-dev'
        code, body = req(base + '/api/v1/devices', 'POST', {'id': dev, 'name': 'E2E Device'})
        print('POST /devices ->', code)
        if code not in (200, 201):
            print('FAIL: 建设备', body)
            return 1
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
            print('FAIL: 建端点', body)
            return 1

        native_params = {
            'area': 'C', 'area_no': 1, 'module': 18, 'column': 42,
            'line': 1, 'count': 1, 'unit_mode': 'current',
            'decode': 'F64_BE', 'transport_size': '0x09',
        }
        task = {
            'id': 't1',
            'schedule': {'mode': 'poll', 'interval_ms': 200},
            'binding': {
                'kind': 'mesa.resources.v1',
                'config': {'selections': [{
                    'resource_id': 'variable',
                    'parameters': native_params,
                    'outputs': [{'output': 'value', 'point_key': 'e2e.value'}],
                }]},
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
            return 1
        stored_params = stored['binding']['config']['selections'][0]['parameters']
        results.append(('任务往返参数逐字节一致（无静默重解释）',
                        stored_params == native_params
                        and stored['binding']['config']['selections'][0]['outputs']
                        == task['binding']['config']['selections'][0]['outputs']))
        print('读回参数是否一致:', stored_params == native_params)

        code, body = req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')
        print('POST /start ->', code)
        results.append(('启动端点', code in (200, 202)))

        # 等待采到 GOOD
        got = None
        for _ in range(40):
            time.sleep(0.5)
            code, body = req(base + '/api/v1/points/latest')
            pts = (body or {}).get('points') or (body or {}).get('items') or []
            for p in pts:
                if p.get('point_key') == 'e2e.value' or p.get('key') == 'e2e.value':
                    got = p
                    break
            if got:
                break
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
        nodes = [n for n in (body or {}).get('nodes', []) if n.get('kind') == 'variable']
        print('browse 模板(filter=rpa) ->', code, '命中', len(nodes))
        if not nodes:
            print('FAIL: 未取到模板叶', body)
            results.append(('取到模板叶', False))
            return 1
        leaf = nodes[0]
        print('leaf label:', leaf.get('label'))
        binding = json.loads(leaf['binding_json'])
        bparams = binding['parameters']
        results.append(('模板只预填固定字段（不含 area_no/line）',
                        'area_no' not in bparams and 'line' not in bparams
                        and binding.get('resource_id') == 'variable'))
        results.append(('模板标注 candidate 且列出待确认项',
                        'candidate' in (leaf.get('label') or '')
                        and '待确认' in (leaf.get('label') or '')))

        bparams['area_no'] = 1
        bparams['line'] = 1
        task2 = json.loads(json.dumps(task))
        task2['id'] = 't2'
        task2['binding']['config']['selections'][0]['parameters'] = bparams
        task2['binding']['config']['selections'][0]['outputs'] = [
            {'output': 'value', 'point_key': 'e2e.preset'}]
        code, body = req(base + '/api/v1/tasks/' + ep, 'PUT', [task2])
        print('PUT /tasks (模板+确认) ->', code)
        results.append(('模板补必填后可保存', code in (200, 201)))
        req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')
        got2 = None
        for _ in range(40):
            time.sleep(0.5)
            _, latest = req(base + '/api/v1/points/latest')
            for p in ((latest or {}).get('points') or []):
                if p.get('key') == 'e2e.preset':
                    got2 = p
                    break
            if got2:
                break
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
        ids_before = {}
        for _ in range(40):
            time.sleep(0.5)
            _, latest = req(base + '/api/v1/points/latest')
            pts = (latest or {}).get('points') or []
            if len(pts) >= 2 and all(p.get('quality') == 'GOOD' for p in pts):
                ids_before = {p.get('key'): p.get('point_id') for p in pts}
                break
        req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')
        code, body = req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')
        time.sleep(3)
        ids_after = {}
        for _ in range(30):
            _, latest = req(base + '/api/v1/points/latest')
            pts = (latest or {}).get('points') or []
            if len(pts) >= 2 and all(p.get('quality') == 'GOOD' for p in pts):
                ids_after = {p.get('key'): p.get('point_id') for p in pts}
                break
            time.sleep(0.5)
        print('重启前:', ids_before)
        print('重启后:', ids_after)
        results.append(('端点重启后两个点都在且 point_id 不变（PointMap 稳定）',
                        len(ids_before) >= 2 and ids_after == ids_before))

        # 5) 负例：旧目录字段必须被明确拒绝
        req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')
        legacy_task = json.loads(json.dumps(task))
        legacy_task['binding']['config']['selections'][0]['parameters'] = {
            'area': 'C', 'area_no': 1, 'block': 'S', 'variable': 'progStatus',
            'line': 1, 'count': 1, 'unit_mode': 'current',
        }
        legacy_task['id'] = 't-legacy'
        code, body = req(base + '/api/v1/tasks/' + ep, 'PUT', [legacy_task])
        msg = json.dumps(body, ensure_ascii=False) if body else ''
        print('PUT /tasks (旧字段) ->', code, msg[:300])
        rejected_at_save = code >= 400
        results.append(('旧字段在保存时被拒', rejected_at_save))

        if not rejected_at_save:
            # 保存未拦则必须在启动 Configure 时被明确拒绝（不得静默跑起来）
            code, body = req(base + '/api/v1/endpoints/%s/start' % ep, 'POST')
            time.sleep(3)
            _, st = req(base + '/api/v1/endpoints')
            detail = ''
            for e in (st or {}).get('endpoints', []):
                if e.get('id') == ep:
                    detail = str((e.get('runtime') or {}).get('detail', ''))
            print('start ->', code, '| runtime detail:', detail[:300])
            results.append(('旧字段在 Configure 被明确拒绝',
                            'INVALID_VARIABLE' in detail or 'block' in detail))

        req(base + '/api/v1/endpoints/%s/stop' % ep, 'POST')
        print('\n=== 结果 ===')
        ok = True
        for name, passed in results:
            print(('PASS  ' if passed else 'FAIL  ') + name)
            ok = ok and passed
        return 0 if ok else 1
    finally:
        for p in (mesad, emu):
            if p and p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == '__main__':
    sys.exit(main())
