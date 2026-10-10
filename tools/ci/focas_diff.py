#!/usr/bin/env python3
"""PR-A 离线 FOCAS 帧提取与请求差分（只读，不连 CNC，不控机床）。

子命令：
  scan <frames-dir>                 帧清单（SHA + GENERIC 子包解析）。
  compare --native <bin> --wire <bin>
                                    Native 请求与 Wire fixture 逐字节比较，
                                    输出 match/first_diff，退出码 0=一致。

frames-dir：从 pcapng 按 10B 帧头（magic a0a0a0a0 + payload_len@8）切出的
  原始帧文件（`open_req.bin/open_resp.bin/generic_req_N.bin/...`）。

FOCAS 帧：`magic(4) + origin(2) + type(2) + len(2) + payload`。
GENERIC 请求子包：`len(2) + dev(2) + func(4) + args(5×i32)` = 至少 28B。
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import struct
import sys

REQ_SUB_MIN = 28  # len(2) + dev(2) + func(4) + args(20)


def sha256_bytes(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        h.update(f.read())
    return h.hexdigest()


def cut_frames(path: str) -> list[bytes]:
    raw = open(path, 'rb').read()
    if not raw:
        raise ValueError(f'{path}: empty input')
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
    """GENERIC 请求子包严格闭合解析（Mesa frame.rs 同纪律）。

    - count==0 即错；每子包 size>=28；off+size 不超 payload；
    - 全子包耗尽 payload（多余尾部即错）；body 恰好 26B（dev+func+args）。
    """
    payload = frame[10:]
    if len(payload) < 2:
        raise ValueError('generic payload < 2B')
    (count,) = struct.unpack('>H', payload[0:2])
    if count == 0:
        raise ValueError('generic count == 0')
    subs = []
    off = 2
    for i in range(count):
        if off + 2 > len(payload):
            raise ValueError(f'sub[{i}]: truncated size field')
        (size,) = struct.unpack('>H', payload[off:off + 2])
        if size < REQ_SUB_MIN:
            raise ValueError(f'sub[{i}]: size {size} < {REQ_SUB_MIN}')
        if off + size > len(payload):
            raise ValueError(f'sub[{i}]: size {size} overruns payload')
        body = payload[off + 2:off + size]
        if len(body) != size - 2:
            raise ValueError(f'sub[{i}]: body length mismatch')
        dev = struct.unpack('>H', body[0:2])[0]
        func = struct.unpack('>I', body[2:6])[0]
        args = list(struct.unpack('>5i', body[6:26]))
        if len(body) != 26:
            raise ValueError(f'sub[{i}]: request body must be 26B, got {len(body)}')
        subs.append({'dev': dev, 'func': f'{func:#010x}', 'args': args})
        off += size
    if off != len(payload):
        raise ValueError(f'{len(payload) - off} trailing bytes after {count} subs')
    return {'count': count, 'subs': subs}


def cmd_scan(args: argparse.Namespace) -> int:
    names = sorted(
        n for n in os.listdir(args.frames_dir)
        if os.path.isfile(os.path.join(args.frames_dir, n)) and n.endswith('.bin')
    )
    if not names:
        print(json.dumps({'error': 'no .bin inputs'}))
        return 1
    for name in names:
        p = os.path.join(args.frames_dir, name)
        try:
            frames = cut_frames(p)
        except ValueError as e:
            print(json.dumps({'file': name, 'error': str(e)}))
            return 1
        for i, f in enumerate(frames):
            doc = {
                'file': name,
                'index': i,
                'sha256': sha256_bytes(f),
                'origin': f'{struct.unpack(">H", f[4:6])[0]:#06x}',
                'ptype': f'{struct.unpack(">H", f[6:8])[0]:#06x}',
                'len': struct.unpack('>H', f[8:10])[0],
            }
            # GENERIC 请求才解析子包；OPEN/CLOSE 只留帧级元数据。
            if doc['ptype'] == '0x2101':
                try:
                    doc['generic'] = parse_generic_request(f)
                except ValueError as e:
                    doc['generic_error'] = str(e)
            print(json.dumps(doc))
    return 0


def cmd_compare(args: argparse.Namespace) -> int:
    try:
        native = open(args.native, 'rb').read()
        wire = open(args.wire, 'rb').read()
    except OSError as e:
        print(json.dumps({'match': False, 'error': str(e)}))
        return 1
    doc: dict = {
        'match': native == wire,
        'native_len': len(native),
        'wire_len': len(wire),
        'native_sha256': sha256_bytes(native),
        'wire_sha256': sha256_bytes(wire),
    }
    if native != wire:
        n = min(len(native), len(wire))
        off = next((i for i in range(n) if native[i] != wire[i]), n)
        doc['first_diff_offset'] = off
        doc['native_byte'] = f'{native[off]:02x}' if off < len(native) else None
        doc['wire_byte'] = f'{wire[off]:02x}' if off < len(wire) else None
        print(json.dumps(doc))
        return 1
    doc['first_diff_offset'] = None
    print(json.dumps(doc))
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest='cmd', required=True)
    s = sub.add_parser('scan', help='帧清单')
    s.add_argument('frames_dir')
    s.set_defaults(fn=cmd_scan)
    c = sub.add_parser('compare', help='Native/Wire 逐字节比较')
    c.add_argument('--native', required=True)
    c.add_argument('--wire', required=True)
    c.set_defaults(fn=cmd_compare)
    args = ap.parse_args(argv)
    return args.fn(args)


if __name__ == '__main__':
    sys.exit(main())
