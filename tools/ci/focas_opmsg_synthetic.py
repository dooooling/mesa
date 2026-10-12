"""操作消息真实 DLL/Wire 差分：本机冻结握手，派生响应，禁止设备透传。"""
import argparse
import json
import os
import pathlib
import struct
import subprocess
import threading

from focas_load_synthetic import FIXTURES, ROOT, LocalServer, save, sha

CASES = [
    ('full256', b'A'*256, 'A'*256, 0),
    ('nul', b' hi\0tail', 'hi', 0),
    ('empty', b'', 'OP:empty', 0),
    ('lossy', b'\xffOK', '\ufffdOK', 0),
    ('password', b'', None, 17),
    ('length', b'', None, 2),
]
CASES.extend((f'status{code}', b'', None, code) for code in
             [1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 12345, -12, -2222])


class Responses(dict):
    def __init__(self, handshake, original):
        super().__init__()
        for tag in ('open1', 'open2', 'sysinfo', 'close'):
            response_tag = 'open' if tag.startswith('open') else tag
            self[handshake[f'{tag}_request.bin']] = (tag, handshake[f'{response_tag}_response.bin'])
        self.original, self.calls = original, 0

    def get(self, request):
        known = super().get(request)
        if known is not None:
            return known
        # 只允许单条 #3006 读请求。真实 DLL 根据握手选择 D0 或 34，Wire 固定 34。
        if len(request) != 40 or request[10:12] != b'\0\1':
            return None
        _, dev, path, cmd, *args = struct.unpack('>4H5i', request[12:])
        if dev != 1 or path != 1 or (cmd, args) not in (
                (0x34, [4, 0, 0, 0, 0]), (0xD0, [0, 4, 1, 0, 0])):
            return None
        i = self.calls
        self.calls += 1
        native_calls = 2*len(CASES)
        if i >= 3*len(CASES):
            return None
        case = CASES[i//2 if i < native_calls else i-native_calls]
        name, raw, _, status = case
        response = bytearray(self.original)
        struct.pack_into('>Hh', response, 18, cmd, status)
        # 网络三个 BE32 字段，ABI 对应三个 short；文本始于 data+12。
        struct.pack_into('>3i', response, 28, 3006, 4, len(raw))
        response[40:296] = raw.ljust(256, b'\0')
        return name, bytes(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-exe', type=pathlib.Path, required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--archive', type=pathlib.Path)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    traffic = args.out/'traffic'
    traffic.mkdir()
    handshake = {p.name: p.read_bytes() for p in (FIXTURES/'load_synthetic_handshake').glob('*.bin')}
    original = (FIXTURES/'opmsg_type4/opmsg_response_frame.bin').read_bytes()
    assert len(original) == 296
    mapping = Responses(handshake, original)
    with LocalServer(mapping, traffic, max_frames=2*len(CASES)+4) as server:
        worker = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.05})
        worker.start()
        try:
            env = dict(os.environ, MESA_FOCAS_OPMSG_PORT=str(server.server_address[1]),
                       MESA_FOCAS_OPMSG_OUT=str(args.out/'samples'),
                       MESA_FOCAS_OPMSG_COUNT=str(len(CASES)))
            result = subprocess.run([str(args.test_exe.resolve()), '--exact',
                'wire::opmsg_parity::opmsg_dll_pair_local', '--ignored', '--nocapture',
                '--test-threads=1'], cwd=ROOT, env=env, stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, timeout=60)
            (args.out/'test.log').write_bytes(result.stdout)
        finally:
            server.shutdown()
            worker.join(timeout=5)
        server.server_close()
        save(args.out/'server.json', {'records': server.records, 'errors': server.errors})
        if result.returncode or server.errors:
            raise RuntimeError(f'实验失败，证据见 {args.out}')
    samples = json.loads((args.out/'samples/comparison.json').read_text(encoding='utf-8'))
    assert mapping.calls == 3*len(CASES)
    for side in ('native', 'wire'):
        for actual, (name, _, expected, rc) in zip(samples[side], CASES, strict=True):
            assert actual['rc'] == rc, (side, name, actual)
            assert actual.get('text') == expected, (side, name, actual)
    save(args.out/'manifest.json', {'kind': 'synthetic-derived', 'all_equal': True,
         'cases': [case[0] for case in CASES], 'source_sha256': sha(original),
         'test_exe_sha256': sha(args.test_exe.read_bytes()),
         'dll_files_on_disk': {p.name: sha(p.read_bytes()) for p in
                  (pathlib.Path(os.environ['TEMP'])/'mesa_focas_embed').glob('*.dll')}})
    if args.archive is not None:
        # 只归档已与实际 DLL 对照通过的 Wire 帧；不改写原始设备捕获。
        args.archive.mkdir(parents=True, exist_ok=False)
        archived = []
        for i, (name, _, expected, rc) in enumerate(CASES):
            record = next(r for r in reversed(server.records) if r['tag'] == name)
            # NUL 是 Windows 保留设备名，案例目录统一增加前缀。
            folder = args.archive/f'case-{name}'
            folder.mkdir()
            for key in ('request', 'response'):
                data = (traffic/record[key]).read_bytes()
                (folder/f'{key}.bin').write_bytes(data)
            save(folder/'expected.json', {'kind': 'synthetic-derived', 'rc': rc,
                 'text': expected, 'native': samples['native'][i],
                 'request_sha256': record['request_sha256'],
                 'response_sha256': record['response_sha256']})
            archived.append({'name': name, 'folder': folder.name})
        modules = json.loads((args.out/'samples/modules.json').read_text(encoding='utf-8'))
        save(args.archive/'index.json', {'kind': 'synthetic-derived', 'cases': archived,
             'source_sha256': sha(original), 'source': str(args.out),
             'loaded_modules': modules, 'hardware_acceptance': False,
             'production_load_gate_changed': False})
    print(json.dumps({'all_equal': True, 'cases': len(CASES), 'out': str(args.out)}, ensure_ascii=False))


if __name__ == '__main__':
    main()
