"""从 pktmon 转出的 pcapng 里提取 165:8193 的 TCP application bytes。

只用标准库：解 pcapng(SHB/IDB/EPB) → 解 802.11/LLC 或 Ethernet →
IPv4/TCP → 按四元组重组流 → 落盘每条流双向字节。
用法: python tools/gate0_extract.py <pcapng路径> [输出目录]
"""
from __future__ import annotations

import os
import struct
import sys

CNC = "192.168.15.165"
PORT = 8193


def mac_str(b: bytes) -> str:
    return ":".join(f"{x:02x}" for x in b)


def parse_pcapng(path: str):
    """产出 (ts_us, eth_payload) 序列。只处理 EPB(6) 与已废弃 PB(3)。"""
    data = open(path, "rb").read()
    if data[:4] != b"\x0a\x0d\x0d\x0a":
        raise ValueError("不是 pcapng（magic 不符）")
    off = 0
    n = len(data)
    while off + 12 <= n:
        btype, blen = struct.unpack_from("<II", data, off)
        if blen < 12 or off + blen > n:
            break
        body = data[off + 8 : off + blen - 4]
        if btype == 6 and len(body) >= 20:  # EPB
            cap_len = struct.unpack_from("<I", body, 20)[0]
            pkt = body[28 : 28 + cap_len]
            yield pkt
        elif btype == 3 and len(body) >= 12:  # PB
            cap_len = struct.unpack_from("<II", body, 4)[0]
            pkt = body[12 : 12 + cap_len]
            yield pkt
        off += blen
    # 诊断
    print(f"文件 {n}B 解析完毕", flush=True)


def parse_eth(pkt: bytes, linktype: int = 1):
    """链路适配（实测 pktmon 混合封装，以 IP 头特征 0x45 定位为准）：
    - 有线：… 08 00 | 45 00 …（ether type 后紧跟 IP）；
    - 无线：… AA AA 03 00 00 00 08 00 | 45 00 …（LLC/SNAP 后紧跟 IP）。
    返回 IP 字节或 None。
    """
    if linktype in (12, 228):  # RAW：无链路头，直接是 IP
        return pkt
    # 通用：在前 64B 内找 IPv4 头（version=4, IHL>=5, protocol=TCP/UDP/ICMP），
    # pktmon 的 radiotap/802.11/Ethernet 头长度不固定，特征定位最可靠。
    for i in range(0, min(64, len(pkt) - 20)):
        if (
            pkt[i] == 0x45
            and pkt[i + 1] == 0x00
            and pkt[i + 9] in (1, 6, 17)
            and (pkt[i] >> 4) == 4
        ):
            return pkt[i:]
    return None


def parse_ipv4(ip: bytes):
    if len(ip) < 20:
        return None
    ver_ihl = ip[0]
    ihl = (ver_ihl & 0x0F) * 4
    if len(ip) < ihl or ip[9] != 6:  # 非 TCP
        return None
    src = ".".join(str(b) for b in ip[12:16])
    dst = ".".join(str(b) for b in ip[16:20])
    return src, dst, ip[ihl:]


def parse_tcp(seg: bytes):
    if len(seg) < 20:
        return None
    sport, dport = struct.unpack_from(">HH", seg, 0)
    off_flags = struct.unpack_from(">H", seg, 12)[0]
    hlen = ((off_flags >> 12) & 0xF) * 4
    if len(seg) < hlen:
        return None
    seq = struct.unpack_from(">I", seg, 4)[0]
    return sport, dport, seq, seg[hlen:]


def main() -> None:
    src = sys.argv[1]
    out_base = sys.argv[2] if len(sys.argv) > 2 else os.path.join(
        os.environ.get("TEMP", "."), "gate0_flows"
    )
    os.makedirs(out_base, exist_ok=True)
    flows: dict[tuple, dict] = {}
    total_pkts = 0
    hit_pkts = 0
    hit_bytes = 0
    for pkt in parse_pcapng(src):
        total_pkts += 1
        ip = parse_eth(pkt)
        if ip is None:
            continue
        r = parse_ipv4(ip)
        if r is None:
            continue
        s_ip, d_ip, seg = r
        t = parse_tcp(seg)
        if t is None:
            continue
        sport, dport, seq, payload = t
        if PORT not in (sport, dport):
            continue
        if CNC not in (s_ip, d_ip):
            continue
        hit_pkts += 1
        # 归一化四元组：key=(client_ip, client_port, server_port)，方向按端口判
        if dport == PORT:
            key = (s_ip, sport)
            direction = "c2s"
        else:
            key = (d_ip, dport)
            direction = "s2c"
        f = flows.setdefault(key, {"c2s": [], "s2c": []})
        if payload:
            f[direction].append((seq, bytes(payload)))
            hit_bytes += len(payload)
    print(f"总包 {total_pkts}，命中 {CNC}:{PORT} 的包 {hit_pkts}，payload {hit_bytes}B", flush=True)
    print(f"流数 {len(flows)}", flush=True)
    for (cip, cport), f in sorted(flows.items()):
        seg_dir = os.path.join(out_base, f"{cip.replace('.', '_')}_{cport}")
        os.makedirs(seg_dir, exist_ok=True)
        for direction in ("c2s", "s2c"):
            chunks = f[direction]
            chunks.sort(key=lambda x: x[0])
            # 简单重组：按 seq 排序拼接（重传包去重，非严格 TCP 重组，
            # Gate 0 只需看清帧边界，足够）。
            seen: set[int] = set()
            buf = bytearray()
            for seq, p in chunks:
                if seq in seen:
                    continue
                seen.add(seq)
                buf += p
            open(os.path.join(seg_dir, f"{direction}.bin"), "wb").write(bytes(buf))
            print(f"  {cip}:{cport} {direction} {len(buf)}B -> {seg_dir}", flush=True)
    print(f"输出目录: {out_base}", flush=True)


if __name__ == "__main__":
    main()
