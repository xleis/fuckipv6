# LXT V7 中间盒阻断实验报告

测试日期：2026-10-30  
测试时间：约 18:20–18:32 CST（10:20–10:32 UTC）  
结论：**A（FORWARD 丢 SYN-ACK）PASS；B（PREROUTING raw 丢 SYN-ACK，conntrack 看不到响应）FAIL；正常基线 PASS；清理 PASS。真实第三方公网中间盒仍未测试。**

---

## 1. 结论摘要

| 测试 | 结果 | 说明 |
|---|---|---|
| 正常基线：netns/NAT + 公网 TCP + 代理 + 18 MiB | **PASS** | 9/9 轮，18,874,368 B，SHA-256 全 OK |
| 正常 UDP 认证 HELLO | **PASS** | `HELLO_ACK verified`，UDP NAT 正常 |
| A 对照：FORWARD 丢 SYN-ACK，无 sidecar | **PASS** | 5.001 s 超时，drop 5 包/300 B，conntrack `SYN_RECV` |
| A 救援：UDP 注入 SYN-ACK + Google HTTPS | **PASS** | HTTP/2 200，86,885 B，`verify=0` |
| A 救援：18 MiB 双向随机分块 | **PASS** | 9/9 轮，18,874,368 B，SHA-256 全 OK |
| B 对照：PREROUTING -300 丢 SYN-ACK，无 sidecar | **PASS** | 5.0007 s 超时，drop 5 包/300 B，conntrack `SYN_SENT [UNREPLIED]` |
| B 救援：UDP 注入 SYN-ACK + Google HTTPS | **FAIL** | 客户端 8 s 超时；服务端代理无 CONNECT；ACK/DATA 以私网源 `10.200.7.2` 离开宿主机 |
| B 救援：18 MiB 双向随机分块 | **FAIL** | 1 MiB smoke 客户端本地 `established` 后 `[FAIL] timed out`；18 MiB 未完成 |
| 临时 nft/netns/veth/sidecar/TUN/key 清理 | **PASS** | 双端无 39090/39091 监听、无 lxt 进程、无实验规则 |
| 真实第三方公网中间盒 | **NOT TESTED** | 本实验的“中间盒”是客户端 VPS 宿主机 netfilter，不是第三方 in-path 设备 |

**一句话结论：** 只要中间盒的 conntrack 能看到 SYN-ACK（A），UDP 注入就能让原生 TCP 的 ACK/DATA 继续通过；如果中间盒在 conntrack 之前把 SYN-ACK 丢掉（B），客户端 TCP 虽然接受注入的 SYN-ACK 并发出 ACK/DATA，但宿主机的 NAT 没有确认回程五元组，ACK/DATA 以私网源地址离开，服务端收不到，B 必然失败。这不是“TCP 状态检查”导致的失败，而是 **NAT/conntrack 回程状态缺失**。

---

## 2. 环境与拓扑

### 2.1 客户端 VPS（同时充当模拟中间路由器）

| 项 | 值 |
|---|---|
| 主机名 | `hcss-ecs-deb1` |
| 系统 | Debian 12 bookworm |
| 内核 | 6.1.0-35-amd64 |
| Python | 3.11.2 |
| 物理网卡 | `eth0` 172.31.0.173/20 |
| 公网出口 | 111.91.22.14（1:1 NAT/EIP） |
| 默认路由 | 172.31.0.1 |
| 现有业务 | nginx 443/8880、sing-box 53543/8881；实验期间保持监听 |
| 实验 netns | `lxtv7` |
| veth | host `lxtv7h` 10.200.7.1/24 ↔ ns `lxtv7n` 10.200.7.2/24 |
| ns 默认路由 | via 10.200.7.1 |
| TUN | `lxtrescue0`（客户端 sidecar 创建） |
| nft | `lxt_v7_nat`（masquerade）、`lxt_v7_obs`（观察计数）、A/B 独立的 drop 表 |
| 工具 | nft、ip、ss、tcpdump、curl、python3、nsenter/unshare；无 iptables、无 conntrack 命令 |

### 2.2 服务端

