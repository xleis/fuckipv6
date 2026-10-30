# LXT V8 — 无 NAT 中间设备与 TCP 状态验证报告

- 日期：2026-10-30（Asia/Shanghai）
- 实验目标：完全去掉 NAT/SNAT，仅在三层隔离网络的中间路由器上丢弃 SYN-ACK，验证 UDP 救援后的**原生 TCP 数据**能否继续传输。
- 核心问题：**如果中间设备在 conntrack 之前丢弃 SYN-ACK，客户端 TCP 已通过 TUN 注入完成握手，后续 ACK/DATA 到底能不能通过？**
- 结论先行：**能，但取决于中间设备是否执行严格的 conntrack invalid 过滤。**
  - V8-B（PREROUTING raw 丢 SYN-ACK，无 NAT，默认放行 invalid）：**PASS**，18 MiB 双向数据、SHA-256 全部成功。
  - V8-C（B + `ct state invalid drop`）：**FAIL**，客户端 ACK/DATA 被 invalid 规则丢弃，服务端代理从未收到 CONNECT。

---

## 1. 环境与拓扑

### 1.1 客户端 VPS

| 项 | 值 |
|---|---|
| 主机 | `hcss-ecs-deb1` |
| 系统 | Debian 12 (bookworm) |
| 内核 | 6.1.0-35-amd64 |
| Python | 3.11.2 |
| 物理网卡 | `eth0` 172.31.0.173/20 |
| 公网出口 | 111.91.22.14（1:1 NAT/EIP） |
| 默认路由 | 172.31.0.1 |
| 既有业务 | nginx 443/8880、sing-box 53543/8881（未改动） |
| 宿主 nft | 初始为空；V8 规则全部在独立 netns 内 |
| 宿主 `ip_forward` | 初始 0；实验期间 1；结束后恢复 0 |

### 1.2 三 Namespace 拓扑

```text
Client NS lxtv8c                 Router NS lxtv8r                 Server NS lxtv8s
10.80.1.2/24  ── v8c0 ── v8r0 ── 10.80.1.1/24
                                 10.80.3.1/24 ── v8r1 ── v8s0 ── 10.80.3.2/24
```

| 组件 | 地址 | 说明 |
|---|---|---|
| Client NS | 10.80.1.2/24 | 静态路由 `10.80.3.0/24 via 10.80.1.1` |
| Router NS | 10.80.1.1/24, 10.80.3.1/24 | `ip_forward=1`，`rp_filter=0`，**无 NAT/SNAT/MASQUERADE** |
| Server NS | 10.80.3.2/24 | 静态路由 `10.80.1.0/24 via 10.80.3.1` |

> 重要：V8 的“服务端”是客户端 VPS 内的 `lxtv8s` Namespace，不是远程 `relay.xlei.cc`。这是为了完全隔离公网与 NAT 变量。远程朋友的服务器在 V8 中未被使用，也未做任何修改。

### 1.3 组件

- V4 `lxt_public_lab.py server-sidecar`：`lxtv8s` 内，`--interface v8s0 --tcp-port 39090 --udp-port 39091`，抓取本机出站 SYN-ACK，经 UDP 39091 HMAC 转发。
- V4 `client-sidecar`：`lxtv8c` 内，`--interface v8c0 --server 10.80.3.2`，TUN `lxtrescue0`，注入 SYN-ACK。
- V6 `lxt_http_proxy.py`：`lxtv8s` 内，监听 0.0.0.0:39090，HTTP CONNECT 代理。
- 本地 HTTP 服务：`lxtv8s` 内 10.80.3.2:8080，返回固定 83 B 页面，SHA-256 `36de7cfa4362ca52572a69f8887921372e9145ebd5a4900f7b858da4f3edbd01`。
- V5 `lxt_bulk_probe.py`：18 MiB 双向随机分块 + 双向 SHA-256。

### 1.4 nftables 观测/实验表

| 表 | 链/hook | 作用 |
|---|---|---|
| `inet lxt_v8_obs` | forward priority 10 | 只计数，不丢包；双向 39090 字节/包数 |
| `inet lxt_v8_ct` | forward priority 20 | `ct state new,established,related counter`；`ct state invalid counter`；不丢包 |
| `inet lxt_v8_a` | forward priority 0 | A：丢弃 `v8r1→v8r0`、源 10.80.3.2、目的 10.80.1.2、sport 39090、SYN+ACK |
| `inet lxt_v8_b` | prerouting priority -300（raw） | B：在 conntrack 之前丢弃同一 SYN-ACK |
| `inet lxt_v8_c` | forward priority 0 | C：`ct state invalid counter drop` |

