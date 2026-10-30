# LXT V5 公网 18 MiB 分块负载与 MSS 诊断报告

- 测试时间：2026-10-30 17:46–17:51（Asia/Shanghai；服务端 UTC 09:46–09:51）
- 测试范围：V4 UDP 辅助 TCP 握手机制上的 V5 分块负载工具，正常路径 18 MiB、SYN-ACK 阻断 + UDP 救援 18 MiB、MSS 抓包诊断
- 结论：**18 MiB 正常路径 PASS；18 MiB UDP 救援 PASS；MSS 不匹配已定位为公网路径上的保守 MSS clamp，不是本次 PMTU 黑洞；机制可作为 LXT 握手救援/bootstrap 模块，但不是通用传输层。**

## 0. 验收摘要

| 测试 | 结果 | 关键证据 |
|---|---|---|
| 18 MiB 正常 TCP | **PASS** | 9/9 rounds，3 连接，18,874,368 B，19.63 s，SHA-256 全 OK |
| 18 MiB UDP 救援 | **PASS** | 9/9 rounds，3 连接，18,874,368 B，20.55 s，SHA-256 全 OK |
| UDP HMAC 认证 | **PASS** | `HELLO_ACK verified`；4 次救援全部认证成功 |
| SYN-ACK 阻断对照 | **PASS** | `connect()` 5.01 s 超时；nft 丢弃 5 个 SYN-ACK（300 B） |
| UDP 辅助 TCP 建连 | **PASS** | 4/4 救援成功（3 主测 + 1 MSS 诊断） |
| TCP 数据双向传输 | **PASS** | 主测 36 MiB + 诊断 2 MiB，19/19 rounds，SHA-256 100% |
| 长连接复用 | **PASS** | 每连接 3 rounds（6 MiB/连接），共 3 连接 |
| 断线后再次救援 | **PASS** | 3 个独立连接各自完成一次救援 |
| MSS 抓包诊断 | **PASS** | 客户端 SYN 1460 → 服务端入口 1332；服务端 SYN-ACK 1460 → 客户端入口 1332；路径 MTU 1500；无重传 |
| 临时规则与进程清理 | **PASS** | nft 表/TUN/sidecar 全清；39090/39091 关闭；既有服务未受影响 |

## 1. 测试目标

V4 已验证：客户端正常 TCP 路径收不到 SYN-ACK 时，服务端 sidecar 捕获内核真实 SYN-ACK，经 HMAC 认证 UDP 转送，客户端注入 TUN，Linux 原生 TCP 进入 ESTABLISHED，后续数据走原生 TCP。

V5 要回答两个问题：

1. 该机制在 **18 MiB 分块、双向 SHA-256 校验** 的负载下是否仍然成立；
2. V4 留下的 MSS 1332 vs 1460 差异，在真实公网路径上是否会造成 PMTU 黑洞或重传。

## 2. 环境

### 2.1 服务端

| 项 | 值 |
|---|---|
| 主机 | `relay.xlei.cc` / `vm521838.akkocloud.com` |
| OS | CentOS Stream 8 |
| Kernel | `5.15.202-1.el8.elrepo.x86_64` |
| Python | 3.9.19 |
| 网卡 | `eth0`，`185.217.110.15/24`，MTU 1500 |
| 默认路由 | `185.217.110.1` |
| 其他接口 | `fuckipv6` 10.8.0.1/24（既有，未触碰） |
| 防火墙 | firewalld inactive；既有 iptables `FKV6F-99E15423` FORWARD 规则，未修改 |
| SELinux | Disabled |
| rp_filter | all=1 / default=0 |
| 测试端口 | TCP 39090（V5 bulk server），UDP 39091（V4 sidecar） |
| 代码目录 | `/root/lxt_public_synack_v4/` |

### 2.2 客户端

| 项 | 值 |
|---|---|
| 主机 | `111.91.22.14`（VPS） |
| OS | Debian 12 bookworm |
| Kernel | `6.1.0-35-amd64` x86_64 |
| Python | 3.11.2 |
| 网卡 | `eth0`，`172.31.0.173/20`，MTU 1500 |
| 公网出口 | `111.91.22.14`（1:1 NAT / EIP） |
| 默认路由 | `172.31.0.1` |
| 防火墙 | 无 iptables；nft 为唯一规则入口（本测试只建独立 `inet lxt_v4` 表） |
| rp_filter | all=0 / default=0 |
| 测试网卡 | `eth0`；TUN `lxtrescue0` |
| 既有业务 | nginx 443/8880、sing-box 8881/53543/127.0.0.1:56672（未触碰） |
| 代码目录 | `/root/lxt_public_synack_v4/` |

