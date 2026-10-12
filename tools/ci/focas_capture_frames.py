"""使用 Scapy 离线解析抓包：source.pcapng out-dir [--host HOST --port PORT]。

逐连接重组，拒绝缺口和冲突重传后才拆帧；只写全新目录，不覆盖原始采集。
"""
import argparse
import collections
import hashlib
import json
import pathlib
import sys
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / 'target/focas-load-research/python-deps'))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / 'tools' / 'ci'))
from scapy.all import PcapNgReader, IP, TCP
from focas_diff import cut_frames, parse_generic_request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('source', type=pathlib.Path)
parser.add_argument('out', type=pathlib.Path)
parser.add_argument('--host', default='192.168.15.165')
parser.add_argument('--port', type=int, default=8193)
args = parser.parse_args()
source, out = args.source.resolve(strict=True), args.out.resolve()
out.mkdir(parents=True, exist_ok=False)
flows = {}
epochs = collections.defaultdict(int)
syns = {}
layers = collections.Counter()
for index, packet in enumerate(PcapNgReader(str(source))):
    layers[packet.__class__.__name__] += 1
    if IP not in packet or TCP not in packet:
        continue
    ip, tcp = packet[IP], packet[TCP]
    if args.host not in (ip.src, ip.dst) or args.port not in (tcp.sport, tcp.dport):
        continue
    if tcp.dport == args.port:
        key = (ip.src, tcp.sport, ip.dst, tcp.dport)
        direction = 'c2s'
    else:
        key = (ip.dst, tcp.dport, ip.src, tcp.sport)
        direction = 's2c'
    if direction == 'c2s' and int(tcp.flags) & 2 and not int(tcp.flags) & 16:
        if key in syns and syns[key] != int(tcp.seq):
            epochs[key] += 1
        syns[key] = int(tcp.seq)
    flow = flows.setdefault((key, epochs[key]), {'c2s': [], 's2c': []})
    # 以 IPv4/TCP 声明长度排除以太网 Padding，不能把补齐字节拼进应用流。
    payload_len = int(ip.len)-int(ip.ihl)*4-int(tcp.dataofs)*4
    if payload_len < 0 or len(bytes(tcp.payload)) < payload_len:
        raise ValueError('捕获包短于 IP/TCP 声明长度')
    payload = bytes(tcp.payload)[:payload_len]
    if payload:
        if len(payload) > 65535:
            raise ValueError('异常 TCP payload 长度')
        flow[direction].append((int(tcp.seq), payload, index))

report = {'source': str(source), 'sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
          'link_layers': dict(layers), 'flows': []}
for number, ((key, epoch), directions) in enumerate(flows.items()):
    record = {'connection': list(key), 'epoch': epoch, 'directions': {}}
    folder = out / ('flow-%03d' % number)
    folder.mkdir(exist_ok=True)
    for direction, segments in directions.items():
        metadata = {'segments': len(segments)}
        record['directions'][direction] = metadata
        if not segments:
            continue
        segments.sort(key=lambda s: s[0])
        start = segments[0][0]
        buf = bytearray()
        errors = []
        for seq, data, index in segments:
            offset = seq-start
            if offset > len(buf):
                errors.append({'packet': index, 'kind': 'gap', 'bytes': offset-len(buf)})
                break
            overlap = min(len(data), len(buf)-offset)
            if buf[offset:offset+overlap] != data[:overlap]:
                errors.append({'packet': index, 'kind': 'conflicting_retransmission'})
                break
            buf.extend(data[overlap:])
            if len(buf) > 10_000_000:
                raise ValueError('流超过本次诊断上限')
        metadata['reassembly_errors'] = errors
        if errors:
            continue
        stream = folder / (direction + '.bin')
        stream.write_bytes(buf)
        metadata['bytes'] = len(buf)
        try:
            frames = cut_frames(str(stream))
        except ValueError as error:
            metadata['frame_error'] = str(error)
            continue
        metadata['frames'] = []
        for fi, frame in enumerate(frames):
            path = folder / ('%s-%03d.bin' % (direction, fi))
            path.write_bytes(frame)
            entry = {'file': str(path), 'type': frame[6:8].hex(), 'bytes': len(frame),
                     'sha256': hashlib.sha256(frame).hexdigest()}
            if frame[6:8] == b'\x21\x01':
                entry['request'] = parse_generic_request(frame)
            metadata['frames'].append(entry)
    report['flows'].append(record)
(out / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
print(json.dumps({'flows': len(flows), 'layers': dict(layers), 'report': str(out/'report.json')}))
