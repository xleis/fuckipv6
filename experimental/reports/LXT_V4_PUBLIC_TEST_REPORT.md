# LXT V4 公网 UDP 辅助 TCP 握手实测报告

- 测试日期：2026-10-30 17:20–17:30（Asia/Shanghai）
- 测试目标：验证客户端正常 TCP 路径收不到 SYN-ACK 时，能否通过已认证 UDP 把服务端内核产生的真实 SYN-ACK 送到客户端，经 TUN 注入后由 Linux 原生 TCP 完成握手，随后业务数据继续走原生 TCP。
- 结论：**核心机制在真实公网路径上成立**；可作为 LXT 的握手救援 / bootstrap 模块，但不是通用 TCP-over-UDP，也不能证明可穿透所有真实中间设备。

---

## 1. 执行摘要

本次测试在两台公网 Linux VPS 之间完成：

- 服务端：`relay.xlei.cc`（`vm521838.akkocloud.com`），CentOS Stream 8，公网 `185.217.110.15`。
- 客户端：`111.91.22.14`，Debian 12，私网 `172.31.0.173`，1:1 NAT 公网出口 `111.91.22.14`。

在客户端用 nftables 临时丢弃服务端发来的 `TCP 39090 SYN+ACK` 后：

1. 无客户端 sidecar 时，标准 `connect()` 超时，证明阻断有效。
2. 双端 sidecar 启用后，服务端捕获自己内核产生的真实 SYN-ACK，通过 UDP 39091 + HMAC 转送；客户端校验后经 TUN `lxtrescue0` 注入，`connect()` 成功进入 ESTABLISHED。
3. 业务数据完全走原生 TCP，没有把 TCP 数据封装进 UDP。
4. 6 条消息长连接复用、服务端断开后重连再救援，均通过。

验收结果：**8/8 PASS**（其中 UDP HMAC 仅做了正向认证测试，未做伪造/重放负向测试，见第 7 节）。

| 测试 | 结果 |
|---|---|
| 公网正常 TCP | PASS |
| UDP HMAC 认证 | PASS（正向） |
| SYN-ACK 阻断对照组 | PASS |
| UDP 辅助 TCP 建连 | PASS |
| TCP 数据双向传输 | PASS |
| TCP 长连接复用 | PASS |
| 断线后再次救援 | PASS |
| 临时防火墙规则清理 | PASS |

---

## 2. 测试拓扑与环境

### 2.1 服务端

| 项目 | 值 |
|---|---|
| 域名 / 主机 | `relay.xlei.cc` / `vm521838.akkocloud.com` |
| 系统 | CentOS Stream 8 |
| 内核 | `5.15.202-1.el8.elrepo.x86_64` |
| Python | 3.9.19 |
| 公网网卡 | `eth0`，`185.217.110.15/24`（公网 IP 直接配置在网卡） |
| 默认网关 | `185.217.110.1` |
| 其他接口 | `fuckipv6` `10.8.0.1/24`（既有业务，未触碰） |
| firewalld | inactive |
| SELinux | Disabled |
| rp_filter | `all=1`，`default=0` |
| 既有 iptables | `FKV6F-99E15423`、`fuckipv6` 相关 FORWARD 规则（未修改） |
| 代码目录 | `/root/lxt_public_synack_v4` |

服务端只运行了两个临时用户态进程：TCP echo server 与 server sidecar。测试结束后均已停止；未新增 iptables/firewalld 规则，未改默认路由，未重启，未安装长期服务。

### 2.2 客户端

| 项目 | 值 |
|---|---|
| 公网 IP | `111.91.22.14` |
| 系统 | Debian 12 (bookworm) |
| 内核 | `6.1.0-35-amd64` |
| 架构 | x86_64 |
| Python | 3.11.2 |
| 私网网卡 | `eth0`，`172.31.0.173/20` |
| 公网出口 | `111.91.22.14`（1:1 NAT / EIP） |
| 默认网关 | `172.31.0.1` |
| rp_filter | `all=0`，`default=0` |
| 防火墙工具 | 无 iptables，只有 nftables（初始 ruleset 为空） |
| 既有业务 | sing-box（UDP 53543、TCP 8881、127.0.0.1:56672）、nginx（443、8880），未受影响 |
| 代码目录 | `/root/lxt_public_synack_v4` |
| TUN | `lxtrescue0`，测试时动态创建，测试后删除 |