### 2.3 路径

- 公网 RTT：约 **230 ms**（客户端 ↔ 服务端 ping）。
- 路径 MTU：**1500**，双向 `ping -M do -s 1472` 均成功。
- 真实公网路径：服务端 `185.217.110.15` ↔ 客户端 NAT `111.91.22.14`；客户端为 VPS，未经家庭代理。V4 早期发现的 PassWall2 代理路径已弃用。
- 端口：TCP 39090 数据；UDP 39091 救援；客户端 TUN `lxtrescue0`。

## 3. 测试工具与命令

V5 ZIP：`lxt_v5_bulk_test.zip`，仅含 `lxt_bulk_probe.py` 和 `README.md`。工具不实现 UDP 救援，复用 V4 修复过 TUN 以太网头 EtherType 的 sidecar。

18 MiB 标准命令：

```bash
python3 lxt_bulk_probe.py client \
  --server 185.217.110.15 --port 39090 \
  --up-mib 1 --down-mib 1 \
  --connections 3 --rounds 3 \
  --mode cycle --rate-mbps 10
```

- 3 个连接 × 每连接 3 rounds × 每 round 上传 1 MiB + 下载 1 MiB = **18 MiB**。
- `cycle` 每 round 切换 tiny/mixed/large 消息模式。
- 每方向每 round 做 SHA-256，服务端 `validated` 后客户端才 PASS。
- MSS 诊断额外跑 1 连接 × 1 round × 2 MiB。

## 4. 结果

### 4.1 正常 18 MiB（无阻断、无客户端 sidecar）

日志：`logs/lxt-v5-bulk-normal.log`

```text
[SUMMARY] PASS rounds=9/9 connections=3 bytes=18874368 duration=19.63s avg_connect=159.4ms
```

- 3 个 TCP 连接，服务端端口：`37862 / 60944 / 60950`。
- 吞吐：7.56–8.82 Mbps（10 Mbps rate cap 下）。
- 建连：avg 159.4 ms。
- 客户端 SYN MSS 1460；服务端真实 SYN-ACK MSS **1332**。
- 客户端上传数据段：IP length **1372** / TCP payload **1320**（正常路径按 1332 MSS 发送）。
- 服务端日志：每个连接 3 个 round 全部 `validated`，共 9/9。

正常路径证明 V5 协议、18 MiB 分块、双向 SHA-256 和公网 TCP 数据面正常。

### 4.2 SYN-ACK 阻断对照（无 sidecar）

客户端 nft 规则：

```nft
table inet lxt_v4 {
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "eth0" ip saddr 185.217.110.15 tcp sport 39090 \
      tcp flags syn,ack / syn,ack counter drop
  }
}
```

无客户端 sidecar，直接 `connect()`：

```text
CONTROL_FAIL TimeoutError timed out elapsed=5.01
nft counter: packets 5 bytes 300 drop
```

抓包：`logs/lxt-v5-control-syn.txt`

- 客户端 SYN：`172.31.0.173:44326`，seq 2966802027，MSS 1460。
- 服务端真实 SYN-ACK：seq 279100540，ack 2966802028，MSS 1332。
- 5 个 SYN-ACK 全部被 nft 丢弃；标准 TCP 5.01 s 超时。
- 对照 PASS。

### 4.3 UDP 救援 18 MiB（阻断保持 + 双端 sidecar）

日志：`logs/lxt-v5-bulk-rescue-nomss.log`

```text
[SUMMARY] PASS rounds=9/9 connections=3 bytes=18874368 duration=20.55s avg_connect=182.6ms
```

- 3 个 TCP 连接，客户端源端口/ISN：
  - `51640 / 173085878`
  - `45140 / 4108243862`
  - `59546 / 1959494703`
