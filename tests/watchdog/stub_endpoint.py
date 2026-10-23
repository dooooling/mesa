#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""离线假端点桩 —— scripts/stop-watchdog.ps1 的可重复故障模拟端（tests/watchdog/ 的一部分）。

用途 / 边界
-----------
* **只监听 127.0.0.1**，只模拟 Core 的端点 REST 形态，不实现任何工业协议；
* 严禁指向真实设备：本文件与 watchdog 的离线边界检查一起，保证测试永不触碰机床；
* 不依赖 mesad / 8132 生产实例 / Rust 代码，Windows 与 Linux 上的 python3 均可直接运行。

支持的端点（形态与 Core 一致，见 crates/core-api/src/lib.rs:2136-2191、1481-1519、2332）：
  GET  /health                      -> {"ok": true}
  GET  /api/v1/endpoints/{id}       -> 端点快照（runtime.state / desired_running / runtime.epoch / connection）
  GET  /api/v1/endpoints/{id}/diagnostics -> {"point_count": N, ...}（任务配置核验用，只读）
  GET  /api/v1/tasks/{id}           -> {"endpoint_id": ..., "revision": ..., "tasks": [...]}（只读）
  POST /api/v1/endpoints/{id}/start -> {"started": id}
  POST /api/v1/endpoints/{id}/stop  -> {"stopped": id, "was_running": bool}

每个请求写一行 JSON 到 --log（jsonl），供断言脚本做**独立**交叉核对
（尤其是"POST /stop 的到达时刻"和"是否真的收到过 START"）。

故障旋钮（与场景的对应关系见 README.md）
----------------------------------------
  --get-delay-running N   M1：desired_running=true 时 GET 挂起 N 秒（观测被阻塞）
  --get-delay N           M8：任何 GET 都挂起 N 秒（观测通道完全无响应 → 心跳失效）
  --start-delay N         M2：POST /start 挂起 N 秒（Start 超时/在途）
  --stop-delay N          M4：POST /stop 挂起 N 秒（Stop 自身挂住）
  --race-after-stop N     M5：Stop 完成后 N 秒把状态翻回 RUNNING（迟到 Start 生效）
  --fail-after N          M6：Start 成功后 N 秒注入通信失败 detail（首次确认即停）
  --device-host/--device-port   快照里 connection 的取值（"停干净"⑤核对的比对目标）
  --point-count / --task-revision / --task-count   任务配置核验（diagnostics / tasks）的返回值
                                                  —— M11/M12 用它构造"匹配/不匹配"两种情形

用法示例：
  python3 stub_endpoint.py --port 18080 --log /tmp/m0.jsonl
  python3 stub_endpoint.py --port 18081 --get-delay-running 36 --log /tmp/m1.jsonl
"""
import argparse
import json
import os
import sys
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ARGS = None
EPID = 'lx-nck-tags-190-connection'


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
                    'point_count': ARGS.point_count,
                    'connection_state': ST.state,
                }
            log('DIAG', point_count=ARGS.point_count)
            return self._send(200, snap)
        if self.path == f'/api/v1/tasks/{EPID}':
            tasks = [{'task_id': f'sim-task-{i}', 'period_ms': 1000} for i in range(ARGS.task_count)]
            log('TASKS', revision=ARGS.task_revision, count=ARGS.task_count)
            return self._send(200, {'endpoint_id': EPID, 'revision': ARGS.task_revision, 'tasks': tasks})
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
                'connection': {'host': ARGS.device_host, 'port': ARGS.device_port},
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
    global ARGS
    p = argparse.ArgumentParser(description='离线假端点桩（仅供 stop-watchdog 离线测试）')
    p.add_argument('--port', type=int, required=True)
    p.add_argument('--endpoint', default=EPID)
    p.add_argument('--driver-id', default='sinumerik-nck',
                   help='快照里的 driver_id（"停干净"④按它精确匹配目标驱动）')
    p.add_argument('--device-host', default='127.0.0.1',
                   help='快照里 connection.host（"停干净"⑤的比对目标）')
    p.add_argument('--device-port', type=int, default=18099,
                   help='快照里 connection.port（指向一个不会有人连的端口）')
    p.add_argument('--get-delay', type=float, default=0.0, help='GET 延迟（秒），任何状态都生效（M8）')
    p.add_argument('--get-delay-running', type=float, default=0.0,
                   help='GET 延迟（秒），仅在 desired_running=true 时生效（M1）')
    p.add_argument('--start-delay', type=float, default=0.0, help='POST /start 延迟（秒）（M2）')
    p.add_argument('--stop-delay', type=float, default=0.0, help='POST /stop 延迟（秒）（M4）')
    p.add_argument('--race-after-stop', type=float, default=0.0,
                   help='Stop 完成后 N 秒把状态翻回 RUNNING（M5）')
    p.add_argument('--fail-after', type=float, default=0.0,
                   help='Start 成功后 N 秒注入通信失败 detail（M6）')
    p.add_argument('--fail-detail', default='driver reported Failed ConnectionError/COTP_RECV_FAIL: early eof')
    p.add_argument('--point-count', type=int, default=12,
                   help='diagnostics 里的 point_count（任务配置核验的比对目标之一）')
    p.add_argument('--task-revision', default='rev13',
                   help='GET /api/v1/tasks/{id} 返回的 revision（任务配置核验的比对目标之一）')
    p.add_argument('--task-count', type=int, default=3,
                   help='GET /api/v1/tasks/{id} 返回的任务条数')
    p.add_argument('--shutdown-after', type=float, default=0.0,
                   help='M3：桩启动 N 秒后直接退出（模拟"端点运行中失联"，之后所有请求都连不上）')
    p.add_argument('--log', default=None)
    ARGS = p.parse_args()

    srv = ThreadingHTTPServer(('127.0.0.1', ARGS.port), Handler)
    srv.daemon_threads = True

    if ARGS.shutdown_after:
        # M3：模拟"端点运行中失联"——到点直接把进程干掉，之后端口连不上（Stop 也发不出去）。
        def _die():
            time.sleep(ARGS.shutdown_after)
            log('SHUTDOWN_BY_STUB', after=ARGS.shutdown_after)
            os._exit(0)
        threading.Thread(target=_die, daemon=True).start()

    log('STUB_READY', port=ARGS.port,
        get_delay=ARGS.get_delay, get_delay_running=ARGS.get_delay_running,
        start_delay=ARGS.start_delay, stop_delay=ARGS.stop_delay,
        race_after_stop=ARGS.race_after_stop, fail_after=ARGS.fail_after,
        device=f'{ARGS.device_host}:{ARGS.device_port}', driver_id=ARGS.driver_id)
    try:
        srv.serve_forever(poll_interval=0.05)
    except KeyboardInterrupt:
        pass
    finally:
        srv.server_close()
        log('STUB_STOP')


if __name__ == '__main__':
    main()