| 项 | 值 |
|---|---|
| 域名 | `relay.xlei.cc` / `vm521838.akkocloud.com` |
| 系统 | CentOS Stream 8 |
| 内核 | 5.15.202-1.el8.elrepo.x86_64 |
| Python | 3.9.19 |
| 网卡 | `eth0` 185.217.110.15/24，公网 IP 直接配在网卡 |
| 默认路由 | 185.217.110.1 |
| SELinux | Disabled |
| firewalld | 未安装/未启用 |
| 现有服务 | haproxy/nginx 等既有业务未改动；实验只用 39090/39091 |
| 代码 | `/root/lxt_public_synack_v4`：V4 sidecar + V6 `lxt_http_proxy.py` + V5 `lxt_bulk_probe.py` |

### 2.3 拓扑

```
lxtv7 (10.200.7.2)
   │  veth lxtv7n
lxtv7h (10.200.7.1)  ← 客户端 VPS 宿主机（模拟中间路由器）
   │  eth0 172.31.0.173 → 公网 111.91.22.14
   │
   ├── TCP 39090 → relay.xlei.cc:39090（HTTP CONNECT 代理 / V5 bulk）
   └── UDP 39091 → relay.xlei.cc:39091（HMAC SYN-ACK 救援）
                          │
                          └── 代理 upstream → https://www.google.com:443
```

客户端程序全部运行在 `lxtv7` 内；宿主机负责转发、masquerade、观察和 A/B 的 SYN-ACK 丢弃。

---

## 3. 方法

1. **正常基线**：netns 内通过服务端 HTTP CONNECT 代理访问 Google；netns 内启动客户端 sidecar，验证 UDP HELLO；再跑 V5 18 MiB。
2. **A**：宿主机 `inet lxt_v7_a`，`forward priority 0`，匹配 `iifname eth0 oifname lxtv7h src=185.217.110.15 dst=10.200.7.2 sport=39090 SYN+ACK`，counter + drop。先无 sidecar 验证标准 TCP 超时，再启动 sidecar 验证 Google 与 18 MiB。
3. **B**：宿主机 `inet lxt_v7_b`，`prerouting priority raw (-300)`，匹配 `iifname eth0 src=185.217.110.15 dst=172.31.0.173 sport=39090 SYN+ACK`，counter + drop。该 hook 在 conntrack 之前，目标是让宿主机 conntrack 看不到 SYN-ACK。先无 sidecar 验证超时，再启动 sidecar 验证 Google 与 V5。
4. 双端记录：nft counter、`/proc/net/nf_conntrack`、客户端/服务端 `ss`、`tcpdump`（eth0/lxtv7h/lxtrescue0）、`nstat`、sidecar/代理/bulk 日志、V5 SHA-256。
5. A/B 的 drop 规则独立命名，仅在实验流量五元组上生效；实验结束删除。

使用的工具与端口：

| 组件 | 端口/接口 |
|---|---|
| HTTP CONNECT 代理 | 服务端 TCP 0.0.0.0:39090 |
| server sidecar | 服务端 UDP 0.0.0.0:39091；抓 TCP sport 39090 的出站 SYN-ACK |
| client sidecar | netns 内 `lxtv7n`，UDP → 39091；TUN `lxtrescue0` |
| V5 bulk | 服务端 TCP 39090（与代理互斥） |
| Google | 服务端代理 upstream 142.251.x.x:443 |

V7 使用新生成的测试 PSK，经 stdin 传入 sidecar，未写入 argv/日志；测试结束删除。此前 V4 的 PSK 已在会话中暴露，仍需轮换。

---

## 4. 正常基线

### 4.1 正常 TCP / HTTP CONNECT / Google

netns 内：

```bash
curl --proxy http://185.217.110.15:39090 --noproxy "" https://www.google.com/
```

结果：

```
NORMAL_PROXY http=200 connect=0.252389 appconnect=0.771534
             total=1.586310 verify=0 size=86818 remote=185.217.110.15
```

