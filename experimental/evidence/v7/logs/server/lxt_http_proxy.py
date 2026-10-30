#!/usr/bin/env python3
"""Minimal HTTP CONNECT proxy for the LXT V6 Google access test.

Only CONNECT is implemented.  This is a temporary lab harness, not a
production proxy: no authentication, no ACL, no accounting.
"""

import argparse
import socket
import socketserver
import sys
import threading

_LOG_LOCK = threading.Lock()


def log(message):
    with _LOG_LOCK:
        print("[proxy] " + message, file=sys.stderr, flush=True)


def read_header(sock, limit=16384):
    data = b""
    while b"\r\n\r\n" not in data and len(data) < limit:
        chunk = sock.recv(4096)
        if not chunk:
            break
        data += chunk
    return data


def pump(src, dst, label, counters):
    try:
        while True:
            chunk = src.recv(65536)
            if not chunk:
                try:
                    dst.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
                break
            dst.sendall(chunk)
            counters[label] += len(chunk)
    except OSError:
        pass
    finally:
        try:
            src.close()
        except OSError:
            pass


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        client = self.request
        peer_ip, peer_port = self.client_address
        client.settimeout(self.server.read_timeout)
        try:
            raw = read_header(client)
        except OSError as exc:
            log("header read failed from %s:%s: %s" % (peer_ip, peer_port, exc))
            return
        if not raw:
            return

        head, _sep, extra = raw.partition(b"\r\n\r\n")
        first = head.split(b"\r\n", 1)[0].decode("latin1", "replace")
        parts = first.split()
        if len(parts) < 2 or parts[0].upper() != "CONNECT":
            log("reject non-CONNECT from %s:%s: %r" % (peer_ip, peer_port, first))
            try:
                client.sendall(b"HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n")
            except OSError:
                pass
            return

        target = parts[1]
        host, sep, port_text = target.rpartition(":")
        if not sep:
            host, port_text = target, "443"
        try:
            port = int(port_text)
        except ValueError:
            log("bad target %r from %s:%s" % (target, peer_ip, peer_port))
            try:
                client.sendall(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
            except OSError:
                pass
            return

        log("CONNECT %s:%s from %s:%s" % (host, port, peer_ip, peer_port))
        try:
            infos = socket.getaddrinfo(host, port, socket.AF_INET, socket.SOCK_STREAM)
            if not infos:
                raise OSError("no IPv4 address")
            upstream = socket.create_connection(infos[0][4], timeout=self.server.connect_timeout)
        except OSError as exc:
            log("upstream failed %s:%s: %s" % (host, port, exc))
            try:
                client.sendall(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
            except OSError:
                pass
            return

        try:
            upstream.settimeout(None)
            client.settimeout(None)
            client.sendall(b"HTTP/1.1 200 Connection Established\r\nProxy-Agent: lxt-v6\r\n\r\n")
            if extra:
                upstream.sendall(extra)
        except OSError as exc:
            log("handshake failed %s:%s: %s" % (host, port, exc))
            try:
                upstream.close()
            except OSError:
                pass
            return

        up_addr = upstream.getpeername()
        log("upstream connected %s:%s via %s:%s" % (host, port, up_addr[0], up_addr[1]))
        counters = {"up": len(extra), "down": 0}
        t_up = threading.Thread(target=pump, args=(client, upstream, "up", counters), daemon=True)
        t_down = threading.Thread(target=pump, args=(upstream, client, "down", counters), daemon=True)
        t_up.start()
        t_down.start()
        t_up.join()
        t_down.join()
        log("tunnel closed %s:%s up=%d down=%d" % (host, port, counters["up"], counters["down"]))
        try:
            upstream.close()
        except OSError:
            pass


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    parser = argparse.ArgumentParser(description="LXT V6 HTTP CONNECT proxy")
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=39090)
    parser.add_argument("--connect-timeout", type=float, default=10.0)
    parser.add_argument("--read-timeout", type=float, default=15.0)
    args = parser.parse_args()

    server = Server((args.bind, args.port), Handler)
    server.connect_timeout = args.connect_timeout
    server.read_timeout = args.read_timeout
    log("listening on %s:%d" % (args.bind, args.port))
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
