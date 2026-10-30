# 科目 04 — UDP 辅助 TCP 建连

## 1. 科目内容

在客户端持续阻断 SYN-ACK 的条件下：

1. 服务端 sidecar 捕获本机内核产生的真实 SYN-ACK；
2. 通过 HMAC 认证的 UDP 39091 转送到客户端；
3. 客户端 sidecar 校验、匹配正在建立的 TCP 连接；
4. 改写目的地址/端口、重算校验和，写入 TUN `lxtrescue0`；
5. 客户端 Linux TCP 栈接受注入的 SYN-ACK，连接进入 ESTABLISHED；
6. 后续 ACK/DATA 继续走原生 TCP（UDP 不承载业务数据）。

## 2. 代码位置与入口

- [`../../code/lxt_public_synack_v4/lxt_public_lab.py`](../../code/lxt_public_synack_v4/lxt_public_lab.py)
  - `run_server_sidecar()`（第 136 行）
  - `run_client_sidecar()`（第 185 行）
  - `tcp_ipv4()`（第 101 行）
  - `fix_checksums()`（第 87 行）
  - `open_tun()`（第 123 行）
- 客户端编排：[`../../code/lxt_public_synack_v4/phase4_client.sh`](../../code/lxt_public_synack_v4/phase4_client.sh)
- 完整报告：[`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)

## 3. 实现细节

### 3.1 服务端 sidecar

- `socket(AF_PACKET, SOCK_RAW, ETH_P_ALL)`，绑定指定网卡；只处理 `meta[2] == PACKET_OUTGOING`。
- 只接受 `sport == --tcp-port`（39090）且 `flags & 0x17 == 0x12`（SYN+ACK）的包。
- 按 SYN-ACK 的目的 IP 查找 `peers[dst]`；peer 必须是 8 秒内发过合法 HELLO 的 UDP 源。
- 同一 `(dst, dport, seq)` 0.25 秒内只转发一次，避免内核重传造成重复注入。
- 对完整 IPv4 包调用 `fix_checksums()`，再 `mac_msg(SYNACK, ip)`，通过 UDP 发回 peer。
- 不修改服务器路由/防火墙；只读 AF_PACKET。

### 3.2 客户端 sidecar

- `socket(AF_PACKET, SOCK_RAW, ETH_P_ALL)` 捕获出站 SYN：`dst == --server`、`dport == --tcp-port`、`flags & 0x17 == 0x02`。
- 记录 `pending = (local_src_ip, local_sport, isn, last_seen)`，20 秒过期。
- 收到 UDP SYNACK 后：
  1. `check_msg()` 通过 HMAC/时间戳/nonce；
  2. 解析裸 IPv4 包，确认 `sport == 39090` 且 `flags & 0x17 == 0x12`；
  3. 要求 `(pending.isn + 1) & 0xffffffff == synack.ack`；
  4. 把目的 IP 改为 `--server` 的地址、目的端口改为本地 `sport`；
  5. `fix_checksums()`；
  6. `os.write(tun, packet)`。
- 服务端真实源 IP 可能是云厂商 1:1 NAT 后的私网地址，所以客户端不按 SYN-ACK 源 IP 匹配，而按 `ack == isn+1` 匹配。

### 3.3 关键 Bug 与修复

`tcp_ipv4()` 要求 14 字节 Ethernet 头且 `frame[12:14] == b"\x08\x00"`。TUN 读到的是**裸 IP**，不是 Ethernet frame。V4 早期代码用：

```python
packet = b"\0" * 14 + payload
```

EtherType 为 `0x0000`，`tcp_ipv4()` 返回 `None`，救援包被静默 `continue`——日志有 `pending SYN` 和 `relayed SYN-ACK`，但客户端永远没有 `injected`。

修复：

```python
packet = b"\0" * 12 + b"\x08\x00" + payload
```

修复后客户端日志出现：

```text
[client-sidecar] injected SYN-ACK to 172.31.0.173:58502 (60 bytes); NAT remote=111.91.22.14:58502
```

### 3.4 TUN

- `open_tun()` 使用 `/dev/net/tun`，`IFF_TUN | IFF_NO_PI`，默认名 `lxtrescue0`。
- 需要 root 或 `CAP_NET_ADMIN`。
- 注入的包目的地址是本地 TCP 连接地址，因此内核会把它当作本地入站 SYN-ACK 处理。

## 4. 复现命令

```bash
# 服务端
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py server-sidecar \
  --interface eth0 --tcp-port 39090 --udp-port 39091

# 客户端（先启动阻断，再启动 sidecar）
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py client-sidecar \
  --interface eth0 --server <SERVER_IP> --tcp-port 39090 --udp-port 39091

# 另一个客户端终端
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4
```

也可用 `phase4_client.sh full` 一次完成对照 + 救援（从 stdin 读 key，trap 清理）。

## 5. V4 结果

- 主救援 3 条 + 长连接 1 条，共 4 次救援成功。
- 3 个主救援连接：

| 客户端端口 | 客户端 ISN | 服务端 SYN-ACK seq | SYN-ACK ack |
|---|---|---|---|
| 52076 | 3822945293 | 420585238 | 3822945294 |
| 52092 | 4169341575 | 345123457 | 4169341576 |
| 52100 | 2567257622 | 3665299090 | 2567257623 |

- nft counter 7 packets / 480 bytes（3 个对照 + 3 个救援 + 1 个重传）；真实 SYN-ACK 全被丢，TUN 注入的同 seq/ack 包被内核接受。
- 长连接救援：端口 43528，ISN 4219380672，服务端 seq 481215890，ack 4219380673。
- **结果：PASS。**

## 6. 关联报告

- V4：4.2 节 Bug、4.4 节主救援、4.5 节长连接。
- V5：救援 18 MiB，3 次救援，MSS 1332 vs 1460。
- V6：救援后经 HTTP CONNECT 代理访问 Google。
- V7：A（FORWARD 丢 SYN-ACK）PASS；B（PREROUTING raw + NAT）FAIL。
- V8：无 NAT 三 netns，B PASS；C（严格 `ct state invalid drop`）FAIL。

## 7. 边界

- 服务端必须能 AF_PACKET 抓到自己产生的 SYN-ACK；容器/云函数/受限网络可能不允许。
- 客户端需要 root + TUN；peer map 只按 IP；IPv4-only。
- 注入 SYN-ACK 的 MSS 可能绕过路径 clamp（V5 发现 1332 vs 1460），大流量/特殊 MTU 路径需额外验证。
- 只解决“收不到 SYN-ACK”的握手问题，不解决中间设备对后续 ACK/DATA 的 conntrack invalid 过滤（见 V8-C）。
