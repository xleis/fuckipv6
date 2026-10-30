# 科目 06 — TCP 长连接复用

## 1. 科目内容

验证 UDP 辅助建连成功后，同一条原生 TCP 连接可以连续承载多条消息，不需要每条消息都重新握手/重新救援。

这是“救援只发生在建连阶段，数据面是普通长连接”的关键证明。

## 2. 代码位置与入口

- [`../../code/lxt_public_synack_v4/lxt_public_lab.py`](../../code/lxt_public_synack_v4/lxt_public_lab.py)
  - `run_echo_server()`：`--close-after 0` 表示不主动关闭，保持连接。
  - `run_probe()`：在同一个 socket 上循环发送 `ping-i` / 接收 `ACK:ping-i`；只有 EOF/错误才重连。
- 完整报告：[`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)

## 3. 实现细节

- 服务端：`echo-server --close-after 0`；每收到一行回复 `ACK:<line>`，不主动关闭。
- 客户端：`probe --count 6 --timeout 4`；`run_probe()` 先建立一条连接，然后在循环中复用；`established` 计数只在新建连接时增加。
- 客户端 sidecar 只在 SYN 时记录 pending，在注入 SYN-ACK 后不再参与数据；后续消息直接走内核 TCP。
- 阻断规则仍然保持：真实 SYN-ACK 被 nft 丢弃；注入的 SYN-ACK 只用于建立这一条连接。
- 判据：`established=1` 且 `completed=6`，而不是 6 条连接各 1 条消息。

## 4. 复现命令

```bash
# 服务端
python3 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 0

# 客户端（阻断 + sidecar 已就绪）
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4
```

## 5. V4 结果与证据

| 指标 | 值 |
|---|---|
| 完成消息 | 6/6 |
| 建连次数 | **1** |
| 客户端端口 | 43528 |
| 客户端 ISN | 4219380672 |
| 服务端 SYN-ACK seq | 481215890 |
| SYN-ACK ack | 4219380673 |
| 真实 SYN-ACK | 17:27:18.251767，被 nft 丢弃 |
| TUN 注入 | 17:27:18.275012 |
| nft counter | 1 packet / 60 B（该连接的真实 SYN-ACK） |

- 6 条消息全部在同一条 TCP 连接上完成。
- 服务端 echo 日志只出现一次 `accepted ('111.91.22.14', 43528)`。
- **结果：PASS。**

## 6. 关联报告

- V4：4.5 节“长连接复用”。
- V5：`--connections 3 --rounds 3` 中 `--rounds` 就是在同一连接上复用 3 轮。
- V7/V8：A/B 的 18 MiB 也使用 `--rounds` 复用连接。

## 7. 边界

- 长连接复用只证明“连接不被重新握手”；不证明应用层心跳/空闲超时/中间盒连接老化。
- 真实网络中的 NAT 空闲超时、云负载均衡连接 idle timeout 仍需单独测试。
