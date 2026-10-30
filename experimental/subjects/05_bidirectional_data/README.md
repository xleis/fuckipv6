# 科目 05 — TCP 数据双向传输

## 1. 科目内容

验证 UDP 辅助握手完成后，业务数据**不再经过 UDP**，而是直接在已建立的 TCP 连接上双向传输：

1. 客户端发送请求，服务端回复；
2. 服务端发送数据，客户端接收；
3. 大块数据（18 MiB）双向传输；
4. 双向 SHA-256 一致。

这是“UDP 只负责救援握手、数据仍走原生 TCP”的核心证明。

## 2. 代码位置与入口

- 小消息：`lxt_public_lab.py` 的 `run_echo_server()` / `run_probe()`。
- 大块负载：[`../../code/lxt_v5_bulk_test/lxt_bulk_probe.py`](../../code/lxt_v5_bulk_test/lxt_bulk_probe.py)
  - `server`：监听 TCP，按 `LXTBULK1` 协议接收/发送分块；
  - `client`：`--up-mib` / `--down-mib` / `--connections` / `--rounds` / `--mode` / `--rate-mbps`；
  - 每轮双向 SHA-256 校验。
- 报告：
  - [`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)
  - [`../../reports/LXT_V5_BULK_TEST_REPORT.md`](../../reports/LXT_V5_BULK_TEST_REPORT.md)

## 3. 实现细节

### 3.1 小消息

- `probe` 发送 `ping-<i>\n`，服务端回复 `ACK:ping-<i>\n`。
- 服务端 `--close-after 2` 时每 2 条关闭；客户端重新连接后继续。
- 数据只走 TCP socket；客户端 sidecar 只处理握手 SYN/SYNACK，不转发业务数据。

### 3.2 18 MiB bulk

- 协议 MAGIC `LXTBULK1`；每条连接先握手，再按 `--rounds` 复用。
- 每轮上传 `up_mib`、下载 `down_mib`；默认 1 MiB/dir/round，最大 64 MiB/dir/round。
- `--connections 3 --rounds 3`：3 条连接 × 3 轮 × 2 MiB = 18 MiB。
- `--rate-mbps 10` 是发送速率上限，避免无节制压力。
- 每轮分别计算上传/下载 SHA-256；服务端和客户端都校验。
- V5 使用 V4 修复过 EtherType 的 sidecar；UDP 只承载救援握手。

## 4. 复现命令

```bash
# 小消息
python3 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 2
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4

# 18 MiB（正常或救援路径均可）
python3 lxt_bulk_probe.py client --server <SERVER_IP> --port 39090 \
  --up-mib 1 --down-mib 1 --connections 3 --rounds 3 \
  --mode cycle --rate-mbps 10
```

服务端：

```bash
python3 lxt_bulk_probe.py server --bind 0.0.0.0 --port 39090 --rate-mbps 10
```

## 5. 结果与证据

### V4 小消息

- 18/18 条 echo 消息成功；数据走原生 TCP。
- 主救援 3 条连接 + 长连接 1 条，共 4 次救援；每次救援后数据正常。
- **PASS。**

### V5 18 MiB

| 场景 | 结果 |
|---|---|
| 正常路径（无阻断） | PASS 9/9，3 连接，18,874,368 B，19.63 s，平均建连 159.4 ms，7.56–8.82 Mbps |
| 救援路径（drop + sidecar） | PASS 9/9，3 连接，18,874,368 B，20.55 s，平均建连 182.6 ms，6.14–8.92 Mbps |

- 双向 SHA-256 全部 OK。
- 救援路径 nft counter 8 packets / 480 B；真实 SYN-ACK 全被丢，3 次注入成功。
- 证据：`evidence/v5/logs/lxt-v5-bulk-normal.log`、`lxt-v5-bulk-rescue-nomss.log`、`lxt-v5-client-sidecar.log`、`lxt-v5-server-sidecar.log`、`lxt-v5-rescue-syn-eth0.txt` / `-tun.txt`。

### V8 18 MiB（隔离无 NAT）

- 正常：9/9，15.70 s，10.2 Mbps。
- A（FORWARD 丢 SYN-ACK）：9/9，18,874,368 B，SHA OK。
- B（PREROUTING raw 丢 SYN-ACK）：9/9，18,874,368 B，SHA OK。
- C（严格 `ct state invalid drop`）：HTTP 救援已失败，18 MiB 未进行。
- 证据：`evidence/v8/logs/bulk-normal.log`、`bulk-a.log`、`bulk-b.log`。

## 6. 结论

- UDP 通道只承载 HELLO/HELLO_ACK/SYNACK；业务数据 100% 走原生 TCP。
- 18 MiB 双向传输在正常和救援路径上都成功，SHA-256 一致。
- 数据成功不等于所有中间盒都兼容：V8-C 证明 strict conntrack invalid 过滤会阻断后续 ACK/DATA。

## 7. 边界

- V5 的 18 MiB 是限速 10 Mbps 的实验负载，不是压力测试。
- MSS 1332 vs 1460 的潜在 PMTU 问题仍需在真实窄 MTU 路径上验证。
- 代理/业务服务本身的认证、并发、超时不在本科目范围。
