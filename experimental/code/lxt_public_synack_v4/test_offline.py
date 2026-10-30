#!/usr/bin/env python3
"""Offline tests; no root, network, or /dev/net/tun needed."""
import collections
import secrets
import socket
import struct
import unittest

import lxt_public_lab as lxt


class PublicLabOfflineTests(unittest.TestCase):
    def test_authenticated_messages(self):
        key = secrets.token_bytes(32)
        replay = collections.OrderedDict()
        packet = lxt.mac_msg(key, lxt.HELLO, b'hello')
        self.assertEqual(lxt.check_msg(key, packet, replay), (lxt.HELLO, b'hello'))
        self.assertIsNone(lxt.check_msg(key, packet, replay))
        packet = bytearray(lxt.mac_msg(key, lxt.SYNACK, b'x'))
        packet[-1] ^= 1
        self.assertIsNone(lxt.check_msg(key, bytes(packet), replay))

    def test_tcp_checksum_and_rewrite(self):
        orig = '10.0.0.2'
        dst = '192.0.2.10'
        h = bytearray(20)
        h[0] = 0x45
        struct.pack_into('!H', h, 2, 40)
        h[8] = 64
        h[9] = socket.IPPROTO_TCP
        h[12:16] = socket.inet_aton(orig)
        h[16:20] = socket.inet_aton(dst)
        t = bytearray(20)
        struct.pack_into('!HHIIHH', t, 0, 39090, 45678, 1234, 2345, (5 << 12) | 0x12, 65535)
        result = lxt.fix_checksums(bytes(h + t))
        self.assertEqual(lxt.checksum(result[:20]), 0)
        self.assertEqual(lxt.checksum(result[12:20] + struct.pack('!BBH', 0, 6, 20) + result[20:]), 0)
        frame = b'\0' * 12 + b'\x08\x00' + result
        data = lxt.tcp_ipv4(frame)
        self.assertIsNotNone(data)
        self.assertEqual(data[2:9], (orig, dst, 39090, 45678, 1234, 2345, 0x12))
        # Emulate return through a home NAT, re-addressed to local client's NIC.
        changed = bytearray(data[0])
        changed[12:16] = socket.inet_aton('198.51.100.17')
        changed[16:20] = socket.inet_aton('10.0.0.23')
        struct.pack_into('!H', changed, data[1] + 2, 52001)
        changed = lxt.fix_checksums(changed)
        self.assertEqual(lxt.checksum(changed[:20]), 0)
        self.assertEqual(lxt.checksum(changed[12:20] + struct.pack('!BBH', 0, 6, 20) + changed[20:]), 0)


if __name__ == '__main__':
    unittest.main()
