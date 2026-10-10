"""15 类 READY 输出的本机实际 DLL 差分；只回复冻结读请求，保留逐帧证据。"""
import argparse
import json
import os
import pathlib
import struct
import subprocess
import threading

from focas_diff import parse_generic_request
from focas_load_review import replies
from focas_load_synthetic import FIXTURES, ROOT, LocalServer, save, sha


def key(sub):
    return sub['dev'], int(sub['func'], 16), tuple(sub['args'])


class Responses(dict):
    def __init__(self):
        super().__init__()
        self.slots = {}
        self.sources = {}
        self.uses = []
        folder = FIXTURES/'load_synthetic_handshake'
        for tag in ('open1', 'open2', 'sysinfo', 'close'):
            rt = 'open' if tag.startswith('open') else tag
            self[(folder/f'{tag}_request.bin').read_bytes()] = (tag, (folder/f'{rt}_response.bin').read_bytes())

    def add(self, group, request, response):
        req = (FIXTURES/group/request).read_bytes()
        res = (FIXTURES/group/response).read_bytes()
        subs = parse_generic_request(req)['subs']
        data = replies(res)
        assert len(subs) == len(data)
        for sub, (dev, path, cmd, payload) in zip(subs, data, strict=True):
            assert (sub['dev'], int(sub['func'], 16)) == (dev, path*65536+cmd)
            k = key(sub)
            assert k not in self.slots or self.slots[k] == payload, (group, k)
            self.slots[k] = payload
            self.sources[k] = {'request': f'{group}/{request}', 'response': f'{group}/{response}',
                               'response_sha256': sha(res)}

    def alias(self, dev, cmd, args, source):
        # Native 和 Wire 在已确认单点语义内可能使用不同的范围/容量参数。
        k = dev, 65536+cmd, tuple(args)
        self.slots[k] = self.slots[source]
        self.sources[k] = dict(self.sources[source], alias_args=args)

    def get(self, request):
        known = super().get(request)
        if known is not None:
            return known
        if request[6:8] != b'\x21\x01':
            return None
        subs = parse_generic_request(request)['subs']
        if len(subs) > 16:
            return None
        payload = bytearray(struct.pack('>H', len(subs)))
        for sub in subs:
            k = key(sub)
            if k not in self.slots:
                return None
            data = self.slots[k]
            dev, func, _ = k
            payload.extend(struct.pack('>4H3hH', 16+len(data), dev, func >> 16, func & 65535, 0, 0, 0, len(data)))
            payload.extend(data)
            self.uses.append({'key': [dev, func, list(k[2])], 'source': self.sources[k]})
        return 'ready-read', request[:4]+b'\0\3\x21\x02'+struct.pack('>H', len(payload))+payload


