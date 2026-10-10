"""本机派生负载报文 → 实际参考 DLL → 正式 Wire 采集百分比差分。

只读 loopback 替身，明确区分合成证据和设备验收。重复批次要求每组各一次读取。
"""
import argparse
import collections
import json
import os
import pathlib
import struct
import subprocess
import threading

from focas_load_synthetic import FIXTURES, ROOT, LocalServer, save, sha
from focas_load_review import replies

CASES = {
    'zero': {'spindle': [(0, 0)], 'servo': [(0, 0)] * 3},
    'fractional': {'spindle': [(-125, 1), (875, 1)], 'servo': [(-270, 1), (560, 1), (-123, 2)]},
    'four': {'spindle': [(7, -1), (99, 2), (-123, 3), (2147483647, 9)],
             'servo': [(-7, -1), (99, 2), (-123, 3), (-2147483648, 0)]},
    'decimal10': {'spindle': [(12345, 10)], 'servo': [(-12345, 10)]},
    'decimal_extremes': {'spindle': [(1, -32768), (1, 32767), (0, -32768), (2147483647, 317)],
                         'servo': [(1, -309), (-1, 32767), (0, -32768), (-1, 309)]},
    'shrinking': {'spindle': [(270, 1), (123, 1)], 'servo': [(-270, 1), (123, 1)], 'before': 2, 'after': 1},
    'empty': {'spindle': [], 'servo': []},
    'legacy_direct': {'legacy': True, 'spindle': [(-125, 1), (875, 1)], 'servo': [(-270, 1), (560, 1), (-123, 2)]},
    'legacy_scaled': {'legacy': True, 'scaled': True, 'spindle': [(-270, 0), (30000, 0)],
                      'servo': [(-270, 1), (560, 1), (-123, 2)], 'std': [100, 200], 'alt': [200, 100]},
    'legacy_global_axes': {'legacy': True, 'family': 2, 'global_axes': 3, 'path_axes': 2,
                           'spindle': [(270, 1), (30000, 0)], 'servo': [(-270, 1), (560, 1), (-123, 2), (99, 0)]},
    'legacy_wrapping': {'legacy': True, 'scaled': True, 'spindle': [(-2147483648, 0), (2147483647, 0)],
                        'servo': [(-2147483648, 0), (2147483647, 0)], 'std': [3, 3], 'alt': [2, 2]},
}


def response(tag, spec):
    original = (FIXTURES/'load_pair_165_20261010'/tag/'native_response.bin').read_bytes()
    slots = replies(original)
    servo = tag == 'servo'
    values = spec['servo' if servo else 'spindle']
    names = b''.join(bytes([name, suffix, 0, 0]) for name, suffix in
                     ([(ord('X'), 0), (ord('Y'), 0), (ord('Z'), 0), (ord('A'), 0)] if servo else
                      [(ord('S'), ord(str(i))) for i in range(1, 5)])[:len(values)])
    data = [struct.pack('>H', spec.get('before', len(values))), names,
            b''.join(struct.pack('>i2sh', raw, b'\x12\x34', decimal) for raw, decimal in values),
            struct.pack('>H', spec.get('after', len(values)))]
    payload = struct.pack('>H', 4) + b''.join(
        struct.pack('>4H3hH', 16+len(body), dev, path, cmd, 0, 0, 0, len(body))+body
        for (dev, path, cmd, _), body in zip(slots, data))
    return original[:8]+struct.pack('>H', len(payload))+payload


def packet(slots):
    payload = struct.pack('>H', len(slots)) + b''.join(struct.pack('>4H5i', 28, 1, 1, command, *args) for command, args in slots)
    return bytes.fromhex('a0a0a0a000012101')+struct.pack('>H', len(payload))+payload


def reply(slots):
    payload = struct.pack('>H', len(slots)) + b''.join(
        struct.pack('>4H3hH', 16+len(data), 1, 1, command, status, 0, 0, len(data))+data
        for command, status, data in slots)
    return bytes.fromhex('a0a0a0a000022102')+struct.pack('>H', len(payload))+payload