A/B/C 规则均带独立表名和 counter，支持超时/异常清理；实验结束全部删除。

---

## 2. 正常基线（无阻断）

### 2.1 连通性

- Client NS 直连 Server NS 的 8080：`HTTP 200`，83 B。
- 经 V6 CONNECT 代理访问 8080：`HTTP 200`，83 B，body SHA-256 与直连一致。
- 代理日志：`CONNECT 10.80.3.2:8080`，`tunnel closed up=78 down=268`。
- UDP 认证：客户端 `HELLO_ACK verified`；服务端 `HELLO verified from 10.80.1.2:41138`；TUN `lxtrescue0` UP；停止 sidecar 后 TUN 消失。

### 2.2 正常 18 MiB

| 指标 | 结果 |
|---|---|
| 轮次 | 9/9 |
| 连接 | 3（每连接 3 轮） |
| 总字节 | 18,874,368 B |
| 用时 | 15.70 s |
| 平均建连 | 1.6 ms |
| 吞吐 | 约 10.2 Mbps |
| SHA-256 | 全部 OK |
| 服务端 accepted | 35488 / 35496 / 51684，各 3 rounds validated |
| obs counter | 7515 pkt / 9,894,325 B；7391 pkt / 9,822,514 B |
| 客户端 nstat | 无重传/超时增量 |

**正常基线 PASS。**

---

## 3. 实验 A：FORWARD 丢 SYN-ACK（conntrack 能看到 SYN-ACK）

### 3.1 规则

`inet lxt_v8_a` forward priority 0：

```text
iifname "v8r1" oifname "v8r0" ip saddr 10.80.3.2 ip daddr 10.80.1.2 \
  tcp sport 39090 tcp flags syn,ack / syn,ack counter drop
```

### 3.2 对照（无客户端 sidecar）

| 指标 | 结果 |
|---|---|
| curl | `(28) Timeout`；`A_CONTROL http=000 total=5.000967` |
| drop A | 5 packets / 300 B |
| conntrack | `SYN_RECV`（conntrack 已看到 SYN-ACK） |
| 客户端 | 无连接 |
| 服务端代理 | 仅 listening，无 CONNECT |

**A 对照 PASS。**

### 3.3 救援（drop A + client sidecar）

| 指标 | 结果 |
|---|---|
| HTTP | 200；0.003293 s；83 B |
| drop A | 7 packets / 420 B（含 1 个 control 遗留 SYN-ACK） |
| 客户端 | `pending SYN local=10.80.1.2:44438 isn=3723037661`；`injected ...` |
| 服务端 | `relayed ... 44438 tcpseq=1485280730` |
| 代理 | `CONNECT 10.80.3.2:8080 from 10.80.1.2:44438`；`tunnel closed up=78 down=268` |
| body SHA | 一致 |

**A 救援 PASS。**

### 3.4 A 的 18 MiB

| 指标 | 结果 |
|---|---|
| 轮次 | 9/9 |
| 连接 | 3：41122 / 41134 / 46752 |
| 字节 | 18,874,368 B |
| 用时 | 15.70 s |
| 平均建连 | 0.8 ms |
| 吞吐 | 10.2 Mbps |
| SHA-256 | 全部 OK |
| drop A 最终 | 10 packets / 600 B（对照 5 + 救援 2 + V5 3） |
| 客户端 nstat | 无重传/超时增量 |

### 3.5 A 的 conntrack 证据

初始 router conntrack count 为 0；加入 `lxt_v8_ct` 后重跑对照：

- `A_CONTROL2 http=000 total=5.001478`
- conntrack：`tcp 6 58 SYN_RECV src=10.80.1.2:45882 dst=10.80.3.2:39090`
- drop A 累计 15/900；`ct` 表 9/1068
- UDP 39091 的 conntrack 条目同时存在

**A 三小项（对照、救援、18 MiB）均 PASS。**
A 的关键：SYN-ACK 在 forward 阶段被丢，但 conntrack 在更早的 hook 已看到它，状态为 `SYN_RECV`；注入 SYN-ACK 后，后续 ACK/DATA 的 conntrack 状态有效，数据通过。

---

## 4. 实验 B：PREROUTING raw 丢 SYN-ACK（conntrack 看不到 SYN-ACK，无 NAT）

### 4.1 规则

`inet lxt_v8_b` prerouting priority -300（raw）：

```text
iifname "v8r1" ip saddr 10.80.3.2 ip daddr 10.80.1.2 \
  tcp sport 39090 tcp flags syn,ack / syn,ack counter drop
```

