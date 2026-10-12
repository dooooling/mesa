"""本机非零负载报文实验：冻结握手 + 派生响应，对照真实 DLL 与 Wire。

仅监听 127.0.0.1 的临时端口，只接受归档中的读请求。原始 fixture 不改写；
合成证据单独标记 synthetic-derived，不能作为真机兼容性或生产准入证据。
"""
import argparse
import collections
import hashlib
import json
import os
import pathlib
import socketserver
import struct
import subprocess
import threading

from focas_load_review import replies

ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXTURES = ROOT / 'drivers/focas2/tests/fixtures/wire'
TAGS = ('type0', 'type1', 'type_all', 'servo')
CASES = {
    'single': {'load': [(270, 1)], 'speed': [(1500, 0)],
               'servo': [(-270, 1), (560, 1), (-123, 2)]},
    'multi_signed': {'load': [(-125, 1), (875, 1)],
                     'speed': [(12345, 1), (6789, 1)],
                     'servo': [(7, 2), (-999, 1), (1001, 2)]},
    'integer_edges': {'load': [(-2147483648, 0), (2147483647, 0)],
                      'speed': [(-2147483648, 0), (2147483647, 0)],
                      'servo': [(-2147483648, 0), (2147483647, 0), (0, 0)]},
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def save(path, value):
    with path.open('x', encoding='utf-8') as output:
        json.dump(value, output, ensure_ascii=False, indent=2)


def derive(original, tag, spec):
    # 只更改计数、数值/小数位；多主轴另增加名称项并重算全部长度。
    slots = replies(original)
    servo = tag == 'servo'
    count = 3 if servo else len(spec['load'])
    names = slots[1][3] if servo else bytes((83, 49, 0, 0, 83, 50, 0, 0))[:count*4]
    sides = [0] if tag in ('servo', 'type0') else [1] if tag == 'type1' else [0, 1]
    payload = bytearray(struct.pack('>H', len(slots)))
    # 保留未指定数值项的原始字节，不借助 Wire 实现计算预期结果。
    expected = []
    for i, (dev, path, cmd, data) in enumerate(slots):
        data = bytearray(data)
        if i in (0, len(slots)-1):
            data = bytearray(struct.pack('>H', count))
        elif i == 1:
            data = bytearray(names)
        else:
            side = sides[i-2]
            values = spec['servo' if servo else 'load' if side == 0 else 'speed']
            for record, (raw, decimal) in enumerate(values):
                struct.pack_into('>i', data, record*8, raw)
                struct.pack_into('>h', data, record*8+6, decimal)
        payload.extend(struct.pack('>4H3hH', 16+len(data), dev, path, cmd, 0, 0, 0, len(data)))
        payload.extend(data)
    for record in range(count):
        for side in sides:
            raw, decimal = spec['servo' if servo else 'load' if side == 0 else 'speed'][record]
            expected.append({'slot': record if servo else record*2+side,
                             'data': (raw if raw == -2147483648 else abs(raw)) if servo else raw, 'decimal': decimal,
                             'unit': side, 'name': list(names[record*4:record*4+3])})
    result = original[:8]+struct.pack('>H', len(payload))+payload
    replies(result)
    return result, expected


class LocalServer(socketserver.ThreadingTCPServer):
    # DLL 同时持有两个连接，不能用单线程服务阻塞第二次 OPEN。
    daemon_threads = False
    allow_reuse_address = False

    def __init__(self, mapping, out, max_frames=16):
        if not 1 <= max_frames <= 256:
            raise ValueError('单连接帧数必须为 1..256')
        self.max_frames = max_frames
        self.mapping, self.out = mapping, out
        self.lock = threading.Lock()
        self.accepted = 0
        self.records, self.errors = [], []
        super().__init__(('127.0.0.1', 0), LocalHandler)

    def verify_request(self, request, client_address):
        with self.lock:
            self.accepted += 1
            if self.accepted > 3:
                self.errors.append('超过 Native 两连接 + Wire 一连接的上限')
                return False
        return client_address[0] == '127.0.0.1'


class LocalHandler(socketserver.BaseRequestHandler):
    def exact(self, count):
        data = bytearray()
        while len(data) < count:
            block = self.request.recv(count-len(data))
            if not block:
                raise ValueError('CLOSE 之前连接断开')
            data.extend(block)
        return bytes(data)

    def handle(self):
        self.request.settimeout(15)
        opened = False
        try:
            # 每连接采用显式有界帧数；未知请求立即拒绝，不实现写入或透传。
            for number in range(self.server.max_frames):
                head = self.exact(10)
                if head[:4] != b'\xa0'*4:
                    raise ValueError('非法帧头')
                frame = head+self.exact(struct.unpack_from('>H', head, 8)[0])
                item = self.server.mapping.get(frame)
                if item is None:
                    raise ValueError('未知请求：'+frame.hex())
                tag, response = item
                if (number == 0) != tag.startswith('open') or (number > 0 and not opened):
                    raise ValueError('会话 OPEN 顺序非法')
                opened = True
                with self.server.lock:
                    seq = len(self.server.records)
                    reqfile, respfile = f'{seq:02d}.request.bin', f'{seq:02d}.response.bin'
                    (self.server.out/reqfile).write_bytes(frame)
                    (self.server.out/respfile).write_bytes(response)
                    self.server.records.append({'tag': tag, 'peer': list(self.client_address),
                                                'request': reqfile, 'response': respfile,
                                                'request_sha256': sha(frame), 'response_sha256': sha(response)})
                # 人为分开发送帧头，同时覆盖客户端 read_exact 的流边界处理。
                self.request.sendall(response[:7])
                self.request.sendall(response[7:13])
                self.request.sendall(response[13:])
                if tag == 'close':
                    return
            raise ValueError('单连接帧数超过上限')
        except Exception as error:
            with self.server.lock:
                self.server.errors.append(str(error))


def run_case(exe, root, name, spec, handshake):
    out = root/name
    out.mkdir()
    traffic = out/'traffic'
    traffic.mkdir()
    mapping = {}
    for tag in ('open1', 'open2', 'sysinfo', 'close'):
        response_tag = 'open' if tag.startswith('open') else tag
        mapping[handshake[f'{tag}_request.bin']] = (tag, handshake[f'{response_tag}_response.bin'])
    expected, derived = {}, {}
    for tag in TAGS:
        folder = FIXTURES/'load_pair_165_20261010'/tag
        request, original = (folder/'native_request.bin').read_bytes(), (folder/'native_response.bin').read_bytes()
        response, candidates = derive(original, tag, spec)
        mapping[request] = (tag, response)
        expected[tag], derived[tag] = candidates, (request, response, sha(original))
    with LocalServer(mapping, traffic) as server:
        thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.05})
        thread.start()
        port = server.server_address[1]
        try:
            env = dict(os.environ, MESA_FOCAS_GATE0_HOST='127.0.0.1', MESA_FOCAS_GATE0_PORT=str(port),
                       MESA_FOCAS_LOAD_PAIR_OUT=str(out/'samples'))
            command = [str(exe), '--exact', 'wire::load_research::tests::load_protocol_pair_live',
                       '--ignored', '--nocapture', '--test-threads=1']
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, timeout=60)
            (out/'test.log').write_bytes(result.stdout)
        finally:
            server.shutdown()
            thread.join(timeout=5)
        # 等待 CLOSE 处理线程退出后才冻结日志，避免丢掉迟到的服务端失败。
        server.server_close()
        save(out/'server.json', {'host': '127.0.0.1', 'port': port, 'accepted': server.accepted,
                                'records': server.records, 'errors': server.errors})
        if result.returncode != 0 or server.errors:
            raise RuntimeError(f'{name} 调用失败，现场保留在 {out}')
        counts = collections.Counter(r['tag'] for r in server.records)
        if counts != collections.Counter(open1=1, open2=2, sysinfo=1, close=3,
                                         type0=2, type1=2, type_all=2, servo=2):
            raise ValueError(f'调用覆盖不完整：{counts}')
    records = []
    for tag in TAGS:
        native = json.loads((out/f'samples/native-{tag}.json').read_text(encoding='utf-8'))
        wire = json.loads((out/f'samples/wire-{tag}.json').read_text(encoding='utf-8'))
        if native['rc'] != 0 or native['num_out'] != (3 if tag == 'servo' else len(spec['load'])):
            raise ValueError('Native 返回码或数量错误')
        if not all(native[k] for k in ('guards_ok', 'tail_ok', 'span_tail_ok', 'untouched_half_ok')):
            raise ValueError('Native 缓冲区检查失败')
        if native['candidates'] != expected[tag] or wire['candidates'] != expected[tag] or not wire['native_candidates_equal']:
            raise ValueError(f'{name}/{tag} 不等于独立设定的非零预期值')
        request, response, original_sha = derived[tag]
        if (out/f'samples/wire-{tag}.request.bin').read_bytes() != request or (out/f'samples/wire-{tag}.response.bin').read_bytes() != response:
            raise ValueError('Wire 保存的请求/响应与服务端不一致')
        folder = out/tag
        folder.mkdir()
        (folder/'request.bin').write_bytes(request)
        (folder/'response.bin').write_bytes(response)
        record = {'tag': tag, 'kind': 'synthetic-derived', 'candidates': expected[tag],
                  'native_wire_expected_equal': True, 'num_out': native['num_out'],
                  'original_response_sha256': original_sha,
                  'request_sha256': sha(request), 'response_sha256': sha(response)}
        save(folder/'expected.json', record)
        records.append(record)
    modules = json.loads((out/'samples/modules-type0.json').read_text(encoding='utf-8'))
    return {'name': name, 'spec': spec, 'records': records,
            'runtime_modules': [{'name': pathlib.Path(m['path']).name, 'sha256': m['sha256']} for m in modules]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-exe', type=pathlib.Path, required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--archive', type=pathlib.Path)
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('Native DLL 实验需要 Windows')
    exe = args.test_exe.resolve(strict=True)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    folder = FIXTURES/'load_synthetic_handshake'
    index = json.loads((folder/'index.json').read_text(encoding='utf-8'))
    handshake = {name: (folder/name).read_bytes() for name in index['files']}
    if any(sha(handshake[name]) != meta['sha256'] for name, meta in index['files'].items()):
        raise ValueError('握手 fixture 哈希不符')
    cases = [run_case(exe, out, name, spec, handshake) for name, spec in CASES.items()]
    result = {'kind': 'synthetic-derived', 'scope': '本机派生报文 → 实际 Native DLL / 既有 Wire 解码器',
              'test_exe_sha256': sha(exe.read_bytes()), 'cases': cases, 'production_gate_changed': False,
              'limits': ['不证明真实设备能生成非零负载', '不覆盖传统分支和状态 4 回退', '不验证完整 FOCAS 协议']}
    save(out/'index.json', result)
    if args.archive:
        archive = args.archive.resolve()
        archive.mkdir(parents=True, exist_ok=False)
        for case in cases:
            for tag in TAGS:
                dst = archive/case['name']/tag
                dst.mkdir(parents=True)
                for filename in ('request.bin', 'response.bin', 'expected.json'):
                    (dst/filename).write_bytes((out/case['name']/tag/filename).read_bytes())
        save(archive/'index.json', result)
    print(json.dumps({'evidence': str(out), 'cases': len(cases), 'verified_operations': 4*len(cases),
                      'kind': result['kind'], 'native_wire_expected_equal': True}, ensure_ascii=False))


if __name__ == '__main__':
    main()