- 3 次 `injected SYN-ACK`；服务端 sidecar 3 条 `relayed SYN-ACK`。
- 真实 SYN-ACK 全部被 nft 丢弃；nft counter 从对照的 5 packets/300 B 增至 **8 packets/480 B**（3 次救援各 1 个真实 SYN-ACK）。
- 吞吐：6.14–8.92 Mbps；建连 avg 182.6 ms。
- 服务端日志：`51640 / 45140 / 59546` 各 3 个 round 全部 `validated`，共 9/9。
- 客户端 sidecar 日志：3 次 `pending SYN` + 3 次 `injected`，TUN `lxtrescue0`。
- 数据完整性：9/9 rounds，双向 SHA-256 全 OK，总计 18,874,368 B。

### 4.4 关键 TCP 序列号 / ACK 证据

| 场景 | 连接 | 客户端 ISN | 服务端 SYN-ACK seq | SYN-ACK ack | 真实/注入 MSS | 结果 |
|---|---|---:|---:|---:|---|---|
| 对照 | 44326 | 2966802027 | 279100540 | 2966802028 | 1332 / — | 被 nft 丢弃，超时 |
| 正常 | 37862 | 2086135389 | 1454280451 | 2086135390 | 1332 / — | 正常 ESTABLISHED |
| 救援 | 51640 | 173085878 | 2569447128 | 173085879 | 1332 / 1460 | 注入成功，18 MiB PASS |
| 救援 | 45140 | 4108243862 | 1126338288 | 4108243863 | 1332 / 1460 | 注入成功，18 MiB PASS |
| 救援 | 59546 | 1959494703 | 3555578796 | 1959494704 | 1332 / 1460 | 注入成功，18 MiB PASS |
| MSS 诊断 | 36898 | 1048328521 | 3613142689 | 1048328522 | 1332 / 1460 | 注入成功，2 MiB PASS |

所有救援连接的 `SYN-ACK ack == 客户端 ISN + 1`；注入包的 seq/ack 与真实 SYN-ACK 完全一致：

- `logs/lxt-v5-rescue-syn-eth0.txt`：真实 SYN-ACK，MSS 1332。
- `logs/lxt-v5-rescue-syn-tun.txt`：TUN 注入 SYN-ACK，MSS 1460，seq/ack 相同。

### 4.5 数据完整性与重传

- 主测正常 9/9 + 救援 9/9，MSS 诊断 1/1：共 **19/19 rounds**，应用层数据 **38 MiB**，SHA-256 全部 OK。
- 救援 18 MiB 期间客户端 `nstat` before/after：
  - `TcpRetransSegs`：10084 → 10084（**无增量**）
  - `TcpExtTCPLostRetransmit`：2554 → 2554（**无增量**）
  - `TcpExtTCPTimeouts`：7920 → 7920（**无增量**）
  - `TcpExtTCPOFOQueue`：8776 → 8776（**无增量**）
  - `TcpExtTCPBacklogDrop`：0 → 0
  - `TcpOutSegs`：+9,529；`TcpInSegs`：+5,842；`TcpActiveOpens`：+3
- 客户端 `ss -ti` 救援连接：`mss:1448 pmtu:1500 rcvmss:1320 advmss:1448`；每连接 `bytes_sent` 约 3.1 MB、`bytes_received` 约 3.0 MB（3 rounds × 1 MiB），无 retrans 字段。
- 结论：18 MiB 救援传输无重传、无超时、无乱序丢包；数据面健康。

### 4.6 清理结果

- 客户端：sidecar 进程已停；nft `inet lxt_v4` 表已删；TTL 定时器无残留；TUN `lxtrescue0` 不存在；UDP 39091 无监听；nginx/sing-box 正常。
- 服务端：bulk server 与 sidecar 已停；TCP 39090 / UDP 39091 均关闭；无 `lxt_public_lab.py` / `lxt_bulk_probe.py` 进程；iptables 与 `fuckipv6` 配置未变；未重启、未改 SSH。
- PSK：V5 使用新生成的 32 字节 hex key，经 stdin 传入，未写入 argv/日志；本地 `/tmp/lxt_v5_key` 已删除。V4 会话中曾暴露的旧 PSK 应在实验结束后轮换。

## 5. MSS 抓包诊断

### 5.1 抓包证据

**客户端出口 SYN（`logs/lxt-v5-rescue-syn-eth0.txt`）**

```text
172.31.0.173.51640 > 185.217.110.15.39090: Flags [S], seq 173085878,
  options [mss 1460,sackOK,TS ...]
```