### 2.3 关于最初本地 Parallels VM 的说明

最初计划用 MacBook Air 上的 Parallels Ubuntu VM 作为客户端。实测发现该 VM 的出站流量经过家中 Kwrt 路由器的 PassWall2 / sing-box 透明代理（TPROXY）到 `95.169.21.76`：

- 服务端 echo 看到的 TCP peer 是 `95.169.21.76`，不是 VM 的公网出口；
- VM 发出的 UDP HELLO 源地址也是 `95.169.21.76`；
- 代理在路由器上终止了客户端 TCP，服务端真实 SYN-ACK 不会回到 VM，ISN 也对不上。

因此这条路径无法验证救援机制。按主人指示，客户端角色迁移到公网 VPS `111.91.22.14`，最终测试两端均为公网 VPS，路径上没有透明代理。

### 2.4 端口与角色

| 角色 | 地址 | 端口 |
|---|---|---|
| TCP echo server | 服务端 `0.0.0.0` | TCP 39090 |
| Server sidecar | 服务端 `eth0` | UDP 39091 |
| Client sidecar | 客户端 `eth0` | UDP 临时源端口（如 33714、59094） |
| TUN | 客户端 | `lxtrescue0`（仅握手注入） |

### 2.5 代码与本次修改

核心文件：`lxt_public_lab.py`，四种模式 `server-sidecar` / `client-sidecar` / `echo-server` / `probe`。

**修复的 Bug（客户端注入失败）**：

- 位置：本地 `lxt_public_synack_v4/lxt_public_lab.py:237`。
- 原代码：`packet = b"\0" * 14 + payload`，构造的以太网头 EtherType 为 `0x0000`。
- `tcp_ipv4()`（同文件 101–114 行）要求 `frame[12:14] == b"\x08\x00"`，因此对救援包返回 `None`，客户端收到 UDP 后静默 `continue`，从不注入。
- 修复：`packet = b"\0" * 12 + b"\x08\x00" + payload`。
- 影响范围：仅客户端解析裸 IP 救援包的路径；服务端抓的是真实以太网帧，不受影响，服务端代码保持原版。

新增测试脚本（仅实验用，不改变核心协议）：

- `client_drop_nft.sh`：`start SERVER_IP NIC [TTL]` / `stop` / `status`，只创建 `table inet lxt_v4` + input chain + 单条 SYN+ACK drop 规则，带 TTL 自动清理。
- `phase4_client.sh`：从 stdin 读 PSK，`full` 模式做对照 + 救援，`rescue` 模式只做救援；trap 清理 sidecar 与 nft 表，并抓 `eth0` / `lxtrescue0` 包。

---

## 3. 验收结果

| 测试 | 结果 | 关键证据 |
|---|---|---|
| 公网正常 TCP | PASS | 6/6 消息，3 条连接，源端口 42006/42014/42024；服务端 echo accepted 111.91.22.14 |
| UDP HMAC 认证 | PASS（正向） | 服务端 `HELLO verified`；客户端 `HELLO_ACK verified`；4 个救援 SYN-ACK 均通过 HMAC 后才注入 |
| SYN-ACK 阻断对照组 | PASS | 无 sidecar，`connect error: timed out`；nft counter 3 packets/180 bytes；tcpdump 抓到 3 个 SYN-ACK 全部被丢 |
| UDP 辅助 TCP 建连 | PASS | 主救援 3 条 + 长连接 1 条，共 4 次救援；均进入 ESTABLISHED |
| TCP 数据双向传输 | PASS | 18/18 条 echo 消息成功；数据走原生 TCP |
| TCP 长连接复用 | PASS | 6 条消息全部在同一条 TCP 连接（端口 43528） |
| 断线后再次救援 | PASS | 主救援组 3 条连接，服务端每 2 条关闭后客户端重连，3 次都重新救援成功 |
| 临时防火墙规则清理 | PASS | nft 表不存在、无 TUN、无 sidecar、无 TTL 定时器；服务端 39090/39091 已关闭，iptables 未改 |

---

