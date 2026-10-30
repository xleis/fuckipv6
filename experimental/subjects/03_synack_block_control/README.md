# 科目 03 — SYN-ACK 阻断对照组

## 1. 科目内容

在**客户端**临时阻断来自测试服务端、目标为本地测试连接的 SYN+ACK，验证：

1. 不启动客户端 sidecar 时，标准 `connect()` 必须超时；
2. 阻断规则确实命中 SYN-ACK（counter > 0）；
3. 没有 TUN、没有 sidecar、没有 UDP 救援参与。

这是救援实验的阴性对照。**不能把“本地防火墙阻断 SYN-ACK”当成“已经突破真实公网中间设备”**；它只证明“客户端收不到 SYN-ACK 时，UDP 救援是否可行”。

## 2. 代码位置与入口

- [`../../code/lxt_public_synack_v4/client_drop_nft.sh`](../../code/lxt_public_synack_v4/client_drop_nft.sh)
  - `start SERVER_IP NIC [TTL]`
  - `stop`
  - `status`
- [`../../code/lxt_public_synack_v4/client_drop_once.sh`](../../code/lxt_public_synack_v4/client_drop_once.sh)（原始 iptables 一次性脚本）
- [`../../code/lxt_public_synack_v4/phase4_client.sh`](../../code/lxt_public_synack_v4/phase4_client.sh) 的 `control` 分支
- 完整报告：[`../../reports/LXT_V4_PUBLIC_TEST_REPORT.md`](../../reports/LXT_V4_PUBLIC_TEST_REPORT.md)

## 3. 实现细节

`client_drop_nft.sh start` 只创建：

```text
table inet lxt_v4
  chain input { type filter hook input priority 0; policy accept; }
    iifname "<NIC>" ip saddr <SERVER_IP> tcp sport 39090 \
      tcp flags syn,ack / syn,ack counter drop
```

要点：

- 只在 `input` hook 丢弃入站 SYN+ACK，不影响出站 SYN。
- 只匹配指定服务端 IP、指定 TCP 源端口 39090、SYN+ACK 标志。
- 规则带 `counter`，用 `nft list table inet lxt_v4` 读取命中包数。
- `TTL` 参数会启动一个后台定时器，到点执行 `nft delete table inet lxt_v4`；`stop` 会主动删除表并杀掉定时器。
- 不使用 `iptables`，因为测试客户端 Debian 12 只有 nft；`client_drop_once.sh` 是原始 iptables 版本，保留作参考。

V4 实测的 nft 语法：`tcp flags syn,ack / syn,ack` 可用；`tcp flags syn,ack == syn,ack` 会报语法错误。

## 4. 复现命令

```bash
# 客户端：创建 600 秒 TTL 的阻断
cd code/lxt_public_synack_v4
./client_drop_nft.sh start <SERVER_IP> eth0 600
./client_drop_nft.sh status

# 确认没有 sidecar / TUN
pgrep -af '[l]xt_public_lab.py client-sidecar' || true
ip link show lxtrescue0 2>&1 || true

# 标准 TCP 对照（应超时）
python3 lxt_public_lab.py probe --server <SERVER_IP> --count 1 --timeout 4

# 查看命中
nft list table inet lxt_v4

# 清理
./client_drop_nft.sh stop
```

## 5. V4 结果与证据

| 指标 | 值 |
|---|---|
| 对照结果 | `[probe] connect error: timed out` |
| probe 退出码 | 2 |
| nft counter | 3 packets / 180 bytes |
| tcpdump | 客户端 eth0 抓到 3 个 SYN-ACK，源 185.217.110.15:39090，目的 172.31.0.173:52064 |
| 关键序列号 | `seq 1247631115, ack 3145776028` |
| 客户端 TCP | 未进入 ESTABLISHED |

**结果：PASS。**

## 6. 关联报告

- V4：第 3 节“SYN-ACK 阻断对照组”、4.3 节。
- V7：A/B 两组对照（netns 路由器版），A conntrack 看到 SYN-ACK，B conntrack 看不到。
- V8：A/B/C 对照（无 NAT 三 netns 版）。

## 7. 边界

- 本地 nft 阻断只模拟“客户端收不到 SYN-ACK”，不等同于真实中间设备阻断。
- 真实设备可能在更早的路径丢弃、做 NAT、做 strict conntrack、做 MSS clamp；这些变量必须分别测试。
- 阻断规则必须只在客户端实验机，且自动清理。