- 响应头：`HTTP/2 200`、`server: gws`、`content-type: text/html; charset=ISO-8859-1`
- body 含 `<title>Google`
- `generate_204` → HTTP 204，connect 0.158760 s
- netns 直连 Google 也 200（connect 0.003964 s），证明 NAT 正常

### 4.2 正常 UDP HELLO

客户端 sidecar 在 netns 内：

```bash
client-sidecar --interface lxtv7n --server 185.217.110.15 \
  --tcp-port 39090 --udp-port 39091
```

- 客户端：`HELLO_ACK verified`，TUN `lxtrescue0` UP
- 服务端：`HELLO verified from 111.91.22.14:51952`
- 宿主机 conntrack：`10.200.7.2:51952 -> 172.31.0.173:51952` UDP NAT
- 停 sidecar 后 TUN 消失

### 4.3 正常 V5 18 MiB

```bash
python3 lxt_bulk_probe.py client --server 185.217.110.15 --port 39090 \
  --up-mib 1 --down-mib 1 --connections 3 --rounds 3 --mode cycle --rate-mbps 10
```

结果：

```
WALL=20.651
[SUMMARY] PASS rounds=9/9 connections=3 bytes=18874368
          duration=20.61s avg_connect=174.0ms
```

- 各轮 6.16–9.10 Mbps，双向 SHA-256 全 OK
- 客户端 nstat：retrans/lost/timeouts 全 0；服务端 nstat 0
- `lxt_v7_obs` 观察 counter：7170 packets / 9,813,073 B
- 服务端 bulk accepted：37628 / 33028 / 40854，均 3 rounds validated
- 结论：netns + NAT + 真实公网 + 代理 + 18 MiB 正常路径 PASS

---

## 5. 实验 A：FORWARD 丢 SYN-ACK（conntrack 已看到响应）

### 5.1 规则

```nft
table inet lxt_v7_a {
  chain forward {
    type filter hook forward priority 0; policy accept;
    iifname "eth0" oifname "lxtv7h" \
      ip saddr 185.217.110.15 ip daddr 10.200.7.2 \
      tcp sport 39090 tcp flags syn,ack / syn,ack \
      counter drop
  }
}
```

### 5.2 A 对照（无 client sidecar）

```
A_CONTROL http=000 connect=0 total=5.001409
curl: (28) Timeout
```

- drop A counter：**5 packets / 300 B**
- 宿主机 conntrack：**`SYN_RECV`**，即 conntrack 已经看到 SYN-ACK
- 客户端无 39090 连接
- 宿主机 eth0 抓到 5 个 SYN-ACK：`seq 2384003190, ack 954667809, MSS 1332`
- `lxtv7h` 和 netns 只有 SYN，没有 SYN-ACK
- 服务端代理日志只有 `listening`，无 CONNECT
- 结论：标准 TCP 在 A 规则下超时，**A 对照 PASS**

### 5.3 A 救援：Google

启动客户端 sidecar（netns 内，HELLO_ACK verified；服务端 `HELLO verified from 111.91.22.14:52105`），drop A 保持：

```
A_RESCUE http=200 connect=0.181954 appconnect=0.516290
         total=0.883495 verify=0 size=86885 remote=185.217.110.15
```

- 响应头：`HTTP/1.1 200 Connection Established`、`Proxy-Agent: lxt-v6`、`HTTP/2 200`、`server: gws`
- body：86,885 B，`<title>Google`
- nft A counter：5 → 6
- TUN `lxtrescue0` 抓包：

```
18:24:49.434979 IP 185.217.110.15.39090 > 10.200.7.2.42700:
  Flags [S.], seq 3797640969, ack 2151217263, win 65160,
  options [mss 1460, ...]
```

- 宿主机 eth0 真实 SYN-ACK：

```
18:24:49.411693 IP 185.217.110.15.39090 > 172.31.0.173.42700:
  Flags [S.], seq 3797640969, ack 2151217263,
  options [mss 1332, ...]
```