## 4. 测试过程与证据

### 4.1 Phase 3：公网正常 TCP（无阻断、无客户端 sidecar）

客户端执行：

```bash
cd /root/lxt_public_synack_v4
timeout 40 python3 lxt_public_lab.py probe \
  --server 185.217.110.15 --count 6 --timeout 4
```

结果：

```text
[probe] ESTABLISHED ('172.31.0.173', 42006) -> ('185.217.110.15', 39090)
...
[PASS] completed=6, established=3
```

- 6/6 消息成功。
- 3 条连接：42006、42014、42024。
- 每 2 条消息后服务端按 `--close-after 2` 关闭，客户端自动重连。
- 服务端 echo 日志对应 3 条 `accepted ('111.91.22.14', ...)`。

结论：公网正常 TCP 路径、echo 服务、客户端重连逻辑均正常。

另做了一次“正常路径 + 客户端 sidecar”的诊断（不计入上表）：2/2 PASS，端口 58044，客户端日志 `pending SYN ... isn=1962068781`，服务端 `relayed SYN-ACK ... tcpseq=2470435828`；这次没有 `injected` 日志，暴露了 4.2 的 Bug。

### 4.2 客户端 sidecar 启动与注入 Bug

**启动问题一：pkill 自匹配**

在 SSH 远端命令里使用：

```bash
pkill -f "lxt_public_lab.py client-sidecar"
```

该 pattern 匹配到了远端 `bash -c` 自己的命令行，SSH 会话被杀死（exit 255，无输出），sidecar 未启动。后续改用 pidfile，或 `pgrep -f '[l]xt_public_lab.py client-sidecar'` 这种不会匹配自身的写法。

**启动问题二：wrapper 子 shell 持有 SSH channel**

最初用 `( cd ...; nohup ... & )` 启动，wrapper 子 shell 持有 SSH channel，SSH 挂起约 60 秒。改为：

```bash
( exec env LXT_LAB_KEY="$KEY" python3 lxt_public_lab.py client-sidecar ... ) \
  >/tmp/lxt-client.log 2>&1 </dev/null &
```

后正常返回。

**注入 Bug**

- 客户端已收到 UDP 救援包（tcpdump：`185.217.110.15.39091 > 172.31.0.173.57158 UDP length 106`），但日志没有 `injected`。
- 根因即 2.5 节的 EtherType 问题：`b"\0" * 14 + payload` 的 `frame[12:14]` 是 `0x0000`，`tcp_ipv4()` 返回 `None`，救援包被静默丢弃。
- 修复后重启客户端 sidecar，2/2 PASS，端口 58502：

```text
[client-sidecar] pending SYN local=172.31.0.173:58502 isn=1231356657
[server-sidecar] relayed SYN-ACK 185.217.110.15:39090 -> 111.91.22.14:58502 tcpseq=3660254215 ...
[client-sidecar] injected SYN-ACK to 172.31.0.173:58502 (60 bytes); NAT remote=111.91.22.14:58502
```

### 4.3 Phase 4：SYN-ACK 阻断对照组（无 sidecar）

客户端没有 iptables，使用 nftables。规则只作用于客户端入站：

```nft
table inet lxt_v4 {
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "eth0" ip saddr 185.217.110.15 tcp sport 39090 \
      tcp flags syn,ack / syn,ack counter drop
  }
}
```

（nft 的 `tcp flags syn,ack == syn,ack` 写法会报语法错误；正确写法是 `tcp flags & (syn|ack) == (syn|ack)`，nft list 显示为 `tcp flags syn,ack / syn,ack`。）

关闭客户端 sidecar，保持阻断，执行 1 次 connect：

```text
[probe] connect error: timed out
control_exit=2
```

nft 计数器：

```text
counter packets 3 bytes 180 drop
```

客户端 `eth0` tcpdump 抓到 3 个 SYN-ACK（同一次 SYN 的重传）：

```text
185.217.110.15.39090 > 172.31.0.173.52064: Flags [S.],
  seq 1247631115, ack 3145776028, ... mss 1332
```

3 个全部命中 drop。标准 TCP 在客户端侧超时，**对照组 PASS**。

### 4.4 Phase 4：UDP 辅助建连（阻断保持，双端 sidecar）

