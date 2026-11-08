#!/usr/bin/env python3
"""Isolated DNS and TCP observer for the W11-09 routing E2E test.

This helper only runs inside the disposable upstream network namespace created
by scripts/test-traffic-routing-netns.sh. It answers A queries for synthetic
.test names and appends one JSON observation per accepted TCP connection.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import socket
import struct
from pathlib import Path


def parse_question(packet: bytes) -> tuple[str, int, bytes]:
    if len(packet) < 17:
        raise ValueError("short DNS packet")
    offset = 12
    labels: list[str] = []
    while True:
        if offset >= len(packet):
            raise ValueError("truncated DNS name")
        length = packet[offset]
        offset += 1
        if length == 0:
            break
        if length & 0xC0:
            raise ValueError("compressed query names are unsupported")
        if length > 63 or offset + length > len(packet):
            raise ValueError("invalid DNS label")
        labels.append(packet[offset : offset + length].decode("ascii").lower())
        offset += length
    if offset + 4 > len(packet):
        raise ValueError("truncated DNS question")
    qtype, _qclass = struct.unpack("!HH", packet[offset : offset + 4])
    return ".".join(labels), qtype, packet[12 : offset + 4]


class DnsProtocol(asyncio.DatagramProtocol):
    def __init__(self, records: dict[str, str]) -> None:
        self.records = records
        self.transport: asyncio.DatagramTransport | None = None

    def connection_made(self, transport: asyncio.BaseTransport) -> None:
        self.transport = transport  # type: ignore[assignment]

    def datagram_received(self, data: bytes, address: tuple[str, int]) -> None:
        try:
            name, qtype, question = parse_question(data)
        except (UnicodeDecodeError, ValueError):
            return
        answer = b""
        answer_count = 0
        if qtype == 1 and name in self.records:
            answer = (
                b"\xc0\x0c"
                + struct.pack("!HHIH", 1, 1, 30, 4)
                + socket.inet_aton(self.records[name])
            )
            answer_count = 1
        # Standard recursive response, one question and zero or one A answer.
        header = data[:2] + struct.pack("!HHHHH", 0x8180, 1, answer_count, 0, 0)
        if self.transport is not None:
            self.transport.sendto(header + question + answer, address)


async def handle_tcp(
    reader: asyncio.StreamReader,
    writer: asyncio.StreamWriter,
    observations,
) -> None:
    del reader
    peer = writer.get_extra_info("peername")
    local = writer.get_extra_info("sockname")
    if isinstance(peer, tuple) and isinstance(local, tuple):
        observation = {
            "source_ip": peer[0],
            "source_port": peer[1],
            "destination_ip": local[0],
            "destination_port": local[1],
        }
        observations.write(json.dumps(observation, sort_keys=True) + "\n")
        observations.flush()
        os.fsync(observations.fileno())
    writer.close()
    await writer.wait_closed()


async def serve(args: argparse.Namespace) -> None:
    loop = asyncio.get_running_loop()
    records = dict(item.split("=", 1) for item in args.record)
    await loop.create_datagram_endpoint(
        lambda: DnsProtocol(records),
        local_addr=(args.dns_address, 53),
    )
    with Path(args.observations).open("a", encoding="utf-8", buffering=1) as observations:
        server = await asyncio.start_server(
            lambda reader, writer: handle_tcp(reader, writer, observations),
            host="0.0.0.0",
            port=args.tcp_port,
        )
        Path(args.ready).write_text("ready\n", encoding="utf-8")
        async with server:
            await server.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dns-address", required=True)
    parser.add_argument("--tcp-port", type=int, default=443)
    parser.add_argument("--record", action="append", default=[])
    parser.add_argument("--observations", required=True)
    parser.add_argument("--ready", required=True)
    asyncio.run(serve(parser.parse_args()))


if __name__ == "__main__":
    main()