- 真实包被 A 规则丢弃；TUN 注入包 MSS 1460，早约 23 ms
- 客户端 sidecar：`pending SYN local=10.200.7.2:42700 isn=2151217262` → `injected ... NAT remote=111.91.22.14:42700`
- 宿主机 conntrack：该流进入 `TIME_WAIT`，NAT 正常
- 服务端代理：`CONNECT www.google.com:443 from 111.91.22.14:42700`，upstream 142.251.151.119:443，`up=815 down=92909`
- 结论：**A Google PASS**

### 5.4 A 救援：V5 18 MiB

停代理、起 bulk server，客户端 sidecar 保持，drop A 保持：

```
PASS 9/9 connections=3 bytes=18874368 duration=20.49s avg_connect=182.1ms
```

- 各轮 7.12–8.85 Mbps，SHA-256 全 OK
- 服务端 bulk accepted：48176 / 54608 / 54612，均 validated
- nft A 最终：**9 packets / 540 B**（对照 5 + Google 1 + V5 3）
- 客户端 sidecar injected 4 次（Google + 3 V5）；服务端 sidecar relayed 4 次
- 客户端 nstat：`TcpRetransSegs` 2 → 7（+5），`TCPLostRetransmit` 1，`TCPTimeouts` 2 不变
- 服务端 nstat：before/after 完全一致（0 变化）
- 结论：**A V5 18 MiB PASS**

A 的三段（对照、Google、18 MiB）全部 PASS。

---

## 6. 实验 B：PREROUTING raw 丢 SYN-ACK（conntrack 看不到响应）

### 6.1 规则

```nft
table inet lxt_v7_b {
  chain prerouting {
    type filter hook prerouting priority raw; policy accept;
    iifname "eth0" \
      ip saddr 185.217.110.15 ip daddr 172.31.0.173 \
      tcp sport 39090 tcp flags syn,ack / syn,ack \
      counter drop
  }
}
```

`priority raw` = -300，在 conntrack 之前。

### 6.2 B 对照（无 client sidecar）

```
B_CONTROL http=000 connect=0 total=5.000703
curl: (28) Timeout
```

- drop B counter：**5 packets / 300 B**
- 宿主机 conntrack：**`SYN_SENT ... [UNREPLIED]`**，即 conntrack 没有看到 SYN-ACK
- 客户端无 39090 连接
- 宿主机 eth0 抓到 5 个 SYN-ACK：`seq 1508845018, ack 773833086, MSS 1332`
- `lxtv7h` 抓包为空（SYN-ACK 在 prerouting 就被丢，未进入 forward）
- 服务端代理日志只有 `listening`，无 CONNECT
- 结论：**B 对照 PASS**

### 6.3 B 救援：Google —— FAIL

启动客户端 sidecar（HELLO_ACK verified；服务端 `HELLO verified from 111.91.22.14:47158`），drop B 保持：

```
B_RESCUE http=000 connect=0.000000 appconnect=0.000000
         total=8.001277 size=0 remote=185.217.110.15
B_RESCUE_FAIL
curl: (28) Proxy CONNECT aborted due to timeout
```

- nft B counter：5 → **8 packets / 480 B**
- TUN 注入 3 次 SYN-ACK（重传），seq/ack 相同：

```
18:27:06.197011 IP 185.217.110.15.39090 > 10.200.7.2.55042:
  Flags [S.], seq 2075018336, ack 1334360668, options [mss 1460, ...]
```

- 宿主机 eth0 真实 SYN-ACK：

```
18:27:06.221648 IP 185.217.110.15.39090 > 172.31.0.173.55042:
  Flags [S.], seq 2075018336, ack 1334360668, options [mss 1332, ...]
```

- 真实包被 B 规则丢弃；客户端 sidecar 多次 `injected`；服务端 sidecar 有对应的 `relayed`
- 宿主机 conntrack 仍为 **`SYN_SENT [UNREPLIED]`**
- 服务端代理无 CONNECT；服务端 `ss` 无 39090 连接
- 客户端最终 `ss`：`FIN-WAIT-1 ... 10.200.7.2:55042 -> 185.217.110.15:39090`
- 结论：**B Google FAIL**

**决定性证据（宿主机 eth0 pcap）：**