保持同一张阻断表，启动客户端 sidecar（HELLO/HELLO_ACK 认证通过，TUN `lxtrescue0` UP），执行：

```bash
probe --server 185.217.110.15 --count 6 --timeout 4
```

结果：

```text
[probe] ESTABLISHED ('172.31.0.173', 52076) -> ('185.217.110.15', 39090)
...
[PASS] completed=6, established=3
```

- 3 条连接：52076、52092、52100。
- 客户端 pending ISN：3822945293、4169341575、2567257622。
- 服务端 sidecar relayed tcpseq：420585238、345123457、3665299090。
- 对应 SYN-ACK ack = ISN+1：3822945294、4169341576、2567257623。
- TUN `lxtrescue0` 上抓到注入的 SYN-ACK，seq/ack 与真实 SYN-ACK 一致。
- nft 计数器从 3 升到 **7 packets / 420 bytes**：对照 3 个 + 救援阶段 3 个新 SYN-ACK + 1 个重传，全部被丢。
- 服务端 echo 记录 `accepted ('111.91.22.14', 52076/52092/52100)`。
- 客户端记录 3 条 `injected`；服务端记录 3 条 `relayed`。

时序（客户端本地时间）：

| 连接 | TUN 注入 | 真实 SYN-ACK 到 eth0 |
|---|---|---|
| 52076 | 17:26:28.853993 | 17:26:28.878146（注入早约 24 ms） |
| 52092 | 17:26:30.082325 | 17:26:30.081359 |
| 52100 | 17:26:31.258049 | 17:26:31.258480 |

无论注入早于还是晚于真实 SYN-ACK，真实包都被 nft 丢弃，连接最终由注入的 SYN-ACK 完成握手。**救援 PASS**。

清理后检查：`table inet lxt_v4` 不存在、无 `lxtrescue0`、无 client-sidecar。

### 4.5 长连接复用

服务端把 echo 改为 `--close-after 0`（不主动关闭），客户端仍保持阻断 + sidecar，执行 6 条消息：

```text
[probe] ESTABLISHED ('172.31.0.173', 43528) -> ('185.217.110.15', 39090)
[probe] 1/6 ... 6/6 ACK:ping-6 (TCP #1)
[PASS] completed=6, established=1
```

- 6 条消息全部在同一条 TCP 连接 43528 上完成。
- 客户端 ISN 4219380672；服务端 SYN-ACK seq 481215890，ack 4219380673。
- 真实 SYN-ACK 于 17:27:18.251767 到达并被丢弃（nft counter 1 packet/60 bytes）；TUN 注入 17:27:18.275012；连接建立。
- 服务端 echo：`accepted ('111.91.22.14', 43528)`；sidecar：`relayed SYN-ACK ... tcpseq=481215890`。

**长连接复用 PASS**。

### 4.6 断线后再次救援

主救援组使用 `--close-after 2`：服务端每 2 条消息关闭连接，客户端重新 `connect()`。3 条连接（52076/52092/52100）各自完成了一次新的 SYN → 真实 SYN-ACK 被丢 → UDP 救援 → TUN 注入 → ESTABLISHED。3 次都成功，**断线重连救援 PASS**。

### 4.7 清理

客户端：

```text
nft list ruleset            -> 空
nft list table inet lxt_v4  -> No such file or directory
lxtrescue0                  -> 不存在
client-sidecar              -> 不存在
nft TTL 定时器              -> 不存在
39090/39091                 -> 无监听
```

服务端：

```text
39090/39091                 -> 已关闭
lxt_public_lab.py 进程      -> 不存在
iptables -S                 -> 仍为 -P INPUT ACCEPT、-N FKV6F-99E15423、
                               -A FORWARD -j FKV6F-99E15423、
                               -i/-o fuckipv6 规则
```

既有 nginx / sing-box 仍在运行，未被影响。**清理 PASS**。

---

## 5. 关键 TCP 序列号与 ACK 证据

