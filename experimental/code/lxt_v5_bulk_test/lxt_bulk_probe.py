#!/usr/bin/env python3
"""LXT TCP data-plane integrity benchmark (Python 3.9+, standard library only).

IMPORTANT: This program does NOT perform UDP rescue. Run it over a native TCP
connection rescued by the V4 sidecars, with a separately controlled SYN-ACK drop.
Application send() chunking is NOT equivalent to IP fragmentation.
"""
import argparse
import hashlib
import os
import random
import socket
import struct
import sys
import time

MAGIC = b"LXTBULK1"
HEADER = struct.Struct("!8sQQB")
MODES = {"tiny": 1, "mixed": 2, "large": 3}
MODE_NAMES = {v: k for k, v in MODES.items()}
MAX_BYTES_PER_DIRECTION = 64 * 1024 * 1024


def chunks(size, mode, rng):
    remain = size
    while remain:
        if mode == 1:
            n = rng.choice((1, 7, 31, 64, 257, 511, 1024))
        elif mode == 2:
            n = rng.choice((1, 7, 31, 257, 1024, 1200, 1332, 1460, 4096, 16384))
        else:
            n = rng.choice((1200, 1332, 1460, 4096, 16384, 32768, 65536))
        n = min(remain, n)
        yield n
        remain -= n


def recv_exact(sock, total):
    buf = bytearray()
    while len(buf) < total:
        data = sock.recv(min(65536, total-len(buf)))
        if not data:
            raise ConnectionError("peer closed while reading control bytes")
        buf.extend(data)
    return bytes(buf)


def read_payload(sock, size):
    h = hashlib.sha256()
    left = size
    while left:
        data = sock.recv(min(65536, left))
        if not data:
            raise ConnectionError("peer closed mid-payload, %d bytes missing" % left)
        h.update(data)
        left -= len(data)
    return h.digest()


def write_payload(sock, size, mode, rate_mbps, seed):
    # Bounded memory; deterministic random payloads, never compressed/repeated zero blocks.
    rng = random.Random(seed)
    pattern = random.Random(seed ^ 0x6A4D9375)
    h = hashlib.sha256()
    sent = 0
    start = time.monotonic()
    for length in chunks(size, mode, pattern):
        payload = rng.randbytes(length)
        h.update(payload)
        sock.sendall(payload)
        sent += length
        # Rate is capped in aggregate, and only sleeps if materially ahead.
        if rate_mbps > 0 and sent % (64 * 1024) < length:
            target = sent * 8 / (rate_mbps * 1_000_000)
            ahead = target - (time.monotonic()-start)
            if ahead > 0.001:
                time.sleep(ahead)
    return h.digest()


def serve(args):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind((args.bind, args.port))
        listener.listen(16)
        print(f"[server] listen {args.bind}:{args.port}; max {MAX_BYTES_PER_DIRECTION // (1024*1024)} MiB/dir/round", flush=True)
        while True:
            conn, peer = listener.accept()
            print(f"[server] accepted {peer}", flush=True)
            with conn:
                conn.settimeout(args.timeout)
                index = 0
                try:
                    while True:
                        try:
                            first = conn.recv(1)
                        except socket.timeout:
                            raise TimeoutError("idle timeout")
                        if not first:
                            break
                        header = first + recv_exact(conn, HEADER.size - 1)
                        magic, up, down, mode = HEADER.unpack(header)
                        if magic != MAGIC or mode not in MODE_NAMES or max(up, down) > MAX_BYTES_PER_DIRECTION:
                            raise ValueError("bad frame header/size")
                        index += 1
                        incoming = read_payload(conn, up)
                        conn.sendall(incoming)  # 32-byte SHA-256 receipt
                        outbound = write_payload(conn, down, mode, args.rate_mbps, seed=(index << 17) ^ down ^ 0xB00B1E)
                        conn.sendall(outbound)
                        if recv_exact(conn, 1) != b"\x06":
                            raise ValueError("client did not confirm downloaded data checksum")
                        print(f"[server] conn={peer} round={index} upload={up} download={down} mode={MODE_NAMES[mode]} validated", flush=True)
                except (OSError, ValueError, ConnectionError, TimeoutError) as exc:
                    print(f"[server] closed with error: {exc}", flush=True)
                else:
                    print(f"[server] closed normally rounds={index}", flush=True)