```
18:27:06.038408 IP 172.31.0.173.55042 > 185.217.110.15.39090:
  Flags [S], seq 1334360667, ...                    ← SYN 被 SNAT 成 172.31.0.173

18:27:06.197059 IP 10.200.7.2.55042 > 185.217.110.15.39090:
  Flags [.], ack 2075018337, ...                    ← ACK 源地址是私网 10.200.7.2

18:27:06.197123 IP 10.200.7.2.55042 > 185.217.110.15.39090:
  Flags [P.], seq 0:120, ack 1, ... length 120      ← HTTP CONNECT 请求，源地址仍是 10.200.7.2
```

客户端 TCP 已经接受注入的 SYN-ACK，并发出 ACK + 120 B 的 HTTP CONNECT；但宿主机 conntrack 没有看到 SYN-ACK，回程五元组未确认，NAT 没有把这些 ACK/DATA 的源地址改写成 172.31.0.173，它们以私网 `10.200.7.2` 离开宿主机，服务端收不到，代理自然不会出现 CONNECT。

这不是“TCP 状态检查”导致失败：客户端内核接受了 SYN-ACK，TCP 状态机和 ACK/DATA 都正常；失败点在 **NAT/conntrack 回程状态**。

### 6.4 B 救援：V5 18 MiB —— FAIL（1 MiB smoke）

B 下先跑 1 MiB × 1 连接 × 1 轮：

```
[client] conn=1/1 established in 228.7 ms
[FAIL] timed out
```

- 客户端 sidecar 注入成功，客户端 TCP `established`（本地），但数据阶段超时
- nft B 最终：**14 packets / 840 B**
- 宿主机 conntrack：`SYN_SENT [UNREPLIED]`
- 服务端 bulk 日志：

```
[server] listen 0.0.0.0:39090; max 64 MiB/dir/round
[server] accepted ('212.95.58.220', 35982)
[server] closed with error: bad frame header/size
```

  `212.95.58.220` **不是测试客户端**（客户端公网出口是 111.91.22.14），是 39090 暴露在公网后遇到的无关扫描/探测连接；它发了不符合 LXTBULK1 的帧，被服务端拒绝。测试客户端自己的连接没有被服务端接受，服务端 `ss` 无 39090 连接。
- 因此 B 的 18 MiB 无法完成；1 MiB smoke 已失败，18 MiB 没有继续跑（避免无意义重试）。
- 结论：**B V5 18 MiB FAIL / NOT COMPLETED**

### 6.5 B 的失败路径（与 A 的对照）

| 阶段 | A | B |
|---|---|---|
| SYN-ACK 丢弃点 | FORWARD priority 0（conntrack 之后） | PREROUTING raw -300（conntrack 之前） |
| 宿主机 conntrack | `SYN_RECV`，看到响应 | `SYN_SENT [UNREPLIED]`，没看到响应 |
| 客户端接受注入 SYN-ACK | 是 | 是 |
| 客户端发出 ACK/DATA | 是 | 是 |
| 宿主机 SNAT 源地址 | 172.31.0.173（正常） | 10.200.7.2（未改写） |
| 服务端是否收到 ACK/DATA | 是 | 否 |
| 代理 CONNECT / Google 200 | 是 | 否 |
| 18 MiB | PASS 9/9 | 1 MiB smoke FAIL；18 MiB 未完成 |

A 的成功条件：中间盒的 conntrack 必须看到 SYN-ACK，这样 NAT 回程五元组才成立；UDP 注入的 SYN-ACK 只是让客户端 TCP 进入 ESTABLISHED，后续 ACK/DATA 仍由宿主机正常 NAT 转发。

B 的失败条件：中间盒在 conntrack 之前丢弃 SYN-ACK，宿主机 NAT 从未确认回程；即使客户端 TCP 接受注入并发出 ACK/DATA，这些包也不会被正确 SNAT，服务端看不到。要修复 B，必须让救援路径 **绕开或补偿这个 conntrack/NAT 状态**，而不是只注入 SYN-ACK。

---

## 7. 计数与数据完整性