该 hook 在 conntrack（priority -200）之前，SYN-ACK 不会进入 conntrack。

### 4.2 对照（无客户端 sidecar）

| 指标 | 结果 |
|---|---|
| curl | `(28) Timeout`；`B_CONTROL http=000 total=5.001436` |
| drop B | 5 packets / 300 B |
| conntrack | **`SYN_SENT ... 47610 [UNREPLIED]`**（未看到 SYN-ACK） |
| 服务端代理 | 仅 listening，无 CONNECT |

**B 对照 PASS。**

### 4.3 救援（B drop + client sidecar）

| 指标 | 结果 |
|---|---|
| HTTP | 200；0.003392 s；83 B |
| drop B | 累计 6 packets / 360 B |
| conntrack | **救援连接仍为 `SYN_SENT ... 52430`**（无 SYN-ACK） |
| `lxt_v8_ct` | `ct state invalid 7 packets / 637 B` 开始出现 |
| 客户端 | `pending SYN local=10.80.1.2:52430 isn=2543773650`；`injected ...` |
| 服务端 | `relayed ... 52430 tcpseq=326269250` |
| 代理 | `CONNECT ... 52430`；`tunnel closed up=78 down=268` |
| body SHA | 一致 |

**B 救援 PASS。** 这是 V8 的核心结果：conntrack 从未看到 SYN-ACK，客户端仍完成了原生 TCP 连接，且代理真实收到了 CONNECT 和 HTTP 数据。

### 4.4 B 的 18 MiB

| 指标 | 结果 |
|---|---|
| 轮次 | **9/9** |
| 连接 | 3：52444 / 52450 / 53862 |
| 字节 | **18,874,368 B** |
| 用时 | 15.70 s |
| 平均建连 | 0.8 ms |
| 吞吐 | 10.2 Mbps |
| SHA-256 | **全部 OK** |
| drop B 最终 | 9 packets / 540 B（对照 5 + 救援 1 + V5 3） |
| `lxt_v8_ct` 最终 | `established,related,new 10909 / 10,006,829`；`invalid 11057 / 10,012,667` |
| obs counter | 27086 / 29,787,381 B；26505 / 29,692,618 B |
| router conntrack | 52430/52444/52450/53862 仍为 `SYN_SENT` |
| 客户端 nstat | 无重传/超时增量 |

**B 的 18 MiB PASS。**

### 4.5 B 的机制解释

1. 客户端 SYN 进入 conntrack，router 建立 `SYN_SENT`。
2. 服务端 SYN-ACK 在 PREROUTING raw 被丢，**conntrack 从未看到 SYN-ACK**，状态保持 `SYN_SENT [UNREPLIED]`。
3. 服务端 sidecar 抓取本机出站 SYN-ACK，UDP 转发；客户端 sidecar 注入 TUN，客户端内核接受并进入 ESTABLISHED。
4. 客户端后续 ACK/DATA 到达 router，conntrack 将它们标记为 **invalid**（因为状态机没有看到 SYN-ACK）。
5. 但 router 的默认 forward 策略是 accept，**没有 `ct state invalid drop`**，所以 invalid 的 ACK/DATA 仍被转发。
6. 服务端收到 ACK/DATA，TCP 连接端到端成立，18 MiB 完成。

**关键结论：conntrack 状态不匹配本身不会阻断转发；只有显式的 invalid 过滤才会阻断。**

---

## 5. 实验 C：B + 严格 TCP conntrack 状态检查

### 5.1 规则

在 B 保持不变的基础上增加：

`inet lxt_v8_c` forward priority 0：

```text
ct state invalid counter drop
```

### 5.2 C 对照（无客户端 sidecar）

| 指标 | 结果 |
|---|---|
| curl | `(28) Timeout`；`C_CONTROL http=000 total=5.001416` |
| drop B | 14 packets / 840 B（含此前 B 的 9 + C 对照 5） |
| C strict | 0 packets |
| conntrack | `SYN_SENT ... 50162 [UNREPLIED]` |
| 服务端代理 | 无 CONNECT |

**C 对照 PASS**（普通 TCP 超时符合预期）。

### 5.3 C 救援（B drop + C strict + client sidecar）

| 指标 | 结果 |
|---|---|
| curl | `(28) Proxy CONNECT aborted due to timeout`；`C_RESCUE3 http=000 total=5.000920` |
| C strict | **10 packets / 1192 B `ct state invalid ... drop`** |
| drop B | 35 packets / 2100 B（含 control 与救援重传） |
| conntrack | 救援连接 `SYN_SENT ... 49612 [UNREPLIED]` |
| 客户端 sidecar | `pending SYN local=10.80.1.2:49612 isn=1385062889`；`injected SYN-ACK ...`（3 次，对应重传） |
| 服务端 sidecar | `relayed SYN-ACK ... 49612 tcpseq=2695888196`（3 次） |
| 服务端代理 | **仅 listening，无 CONNECT** |
| 18 MiB | **未进行**（HTTP 救援已失败，无法进入 bulk 阶段） |