| 场景 | 客户端 ISN | 服务端 SYN-ACK seq | SYN-ACK ack | 结果 |
|---|---|---|---|---|
| 对照组（无 sidecar） | 3145776027（由 ack 反推） | 1247631115 | 3145776028 | 3 个 SYN-ACK 被 nft 丢，connect 超时 |
| 救援 1（52076） | 3822945293 | 420585238 | 3822945294 | TUN 注入，ESTABLISHED |
| 救援 2（52092） | 4169341575 | 345123457 | 4169341576 | TUN 注入，ESTABLISHED |
| 救援 3（52100） | 2567257622 | 3665299090 | 2567257623 | TUN 注入，ESTABLISHED |
| 长连接（43528） | 4219380672 | 481215890 | 4219380673 | TUN 注入，6 条消息同一连接 |

共同规律：`ack == ISN + 1`，客户端 sidecar 用这个条件匹配待建立连接；注入包的 seq/ack 与服务端真实 SYN-ACK 一致，因此 Linux TCP 栈接受它并完成三次握手。

---

## 6. 真实公网测量与人工阻断实验的区别

这一点必须分开表述：

### 6.1 真实公网路径

Phase 3 没有任何阻断，客户端内核直接收到服务端公网发来的真实 SYN-ACK，三次握手正常完成。这证明：

- 两台 VPS 之间的公网 TCP 路径可用；
- 服务端 39090、客户端出口 NAT 正常；
- echo 与重连逻辑正常。

但它**不能**证明救援机制——因为真实 SYN-ACK 本来就到了。

### 6.2 人工阻断实验

Phase 4 在客户端本机用 nftables 丢弃真实 SYN-ACK。nft 计数器和 tcpdump 证明真实 SYN-ACK 确实到达客户端网卡后被丢弃；客户端 TCP 栈最终接受的唯一 SYN-ACK 是 UDP → TUN 注入的那一个。这证明：

- 客户端在“收不到真实 SYN-ACK”的条件下，仍能完成握手；
- 服务端捕获、HMAC UDP 转送、客户端匹配、TUN 注入、内核接受这条链路是通的；
- 后续数据确实走原生 TCP。

### 6.3 不能等同的部分

客户端本机 nft 阻断**不等于**穿透了真实中间设备。真实中间设备可能：

- 在更上游丢弃 SYN-ACK（客户端看不到包），行为与本次类似，但 UDP 是否放行、服务端是否能抓到自己的 SYN-ACK 需要单独验证；
- 同时阻断 UDP 39091，则救援路径失效；
- 阻断整个 TCP 流（不仅是 SYN-ACK），则握手即使完成，数据也过不去；
- 做 NAT/端口改写、负载均衡或 TCP 代理，服务端可能根本看不到自己内核产生的 SYN-ACK。

本次没有做“云安全组/上游 ACL 丢弃 SYN-ACK、放行 UDP”的真实中间设备测试，因此结论应表述为：**在真实公网路径上，客户端收不到 SYN-ACK 时 UDP 救援机制成立；尚未证明可穿透真实中间设备**。这与主人的提醒一致。

---

## 7. 发现与限制

1. **MSS clamp 差异（重要）**
   - 客户端 `eth0` 上真实 SYN-ACK 的 MSS 是 **1332**（已经被路径/NAT clamp）。
   - TUN 注入的 SYN-ACK 的 MSS 是 **1460**（服务端 sidecar 抓到的是本机出站、尚未被路径 clamp 的原始 SYN-ACK）。
   - 小消息（ping-i，几字节）不受影响；大流量时客户端可能按 1460 发送，遇到 PMTU 黑洞。需要下一阶段修 MSS 并做 bulk 测试。

2. **Server peer map 只按 dst IP**
   - 服务端 sidecar 用“SYN-ACK 的目的 IP”匹配 8 秒内认证过的 HELLO peer。
   - 多个客户端在同一个 NAT 公网 IP 后面时，会互相覆盖，可能把 SYN-ACK 转给错误的 UDP 端口。
   - 需要改成 `(client_ip, client_tcp_port)` 或带 session id 的映射。

3. **仅 IPv4**
   - 代码只处理 IPv4 TCP；没有 IPv6 路径。服务端有 `fuckipv6` 接口，但本次未测试。

4. **UDP HMAC 只做了正向测试**
   - 认证通过的 HELLO/HELLO_ACK/SYNACK 均被接受；没有做错误密钥、过期时间戳、重放 nonce 的负向测试。
   - 代码有时间戳 ±30s、nonce 重放表、HMAC-SHA256 前 16 字节，但负向行为未实测。