**服务端入口 SYN（`logs/lxt-v5-mss-server.txt`，诊断连接 36898）**

```text
111.91.22.14.36898 > 185.217.110.15.39090: Flags [S], seq 1048328521,
  options [mss 1332,sackOK,TS ...]
```

→ 客户端 eth0 上是 1460，服务端 eth0 收到的是 **1332**：client→server 方向存在 **in-path MSS clamp**。

**服务端出口 SYN-ACK（同一抓包 + sidecar 注入包）**

```text
185.217.110.15.39090 > 111.91.22.14.36898: Flags [S.], seq 3613142689,
  ack 1048328522, options [mss 1460,sackOK,TS ...]
```

**客户端入口真实 SYN-ACK（`logs/lxt-v5-rescue-syn-eth0.txt`）**

```text
185.217.110.15.39090 > 172.31.0.173.51640: Flags [S.], seq 2569447128,
  ack 173085879, options [mss 1332,sackOK,TS ...]
```

**客户端 TUN 注入 SYN-ACK（`logs/lxt-v5-rescue-syn-tun.txt`）**

```text
185.217.110.15.39090 > 172.31.0.173.51640: Flags [S.], seq 2569447128,
  ack 173085879, options [mss 1460,sackOK,TS ...]
```

→ 服务端出口是 1460，客户端入口真实 SYN-ACK 是 **1332**：server→client 方向也存在 **in-path MSS clamp**。sidecar 抓到的是服务端 **clamp 前** 的出站 SYN-ACK，所以注入的是 1460。

**路径 MTU**

- 客户端 → 服务端：`ping -M do -s 1472` 成功。
- 服务端 → 客户端：`ping -M do -s 1472` 成功。
- 两端 `eth0` MTU 均为 1500。

→ clamp 值 1332（对应 MTU 1372）**不是路径真实 MTU**，而是中间设备的保守/策略性 MSS clamp。

### 5.2 数据段实际大小

- 正常路径：客户端按真实 SYN-ACK 1332 发送，IP length 1372 / payload 1320。
- 救援路径：客户端按注入 SYN-ACK 1460 发送，IP length **1500 / payload 1448**；服务端抓包同样看到 1500/1448 的数据段并被服务端接收。
- 服务端下载方向：客户端 `ss` 的 `rcvmss:1320` 表明服务端发送段为 1320 B——因为服务端 TCP 栈看到的是被 clamp 的客户端 SYN（1332），其发送 MSS 为 1320。
- 因此救援路径是 **上行 1448 / 下行 1320** 的非对称 MSS：注入只影响客户端发送方向，服务端发送方向仍受其收到的 SYN MSS 约束。

### 5.3 解释

1. 公网路径上存在双向 MSS clamp：SYN 和 SYN-ACK 的 MSS 都从 1460 被改成 1332。
2. 服务端 sidecar 在 AF_PACKET 层抓到自己产生的 **clamp 前** SYN-ACK（1460），UDP 转送后客户端注入 1460；真实 SYN-ACK 的 1332 被 nft 丢弃。
3. 客户端按 1460 计算有效发送 MSS 1448，发出 1500 B IP 包；服务端虽然广告 1332，但 MSS 只是建议值，实际接收 1500 B 包没有问题。
4. 服务端发送方向仍按收到的 SYN MSS 1332 计算，发送 1320 B；所以 `ss` 同时显示 `mss:1448`（客户端发送）与 `rcvmss:1320`（服务端发送）。
5. 18 MiB 传输无重传/超时，SHA-256 全 OK：**本次没有发生 PMTU 黑洞**。V4 担心的“MSS 1332 vs 1460 在大流量下会炸”在这条路径上不成立；1332 是保守 clamp，不是真实 MTU。
6. 但风险仍然存在：如果某条路径的 1332 是真实 MTU/PMTU 限制，注入 1460 会让客户端发出超 MTU 包并被丢弃，形成黑洞。当前设计“盲目使用服务端 clamp 前 MSS”不具备普适安全性。

### 5.4 修复建议