| 项目 | 正常 | A | B |
|---|---|---|---|
| 端到端 TCP 建连 | 3 | 4（1 Google + 3 V5） | 0 |
| 客户端本地建连（注入后） | 0 | 4 | 2（Google + V5 各 1，均本地） |
| SYN-ACK 被 nft 丢弃 | 0 | 9（5 对照 + 1 Google + 3 V5） | 14（5 对照 + 3 Google + 6 V5） |
| UDP 救援尝试 | 1 次 HELLO（非救援） | 4 | 4（3 Google + 1 V5） |
| UDP 救援端到端成功 | — | 4 | 0 |
| 18 MiB 轮次 | 9/9 | 9/9 | 1 MiB smoke 0/1；18 MiB 未完成 |
| 传输字节 | 18,874,368 | 18,874,368（+ Google 86,885） | 0 |
| SHA-256 | 全 OK | 全 OK | 无数据 |
| Google | HTTP/2 200，86,818 B | HTTP/2 200，86,885 B | HTTP 000，超时 |
| 客户端 TCP 重传 | 0 | +5 | 最终较 A 后 +9（10,186 vs 10,177） |
| 服务端 TCP 重传 | 0 | 0（before/after 一致） | 未单独隔离（宿主总量很大，不能归因） |

说明：B 的“客户端本地建连 2”只表示客户端内核通过注入的 SYN-ACK 进入了 ESTABLISHED；服务端没有收到有效 ACK，端到端仍是 0。

关键序列号/ACK 证据：

| 场景 | 客户端 ISN | 服务端 SYN-ACK seq | ACK |
|---|---|---|---|
| A 对照 | 954667808 | 2384003190 | 954667809 |
| A Google | 2151217262 | 3797640969 | 2151217263 |
| B 对照 | 773833085 | 1508845018 | 773833086 |
| B Google | 1334360667 | 2075018336 | 1334360668 |

A V5 的三条连接（服务端 accepted 48176 / 54608 / 54612）的完整 seq/ack 在 `logs/client/client-a.log` 和 `logs/server/server-sidecar.log`；A Google 与 A V5 的 TUN/eth0 抓包在 `logs/client/a-rescue-*.pcap`。

---

## 8. 真实公网中间盒 vs 本实验的人工阻断

本实验的“中间盒”是**客户端 VPS 宿主机自己的 netfilter**（`lxtv7` 的 veth 转发路径），不是第三方公网路由器/运营商设备。客户端到服务端之间的 TCP/UDP 数据确实经过真实公网（111.91.22.14 ↔ 185.217.110.15），但 SYN-ACK 的丢弃点是本地宿主机。

- A 证明：在本机中间盒的 FORWARD 阶段丢弃 SYN-ACK（conntrack 已看到）时，UDP 注入 + 原生 TCP 数据可以通过公网到服务端并访问 Google。
- B 证明：在本机中间盒的 PREROUTING raw 阶段丢弃 SYN-ACK（conntrack 未看到）时，NAT 回程不成立，客户端 ACK/DATA 以私网源离开，端到端失败。
- **真实第三方中间盒仍未测试。** 真实 in-path 设备可能：
  - 在 conntrack 之前丢 SYN-ACK（类似 B）→ 预计失败；
  - 在 conntrack 之后丢（类似 A）→ 可能成功；
  - 有自己的 NAT/CGNAT，回程状态行为可能不同；
  - 对 UDP 39091 有限制或对 SYN-ACK 有 DPI。
  
  因此不能把 A 的 PASS 外推为“穿透真实公网封锁”。V4/V5/V6 报告中的“真实中间设备 NOT TESTED”在 V7 仍然成立；V7 只是把中间盒行为拆成 A/B 两类，并证明 B 类在当前设计下失败。

---

## 9. 清理与安全

### 9.1 客户端

