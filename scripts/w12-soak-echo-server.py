#!/usr/bin/env python3
"""Small TCP canary server used by the W12-09 soak monitor.

The server binds only to the node's virtual IP. Each client sends one JSON
header line followed by a fixed-size payload; the server verifies both length
and SHA-256, returns a JSON result, and records one JSONL line.
"""

import argparse
import hashlib
import json
import socket
import time


MAX_HEADER_BYTES = 4096
MAX_PAYLOAD_BYTES = 1024 * 1024


def recv_exact(conn: socket.socket, size: int) -> bytes:
    chunks = []  # type: list
    remaining = size
    while remaining:
        chunk = conn.recv(min(65536, remaining))
        if not chunk:
            raise RuntimeError(f"short read: {remaining} bytes missing")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def recv_line(conn: socket.socket) -> bytes:
    data = bytearray()
    while len(data) <= MAX_HEADER_BYTES:
        chunk = conn.recv(1)
        if not chunk:
            break
        data += chunk
        if chunk == b"\n":
            return bytes(data)
    raise RuntimeError("canary header is missing or too large")


def handle(conn: socket.socket, peer: tuple) -> dict:
    started = time.monotonic()
    request = json.loads(recv_line(conn))
    size = int(request["bytes"])
    expected_sha256 = str(request["sha256"])
    if not 0 <= size <= MAX_PAYLOAD_BYTES:
        raise ValueError(f"invalid payload size: {size}")
    payload = recv_exact(conn, size)
    digest = hashlib.sha256(payload).hexdigest()
    ok = digest == expected_sha256
    result = {
        "ok": ok,
        "peer": [peer[0], peer[1]],
        "bytes": len(payload),
        "sha256": digest,
        "expected_sha256": expected_sha256,
        "cycle": request.get("cycle"),
        "elapsed_secs": time.monotonic() - started,
    }
    conn.sendall((json.dumps(result, sort_keys=True) + "\n").encode())
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", required=True)
    parser.add_argument("--port", required=True, type=int)
    args = parser.parse_args()

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind((args.host, args.port))
        listener.listen(16)
        listener.settimeout(30)
        print(
            json.dumps(
                {
                    "event": "listening",
                    "host": args.host,
                    "port": args.port,
                    "unix_ms": int(time.time() * 1000),
                },
                sort_keys=True,
            ),
            flush=True,
        )
        while True:
            try:
                conn, peer = listener.accept()
            except socket.timeout:
                continue
            with conn:
                conn.settimeout(30)
                try:
                    result = handle(conn, peer)
                except Exception as exc:  # Keep the 24h server alive after bad clients.
                    result = {"ok": False, "peer": [peer[0], peer[1]], "error": str(exc)}
                    try:
                        conn.sendall((json.dumps(result, sort_keys=True) + "\n").encode())
                    except OSError:
                        pass
                result["event"] = "request"
                result["unix_ms"] = int(time.time() * 1000)
                print(json.dumps(result, sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
