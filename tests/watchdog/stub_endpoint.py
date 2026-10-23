#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""离线假端点桩 —— scripts/stop-watchdog.ps1 的可重复故障模拟端（tests/watchdog/ 的一部分）。

用途 / 边界
-----------
* **只监听 127.0.0.1**（可用 --bind 改为本机其它地址，仅供"非回环地址"负向用例），
  只模拟 Core 的端点 REST 形态，不实现任何工业协议；
* 严禁指向真实设备：本文件与 watchdog 的离线边界检查一起，保证测试永不触碰机床；
* 不依赖 mesad / 8132 生产实例 / Rust 代码，Windows 与 Linux 上的 python3 均可直接运行。

支持的端点（形态与 Core 一致，见 crates/core-api/src/lib.rs:1844-1887、1481-1519、2317-2337）
-----------------------------------------------------------------------------------------
  GET  /health                      -> {"ok": true}
  GET  /api/v1/endpoints/{id}       -> 端点快照（runtime / desired_running / runtime.epoch /
                                       connection：host / port / local_tsap / remote_tsap / pdu_length）
  GET  /api/v1/endpoints/{id}/diagnostics -> {"point_count": N, ...}（任务配置核验用，只读）
  GET  /api/v1/tasks?endpoint={id}  -> {"endpoint_id", "revision", "tasks": [...]}（**Core 真实形态**）
  GET  /api/v1/tasks/{id}           -> 同上（旧桩形态；watchdog 在 404/405 时回退到它）
  POST /api/v1/endpoints/{id}/start -> {"started": id}
  POST /api/v1/endpoints/{id}/stop  -> {"stopped": id, "was_running": bool}

每个请求写一行 JSON 到 --log（jsonl），供断言脚本做**独立**交叉核对
（尤其是"POST /stop 的到达时刻"和"是否真的收到过 START"）。

故障旋钮（与场景的对应关系见 README.md）
----------------------------------------
  --get-delay-running N   M1/M8：desired_running=true 时 GET 挂起 N 秒（观测被阻塞）
  --get-delay N           任何 GET 都挂起 N 秒（观测通道完全无响应）
  --start-delay N         M2：POST /start 挂起 N 秒（Start 超时/在途）
  --stop-delay N          M4：POST /stop 挂起 N 秒（Stop 自身挂住）
  --race-after-stop N     M5：Stop 完成后 N 秒把状态翻回 RUNNING（迟到 Start 生效）
  --fail-after N          M6：Start 成功后 N 秒注入通信失败 detail（首次确认即停）
  --device-host/--device-port   快照里 connection 的取值（"停干净"⑤核对的比对目标）
  --local-tsap/--remote-tsap/--pdu-length
                        connection 里的 TSAP / PDU（B3：连接配置完整核对的比对目标）
  --point-count / --task-revision / --task-count   任务配置核验（diagnostics / tasks）的返回值
                                                  —— M11/M12/M15/M16 用它构造"匹配/不匹配"两种情形
  --tasks-file PATH     用文件里的 tasks 数组（完整点位 Key 与参数）；缺省按 --task-count 生成
  --bind ADDR           监听地址（缺省 127.0.0.1；M13 用 0.0.0.0 + 主机名做"非回环"负向用例）

独立工具（不算 HTTP 服务）
--------------------------
  --fingerprint-file PATH
                        对文件里的 tasks 数组算**规范化指纹**（sha256:<hex>）并打印到 stdout。
                        实现与 PowerShell 侧 ConvertTo-WdCanonicalNode / Get-WdTasksFingerprint
                        独立且逐字节等价（sort_keys + 紧凑分隔符），用于交叉核对获批配置指纹。

用法示例：
  python3 stub_endpoint.py --port 18080 --log /tmp/m0.jsonl
  python3 stub_endpoint.py --port 18081 --get-delay-running 36 --log /tmp/m1.jsonl
  python3 stub_endpoint.py --fingerprint-file tests/watchdog/fixtures/tasks.json