- **不要简单地把注入 MSS 固定改成 1332**：在本路径上会无谓降低上行吞吐；应先区分“保守 clamp”与“真实 MTU”。
- 可选方案：
  1. 客户端 sidecar 增加可配置 `--mss`（例如 1332），为保守路径提供兜底；
  2. 在 HELLO/SYNACK 救援 payload 中携带服务端出口 MTU/MSS 或路径探测结果；
  3. 实现 PMTU 黑洞检测：救援连接建立后发送小探测段，若大段持续无 ACK，自动降低 MSS 并重试；
  4. 服务端 sidecar 记录并上报自己看到的客户端 SYN MSS 与服务端出口 SYN-ACK MSS，便于客户端选择 min 值；
  5. 生产环境优先使用内核 `TCP_MAXSEG`/路由 `advmss` 或 TUN 侧 MSS clamp，而不是在用户态硬编码。
- 本路径的推荐策略：默认 1460（实际路径支持 1500），检测到黑洞时回退 1332。

## 6. 真实公网测量 vs 人工阻断

- **真实部分**：两端是真实公网主机（服务端 185.217.110.15，客户端 1:1 NAT 111.91.22.14），RTT 约 230 ms；数据面是原生 TCP；MSS clamp 是公网路径上真实观察到的中间设备行为（客户端出口 1460 → 服务端入口 1332；服务端出口 1460 → 客户端入口 1332）。
- **人工部分**：SYN-ACK 阻断是客户端本机 nft 规则，在 SYN-ACK 已经走完公网、到达客户端 `eth0` 后才丢弃。它验证的是“客户端 TCP 栈收不到 SYN-ACK”这一失败模式，**不等于**“公网中间设备在路径上阻断 SYN-ACK”。
- 真实 SYN-ACK 确实到达了客户端（tcpdump 可见），随后被 nft 丢弃；客户端 TCP 栈只看到 TUN 注入的 SYN-ACK。因此救援逻辑本身是有效的；但“穿透真实中间设备封锁”仍需下一阶段用云安全组/ACL 或上游防火墙在路径上丢弃 SYN-ACK、同时放行 UDP 39091 来验证。
- 本次没有为了让实验通过而取消阻断：nft counter 9 packets/540 B 证明 5 个对照 + 3 个主测 + 1 个诊断的真实 SYN-ACK 都被丢弃。

## 7. 遗留问题与限制

1. **未测真实中间设备 SYN-ACK 阻断**：当前是客户端本机 nft；需云 ACL/安全组或上游设备在路径上丢弃 SYN-ACK。
2. **MSS/PMTU 普适性**：本路径 1332 是保守 clamp；真实 MTU 1372 的路径可能因注入 1460 而黑洞。需要可配置 MSS 或 PMTU 黑洞检测。
3. **peer map 只按目标 IP**：服务端 sidecar 用客户端目标 IP 匹配 peer；同一 NAT 后多客户端会冲突，需要 4 元组/5 元组或 HELLO 里带连接标识。
4. **IPv4 only**：IPv6 路径、UDP 分片、MTU 1280 未测。
5. **权限与平台**：服务端 AF_PACKET 需要 root/CAP_NET_RAW；客户端 TUN 需要 root/CAP_NET_ADMIN；当前仅 Linux。
6. **UDP 可达性**：若 UDP 39091 被完全阻断，机制失效；需要 UDP 备用端口/伪装/QUIC 类传输的评估。
7. **PSK 管理**：V5 key 未泄漏并已删除；V4 旧 key 曾在会话记录中出现，必须轮换。生产环境应使用密钥管理系统，不要在 argv/日志中出现。
8. **HELLO 频率**：客户端 sidecar 日志出现大量 `HELLO_ACK verified`，说明重认证较频繁；生产环境应调整间隔并做速率限制。
9. **服务端 checksum offload**：服务端 tcpdump 显示 SYN-ACK checksum `incorrect -> ...`，是网卡 offload 的正常现象；sidecar `fix_checksums` 后注入包校验和正确，客户端接受。不是 bug，但抓包分析时要注意。
10. **吞吐不是基准**：10 Mbps rate cap、Python 用户态、230 ms RTT；18 MiB 只证明正确性，不证明高吞吐。

## 8. 下一阶段建议

