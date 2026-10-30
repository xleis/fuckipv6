#!/usr/bin/env python3
"""LXT public-network SYN-ACK rescue research PoC (Linux only).

Requires root for AF_PACKET and client TUN. IPv4, 1:1 simple NAT supported
via matching TCP initial sequence numbers and rewriting endpoint addresses.
Not a production proxy or a guarantee against filtering.
"""
from __future__ import annotations

import argparse
import collections
import fcntl
import hashlib
import hmac
import ipaddress
import os
import secrets
import selectors
import socket
import struct
import subprocess
import sys
import time

ETH_P_ALL = 0x0003
ETH_P_IP = 0x0800
PACKET_OUTGOING = 4
IFF_TUN = 0x0001
IFF_NO_PI = 0x1000
TUNSETIFF = 0x400454CA
MAGIC = b"LXTPUB4"
HELLO, HELLO_ACK, SYNACK = 1, 2, 3
TAG_LEN = 16
MAX_UDP = 4096


def load_key() -> bytes:
    value = os.environ.get("LXT_LAB_KEY", "")
    try:
        key = bytes.fromhex(value)
    except ValueError as exc:
        raise ValueError("LXT_LAB_KEY must be a hex string") from exc
    if len(key) < 32:
        raise ValueError("LXT_LAB_KEY requires >= 32 bytes; use `openssl rand -hex 32`")
    return key


def mac_msg(key: bytes, mtype: int, body: bytes = b"") -> bytes:
    if len(body) > 2000:
        raise ValueError("body too large")
    packed = MAGIC + bytes([mtype]) + struct.pack("!Q", int(time.time())) + secrets.token_bytes(12) + struct.pack("!H", len(body)) + body
    return packed + hmac.new(key, packed, hashlib.sha256).digest()[:TAG_LEN]


def check_msg(key: bytes, data: bytes, replay: collections.OrderedDict[bytes, float]):
    header = len(MAGIC) + 1 + 8 + 12 + 2
    if len(data) < header + TAG_LEN or not data.startswith(MAGIC):
        return None
    mtype = data[len(MAGIC)]
    timestamp = struct.unpack_from("!Q", data, len(MAGIC) + 1)[0]
    if abs(time.time() - timestamp) > 30:
        return None
    body_len = struct.unpack_from("!H", data, header - 2)[0]
    if len(data) != header + body_len + TAG_LEN:
        return None
    tag = hmac.new(key, data[:-TAG_LEN], hashlib.sha256).digest()[:TAG_LEN]
    if not hmac.compare_digest(tag, data[-TAG_LEN:]):
        return None
    nonce = data[len(MAGIC) + 9:len(MAGIC) + 21]
    if nonce in replay:
        return None
    replay[nonce] = time.monotonic()
    while len(replay) > 4096:
        replay.popitem(last=False)
    return mtype, data[header:-TAG_LEN]


def checksum(data: bytes) -> int:
    if len(data) % 2:
        data += b"\x00"
    acc = sum(struct.unpack(f"!{len(data)//2}H", data))
    while acc >> 16:
        acc = (acc & 0xFFFF) + (acc >> 16)
    return (~acc) & 0xFFFF


def fix_checksums(packet: bytes) -> bytes:
    ip = bytearray(packet)
    ihl = (ip[0] & 15) * 4
    total = struct.unpack_from("!H", ip, 2)[0]
    ip[10:12] = b"\0\0"
    struct.pack_into("!H", ip, 10, checksum(bytes(ip[:ihl])))
    tcp = bytearray(ip[ihl:total])
    tcp[16:18] = b"\0\0"
    pseudo = ip[12:20] + struct.pack("!BBH", 0, socket.IPPROTO_TCP, len(tcp))
    struct.pack_into("!H", tcp, 16, checksum(pseudo + tcp))
    ip[ihl:total] = tcp
    return bytes(ip[:total])