def setup(profile='baseline'):
    m = Responses()
    sources = [
        ('statinfo_mem', 'statinfo_request_frame2.bin', 'statinfo_response_frame2.bin'),
        ('axis1', 'axis_request_frame1.bin', 'axis_response_frame1.bin'),
        ('axis1', 'axis_request_frame2.bin', 'axis_response_frame2.bin'),
        ('spindle2', 'spindle_request_frame.bin', 'spindle_response_frame.bin'),
        ('macro1', 'macro_request_frame.bin', 'macro_response_frame.bin'),
        ('pmc_y0', 'pmc_request_frame.bin', 'pmc_response_frame.bin'),
        ('pmc_r100', 'pmc_request_frame.bin', 'pmc_response_frame.bin'),
        ('pmc_d0', 'pmc_request_frame.bin', 'pmc_response_frame.bin'),
        ('pmc_x0b3', 'pmc_request_frame.bin', 'pmc_response_frame.bin'),
        ('param_6711_c2', 'param_request_frame.bin', 'param_response_frame.bin'),
        ('diagnosis_301a3', 'diagnosis_request_frame.bin', 'diagnosis_response_frame.bin'),
        ('gear_s1', 'spindleword_request_frame.bin', 'spindleword_response_frame.bin'),
        ('maxrpm_s1', 'spindleword_request_frame.bin', 'spindleword_response_frame.bin'),
        ('opmsg_type4', 'opmsg_request_frame.bin', 'opmsg_response_frame.bin'),
        ('alarm_ps0010', 'alarm_request_frame.bin', 'alarm_response_frame.bin'),
        ('tool_tofs_08', 'tool16-type1-value5000.req.bin', 'tool16-type1-value5000.res.bin'),
        ('tool_tofs_08', 'tool16-type3-value10000.req.bin', 'tool16-type3-value10000.res.bin'),
        ('tool_zofs_0b', 'g54x-12345.req.bin', 'g54x-12345.res.bin'),
    ]
    for row in sources:
        m.add(*row)
    for selector in (1, 6, 7):
        m.alias(1, 0x26, [selector, 1, 0, 0, 0], (1, 65536+0x26, (4, 1, 0, 0, 0)))
    m.alias(1, 0xD0, [0, 4, 1, 0, 0], (1, 65536+0x34, (4, 0, 0, 0, 0)))
    m.alias(1, 0x23, [-1, 1, 2, 32, 0], (1, 65536+0x23, (-1, 29, 2, 32, 0)))
    # DLL RVA 0x68A24：网络四个 BE32 + 文本32；ABI 才是44B。
    # 旧 PS0010 文件把 ABI 的短字段误当网络布局，保留原件但不再作正确帧 oracle。
    alarm = struct.pack('>4i', 10, 3, 0, 15)+b'IMPROPER G-CODE'.ljust(32, b'\0')
    for capacity in (1, 29):
        k = 1, 65536+0x23, (-1, capacity, 2, 32, 0)
        m.slots[k] = alarm
        m.sources[k] = {'kind': 'synthetic-derived', 'abi_basis': 'FWLIBE64 cnc_rdalmmsg RVA 0x68A24',
                        'rejected_layout': 'alarm_ps0010/alarm_response_frame.bin'}
    # 动态读取的已捕获九槽；共享 feed=100，避免取不同窗口的单槽 200。
    for dev, path, cmd, data in replies((FIXTURES/'feed/feed_response_9pack.bin').read_bytes()):
        if cmd == 0x26 or cmd == 0x25:
            continue
        k = dev, path*65536+cmd, (0, 0, 0, 0, 0)
        m.slots[k] = data
        m.sources[k] = {'response': 'feed/feed_response_9pack.bin', 'response_sha256': sha((FIXTURES/'feed/feed_response_9pack.bin').read_bytes())}
    cases = [
        ('status', 'status', {'U32': 1}, None),
        ('feed', 'feed', {'U32': 100}, None),
        ('speed', 'active_spindle', {'I32': 1002}, None),
        ('absolute', 'axis.abs.1', {'I32': -2880}, None),
        ('gear', 'spindle.gear.1', {'I32': 672}, 'gear'),
        ('maxrpm', 'spindle.maxrpm.1', {'I32': 874}, 'maxrpm'),
        ('macro', 'macro.501', {'F64': 25.0}, None),
        ('pmc-byte', 'pmc.Y0', {'I32': 4}, None),
        ('pmc-word', 'pmc.R100', {'I32': 0}, None),
        ('pmc-dword', 'pmc.D0', {'I32': 4}, None),
        ('pmc-bit', 'pmc.X0.3', {'Bool': False}, None),
        ('param', 'param.6711', {'I32': 456}, None),
        ('diagnosis', 'diagnosis301axis3', {'F64': -0.01}, 'diagnosis'),
        ('opmsg', 'opmsg', {'String': 'OPMSG TEST 123'}, None),
        ('alarm', 'alarm', {'StringArray': ['IMPROPER G-CODE']}, 'alarm'),
        ('offset', 'tool.offset.16', {'F64': 5.0}, None),
        ('length', 'tool.length.16', {'F64': 10.0}, None),
        ('zofs', 'tool.zofs.1', {'F64': 12.345}, None),
    ]
    cases = [{'name': n, 'address': a, 'expected': v, 'guarded': g} for n,a,v,g in cases]
    if profile != 'baseline':
        minimum = profile == 'minimum'
        raw = -2147483648 if minimum else 2147483647
        word = -32768 if minimum else 32767
        # 按独立网络结构设置边界；预期直接由给定整数/业务比例计算，不调用 Wire。
        by_name = {c['name']: c for c in cases}
        updates = [
            ('absolute', (1, 65536+0x26, (4,1,0,0,0)), 0, '>i', raw, {'I32':raw}),
            ('speed', (1, 65536+0x25, (0,0,0,0,0)), 0, '>i', raw, {'I32':raw}),
            ('macro', (1, 65536+0x15, (501,501,0,0,0)), 0, '>i', raw, {'F64':raw/10**7}),
            ('param', (1, 65536+0x8D, (6711,6711,0,0,0)), 8, '>i', raw, {'I32':raw}),
            ('pmc-word', (2, 65536+0x8001, (100,101,5,1,0)), 0, '>h', word, {'I32':word}),
            ('pmc-dword', (2, 65536+0x8001, (0,3,9,2,0)), 0, '>i', raw, {'I32':raw}),
            ('pmc-byte', (2, 65536+0x8001, (0,0,2,0,0)), 0, '>B', 0 if minimum else 255, {'I32':0 if minimum else 255}),
            ('pmc-bit', (2, 65536+0x8001, (0,0,3,0,0)), 0, '>B', 0 if minimum else 255, {'Bool':not minimum}),
            ('gear', (1, 65536+0x40, (2,1,0,0,0)), 2, '>h', word, {'I32':word}),
            ('maxrpm', (1, 65536+0x40, (1,1,0,0,0)), 2, '>h', word, {'I32':word}),
            ('diagnosis', (1, 65536+0x93, (301,301,3,0,0)), 8, '>i', raw, {'F64':raw/1000}),
            ('offset', (1, 65536+0x08, (16,16,1001,0,0)), 0, '>i', raw, {'F64':raw/1000}),
            ('length', (1, 65536+0x08, (16,16,1003,0,0)), 0, '>i', raw, {'F64':raw/1000}),
            ('zofs', (1, 65536+0x0B, (1,1,1,0,0)), 0, '>i', raw, {'F64':raw/1000}),
            ('feed', (1, 65536+0x24, (0,0,0,0,0)), 0, '>i', raw, {'U32':raw & 0xffffffff}),
        ]
        for name, k, offset, fmt, value, expected in updates:
            data = bytearray(m.slots[k])
            struct.pack_into(fmt, data, offset, value)
            m.slots[k] = bytes(data)
            m.sources[k] = dict(m.sources[k], kind='synthetic-derived', profile=profile)
            by_name[name]['expected'] = expected
        text = b' hi\0tail' if minimum else b'A'*32
        expected = 'hi' if minimum else 'A'*32
        for capacity in (1,29):
            m.slots[(1,65536+0x23,(-1,capacity,2,32,0))] = struct.pack('>4i', 10, 3, 0, len(text))+text.ljust(32,b'\0')
        by_name['alarm']['expected'] = {'StringArray':[expected]}
    return m, cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-exe', required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--profile', choices=['baseline','minimum','maximum'], default='baseline')
    parser.add_argument('--archive', type=pathlib.Path)
    args = parser.parse_args()
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=False)
    traffic = args.out/'traffic'
    traffic.mkdir()
    mapping, cases = setup(args.profile)
    save(args.out/'cases.json', cases)
    with LocalServer(mapping, traffic, max_frames=128) as server:
        worker = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.05})
        worker.start()
        try:
            env = dict(os.environ, MESA_FOCAS_READY_PORT=str(server.server_address[1]),
                       MESA_FOCAS_READY_OUT=str(args.out/'samples'), MESA_FOCAS_READY_CASES=str(args.out/'cases.json'))
            result = subprocess.run([args.test_exe, '--exact', 'wire::dll_parity::ready_dll_pair_local',
                '--ignored', '--nocapture', '--test-threads=1'], cwd=ROOT, env=env,
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=90)
            (args.out/'test.log').write_bytes(result.stdout)
        finally:
            server.shutdown()
            worker.join(timeout=5)
        server.server_close()
        save(args.out/'server.json', {'records': server.records, 'errors': server.errors, 'sources': mapping.uses})
        if result.returncode or server.errors:
            raise RuntimeError(f'差分失败，现场见 {args.out}')
    rows = json.loads((args.out/'samples/comparison.json').read_text(encoding='utf-8'))
    assert len(rows) == len(cases) and all(r['equal'] for r in rows)
    if args.archive is not None:
        args.archive.mkdir(parents=True, exist_ok=False)
        # Wire 是最后一个 OPEN2 会话；只归档这个会话，保留完整请求序列与 CLOSE。
        peer = next(r['peer'] for r in reversed(server.records) if r['tag'] == 'open2')
        replay = [r for r in server.records if r['peer'] == peer]
        assert replay[0]['tag'] == 'open2' and replay[-1]['tag'] == 'close'
        for r in replay:
            for field in ('request','response'):
                (args.archive/r[field]).write_bytes((traffic/r[field]).read_bytes())
        save(args.archive/'cases.json', cases)
        save(args.archive/'index.json', {'kind':'synthetic-derived','profile':args.profile,
             'ready_categories':15,'cases':len(rows),'all_equal':True,'hardware_acceptance':False,
             'loaded_modules':json.loads((args.out/'samples/modules.json').read_text(encoding='utf-8')),
             'replay':replay,'comparisons':rows,
             'files':{p.name:sha(p.read_bytes()) for p in args.archive.iterdir() if p.is_file()}})
    save(args.out/'manifest.json', {'kind': 'synthetic-derived', 'profile':args.profile,'ready_categories': 15,
        'cases': len(rows), 'all_equal': True, 'hardware_acceptance': False,
        'loaded_modules': json.loads((args.out/'samples/modules.json').read_text(encoding='utf-8')),
        'files': {str(p.relative_to(args.out)): sha(p.read_bytes()) for p in args.out.rglob('*') if p.is_file()}})
    print(json.dumps({'all_equal': True, 'cases': len(rows), 'ready_categories': 15, 'out': str(args.out)}, ensure_ascii=False))


if __name__ == '__main__':
    main()