def client(args):
    up = int(args.up_mib * 1024 * 1024)
    down = int(args.down_mib * 1024 * 1024)
    if not (0 < up <= MAX_BYTES_PER_DIRECTION and 0 < down <= MAX_BYTES_PER_DIRECTION):
        raise ValueError("up/down each must be > 0 and <= 64 MiB")
    if args.connections < 1 or args.connections > 50 or args.rounds < 1 or args.rounds > 30:
        raise ValueError("connections must be 1..50; rounds 1..30")
    success, total_bytes, establish_times = 0, 0, []
    began = time.monotonic()
    for connection in range(1, args.connections+1):
        t0 = time.monotonic()
        with socket.create_connection((args.server, args.port), timeout=args.timeout) as sock:
            connect_ms = (time.monotonic() - t0) * 1000
            establish_times.append(connect_ms)
            sock.settimeout(args.timeout)
            print(f"[client] conn={connection}/{args.connections} established in {connect_ms:.1f} ms", flush=True)
            for roundnum in range(1, args.rounds+1):
                mode_name = args.mode
                if mode_name == "cycle":
                    mode_name = ("tiny", "mixed", "large")[(roundnum-1) % 3]
                mode = MODES[mode_name]
                t1 = time.monotonic()
                sock.sendall(HEADER.pack(MAGIC, up, down, mode))
                sent_digest = write_payload(sock, up, mode, args.rate_mbps,
                    seed=(connection << 30) ^ (roundnum << 17) ^ up ^ 0xAE5)
                remote_digest = recv_exact(sock, 32)
                if remote_digest != sent_digest:
                    raise ValueError(f"upload digest mismatch conn={connection} round={roundnum}")
                received_digest = read_payload(sock, down)
                expected_digest = recv_exact(sock, 32)
                if received_digest != expected_digest:
                    raise ValueError(f"download digest mismatch conn={connection} round={roundnum}")
                sock.sendall(b"\x06")  # final verified receipt to server
                duration = time.monotonic()-t1
                total_bytes += up+down
                success += 1
                print(f"[PASS] conn={connection} round={roundnum} mode={mode_name} "
                      f"up={up} down={down} sha256=OK throughput={(up+down)*8/duration/1e6:.2f} Mbps "
                      f"duration={duration:.2f}s", flush=True)
        if connection < args.connections:
            time.sleep(args.reconnect_delay)
    elapsed = time.monotonic()-began
    print(f"[SUMMARY] PASS rounds={success}/{args.connections*args.rounds} "
          f"connections={args.connections} bytes={total_bytes} duration={elapsed:.2f}s "
          f"avg_connect={sum(establish_times)/len(establish_times):.1f}ms", flush=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    s = p.add_subparsers(dest="cmd", required=True)
    server = s.add_parser("server")
    server.add_argument("--bind", default="0.0.0.0")
    server.add_argument("--port", type=int, default=39090)
    server.add_argument("--rate-mbps", type=float, default=10)
    server.add_argument("--timeout", type=float, default=45)
    server.set_defaults(func=serve)
    cli = s.add_parser("client")
    cli.add_argument("--server", required=True)
    cli.add_argument("--port", type=int, default=39090)
    cli.add_argument("--up-mib", type=float, default=1)
    cli.add_argument("--down-mib", type=float, default=1)
    cli.add_argument("--connections", type=int, default=3)
    cli.add_argument("--rounds", type=int, default=3)
    cli.add_argument("--mode", choices=["tiny", "mixed", "large", "cycle"], default="cycle")
    cli.add_argument("--rate-mbps", type=float, default=10)
    cli.add_argument("--timeout", type=float, default=45)
    cli.add_argument("--reconnect-delay", type=float, default=0.5)
    cli.set_defaults(func=client)
    args = p.parse_args()
    if not 1025 <= args.port <= 65535:
        p.error("port should be >1024")
    if args.rate_mbps < 0 or args.rate_mbps > 50:
        p.error("rate-mbps must be 0..50 (50 Mbps cap)")
    try:
        args.func(args)
    except (OSError, ValueError, ConnectionError, TimeoutError) as exc:
        print(f"[FAIL] {exc}", file=sys.stderr, flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