def tcp_ipv4(frame: bytes):
    if len(frame) < 54 or frame[12:14] != b"\x08\x00":
        return None
    ip = frame[14:]
    if ip[0] >> 4 != 4:
        return None
    ihl = (ip[0] & 15) * 4
    if ihl < 20 or len(ip) < ihl + 20 or ip[9] != socket.IPPROTO_TCP:
        return None
    total = struct.unpack_from("!H", ip, 2)[0]
    frag = struct.unpack_from("!H", ip, 6)[0]
    if total < ihl + 20 or total > len(ip) or (frag & 0x3FFF):
        return None
    return ip[:total], ihl, socket.inet_ntoa(ip[12:16]), socket.inet_ntoa(ip[16:20]), *struct.unpack_from("!HH", ip, ihl), *struct.unpack_from("!II", ip, ihl+4), ip[ihl + 13]


def packet_socket(interface: str):
    s = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(ETH_P_ALL))
    s.bind((interface, ETH_P_ALL))
    return s


def open_tun(name: str):
    if len(name) > 15 or not name.isascii():
        raise ValueError("TUN name must be <= 15 ASCII characters")
    fd = os.open("/dev/net/tun", os.O_RDWR | os.O_CLOEXEC | os.O_NONBLOCK)
    try:
        fcntl.ioctl(fd, TUNSETIFF, struct.pack("16sH", name.encode("ascii"), IFF_TUN | IFF_NO_PI))
        subprocess.run(["ip", "link", "set", "dev", name, "up"], check=True)
        return fd
    except Exception:
        os.close(fd)
        raise


def run_server_sidecar(args):
    key = load_key()
    capture = packet_socket(args.interface)
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind((args.bind, args.udp_port))
    udp.setblocking(False)
    capture.setblocking(False)
    sel = selectors.DefaultSelector()
    sel.register(capture, selectors.EVENT_READ)
    sel.register(udp, selectors.EVENT_READ)
    peers: dict[str, tuple[tuple[str, int], float]] = {}
    replay: collections.OrderedDict[bytes, float] = collections.OrderedDict()
    last_packet: dict[tuple[str, int, int], float] = {}
    print(f"[server-sidecar] iface={args.interface} UDP={args.bind}:{args.udp_port} TCP sport={args.tcp_port}; waiting for authenticated HELLO", flush=True)
    while True:
        for keysel, _ in sel.select(timeout=1):
            if keysel.fileobj is udp:
                raw, addr = udp.recvfrom(MAX_UDP)
                parsed = check_msg(key, raw, replay)
                if parsed is None or parsed[0] != HELLO or raw is None:
                    continue
                peers[addr[0]] = (addr, time.monotonic())
                udp.sendto(mac_msg(key, HELLO_ACK), addr)
                print(f"[server-sidecar] HELLO verified from {addr[0]}:{addr[1]}", flush=True)
            else:
                frame, meta = capture.recvfrom(65535)
                if len(meta) < 3 or meta[2] != PACKET_OUTGOING:
                    continue
                parsed = tcp_ipv4(frame)
                if parsed is None:
                    continue
                ip, ihl, src, dst, sport, dport, seq, ack, flags = parsed
                if sport != args.tcp_port or flags & 0x17 != 0x12:
                    continue
                peer = peers.get(dst)
                if peer is None or time.monotonic() - peer[1] > 8:
                    continue
                id_ = (dst, dport, seq)
                if time.monotonic() - last_packet.get(id_, 0) < 0.25:
                    continue
                last_packet[id_] = time.monotonic()
                ip = fix_checksums(ip)
                packet = mac_msg(key, SYNACK, ip)
                udp.sendto(packet, peer[0])
                print(f"[server-sidecar] relayed SYN-ACK {src}:{sport} -> {dst}:{dport} tcpseq={seq} via UDP -> {peer[0][0]}:{peer[0][1]}", flush=True)
        if len(last_packet) > 8192:
            last_packet.clear()