- 停 client sidecar，TUN `lxtrescue0` 消失
- 删除 `lxt_v7_a` / `lxt_v7_b` / `lxt_v7_nat` / `lxt_v7_obs` 及 A/B timer
- 删除 netns `lxtv7` 和 veth `lxtv7h`
- `ip_forward` 恢复 0
- 39090/39091 无监听，无 lxt 进程
- 实验目录 `/root/lxt_v7`、`/tmp/lxt7`、PSK 文件删除
- 既有 nginx 443/8880 仍由 nginx 进程监听；`systemctl is-active nginx` 返回 `inactive`，但进程仍在（可能是手动启动/不同 unit），V7 未对 nginx 做任何操作；sing-box 仍 active
- 宿主机 conntrack 残留一条 UDP `10.200.7.2:47158 -> 185.217.110.15:39091`，netns 已删，无流量，会自然过期
- 客户端无 iptables（本来就没有），nft ruleset 为空

### 9.2 服务端

- 停 server sidecar、V5 bulk、V6 代理；39090/39091 无监听，无 lxt 进程
- 删除 `/root/lxt_v7`、`/tmp/lxt7-server.tgz`
- iptables 未改：仍为 `-P INPUT ACCEPT`、`-N FKV6F-99E15423`、`-A FORWARD -j FKV6F-99E15423`、`fuckipv6` 规则
- 默认路由未改（185.217.110.1），SSH 22/root 正常，SELinux Disabled，firewalld inactive
- 既有 haproxy/sing-box/xray 等监听未改动
- 服务端最终 `nstat` 是宿主自启动以来的总量（数百万重传），不是 V7 的增量；A 的 before/after 已单独确认 0 变化

### 9.3 密钥与日志

- V7 PSK 仅通过 stdin 传入，未出现在 argv/日志；测试后删除
- SSH 密码只进入过临时 askpass/ControlMaster，测试后删除临时文件并关闭 master
- 证据日志已复制到本地 `lxt_v7_middlebox/logs/`；日志经检查不含 `LXT_LAB_KEY`
- V4 旧 PSK 曾在会话中暴露，建议轮换
- **安全事件（低风险）**：B 阶段 39090 bulk server 绑定 0.0.0.0，暴露期间收到一个无关源 `212.95.58.220` 的连接，发了错误帧后被拒；没有测试数据泄露，服务端已停止。下一阶段应把实验端口绑定到客户端公网 IP 或用临时 nft allowlist。

---

## 10. 遗留问题

1. **B 类中间盒在当前设计下不可用。** 需要 conntrack/NAT 感知的修复，不能只注入 SYN-ACK。
2. **真实第三方公网中间盒未测试。** 本实验的 A/B 是客户端宿主机 netfilter。
3. **B 的 18 MiB 未完成。** 1 MiB smoke 已失败，18 MiB 无意义；报告按 FAIL/NOT COMPLETED 记录。
4. **MSS 1332 vs 1460。** 服务端/路径真实 SYN-ACK MSS 1332；V4 sidecar 抓到的出站 SYN-ACK 是 1460，注入后客户端有效发送 MSS 1448，A 的 18 MiB 未触发 PMTU 失败（该路径 client→server 可承载 1500 B），但真实 MTU < 1500 的路径仍有风险，应在注入时 clamp 到路径 MSS。
5. **代理与 bulk 没有认证/ACL/限流。** V6 代理是实验性最小实现；V7 的 39090 暴露还引来了扫描。生产化必须加认证、allowlist、超时和连接上限。
6. **多客户端 peer map 仍按 IP。** 同一 NAT 后多个客户端会冲突。
7. **客户端 nginx 的 systemd unit 状态异常（inactive 但进程在监听）。** V7 未触碰 nginx；如主人关心，可单独核对 unit，但不要在本实验收尾时重启业务。
8. **B 的失败可能因真实中间盒的具体 hook/NAT 行为而不同。** 需要用真实设备复现 A/B 两类，而不是只靠本机 netfilter。

---

## 11. 下一阶段建议

### 11.1 修复 B 类失败

优先级从高到低：