**C 救援 FAIL。**

### 5.4 C 的失败原因

- 客户端内核已接受注入的 SYN-ACK，TCP 进入 ESTABLISHED，并发出 ACK/DATA。
- router conntrack 仍是 `SYN_SENT`，因此这些 ACK/DATA 被标记为 `invalid`。
- C 的 `ct state invalid drop` 在 forward priority 0 命中并丢弃它们。
- 服务端从未收到 ACK/DATA，代理没有 CONNECT，客户端 curl 超时。
- 这是**严格 conntrack 状态检查**导致的失败，不是路由问题、不是 NAT、不是 TCP 状态机本身的问题。

> 说明：C 的前两次救援尝试分别因 harness 错误失败——第一次 key 路径写错（`LXT_LAB_KEY requires >= 32 bytes`），第二次 sidecar 在宿主 netns 启动（`[Errno 19] No such device`）。这两次不计入 C 的实验结果；第三次将 sidecar 放入 `lxtv8c` 后，才是有效的 C 救援。

---

## 6. V7-B 与 V8-B 的失败机制比较

| 项 | V7-B | V8-B |
|---|---|---|
| 拓扑 | 宿主 netns + host NAT | 三 netns，无 NAT |
| SYN-ACK 丢弃点 | PREROUTING raw -300 | PREROUTING raw -300 |
| conntrack | `SYN_SENT [UNREPLIED]` | `SYN_SENT [UNREPLIED]` |
| 客户端 TCP | 本地 ESTABLISHED | 本地 ESTABLISHED |
| 后续 ACK/DATA | 源地址 10.200.7.2，NAT 未 SNAT，服务端未收到 | 源 10.80.1.2 / 目的 10.80.3.2，默认 forward 转发，服务端收到 |
| 结果 | **FAIL** | **PASS** |
| 根因 | NAT/SNAT 未对回程做转换 | conntrack invalid 但未被显式丢弃 |
| 18 MiB | 1 MiB smoke 失败 | 9/9，18,874,368 B，SHA OK |

**V7-B 的失败是 NAT 失败，不是 conntrack 状态检查失败。** V8 去掉 NAT 后，B 证明 conntrack 状态不匹配本身不会阻断数据；C 证明一旦中间设备显式丢弃 invalid，数据会被阻断。

---

## 7. 验收表

| 测试 | 结果 |
|---|---|
| 正常 TCP + 代理 | PASS |
| 正常 UDP HMAC | PASS |
| 正常 18 MiB + SHA-256 | PASS |
| A 对照（普通 TCP 超时） | PASS（5.000967 s） |
| A 救援（UDP 注入 + 原生 TCP） | PASS |
| A 18 MiB + SHA-256 | PASS（9/9） |
| B 对照（普通 TCP 超时） | PASS（5.001436 s） |
| B 救援（conntrack 未见 SYN-ACK） | PASS |
| B 18 MiB + SHA-256 | PASS（9/9） |
| C 对照（普通 TCP 超时） | PASS（5.001416 s） |
| C 救援（严格 invalid drop） | FAIL（10 pkt / 1192 B 被丢） |
| C 18 MiB | N/A（HTTP 救援已失败） |
| 临时 netns/nft/进程/密钥清理 | PASS |
| 真实第三方中间盒 | NOT TESTED |

---

## 8. 结论与架构判断

### 8.1 核心问题的答案

**完全去掉 NAT、只在 conntrack 之前丢弃 SYN-ACK，UDP 救援后的原生 TCP 可以正常传输数据——前提是中间设备不执行严格的 `ct state invalid drop`。**

- V8-B：可以。conntrack 全程 `SYN_SENT`，ACK/DATA 被标记 invalid，但默认 forward 放行；18 MiB 双向数据与 SHA-256 成功。
- V8-C：不可以。显式 `ct state invalid drop` 会丢弃客户端 ACK/DATA，连接在服务端看来从未建立。

### 8.2 是否值得针对 conntrack 修改 LXT 架构

