#!/usr/bin/env python3
"""PR-A 离线 FOCAS 帧提取与请求差分（只读，不连 CNC，不控机床）。

用法：
  python3 tools/ci/focas_diff.py <pcapng-derived-frames-dir> <wire-fixture-dir>

输入：
  frames-dir：从 pcapng 按 10B 帧头（magic a0a0a0a0 + payload_len@8）切出的
    原始帧文件（`open_req.bin/open_resp.bin/generic_req_N.bin/...`）。
  wire-dir：PR #91 fixture（如 spindle_load_165/type0/request_frame.bin）。

输出：逐字节差异 + 分支归属（stdout JSONL；不写库、不改码）。

FOCAS 帧：`magic(4) + origin(2) + type(2) + len(2) + payload`。
GENERIC 请求：`count(2) + subpackets[len(2)+dev(2)+func(4)+args(20)]`。
"""
from __future__ import annotations

import hashlib
import json
import os
import struct
import sys


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        h.update(f.read())
    return h.hexdigest()


def cut_frames(path: str) -> list[bytes]:
    raw = open(path, 'rb').read()
    out: list[bytes] = []
    off = 0
    while off + 10 <= len(raw):
        if raw[off:off + 4] != b'\xa0\xa0\xa0\xa0':
            raise ValueError(f'{path}: bad magic at {off}')
        (plen,) = struct.unpack('>H', raw[off + 8:off + 10])
        frame = raw[off:off + 10 + plen]
        if len(frame) < 10 + plen:
            raise ValueError(f'{path}: truncated frame at {off}')
        out.append(frame)
        off += 10 + plen
    if off != len(raw):
        raise ValueError(f'{path}: {len(raw) - off} trailing bytes')
    return out


def parse_generic_request(frame: bytes) -> dict:
    payload = frame[10:]
    (count,) = struct.unpack('>H', payload[0:2])
    subs = []
    off = 2
    for _ in range(count):
        (size,) = struct.unpack('>H', payload[off:off + 2])
        body = payload[off + 2:off + size]
        dev = struct.unpack('>H', body[0:2])[0]
        func = struct.unpack('>I', body[2:6])[0]
        args = list(struct.unpack('>5i', body[6:26]))
        subs.append({'dev': dev, 'func': f'{func:#010x}', 'args': args})
        off += size
    return {'count': count, 'subs': subs}


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    frames_dir, wire_dir = sys.argv[1], sys.argv[2]
    for name in sorted(os.listdir(frames_dir)):
        p = os.path.join(frames_dir, name)
        if not os.path.isfile(p) or not name.endswith('.bin'):
            continue
        frames = cut_frames(p)
        for i, f in enumerate(frames):
            doc = {
                'file': name,
                'index': i,
                'sha256': hashlib.sha256(f).hexdigest(),
                'origin': f'{struct.unpack(">H", f[4:6])[0]:#06x}',
                'ptype': f'{struct.unpack(">H", f[6:8])[0]:#06x}',
                'len': struct.unpack('>H', f[8:10])[0],
            }
            # GENERIC 请求才解析子包；OPEN/CLOSE 只留帧级元数据。
            if doc['ptype'] == '0x2101':
                doc['generic'] = parse_generic_request(f)
            print(json.dumps(doc))
    # Wire fixture 侧：只输出 SHA（差分由人工/脚本二次比对，不在此硬编码期望）。
    for root, _, files in os.walk(wire_dir):
        for fn in sorted(files):
            if fn.endswith('.bin'):
                p = os.path.join(root, fn)
                print(json.dumps({'fixture': os.path.relpath(p, wire_dir), 'sha256': sha256_file(p)}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