def run_client_sidecar(args):
    key = load_key()
    server_ip = str(ipaddress.IPv4Address(args.server))
    capture = packet_socket(args.interface)
    tun = open_tun(args.tun)
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(("0.0.0.0", 0))
    udp.setblocking(False)
    capture.setblocking(False)
    sel = selectors.DefaultSelector()
    sel.register(capture, selectors.EVENT_READ)
    sel.register(udp, selectors.EVENT_READ)
    replay: collections.OrderedDict[bytes, float] = collections.OrderedDict()
    # (local_tcp_src_ip, local_tcp_src_port, syn_isn, last_seen)
    pending: list[tuple[str, int, int, float]] = []
    last_hello = 0.0
    endpoint = (server_ip, args.udp_port)
    print(f"[client-sidecar] iface={args.interface} TUN={args.tun} server={server_ip} UDP={udp.getsockname()[1]} -> {args.udp_port}", flush=True)
    while True:
        now = time.monotonic()
        if now - last_hello > args.hello_interval:
            udp.sendto(mac_msg(key, HELLO), endpoint)
            last_hello = now
        for keysel, _ in sel.select(timeout=0.3):
            if keysel.fileobj is capture:
                frame, meta = capture.recvfrom(65535)
                if len(meta) < 3 or meta[2] != PACKET_OUTGOING:
                    continue
                parsed = tcp_ipv4(frame)
                if parsed is None:
                    continue
                ip, ihl, src, dst, sport, dport, seq, ack, flags = parsed
                if dst != server_ip or dport != args.tcp_port or flags & 0x17 != 0x02:
                    continue
                pending = [it for it in pending if now - it[3] < 20]
                entry = (src, sport, seq, now)
                if not any(it[:3] == entry[:3] for it in pending):
                    pending.append(entry)
                    print(f"[client-sidecar] pending SYN local={src}:{sport} isn={seq}", flush=True)
            else:
                data, addr = udp.recvfrom(MAX_UDP)
                if addr != endpoint:
                    continue
                parsed = check_msg(key, data, replay)
                if parsed is None:
                    continue
                mtype, payload = parsed
                if mtype == HELLO_ACK:
                    print("[client-sidecar] HELLO_ACK verified", flush=True)
                    continue
                if mtype != SYNACK:
                    continue
                packet = b"\0" * 12 + b"\x08\x00" + payload
                tcp = tcp_ipv4(packet)
                if tcp is None:
                    continue
                ip, ihl, src, dst, sport, dport, seq, ack, flags = tcp
                if sport != args.tcp_port or flags & 0x17 != 0x12:
                    continue
                # Server src IP may be private behind 1:1 cloud NAT; on the
                # internet, packets to client originate from advertised server_ip.
                pending = [it for it in pending if time.monotonic() - it[3] < 20]
                matched = next((it for it in reversed(pending) if (it[2] + 1) & 0xffffffff == ack), None)
                if matched is None:
                    print(f"[client-sidecar] no matching active SYN for ack={ack}, ignoring", flush=True)
                    continue
                local_ip, local_port, _isn, _last = matched
                translated = bytearray(ip)
                translated[12:16] = socket.inet_aton(server_ip)
                translated[16:20] = socket.inet_aton(local_ip)
                struct.pack_into("!H", translated, ihl+2, local_port)
                translated = fix_checksums(translated)
                n = os.write(tun, translated)
                print(f"[client-sidecar] injected SYN-ACK to {local_ip}:{local_port} ({n} bytes); NAT remote={dst}:{dport}", flush=True)


def run_echo_server(args):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((args.bind, args.tcp_port))
    s.listen(8)
    print(f"[tcp-server] listening {args.bind}:{args.tcp_port} close_after={args.close_after}", flush=True)
    while True:
        conn, peer = s.accept()
        with conn:
            conn.settimeout(15)
            print(f"[tcp-server] accepted {peer}", flush=True)
            num = 0
            buf = b""
            while True:
                try:
                    chunk = conn.recv(2048)
                except socket.timeout:
                    break
                if not chunk:
                    break
                buf += chunk
                if len(buf) > 8192:
                    break
                while b"\n" in buf:
                    line, buf = buf.split(b"\n", 1)
                    conn.sendall(b"ACK:" + line + b"\n")
                    num += 1
                    if args.close_after and num >= args.close_after:
                        break
                if args.close_after and num >= args.close_after:
                    break