- **不需要为“B 类中间盒”改架构。** B 的结果说明 LXT 的 UDP 救援与原生 TCP 数据路径在无 NAT、无严格 invalid 过滤的环境下成立。
- **需要为“C 类中间盒”增加部署约束或兼容模式。** 如果中间设备启用 strict conntrack invalid 过滤，单靠 UDP 注入 SYN-ACK 无法让数据通过；这不是 LXT 能绕过的问题，除非中间设备被配置为放行，或 LXT 增加与中间盒协同的机制。
- 建议在 LXT 部署前做**中间盒兼容性探测**：
  1. 是否存在 SYN-ACK 阻断；
  2. 是否放行 invalid ACK/DATA；
  3. 是否存在 NAT/SNAT；
  4. 是否有严格 conntrack 策略。
- 不建议为了 C 类场景修改 LXT 的核心传输架构；应将其记录为部署限制，并在产品层提供检测/告警。

### 8.3 适用边界

V8 是隔离 netns 实验，不是真实第三方中间盒。它证明的是**机制在无 NAT、可放行 invalid 的三层路径上成立**；不能外推为“所有公网中间盒都能绕过”。真实中间盒、云安全组、运营商 ACL 仍需单独测试。

---

## 9. 遗留问题

1. **真实第三方中间盒未测**：V8 的 router 是本地 netns，规则由我们控制；真实设备可能同时做 NAT、strict conntrack、SYN-ACK 过滤、MSS clamp。
2. **C 的 18 MiB 未执行**：C 在 HTTP 救援阶段已失败，无法进入 bulk；这是预期失败，不是遗漏。
3. **C 的前两次 harness 失败**：key 路径、sidecar netns 放置；已在报告中区分，不计入实验结果。
4. **MSS 1332 vs 1460**：V5/V6 已发现注入 SYN-ACK 的 MSS 1460 绕过路径 clamp 1332；V8 的隔离路径 MTU 1500，未触发 PMTU 问题，但真实路径仍需 clamp 验证。
5. **代理无认证/ACL/限流/IPv6**：V6 代理是实验代码，不是生产代理。
6. **V7/V4 旧 PSK 曾在会话记录中暴露**：建议轮换；V8 使用的新临时 PSK 已删除。
7. **抓包 pcap 不完整**：A 阶段的 tcpdump 因缓冲/超时未取到有效包；nft counter、conntrack、sidecar/proxy 日志作为主要证据。V8-B/C 的 nft counter 与 conntrack 足以定位机制。

---

## 10. 清理结果

- 客户端 VPS：
  - 停止 client/server sidecar、proxy、bulk server；
  - 删除 `lxtv8c` / `lxtv8r` / `lxtv8s` 三个 netns；
  - 删除 `v8c0` / `v8r0` / `v8r1` / `v8s0` veth；
  - 删除 `lxt_v8_obs` / `lxt_v8_ct` / `lxt_v8_a` / `lxt_v8_b` / `lxt_v8_c`；
  - 恢复 `net.ipv4.ip_forward=0`；
  - 删除 `/tmp/lxt8/key`、`/root/lxt_v8`；
  - 宿主 nft ruleset 为空；39090/39091 无监听；无 lxt 进程；
  - 既有 nginx 443/8880、sing-box 53543/8881 正常。
- 远程 `relay.xlei.cc`：V8 未使用，未修改 SSH、iptables、默认路由、SELinux、既有业务。
- 本地证据：`/Users/lingxifox/Documents/deepseek-harness/default-workspace/lxt_v8_no_nat/logs/`（client/server sidecar、proxy、bulk、normal/A/B/C 日志）。
- 清理 PASS。

---

## 11. 证据文件

- 本报告：`/Users/lingxifox/Documents/deepseek-harness/default-workspace/lxt_v8_no_nat/LXT_V8_NO_NAT_REPORT.md`
- 日志目录：`/Users/lingxifox/Documents/deepseek-harness/default-workspace/lxt_v8_no_nat/logs/`
  - `bulk-normal.log`、`bulk-a.log`、`bulk-b.log`
  - `client-sidecar-baseline.log`、`client-sidecar-a.log`、`client-sidecar-b.log`
  - `client-c3.log`（有效 C 救援）
  - `server-sidecar.log`
  - `proxy-a.log`、`proxy-b.log`、`proxy-c.log`
  - `v5-normal.log`、`v5-a.log`、`v5-b.log`

---

## 12. 一句话总结

**V8-B 证明：没有 NAT、只在 conntrack 前丢 SYN-ACK 时，UDP 救援后的原生 TCP 可以完成 18 MiB 双向传输；V8-C 证明：一旦中间设备严格丢弃 conntrack invalid，数据路径会被切断。LXT 不需要为前者改架构，但必须把后者作为部署限制。**