5. **服务端必须能抓到自己产生的 SYN-ACK**
   - 依赖 AF_PACKET `PACKET_OUTGOING`。如果服务端前面有云 LB、NAT 或 TCP 代理，SYN-ACK 可能不是本机内核产生，sidecar 抓不到。
   - 本次服务端公网 IP 直接配在 `eth0`，满足条件；未测试 LB/NAT 场景。

6. **客户端需要 root + TUN**
   - 注入依赖 `/dev/net/tun` 和修改路由/写 TUN 的权限；普通用户不可用。
   - macOS/Windows 客户端需要不同的注入机制，本次未涉及。

7. **本地 nft 阻断 vs 真实中间设备**
   - 见第 6 节；需要云安全组/上游 ACL 的真实阻断测试。

8. **日志覆盖**
   - 客户端 `/tmp/lxt-client.log` 被长连接测试覆盖，主救援组的客户端日志以测试控制台输出为准；服务端 `/tmp/lxt-server.log` 保留了全部 relay 记录。

9. **密钥暴露**
   - 一次 `pgrep -af` 把启动 wrapper 的完整命令行（含 `LXT_LAB_KEY`）打印到了工具输出；密钥已出现在本次会话记录中。
   - 本报告不包含密钥；建议**立即轮换 PSK**，并清理服务端 shell history 中可能残留的启动命令。

---

## 8. 安全性

- 服务端未修改 SSH、SSH 防火墙、iptables/firewalld、默认路由、SELinux；未重启；未安装长期服务；未影响既有 `fuckipv6` 业务。
- 客户端未修改既有 nginx/sing-box；nft 规则只存在于独立 `table inet lxt_v4`，测试后删除；无残留 TUN/sidecar/timer。
- 所有 SYN-ACK 阻断仅在客户端本机，且自动清理。
- PSK 未写入本报告、未提交 Git；测试脚本从 stdin 读取。由于会话记录中曾出现密钥，仍建议轮换。
- UDP 通道是认证但不加密的；SYN-ACK 本身不是机密数据，但 TUN 注入是 root 权限操作，生产环境需要限制本地攻击面。

---

## 9. 是否具备扩展为 LXT 传输模块的条件

**结论：具备作为“握手救援 / bootstrap 模块”的条件，但不具备作为通用传输层的条件。**

它验证了 LXT 想要的关键能力：**不依赖客户端收到真实 SYN-ACK，也能让原生 TCP 进入 ESTABLISHED，然后继续用原生 TCP 传输数据**。这与 FakeTCP、TCP-over-UDP 是不同路线，优势是数据面不需要额外封装、不需要用户态转发数据。

要产品化，至少需要满足：

| 条件 | 本次状态 | 说明 |
|---|---|---|
| 服务端能抓到自己的 SYN-ACK | 满足 | 需要 root/AF_PACKET；LB/NAT 场景待验证 |
| 客户端 UDP 可达服务端 39091 | 满足 | 真实中间设备可能阻断 UDP |
| 客户端有 root + TUN | 满足 | 部署约束；移动端/非 Linux 需另做 |
| 仅 SYN-ACK 方向受阻 | 人工满足 | 真实“全 TCP 阻断”场景无效 |
| 数据路径正常 | 满足 | 本次只阻断 SYN-ACK，数据未被阻断 |
| IPv4 | 满足 | IPv6 未支持 |
| 多客户端 NAT | 未满足 | peer map 只按 IP，存在冲突风险 |
| 大流量 / MTU | 未验证 | MSS 1460 vs 1332，存在 PMTU 风险 |
| 密钥管理 | 实验级 | 共享 PSK、环境变量、需轮换与限流 |

因此建议定位为：**LXT 的可选握手救援模块**——正常路径优先；检测到 SYN-ACK 丢失/被阻断时，启用 UDP 侧信道把真实 SYN-ACK 送回客户端；握手完成后立即回到原生 TCP，不参与数据面。它不是“翻墙传输协议”，也不能解决数据面被阻断的问题。

---

## 10. 下一阶段改进建议

