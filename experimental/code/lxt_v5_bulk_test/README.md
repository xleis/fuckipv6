# LXT V5 TCP 数据面碎片化负载测试（附加工具）

**适用**：测试远程 Linux（CentOS Stream 8 / Python 3.9）与客户端 Linux（Debian 12 / Python 3.11）。使用 **V4 修复过 EtherType 的 Sidecar**；这个工具本身不实现 UDP 救援，也不修改防火墙。

## 实验原理

测量三件事：
1. UDP 救援之后，原生 TCP 是否能在连续、多次 `send()` 的分块写入下完成业务数据收发；
2. 两个方向的 SHA-256 是否逐字节一致；
3. 长连接复用及新建 TCP 连接后的重复救援是否可靠。

`tiny/mixed/large` 是 **应用层 `send()` 分块**，**不等于 IP 分片或 TCP 段**。真实 MSS / PMTU 需要抓包和单独验证。

## 操作顺序

1. 两端部署 `lxt_bulk_probe.py`，启动 V4 的 Server Sidecar 和 Client Sidecar，使用已轮换的 PSK；客户端仅在指定服务端 IP / TCP 源端口 39090 上维持有自动过期的 SYN-ACK nft 阻断。**不要触碰远程 SSH、默认路由、firewalld**。确认阻断对照组 connect() 超时；确认 UDP 救援日志实际出现。
2. **测试远程 Linux** 暂停原先 `lxt_public_lab.py echo-server`（同一个 TCP 39090 不能同时监听），换成：
   ```bash
   python3.9 lxt_bulk_probe.py server --port 39090 --rate-mbps 10
   ```
3. **客户端 Linux** 执行：
   ```bash
   python3 lxt_bulk_probe.py client --server 185.217.110.15 --port 39090 \
       --up-mib 1 --down-mib 1 --connections 3 --rounds 3 \
       --mode cycle --rate-mbps 10
   ```
   3 个新连接、每个连接 3 轮，合计 18 MiB 双向数据。每轮验证上传与下载 SHA-256。
4. 成功后才逐步测试更大数据，如：
   ```bash
   python3 lxt_bulk_probe.py client --server 185.217.110.15 \
       --up-mib 8 --down-mib 8 --connections 1 --rounds 1 \
       --mode mixed --rate-mbps 10
   ```
5. 测试结束后移除客户端**仅实验用途**的阻断规则，停止两端 Sidecar 与 bulk server，确认朋友的现有服务正常。

## 推荐检查

- `tcpdump` 对比真实 SYN-ACK 和 TUN 注入 SYN-ACK 中的 MSS；报告此前记录路径真实 MSS=1332、注入 MSS=1460。开始长时间/大流量测试前**应优先修复为可靠 PMTU/MSS**，不要用这段小负载测试当作已经修复 PMTU 的证据。
- 统计 `ss -ti` 的 retrans、rtt、cwnd、pmtu、mss，结合 `tcpdump` 的重传与 ICMP Fragmentation Needed 观察。
- 区分 **连续复用同一 TCP Socket** 与 **Socket 关闭后经 UDP 重新救援**；程序 `--rounds` 复用，`--connections` 重建。
- 提供三组对照：正常 TCP / 阻断且未开启 Sidecar（预期失败）/ 阻断且开启 Sidecar（预期成功）。
- 有任何数据摘要不一致、静默超时、过多重传或中途断开，都算测试未通过，不要静默重试整轮掩盖问题。

## 安全/资源限制

- 仅测试已经获授权的两台服务器；Python 标准库，默认每方向 1 MiB/轮；默认双向总流量 18 MiB，单方向每轮上限 64 MiB，速率上限 50 Mbps。默认单端发包限速 10 Mbps；不同方向分别限速。
- 当前数据**没有应用层加密**，只适合随机测试数据；不传用户文件、密钥或隐私内容。
- 该程序没有实现跨 TCP 断线继续同一轮的机制。`--connections` 是主动新建连接，观察重新救援；单轮中途断线应该 FAIL。