def run_probe(args):
    completed = 0
    established = 0
    conn = None
    buf = b""
    for i in range(1, args.count + 1):
        for attempt in range(args.retries + 1):
            if conn is None:
                try:
                    print(f"[probe] connect #{established+1} to {args.server}:{args.tcp_port}", flush=True)
                    conn = socket.create_connection((args.server, args.tcp_port), args.timeout)
                    conn.settimeout(args.timeout)
                    established += 1
                    print(f"[probe] ESTABLISHED {conn.getsockname()} -> {conn.getpeername()}", flush=True)
                except OSError as exc:
                    print(f"[probe] connect error: {exc}", flush=True)
                    conn = None
                    if attempt == args.retries:
                        sys.exit(2)
                    time.sleep(0.3)
                    continue
            try:
                conn.sendall(f"ping-{i}\n".encode())
                while b"\n" not in buf:
                    part = conn.recv(1024)
                    if not part:
                        raise ConnectionError("connection closed")
                    buf += part
                line, buf = buf.split(b"\n", 1)
                expected = f"ACK:ping-{i}".encode()
                if line != expected:
                    raise ConnectionError(f"unexpected response: {line!r}")
                completed += 1
                print(f"[probe] {completed}/{args.count} {line.decode()} (TCP #{established})", flush=True)
                break
            except (OSError, ConnectionError) as exc:
                print(f"[probe] reconnect after: {exc}", flush=True)
                conn.close()
                conn = None
                buf = b""
                if attempt == args.retries:
                    sys.exit(3)
                time.sleep(0.2)
        time.sleep(args.interval)
    if conn is not None:
        conn.close()
    print(f"[PASS] completed={completed}, established={established}", flush=True)


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    subs = p.add_subparsers(dest="mode", required=True)
    s = subs.add_parser("server-sidecar")
    s.add_argument("--interface", required=True)
    s.add_argument("--bind", default="0.0.0.0")
    s.add_argument("--tcp-port", type=int, default=39090)
    s.add_argument("--udp-port", type=int, default=39091)
    s.set_defaults(func=run_server_sidecar)
    s = subs.add_parser("client-sidecar")
    s.add_argument("--interface", required=True)
    s.add_argument("--tun", default="lxtrescue0")
    s.add_argument("--server", required=True)
    s.add_argument("--tcp-port", type=int, default=39090)
    s.add_argument("--udp-port", type=int, default=39091)
    s.add_argument("--hello-interval", type=float, default=1.5)
    s.set_defaults(func=run_client_sidecar)
    s = subs.add_parser("echo-server")
    s.add_argument("--bind", default="0.0.0.0")
    s.add_argument("--tcp-port", type=int, default=39090)
    s.add_argument("--close-after", type=int, default=0)
    s.set_defaults(func=run_echo_server)
    s = subs.add_parser("probe")
    s.add_argument("--server", required=True)
    s.add_argument("--tcp-port", type=int, default=39090)
    s.add_argument("--count", type=int, default=6)
    s.add_argument("--interval", type=float, default=0.25)
    s.add_argument("--timeout", type=float, default=3)
    s.add_argument("--retries", type=int, default=2)
    s.set_defaults(func=run_probe)
    return p


if __name__ == "__main__":
    try:
        args = parser().parse_args()
        if getattr(args, "tcp_port", 39090) <= 1024 or getattr(args, "tcp_port", 39090) > 65535:
            raise ValueError("choose a TCP port between 1025 and 65535")
        if getattr(args, "udp_port", 39091) <= 1024 or getattr(args, "udp_port", 39091) > 65535:
            raise ValueError("choose a UDP port between 1025 and 65535")
        args.func(args)
    except (OSError, ValueError, subprocess.CalledProcessError) as exc:
        print(f"[error] {exc}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("[stopped]", file=sys.stderr)
        sys.exit(130)