### P0（优先）

1. **轮换 PSK**，清理服务端 shell history 中可能含密钥的启动命令；后续用 secret file（0600）或密钥管理服务，不再放 argv/env。
2. **合并 EtherType 修复**到项目主线，并加回归测试：构造 `b"\0"*12 + b"\x08\x00" + ip_payload`，断言 `tcp_ipv4()` 能解析。
3. **修 MSS**：注入前把 SYN-ACK 的 MSS 改成路径观测值（本次 1332）或保守值（1200），或同时 clamp 客户端 MSS；加 1 MB 级 bulk 传输 + PMTU 测试。
4. **真实中间设备测试**：用云安全组/上游 ACL 在客户端入站丢弃 `TCP 39090 SYN+ACK`、放行 `UDP 39091`，重复本实验；再测“阻断全部 TCP”场景，明确数据面限制。

### P1

5. **peer map 改为 `(client_ip, client_tcp_port)` 或 session id**，支持同一 NAT 后多客户端；加双客户端并发测试。
6. **IPv6 支持**（至少双栈解析与注入路径），在 `fuckipv6` 或独立双栈环境验证。
7. **HMAC 负向测试**：错误密钥、过期时间戳、重放 nonce、篡改 SYNACK，确认全部拒绝且不注入。
8. **可观测性**：结构化日志与计数器（HELLO、relay、inject、invalid HMAC、peer 过期），避免记录密钥/载荷。

### P2

9. **竞态/重复处理**：真实 SYN-ACK 与注入包同时到达、重复 SYN-ACK、RST 场景；确认不会误注入或产生 RST。
10. **服务端 LB/NAT 场景**：如果 SYN-ACK 不是本机产生，设计 fallback（用户态代理或其它 bootstrap），并写清部署前提。
11. **弱网测试**：丢包、乱序、高 RTT、UDP 限速下的救援成功率与延迟；当前只测了正常公网。
12. **安全评审**：TUN 注入权限、PSK 生命周期、重放窗口、限流与防滥用；生产环境不要用共享长期 PSK。

---

## 11. 附录：复现命令与文件位置

### 服务端

```bash
cd /root/lxt_public_synack_v4
# echo（Phase 3/4 主测试）
python3.9 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 2
# echo（长连接测试）
python3.9 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 0
# server sidecar
LXT_LAB_KEY=... python3.9 lxt_public_lab.py server-sidecar \
  --interface eth0 --tcp-port 39090 --udp-port 39091
```

### 客户端

```bash
cd /root/lxt_public_synack_v4
# 正常探测
python3 lxt_public_lab.py probe --server 185.217.110.15 --count 6 --timeout 4
# 客户端 sidecar
LXT_LAB_KEY=... python3 lxt_public_lab.py client-sidecar \
  --interface eth0 --server 185.217.110.15 \
  --tcp-port 39090 --udp-port 39091
# Phase 4 对照 + 救援（stdin 读 key）
bash phase4_client.sh full 6
# 仅救援（长连接测试）
bash phase4_client.sh rescue 6
```

### 文件

- 报告：`/Users/lingxifox/Documents/deepseek-harness/default-workspace/lxt_public_synack_v4/LXT_V4_PUBLIC_TEST_REPORT.md`
- 核心代码：`lxt_public_lab.py`（客户端已含 EtherType 修复）
- 测试脚本：`client_drop_nft.sh`、`phase4_client.sh`
- 服务端日志：`/tmp/lxt-server.log`、`/tmp/lxt-echo.log`、`/tmp/lxt-echo-long.log`
- 客户端日志：`/tmp/lxt-client.log`（长连接测试覆盖了主救援组日志）

### 本次修改清单

| 文件 | 修改 |
|---|---|
| `lxt_public_lab.py` | 第 237 行：`b"\0"*14 + payload` → `b"\0"*12 + b"\x08\x00" + payload`（本地与客户端） |
| `client_drop_nft.sh` | 新增：nft 临时阻断 start/stop/status + TTL 清理 |
| `phase4_client.sh` | 新增：Phase 4 对照/救援自动化 + 抓包 + 清理 |
| `LXT_V4_PUBLIC_TEST_REPORT.md` | 新增：本报告 |