1. **真实阻断验证**：在云厂商安全组/ACL 上丢弃客户端入站 TCP 39090 的 SYN-ACK，放行 UDP 39091；重跑 18 MiB 救援，确认不是“本机 nft 特例”。
2. **MSS/PMTU 模块**：加入 `--mss` 配置、PMTU 黑洞探测与回退；在真实 MTU 1372 的路径上对比 1332/1460；把服务端出口 MSS/MTU 带入救援 payload。
3. **多客户端 NAT**：peer map 改为包含源端口、目标端口、ISN 的键；测试同一 NAT 后两个客户端并发救援。
4. **IPv6/双栈**：验证 IPv6 下 AF_PACKET/TUN 注入、UDP 分片和 1280 MTU。
5. **生产化 sidecar**：用 systemd + `CAP_NET_RAW`/`CAP_NET_ADMIN`，避免 root；加入指标、日志脱敏、TUN 生命周期、失败回退。
6. **性能**：提高 rate cap、测试 64 MiB/多并发，测量救援握手额外延迟、服务端 SYN-ACK 捕获开销、BPF 过滤优化。
7. **安全**：轮换 PSK；nonce/时间窗/重放表持久化；UDP 限速；HMAC 密钥不出现在进程 argv；日志脱敏。
8. **LXT 集成**：把“握手救援”作为独立 bootstrap 模块，不替代数据面；正常 TCP 可用时直接走 TCP，仅在 SYN-ACK 缺失时触发 UDP 救援；救援成功后把连接交给原生 TCP 栈。

## 9. 结论：能否扩展为 LXT 传输模块

**可以作为 LXT 的握手救援/bootstrap 模块，但不能作为通用传输层。**

支持理由：

- 18 MiB 双向、3 连接 × 3 rounds、SHA-256 100% 通过；
- 救援后数据全部走原生 TCP，未封装进 UDP，未使用 FakeTCP；
- 无重传/超时，吞吐 6–9 Mbps（10 Mbps cap，230 ms RTT）；
- 真实公网路径 + 真实内核 SYN-ACK + HMAC UDP + TUN 注入链路完整；
- 失败模式清晰：仅 SYN-ACK 方向受阻、UDP 双向可达、服务端能抓自己的出站 SYN-ACK、客户端有 root+TUN。

不能直接作为通用传输层的理由：

- 依赖服务端 AF_PACKET 抓包与客户端 TUN/root；
- 仅救援握手，不解决数据面被阻断；
- peer map/多客户端 NAT、IPv6、MSS/PMTU、真实中间设备阻断都未完成；
- 安全与运维（PSK、nonce、限速、日志）仍需生产化。

**建议定位**：LXT 的“TCP 握手救援”或“bootstrap fallback”子模块；数据面继续使用原生 TCP，UDP 只承载被阻断的 SYN-ACK（以及必要的控制消息）。

## 10. 附件

本地证据目录：`lxt_v5_bulk_test/logs/`

| 文件 | 内容 |
|---|---|
| `lxt-v5-bulk-normal.log` | 正常 18 MiB 客户端结果 |
| `lxt-v5-bulk-rescue-nomss.log` | 救援 18 MiB 客户端结果 |
| `lxt-v5-bulk-server.log` | 服务端 6 连接 + 19 rounds validated |
| `lxt-v5-client-sidecar.log` | 客户端 HELLO/pending/injected |
| `lxt-v5-server-sidecar.log` | 服务端 HELLO/relayed SYN-ACK |
| `lxt-v5-normal-syn.txt` | 正常路径 SYN/SYN-ACK（MSS 1460/1332） |
| `lxt-v5-control-syn.txt` | 对照路径 SYN/SYN-ACK 被 nft 丢弃 |
| `lxt-v5-rescue-syn-eth0.txt` | 救援真实 SYN-ACK（MSS 1332）+ 1500 B 数据段 |
| `lxt-v5-rescue-syn-tun.txt` | TUN 注入 SYN-ACK（MSS 1460） |
| `lxt-v5-mss-server.txt` | 服务端入口 SYN MSS 1332、出口 SYN-ACK MSS 1460、客户端 1500 B 数据段 |
| `lxt-v5-mss-client.log` | MSS 诊断 2 MiB 结果 |
| `lxt-v5-rescue-ss.log` | 客户端 `ss -ti` 救援连接 MSS/PMTU |
| `lxt-v5-nstat-before.txt` / `after.txt` | 重传/超时计数器 before/after |

所有日志不含 PSK；V5 PSK 文件已删除。
