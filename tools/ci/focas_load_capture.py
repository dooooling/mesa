"""只读 Native/Wire 负载对照采集：Npcap + 已构建的 ignored 测试。

需要 Windows、Npcap、Scapy；--test-exe 必须指向本仓库最新 lib 测试二进制。
全新目录保存抓包、进程输出与样本；不操作面板，不解除生产门禁。
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import threading
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
# 可使用隔离在研究目录的依赖，不要求修改全局 Python 环境。
sys.path.insert(0, str(ROOT / 'target/focas-load-research/python-deps'))


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-exe', type=pathlib.Path, required=True)
    parser.add_argument('--host', required=True)
    parser.add_argument('--port', type=int, default=8193)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--test-name', default='wire::load_research::tests::load_protocol_pair_live',
                        choices=['wire::load_research::tests::load_protocol_pair_live',
                                 'wire::opmsg_parity::guarded_ready_dll_pair_live'])
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('本入口使用 Windows Native DLL 与 Npcap')
    if not 1 <= args.port <= 65535:
        parser.error('端口必须为 1..65535')
    # 解析为数值 IPv4 后再拼捕获过滤器，避免自由文本改变过滤范围。
    import ipaddress
    host = str(ipaddress.IPv4Address(args.host))
    exe = args.test_exe.resolve(strict=True)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    from scapy.all import AsyncSniffer, PcapNgWriter, conf
    iface, local, gateway = conf.route.route(host)
    writer = PcapNgWriter(str(out / 'capture.pcapng'))
    ready = threading.Event()
    packets = 0
    overflow = False

    def packet_seen(packet):
        nonlocal packets, overflow
        packets += 1
        if packets <= 20000:
            writer.write(packet)
        else:
            overflow = True

    sniffer = AsyncSniffer(iface=iface, filter=f'tcp and host {host} and port {args.port}',
                           store=False, prn=packet_seen, started_callback=ready.set)
    command = [str(exe), '--exact', args.test_name,
               '--ignored', '--nocapture', '--test-threads=1']
    metadata = {'host':host, 'port':args.port, 'local':local, 'interface':str(iface),
                'gateway':gateway, 'test_exe':str(exe), 'test_exe_sha256':digest(exe),
                'command':command, 'packet_limit':20000, 'dropped_packets':None,
                'drop_note':'未获取 Npcap 内核丢包统计；需另外核对 TCP 流闭合与预期调用覆盖',
                'git_sha':subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT, text=True).strip(),
                'started_unix_ns':time.time_ns(), 'production_gate_changed':False}
    sniffer.start()
    try:
        if not ready.wait(5):
            raise RuntimeError('Npcap 未就绪，拒绝无抓包运行采样')
        env = dict(os.environ, MESA_FOCAS_GATE0_HOST=host, MESA_FOCAS_GATE0_PORT=str(args.port),
                   MESA_FOCAS_LOAD_PAIR_OUT=str(out / 'samples'))
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=90)
        (out / 'test.log').write_bytes(result.stdout)
        metadata['test_exit_code'] = result.returncode
        print(result.stdout.decode('utf-8', errors='replace'))
        # 收尾报文也属于证据；短暂留出 TCP FIN/ACK 的捕获窗口。
        time.sleep(0.3)
    finally:
        if sniffer.running:
            sniffer.stop()
        writer.close()
        metadata.update(finished_unix_ns=time.time_ns(), packets=packets, overflow=overflow)
        capture = out / 'capture.pcapng'
        metadata['capture_sha256'] = digest(capture)
        (out / 'capture.json').write_text(json.dumps(metadata, ensure_ascii=False, indent=2), encoding='utf-8')
    if overflow or metadata.get('test_exit_code') != 0:
        raise RuntimeError(f'取证失败，保留现场：{out}')
    # 防止过滤名错误、零测试或未来被忽略的平台分支冒充成功。
    if not (out / 'samples/comparison.json').is_file():
        raise RuntimeError(f'没有对照样本，不能以进程退出 0 判定成功：{out}')
    print(f'CAPTURE_EVIDENCE={out}')


if __name__ == '__main__':
    main()
