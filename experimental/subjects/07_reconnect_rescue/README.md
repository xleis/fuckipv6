# 科目 07 — 断线后再次救援

## 1. 科目内容

验证服务端主动关闭连接后，客户端重新 `connect()` 会产生新的 SYN/ISN，客户端 sidecar 能重新匹配并完成**第二次、第三次 UDP 救援**，而不是只在第一次建连时有效。

## 2. 代码位置与入口

- [`../../code/lxt_public_synack_v4/lxt_public_lab.py`](../../code/lxt_public_synack_v4/lxt_public_lab.py)
  - `run_echo_server()`：`--close-after 2`，每 2 条消息关闭连接。
  - `run_probe()`：检测 EOF/错误后重新连接；每次连接使用新源端口和新 ISN。
  - `run_client_sidecar()`：`pending` 列表按 `(src, sport, isn)` 记录；新 SYN 会新增条目，旧条目 20 秒过期。
- 完整报告：[`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)

## 3. 实现细节

- 服务端每 2 条消息关闭；客户端 `--count 6` 产生 3 条连接。
- 每次重连：
  1. 客户端内核选择新源端口、生成新 ISN；
  2. 客户端 sidecar 捕获出站 SYN，加入 `pending`；
  3. 真实 SYN-ACK 仍被 nft 丢弃；
  4. 服务端 sidecar 按 0.25 秒去重后转发新 SYN-ACK；
  5. 客户端 sidecar 用 `ack == isn+1` 找到新条目并注入。
- 关键点：`pending` 不是全局单条，而是按连接列表；否则第二次重连会被第一次的旧 ISN 干扰。
- 阻断规则全程保持，不因重连而取消。

## 4. 复现命令

```bash
# 服务端
python3 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 2

# 客户端（drop + sidecar 已就绪）
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 6 --timeout 4
```

## 5. V4 结果与证据

| 连接 | 客户端端口 | 客户端 ISN | 服务端 SYN-ACK seq | ack |
|---|---|---|---|---|
| 1 | 52076 | 3822945293 | 420585238 | 3822945294 |
| 2 | 52092 | 4169341575 | 345123457 | 4169341576 |
| 3 | 52100 | 2567257622 | 3665299090 | 2567257623 |

- 3 条连接全部完成 UDP 救援，6/6 消息成功。
- nft counter 7 packets / 480 bytes（3 个对照 + 3 个救援 + 1 个重传）。
- 客户端日志出现 3 次 `pending SYN` + 3 次 `injected`；服务端日志 3 次 `relayed`。
- 真实 SYN-ACK 全部被 nft 丢弃；连接靠注入完成。
- **结果：PASS。**

## 6. 关联报告

- V4：4.4 节主救援、4.6 节“断线后再次救援”。
- V7：A 组 3 条连接全部救援成功；B 组因 NAT 失败。
- V8：B 组 3 条连接全部救援成功，18 MiB PASS；C 组因 strict invalid drop 失败。

## 7. 边界

- V4 的 3 次重连是服务端主动 `close-after 2` 触发的；不是网络中断/进程崩溃后的恢复。
- 真实断线可能伴随 NAT 映射过期、端口复用、时间等待（TIME_WAIT）等问题，需单独测试。