def legacy_mapping(spec):
    values = spec['spindle']
    names = b''.join(bytes([83, ord(str(i+1)), ord('2') if i == 1 else 0, 0]) for i in range(len(values)))
    numeric = lambda values: b''.join(struct.pack('>i2sh', raw, b'\x12\x34', decimal) for raw, decimal in values)
    direct = packet([(0x40, [4,-1,0,0,0]), (0x40, [5,-1,0,0,0]), (0x8a, [0]*5)])
    slots = [(0x40, 4 if spec.get('scaled') else 0, numeric(values)), (0x40, 0, numeric(values)), (0x8a, 0, names)]
    mapping = {direct: ('type0', reply(slots))}
    if spec.get('scaled'):
        parameters = [4127, 4274, 4020, 4196]
        request = packet([(0x40, [0,-1,0,0,0]), (0x40, [1,-1,0,0,0]), (0x8a, [0]*5)] + [(0x0e, [p,p,-1,0,0]) for p in parameters])
        data = [(0x40, 0, numeric(values)), (0x40, 0, numeric(values)), (0x8a, 0, names)]
        for parameter, coefficients in zip(parameters, [spec['std'], spec['alt'], spec['std'], spec['alt']]):
            data.append((0x0e, 0, struct.pack('>2i', parameter, 2)+b''.join(struct.pack('>i', n) for n in coefficients)))
        mapping[request] = ('scaled', reply(data))
    servos = spec['servo']
    axis_names = b''.join(bytes([n, 0, 0, 0]) for n in b'XYZA'[:len(servos)])
    mapping[packet([(0x56, [1,0,0,0,0]), (0x89, [0]*5)])] = ('servo', reply([(0x56,0,numeric(servos)), (0x89,0,axis_names)]))
    return mapping