"""
import argparse
import hashlib
import json
import os
import sys
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ARGS = None
EPID = 'lx-nck-tags-190-connection'
# 任务快照（GET /api/v1/tasks[?endpoint=]）返回的 tasks 数组；main() 里按 --tasks-file /
# --task-count 初始化。放在模块级是因为 Handler 实例每请求新建、无状态。
TASKS = []


def canonical_json(node):
    """规范化 JSON：对象键排序 + 紧凑分隔符（无空白）。

    与 watchdog（PowerShell）侧 ConvertTo-WdCanonicalNode + ConvertTo-Json -Compress 逐字节等价；
    两侧是**独立实现**，互为交叉核对：任一侧改了规范化规则，M15/M16 的指纹用例立刻失败。
    """
    return json.dumps(node, sort_keys=True, separators=(',', ':'), ensure_ascii=False)


def tasks_fingerprint(tasks):
    """任务配置指纹 = SHA-256(规范化 tasks 数组)，返回 ('sha256:<64 hex>', payload)。

    排序规则与 PowerShell 侧一致：按 task.id 排序（同 id 用规范 JSON 兜底，保证确定性）。
    """
    ordered = sorted(tasks, key=lambda t: (str(t.get('id', '')), canonical_json(t)))
    payload = canonical_json(ordered)
    return 'sha256:' + hashlib.sha256(payload.encode('utf-8')).hexdigest(), payload


def point_count():
    """diagnostics 里的 point_count：缺省取 tasks 里的点位总数（真实 Core 用 point_map 长度）。"""
    if ARGS.point_count is not None:
        return ARGS.point_count
    total = 0
    for t in TASKS:
        cfg = (t.get('binding') or {}).get('config') or {}
        total += len(cfg.get('points') or [])
    return total


def default_tasks(count, points_per_task):
    """缺省生成的 tasks：形态与真实 AcquisitionTask 一致（crates/core-types/src/lib.rs:500-503）。

    schedule 是内部 tagged（{"mode":"poll","interval_ms":N}，见 :436-452）；
    binding.kind 用 NCK 的点位信封（"nck.points.v1"），binding.config.points[] 是
    PointSelection（point_key / kind / parameters，见 crates/core-types/src/point.rs:100-120）。
    这些字段正是 B3 要逐点核对的"完整 Key 与参数"。
    """
    out = []
    for i in range(count):
        points = []
        for j in range(points_per_task):
            points.append({
                'point_key': f'nck.task{i + 1}.p{j + 1}',
                'kind': 'variable',
                'parameters': {
                    'variable_id': 1000 + i * 10 + j,
                    'area': 'r',
                    'offset': j,
                    'type': 'real',
                },
            })
        out.append({
            'id': f'nck-tags-{i + 1}',
            'schedule': {'mode': 'poll', 'interval_ms': 1000 * (i + 1)},
            'binding': {'kind': 'nck.points.v1', 'config': {'points': points}},
        })
    return out


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.state = 'STOPPED'
        self.desired = False
        self.epoch = 1000
        self.detail = None
        self.stop_calls = 0
        self.start_calls = 0
        self.race_done = False


ST = State()


def log(event, **kw):
    rec = {
        'ts_local': datetime.now().astimezone().isoformat(),
        'ts_utc': datetime.now(timezone.utc).isoformat(),
        'mono': round(time.monotonic(), 3),
        'event': event,
        'pid': os.getpid(),
    }
    rec.update(kw)
    line = json.dumps(rec, ensure_ascii=False)
    if ARGS.log:
        with open(ARGS.log, 'a', encoding='utf-8') as f:
            f.write(line + '\n')
            f.flush()
    print(line, flush=True)


class Handler(BaseHTTPRequestHandler):
    # HTTP/1.0：每个请求一条连接。避免 keep-alive 让"挂起的 GET"把后面的 POST 堵在同一条连接上。
    protocol_version = 'HTTP/1.0'

    def _send(self, code, obj):
        body = json.dumps(obj, ensure_ascii=False).encode('utf-8')
        try:
            self.send_response(code)
            self.send_header('Content-Type', 'application/json; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            # 客户端已取消（例如观测超时后 watchdog 不再读）——不是错误，记一笔即可。
            log('SEND_ABORTED', code=code, path=self.path)

    def do_GET(self):
        if self.path == '/health':
            return self._send(200, {'ok': True})
        # ── 任务配置核验用的**只读**端点（与 Core 形态一致，见 core-api:1481-1519 / 2332）──
        if self.path == f'/api/v1/endpoints/{EPID}/diagnostics':
            with ST.lock:
                snap = {
                    'endpoint_id': EPID,
                    'driver_id': ARGS.driver_id,
                    'desired_running': ST.desired,
                    'runtime': {'endpoint_id': EPID, 'state': ST.state, 'epoch': ST.epoch, 'detail': ST.detail},
                    'point_count': point_count(),
                    'connection_state': ST.state,
                }
            log('DIAG', point_count=point_count())
            return self._send(200, snap)
        if self.path == f'/api/v1/tasks/{EPID}':
            log('TASKS', revision=ARGS.task_revision, count=len(TASKS), source='path')
            return self._send(200, {'endpoint_id': EPID, 'revision': ARGS.task_revision, 'tasks': TASKS})
        if self.path == f'/api/v1/tasks?endpoint={EPID}':
            # Core 的真实形态（crates/core-api/src/lib.rs:2317-2337：list_tasks 用 ?endpoint=）。
            log('TASKS', revision=ARGS.task_revision, count=len(TASKS), source='query')
            return self._send(200, {'endpoint_id': EPID, 'revision': ARGS.task_revision, 'tasks': TASKS})
        if self.path.startswith('/api/v1/tasks'):
            return self._send(400, {'code': 'VALIDATION_ERROR', 'message': '缺少查询参数 endpoint'})
        if not self.path.startswith('/api/v1/endpoints/'):
            return self._send(404, {'code': 'NOT_FOUND', 'message': self.path})
        with ST.lock:
            running = ST.desired
        delay = ARGS.get_delay_running if running else ARGS.get_delay
        log('GET_begin', path=self.path, delay=delay, running=running)
        if delay:
            time.sleep(delay)
        with ST.lock:
            snap = {
                'id': EPID,
                'device_id': 'lx-nck-tags-190',
                'driver_id': ARGS.driver_id,
                'desired_running': ST.desired,
                # connection 的字段名与 sinumerik-nck 的 connection schema 一致
                # （drivers/sinumerik-nck/src/config.rs:26-41）：host/port/local_tsap/remote_tsap/pdu_length。
                'connection': {
                    'host': ARGS.device_host,
                    'port': ARGS.device_port,
                    'local_tsap': ARGS.local_tsap,
                    'remote_tsap': ARGS.remote_tsap,
                    'pdu_length': ARGS.pdu_length,
                },
                'runtime': {
                    'endpoint_id': EPID,
                    'state': ST.state,
                    'epoch': ST.epoch,
                    'detail': ST.detail,
                },
                'updated_at_ns': int(time.time() * 1e9),
            }
        log('GET_end', path=self.path, state=snap['runtime']['state'])
        return self._send(200, snap)

    def do_POST(self):
        if self.path.endswith('/start'):
            with ST.lock:
                ST.start_calls += 1
                n = ST.start_calls
            log('START_begin', n=n, delay=ARGS.start_delay)
            if ARGS.start_delay:
                time.sleep(ARGS.start_delay)
            with ST.lock:
                ST.desired = True
                ST.state = 'RUNNING'
                ST.epoch += 1
                ST.detail = None
                epoch = ST.epoch
            log('START_done', n=n, epoch=epoch)
            if ARGS.fail_after:
                threading.Thread(target=self._fail, daemon=True).start()
            return self._send(200, {'started': EPID})
        if self.path.endswith('/stop'):
            with ST.lock:
                was = ST.desired
                ST.stop_calls += 1
                n = ST.stop_calls
            log('STOP_arrive', n=n, was_running=was, delay=ARGS.stop_delay)
            if ARGS.stop_delay:
                with ST.lock:
                    if ST.state != 'STOPPED':
                        ST.state = 'STOPPING'
                time.sleep(ARGS.stop_delay)
            with ST.lock:
                ST.desired = False
                ST.state = 'STOPPED'
            log('STOP_done', n=n)
            if ARGS.race_after_stop:
                # M5：模拟"Stop 之后 Start 迟到生效"——Core 生命周期竞态（§5.3）。
                threading.Thread(target=self._race, args=(n,), daemon=True).start()
            return self._send(200, {'stopped': EPID, 'was_running': was})
        return self._send(404, {'code': 'NOT_FOUND', 'message': self.path})

    def _race(self, n):
        time.sleep(ARGS.race_after_stop)
        with ST.lock:
            if ST.race_done:
                return
            ST.race_done = True
            ST.desired = True
            ST.state = 'RUNNING'
            ST.epoch += 1
            epoch = ST.epoch
        log('RACE_LATE_START', n=n, epoch=epoch)

    def _fail(self):
        time.sleep(ARGS.fail_after)
        with ST.lock:
            if not ST.desired or ST.state == 'STOPPED':
                return
            ST.state = 'RECONNECTING'
            ST.detail = ARGS.fail_detail
        log('INJECT_COMM_FAIL', detail=ARGS.fail_detail)

    def log_message(self, fmt, *args):
        # 关掉 BaseHTTPRequestHandler 的 stderr 噪音：证据只看 --log 的 jsonl。
        pass


def main():
    global ARGS, TASKS
    p = argparse.ArgumentParser(description='离线假端点桩（仅供 stop-watchdog 离线测试）')
    # --port 在 --fingerprint-file 模式下不是必需的（该模式不监听端口）。
    p.add_argument('--port', type=int, default=0)
    p.add_argument('--endpoint', default=EPID)
    p.add_argument('--bind', default='127.0.0.1',
                   help='监听地址（缺省 127.0.0.1；M13 的"非回环地址"负向用例用 0.0.0.0 + 主机名）')
    p.add_argument('--driver-id', default='sinumerik-nck',
                   help='快照里的 driver_id（"停干净"④按它精确匹配目标驱动）')
    p.add_argument('--device-host', default='127.0.0.1',
                   help='快照里 connection.host（"停干净"⑤的比对目标）')
    p.add_argument('--device-port', type=int, default=18099,
                   help='快照里 connection.port（指向一个不会有人连的端口）')
    p.add_argument('--local-tsap', type=int, default=256,
                   help='connection.local_tsap（B3 连接配置核对项；0x0100）')
    p.add_argument('--remote-tsap', type=int, default=256,
                   help='connection.remote_tsap（B3 连接配置核对项；0x0100）')
    p.add_argument('--pdu-length', type=int, default=240,
                   help='connection.pdu_length（B3 连接配置核对项）')
    p.add_argument('--get-delay', type=float, default=0.0, help='GET 延迟（秒），任何状态都生效')
    p.add_argument('--get-delay-running', type=float, default=0.0,
                   help='GET 延迟（秒），仅在 desired_running=true 时生效（M1/M8）')
    p.add_argument('--start-delay', type=float, default=0.0, help='POST /start 延迟（秒）（M2）')
    p.add_argument('--stop-delay', type=float, default=0.0, help='POST /stop 延迟（秒）（M4）')
    p.add_argument('--race-after-stop', type=float, default=0.0,
                   help='Stop 完成后 N 秒把状态翻回 RUNNING（M5）')
    p.add_argument('--fail-after', type=float, default=0.0,
                   help='Start 成功后 N 秒注入通信失败 detail（M6）')
    p.add_argument('--fail-detail', default='driver reported Failed ConnectionError/COTP_RECV_FAIL: early eof')
    p.add_argument('--point-count', type=int, default=None,
                   help='diagnostics 里的 point_count；缺省 = tasks 里点位总数（与真实 Core 的 point_map 长度一致）')
    p.add_argument('--task-revision', default='rev13',
                   help='任务快照返回的 revision（任务配置核验的比对目标之一）')
    p.add_argument('--task-count', type=int, default=3,
                   help='缺省生成的任务条数（给了 --tasks-file 时忽略）')
    p.add_argument('--points-per-task', type=int, default=4,
                   help='缺省生成时每个任务里的点位数（B3 逐点核对的样本）')
    p.add_argument('--tasks-file', default=None,
                   help='用文件里的 tasks 数组（完整点位 Key 与参数），替代 --task-count 生成')
    p.add_argument('--fingerprint-file', default=None,
                   help='独立工具模式：对文件里的 tasks 数组算规范化指纹并打印（不启动 HTTP 服务）')
    p.add_argument('--emit-tasks', default=None,
                   help='独立工具模式：把缺省生成的 tasks 数组写到指定文件（不启动 HTTP 服务），'
                        '供场景驱动与"获批任务配置"共用同一份点位定义')
    p.add_argument('--shutdown-after', type=float, default=0.0,
                   help='M3：桩启动 N 秒后直接退出（模拟"端点运行中失联"，之后所有请求都连不上）')
    p.add_argument('--log', default=None)
    ARGS = p.parse_args()

    # ── 独立工具模式：只算指纹（供场景驱动交叉核对获批配置的指纹，不启服务）─────────────
    if ARGS.fingerprint_file:
        with open(ARGS.fingerprint_file, encoding='utf-8') as f:
            doc = json.load(f)
        tasks = doc.get('tasks') if isinstance(doc, dict) else doc
        fp, payload = tasks_fingerprint(tasks)
        print(fp)
        print(payload, file=sys.stderr)
        return 0

    # ── 独立工具模式：导出缺省 tasks（桩与"获批配置"共用同一份点位定义）────────────────
    if ARGS.emit_tasks:
        tasks = default_tasks(ARGS.task_count, ARGS.points_per_task)
        with open(ARGS.emit_tasks, 'w', encoding='utf-8') as f:
            json.dump(tasks, f, ensure_ascii=False, indent=2)
            f.write('\n')
        print(ARGS.emit_tasks)
        return 0

    if not ARGS.port:
        p.error('--port 必填（--fingerprint-file 模式除外）')

    if ARGS.tasks_file:
        with open(ARGS.tasks_file, encoding='utf-8') as f:
            doc = json.load(f)
        TASKS = doc.get('tasks') if isinstance(doc, dict) else doc
    else:
        TASKS = default_tasks(ARGS.task_count, ARGS.points_per_task)

    srv = ThreadingHTTPServer((ARGS.bind, ARGS.port), Handler)
    srv.daemon_threads = True

    if ARGS.shutdown_after:
        # M3：模拟"端点运行中失联"——到点直接把进程干掉，之后端口连不上（Stop 也发不出去）。
        def _die():
            time.sleep(ARGS.shutdown_after)
            log('SHUTDOWN_BY_STUB', after=ARGS.shutdown_after)
            os._exit(0)
        threading.Thread(target=_die, daemon=True).start()

    log('STUB_READY', port=ARGS.port, bind=ARGS.bind,
        get_delay=ARGS.get_delay, get_delay_running=ARGS.get_delay_running,
        start_delay=ARGS.start_delay, stop_delay=ARGS.stop_delay,
        race_after_stop=ARGS.race_after_stop, fail_after=ARGS.fail_after,
        device=f'{ARGS.device_host}:{ARGS.device_port}', driver_id=ARGS.driver_id,
        tsap=f'{ARGS.local_tsap}/{ARGS.remote_tsap}', pdu_length=ARGS.pdu_length,
        task_count=len(TASKS), tasks_fingerprint=tasks_fingerprint(TASKS)[0])
    try:
        srv.serve_forever(poll_interval=0.05)
    except KeyboardInterrupt:
        pass
    finally:
        srv.server_close()
        log('STUB_STOP')
    return 0


if __name__ == '__main__':
    sys.exit(main())