1. **在 conntrack 之前完成救援注入。** 如果中间盒是客户端宿主机，可在 netns 内、宿主机 NAT 之前让客户端 TCP 直接看到 SYN-ACK；但真实中间盒在客户端之外时，这不可行。
2. **让 NAT 对救援流无状态/免 conntrack。** 在客户端宿主机对 39090 的救援流使用 `NOTRACK` / 独立 stateless NAT，或用 eBPF/tc 在 conntrack 之外做源地址改写；这要求能控制中间盒，不适合第三方设备。
3. **把客户端的 ACK/DATA 也纳入 UDP 救援路径。** 即救援不只是 SYN-ACK，还要在客户端与服务器之间建立一条可绕过故障 middlebox 的带外通道；这会走向“部分 TCP-over-UDP”，与“原生 TCP 数据”目标冲突，需明确边界。
4. **让中间盒看到 SYN-ACK。** 例如服务端通过另一条路径重传 SYN-ACK，或让客户端先发一个能被 conntrack 关联的探测；但这等于绕过“中间盒看不到 SYN-ACK”的故障条件，不符合 B 的题意。
5. **换一个部署点。** 如果客户端能控制自己的 NAT，A 类（conntrack 之后丢）是可行场景；如果故障在运营商/CGNAT，B 类失败是硬限制。

### 11.2 测试真实中间盒

- 在客户端与服务端之间放一台真实路由器/云安全组/防火墙，分别配置“conntrack 后丢 SYN-ACK”和“conntrack 前丢 SYN-ACK”。
- 用云安全组/ACL 模拟“只阻断 SYN-ACK、放行 UDP 39091、放行后续 ACK/DATA”，这是最接近真实公网封锁的测试。
- 记录中间盒的 conntrack 表、NAT 回程、UDP 超时、ICMP 反馈。

### 11.3 其他

- 注入 SYN-ACK 时按路径 MSS clamp（至少 min(1460, 真实 SYN-ACK MSS)），避免 PMTU 黑洞。
- 代理加认证/allowlist/连接上限/空闲超时；实验端口绑定客户端公网 IP。
- peer map 改为 (IP, port) 或加 client ID，支持同 NAT 多客户端。
- 轮换 V4/V6/V7 所有测试 PSK；清理旧日志中的敏感信息。
- 对真实 Google 的 TLS 证书做 `openssl s_client` 校验（V6/V7 已用 curl `verify=0`，但报告应附证书链/主体证据）。

---

## 12. 证据文件

本地目录：`/Users/lingxifox/Documents/deepseek-harness/default-workspace/lxt_v7_middlebox/logs/`

客户端：

- `client/a-control-eth0.pcap`、`a-control-lxtv7h.pcap`、`a-control-netns.pcap`
- `client/a-rescue-eth0.pcap`、`a-rescue-host.pcap`、`a-rescue-tun.pcap`
- `client/a-rescue-google.headers`、`a-rescue-google.html`
- `client/b-control-eth0.pcap`、`b-control-lxtv7h.pcap`
- `client/b-rescue-eth0.pcap`、`b-rescue-host.pcap`、`b-rescue-tun.pcap`
- `client/b-rescue-google.headers`（空，超时）
- `client/client-a.log`、`client/client-b.log`、`client/client-normal.log`
- `client/v5-normal.log`、`v5-a.log`、`v5-b.log`
- `client/nstat-normal-before/after.txt`、`nstat-a-before/after.txt`
- `client/final-client.txt`（B 阶段状态）、`client/lxt7-final-client.txt`（清理后状态）
- `client/drop_a.nft`、`drop_b.nft`

服务端：

- `server/bulk-normal.log`、`bulk-a.log`、`bulk-b.log`
- `server/server-sidecar.log`、`proxy-a.log`、`proxy-b.log`
- `server/nstat-server-before/after.txt`、`nstat-server-a-before/after.txt`
- `server/final-server.txt`、`server/lxt7-final-server.txt`
- `server/lxt_http_proxy.py`

关键文件：

- 本报告：`LXT_V7_MIDDLEBOX_REPORT.md`
- A 的成功证据：`a-rescue-google.headers`、`a-rescue-tun.pcap`、`v5-a.log`
- B 的失败证据：`b-rescue-eth0.pcap`（源 10.200.7.2 的 ACK/DATA）、`v5-b.log`、`bulk-b.log`、`lxt7-final-client.txt`