def run_case(exe, root, name, spec, handshake):
    out = root/name
    out.mkdir()
    traffic = out/'traffic'
    traffic.mkdir()
    mapping = {}
    for tag in ('open1', 'open2', 'sysinfo', 'close'):
        mapping[handshake[f'{tag}_request.bin']] = (tag, handshake[f'{"open" if tag.startswith("open") else tag}_response.bin'])
    for tag in (() if spec.get('legacy') else ('type0', 'servo')):
        request = (FIXTURES/'load_pair_165_20261010'/tag/'native_request.bin').read_bytes()
        mapping[request] = (tag, response(tag, spec))
    if spec.get('legacy'):
        # 旧 OPEN 每路径8字节，series=3 不触发额外系统参数读取。
        data = struct.pack('>8H4H', 7, spec.get('family', 3), 4, 4, 1, 3, 0, 0, 0x204d, 1, spec.get('path_axes', len(spec['servo'])), len(spec['spindle']))
        old_open = bytes.fromhex('a0a0a0a000020102')+struct.pack('>H', len(data))+data
        for request, (tag, response_data) in list(mapping.items()):
            if tag.startswith('open'):
                mapping[request] = (tag, old_open)
        mapping.update(legacy_mapping(spec))
        if spec.get('family') == 2:
            sysinfo_data = replies(handshake['sysinfo_response.bin'])[0][3]
            axes = struct.pack('>2H', spec['global_axes'], spec['global_axes'])
            mapping[packet([(0x18, [0]*5), (0x8c, [0]*5)])] = ('sysinfo-axes', reply([(0x18, 0, sysinfo_data), (0x8c, 0, axes)]))
            mapping[packet([(0x8d, [9153,9153,0,0,0])])] = ('flags', reply([(0x8d, 0, struct.pack('>3i', 9153, 0, 0))]))
            mapping[packet([(0x8c, [0]*5)])] = ('global-axes', reply([(0x8c, 0, axes)]))
    with LocalServer(mapping, traffic) as server:
        thread = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': 0.05})
        thread.start()
        try:
            env = dict(os.environ, MESA_FOCAS_GATE0_HOST='127.0.0.1', MESA_FOCAS_GATE0_PORT=str(server.server_address[1]),
                       MESA_FOCAS_LOAD_PAIR_OUT=str(out/'samples'))
            process = subprocess.run([str(exe), '--exact', 'wire::load_research::tests::load_production_pair_live',
                                      '--ignored', '--nocapture', '--test-threads=1'], cwd=ROOT, env=env,
                                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60)
            (out/'test.log').write_bytes(process.stdout)
        finally:
            server.shutdown()
            thread.join(timeout=5)
        server.server_close()
        save(out/'server.json', {'records': server.records, 'errors': server.errors})
        if process.returncode or server.errors:
            raise RuntimeError(f'{name} 差分失败，详见 {out}')
        counts = collections.Counter(r['tag'] for r in server.records)
        expected_counts = collections.Counter(open1=1, open2=2, sysinfo=1, close=3, type0=4, servo=3)
        if spec.get('scaled'):
            expected_counts.update(type0=-2, scaled=4)
        if spec.get('family') == 2:
            expected_counts.update({'sysinfo':-1, 'sysinfo-axes':1, 'flags':1, 'global-axes':1})
        expected_counts += collections.Counter()
        if counts != expected_counts:
            raise ValueError(f'{name} 批内去重/重复批次调用数错误：{counts}')
    rows = json.loads((out/'samples/production-comparison.json').read_text(encoding='utf-8'))
    if len(rows) != 24 or not all(row['equal'] for row in rows):
        raise ValueError('正式适配器输出存在不一致')
    # 独立设定值再次与 DLL 比较，避免两侧共同解错小数位还通过差分。
    for tag, key in [('type0', 'spindle'), ('servo', 'servo')]:
        sample = json.loads((out/f'samples/native-{tag}.json').read_text(encoding='utf-8'))
        count = min(4, len(spec[key]), spec.get('before', len(spec[key])), spec.get('after', len(spec[key])))
        if key == 'servo' and spec.get('global_axes'):
            count = min(count, spec['global_axes'])
        if sample['rc'] or sample['num_out'] != count:
            raise ValueError('DLL 数量/返回码不同于独立预期')
        for index, record in enumerate(sample['candidates']):
            raw, decimal = spec[key][index]
            expected_raw = (abs(raw) if raw != -2147483648 else raw) if key == 'servo' else raw
            if key == 'spindle' and spec.get('scaled'):
                coefficient = spec['alt' if index == 1 else 'std'][index]
                product = (abs(raw)*coefficient) & 0xffffffff
                product = product - 0x100000000 if product >= 0x80000000 else product
                expected_raw = (abs(product)//32767) * (-1 if product < 0 else 1)
                decimal = 0
            if record['data'] != expected_raw or record['decimal'] != decimal or record['unit'] != 0:
                raise ValueError('DLL ABI 数值/小数位/单位不同于独立预期')
    modules = json.loads((out/'samples/modules-type0.json').read_text(encoding='utf-8'))
    files = {str(p.relative_to(out)).replace('\\', '/'): sha(p.read_bytes()) for p in out.rglob('*') if p.is_file()}
    result = {'kind': 'synthetic-derived', 'name': name, 'spec': spec, 'rows': len(rows), 'all_equal': True,
              'loaded_modules': modules, 'files': files}
    save(out/'manifest.json', result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-exe', type=pathlib.Path, required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--archive', type=pathlib.Path)
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('实际 DLL 差分需要 Windows')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    folder = FIXTURES/'load_synthetic_handshake'
    meta = json.loads((folder/'index.json').read_text(encoding='utf-8'))
    handshake = {name: (folder/name).read_bytes() for name in meta['files']}
    if any(sha(handshake[name]) != record['sha256'] for name, record in meta['files'].items()):
        raise ValueError('握手归档哈希不符')
    cases = [run_case(args.test_exe.resolve(strict=True), out, name, spec, handshake) for name, spec in CASES.items()]
    result = {'scope': '新旧分支生产百分比/批次读取/状态4回退', 'kind': 'synthetic-derived', 'all_equal': True,
              'cases': cases, 'limits': ['不证明真实设备生成非零负载']}
    save(out/'index.json', result)
    if args.archive:
        import shutil
        shutil.copytree(out, args.archive.resolve(), dirs_exist_ok=False)
    print(json.dumps({'cases': len(cases), 'rows': sum(c['rows'] for c in cases), 'all_equal': True}))


if __name__ == '__main__':
    main()
