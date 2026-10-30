# LXT V6 — 经 UDP 辅助握手访问 Google（HTTP CONNECT 代理）公网测试报告

- 日期：2026-10-30（UTC；客户端本地时区 Asia/Shanghai）
- 测试目标：客户端正常 TCP 路径收不到 SYN-ACK 时，用已认证 UDP 把服务端内核真实 SYN-ACK 送到客户端，经 TUN 注入后让原生 TCP 连接 ESTABLISHED；随后通过这条原生 TCP 连接访问服务端的 HTTP CONNECT 代理，并由代理真实访问 `https://www.google.com`。
- 实验范围：V6 独立测试；复用 V4 的 `lxt_public_lab.py` sidecar，新增一个最小 HTTP CONNECT 代理作为服务端出口。
- 结论：**PASS（实验范围内）**。三次真实 HTTPS 请求（Google 首页 1 次、`generate_204` 2 次）都经 UDP 救援完成 TCP 握手，客户端拿到 HTTP/2 200 / 204；三次真实 SYN-ACK 全部被客户端 nft 丢弃，三次注入 SYN-ACK 全部被内核接受；测试后规则、TUN、sidecar、代理全部清理。
- 重要边界：SYN-ACK 阻断是**客户端本地 nft**，不是真实中间设备。本报告证明“客户端收不到 SYN-ACK 时 UDP 救援可用”，**不证明**“穿透了真实公网中间设备的 SYN-ACK 封锁”。

## 1. 两端环境

| 项目 | 服务端 | 客户端 |
|---|---|---|
| 主机 | `relay.xlei.cc`（`vm521838.akkocloud.com`） | `111.91.22.14` |
| 系统 | CentOS Stream 8 | Debian 12 (bookworm) |
| 内核 | `5.15.202-1.el8.elrepo.x86_64` | `6.1.0-35-amd64` |
| 架构 | x86_64 | x86_64 |
| Python | 3.9.19（`python3.9`） | 3.11.2（`python3`） |
| 网卡 / 地址 | `eth0` `185.217.110.15/24`（公网 IP 直接在网卡） | `eth0` `172.31.0.173/20`，公网出口 `111.91.22.14`（1:1 NAT/EIP） |
| 默认路由 | `185.217.110.1` dev eth0 | `172.31.0.1` dev eth0 |
| rp_filter | all=1 / default=0 | all=0 / default=0 |
| 防火墙 | firewalld 未安装/未启用；iptables 既有 `FKV6F-99E15423` 与 `fuckipv6` 规则，未改动 | 无 iptables；使用 nft，测试前 ruleset 为空 |
| SELinux | Disabled（未改动） | 不适用 |
| curl | 7.61.1 | 7.88.1 |
| 其他 | 另有 `fuckipv6` 10.8.0.1/24，未触碰 | 既有 nginx（443/8880）与 sing-box（53543/8881），未触碰 |

服务端到 Google 的前置检查：

```text
getent ahostsv4 www.google.com
142.251.150.119 STREAM www.google.com
142.251.156.119 STREAM www.google.com

curl -4 -sS -I https://www.google.com
HTTP/2 200
server: gws
date: Fri, 30 Oct 2026 09:56:10 GMT
```

客户端前置检查：curl 7.88.1；CA `/etc/ssl/certs/ca-certificates.crt` 存在；无 `http_proxy`/`https_proxy`/`all_proxy` 环境变量；39090/39091 空闲。

## 2. 设计与数据路径

```text
curl --proxy http://185.217.110.15:39090 https://www.google.com/
        |
        | 1. client TCP SYN -> 185.217.110.15:39090
        v
  client nft: drop SYN-ACK from 185.217.110.15:39090
        |
        | 2. 服务端内核产生真实 SYN-ACK（sport 39090）
        v
  server-sidecar (AF_PACKET, PACKET_OUTGOING)
        |
        | 3. 认证 UDP 39091：SYN-ACK + HMAC + nonce
        v
  client-sidecar：HMAC 校验、ISN+1 匹配、改目的地址/端口、fix checksum
        |
        | 4. 写入 TUN lxtrescue0
        v
  Linux TCP 栈收到 SYN-ACK -> ESTABLISHED
        |
        | 5. 之后全部是原生 TCP（TLS 到 Google 的端到端连接）
        v
  server HTTP CONNECT proxy（TCP 39090）
        |
        | 6. 代理自己用正常公网 TCP 连 Google 443
        v
  https://www.google.com
```

关键点：

- 代理只做 HTTP CONNECT 隧道，**不做 TLS 中间人**。客户端的 TLS 会话是端到端到 Google 的；代理看到的是 CONNECT 行和之后的加密字节。
- UDP 只承担“把服务端真实 SYN-ACK 送回去”的救援；业务数据全部走原生 TCP，不是 FakeTCP，也不是 TCP-over-UDP。
- 服务端代理监听 39090；V4 server-sidecar 只过滤 `sport == 39090` 的 SYN-ACK，因此代理到 Google 的 outbound 连接（sport 为临时端口）不会被误转发。
- 客户端 nft 规则只丢弃来自 `185.217.110.15:39090` 的 SYN+ACK，不影响其他流量。

## 3. 执行与证据

### 3.1 对照组：无客户端 sidecar，阻断生效

客户端先启动 nft 丢弃规则：

```bash
cd /root/lxt_public_synack_v4
./client_drop_nft.sh start 185.217.110.15 eth0 600
```

规则：

```text
table inet lxt_v4 {
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "eth0" ip saddr 185.217.110.15 tcp sport 39090 \
      tcp flags syn,ack / syn,ack counter packets 0 bytes 0 drop
  }
}
```

没有客户端 sidecar 时执行：

```bash
curl --proxy http://185.217.110.15:39090 --noproxy "" \
  --connect-timeout 5 -m 8 -sS -o /dev/null \
  -w 'HTTP=%{http_code} time=%{time_total}\n' \
  https://www.google.com/
```

结果（`CURL_EXIT=28`）：

```text
2026-10-30T09:58:38Z
curl: (28) Failed to connect to 185.217.110.15 port 39090 after 5001 ms: Timeout was reached
HTTP=000 time=5.001361 err=Failed to connect to 185.217.110.15 port 39090 after 5001 ms: Timeout was reached
CURL_EXIT=28
nft: packets 4 bytes 240 drop
no_client_sidecar
```

结论：服务端代理在监听，但客户端收不到 SYN-ACK，标准 `connect()` 超时；没有 UDP 救援，连接无法建立。控制阶段共丢弃 4 个 SYN-ACK（随后服务端一次重传使首个救援前的计数变为 5）。

### 3.2 救援组：UDP 救援 + 原生 TCP 访问 Google

客户端 sidecar 启动后：

```text
[client-sidecar] iface=eth0 TUN=lxtrescue0 server=185.217.110.15 UDP=44162 -> 39091
[client-sidecar] HELLO_ACK verified
lxtrescue0       UNKNOWN        <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP>
```

客户端 sidecar 的 UDP 本地端口为 44162；服务端 sidecar 日志确认：

```text
[server-sidecar] iface=eth0 UDP=0.0.0.0:39091 TCP sport=39090; waiting for authenticated HELLO
[server-sidecar] HELLO verified from 111.91.22.14:44162
[server-sidecar] relayed SYN-ACK 185.217.110.15:39090 -> 111.91.22.14:37588 tcpseq=1057256392 via UDP -> 111.91.22.14:44162
[server-sidecar] relayed SYN-ACK 185.217.110.15:39090 -> 111.91.22.14:42278 tcpseq=2532541571 via UDP -> 111.91.22.14:44162
[server-sidecar] relayed SYN-ACK 185.217.110.15:39090 -> 111.91.22.14:41518 tcpseq=2018536621 via UDP -> 111.91.22.14:44162
```

三次请求结果：

| # | URL | curl 结果 | 客户端源端口 | 客户端 ISN | 服务端 SYN-ACK seq | SYN-ACK ack | 代理上游 Google | 请求后 nft 计数 |
|---|---|---|---|---|---|---|---|---|
| 1 | `https://www.google.com/` | HTTP 200，86,889 B，1.0495 s | 37588 | 3048840708 | 1057256392 | 3048840709 | 142.251.151.119:443 | 6 |
| 2 | `https://www.google.com/generate_204` | HTTP 204，0 B，1.1801 s | 42278 | 3394200295 | 2532541571 | 3394200296 | 142.251.153.119:443 | 7 |
| 3 | `https://www.google.com/generate_204` | HTTP 204，0 B，1.2847 s | 41518 | 3403452688 | 2018536621 | 3403452689 | 142.251.154.119:443 | 8 |

第 1/2 次的 `ack` 由 client-sidecar 的 `(isn+1)&0xffffffff == ack` 匹配条件和 `injected` 日志确认；第 3 次为 tcpdump 实测。三次 `curl` 都返回 `CURL_EXIT=0`；`connect` 时间为 0.1816 / 0.2504 / 0.2513 s，TLS `ssl` 时间为 0.5207 / 0.7171 / 0.7700 s。

第一次请求的 HTTP 头（代理 200 + Google HTTP/2 200）：

```text
HTTP/1.1 200 Connection Established
Proxy-Agent: lxt-v6

HTTP/2 200 
content-type: text/html; charset=ISO-8859-1
date: Fri, 30 Oct 2026 09:59:01 GMT
server: gws
...
```

响应体 86,889 字节，标题：

```html
<title>Google
```

代理日志（三次 CONNECT、上游连接、隧道字节数）：

```text
[proxy] listening on 0.0.0.0:39090
[proxy] CONNECT www.google.com:443 from 111.91.22.14:37588
[proxy] upstream connected www.google.com:443 via 142.251.151.119:443
[proxy] tunnel closed www.google.com:443 up=815 down=92907
[proxy] CONNECT www.google.com:443 from 111.91.22.14:42278
[proxy] upstream connected www.google.com:443 via 142.251.153.119:443
[proxy] tunnel closed www.google.com:443 up=825 down=3589
[proxy] CONNECT www.google.com:443 from 111.91.22.14:41518
[proxy] upstream connected www.google.com:443 via 142.251.154.119:443
[proxy] tunnel closed www.google.com:443 up=825 down=3588
```

### 3.3 关键证据：真实 SYN-ACK 被丢，注入 SYN-ACK 被接受

第三次请求（源端口 41518）在客户端同时抓 `eth0` 与 `lxtrescue0`：

```text
# lxtrescue0（注入的 SYN-ACK）
17:59:40.737510 IP 185.217.110.15.39090 > 172.31.0.173.41518: Flags [S.],
  seq 2018536621, ack 3403452689, win 65160,
  options [mss 1460,sackOK,TS val 2237257506 ecr 1107494080,nop,wscale 11], length 0

# eth0（公网上的真实 SYN-ACK）
17:59:40.739862 IP 185.217.110.15.39090 > 172.31.0.173.41518: Flags [S.],
  seq 2018536621, ack 3403452689, win 65160,
  options [mss 1332,sackOK,TS val 2237257506 ecr 1107494080,nop,wscale 11], length 0
```

- 注入包比公网真实 SYN-ACK **早 2.352 ms**（17:59:40.737510 vs .739862）。
- 两者 `seq=2018536621`、`ack=3403452689` 完全一致；客户端 ISN 为 3403452688，`ack = ISN+1`。
- 注入包 MSS **1460**，真实包 MSS **1332**；这是 V4/V5 已诊断的 in-path clamp 差异。
- 真实 SYN-ACK 到达后仍然命中 nft 丢弃规则；请求结束后计数变为：

```text
iifname "eth0" ip saddr 185.217.110.15 tcp sport 39090 \
  tcp flags syn,ack / syn,ack counter packets 8 bytes 480 drop
```

8 个计数 = 控制阶段 5 个（含重传）+ 三次救援各 1 个真实 SYN-ACK。三次连接都不是靠公网真实 SYN-ACK 完成的。

客户端 sidecar 对三次连接的注入记录：

```text
[client-sidecar] pending SYN local=172.31.0.173:37588 isn=3048840708
[client-sidecar] injected SYN-ACK to 172.31.0.173:37588 (60 bytes); NAT remote=111.91.22.14:37588
[client-sidecar] pending SYN local=172.31.0.173:42278 isn=3394200295
[client-sidecar] injected SYN-ACK to 172.31.0.173:42278 (60 bytes); NAT remote=111.91.22.14:42278
[client-sidecar] pending SYN local=172.31.0.173:41518 isn=3403452688
[client-sidecar] injected SYN-ACK to 172.31.0.173:41518 (60 bytes); NAT remote=111.91.22.14:41518
```

### 3.4 验收表（V6）

| 测试 | 结果 | 证据 |
|---|---|---|
| 服务端代理监听 TCP 39090 | PASS | `ss -lntp`，proxy `listening` |
| 服务端可正常访问 Google | PASS | `HTTP/2 200`，`server: gws` |
| 无 sidecar 对照超时 | PASS | `curl (28)`，5.001 s，nft 4/240 |
| UDP HMAC / HELLO | PASS | `HELLO_ACK verified`，服务端 `HELLO verified` |
| 服务端捕获真实 SYN-ACK | PASS | 3 条 `relayed SYN-ACK` |
| 客户端注入 TUN | PASS | 3 条 `injected`，TUN 抓包 |
| 客户端到代理 TCP 建连 | PASS | 3 次连接，connect 0.18–0.25 s |
| Google HTTPS 首页 | PASS | HTTP/2 200，86,889 B，标题 Google |
| Google `generate_204` | PASS | HTTP 204 ×2 |
| 连续/重复救援 | PASS | 3 个不同源端口、3 个不同 ISN |
| 数据双向传输 | PASS | 代理 `up/down` 字节数；TLS 端到端 |
| 临时规则清理 | PASS | nft 表、TUN、sidecar、代理全部移除 |
| 真实中间设备阻断 | **NOT TESTED** | 本次是客户端本地 nft |

## 4. 真实公网测量与人工阻断的区别

本次测试中“真实”的部分：

- 客户端到服务端走真实公网 TCP/UDP；客户端公网出口 `111.91.22.14` 与服务端看到的源地址一致。
- 服务端到 Google 是真实公网 TCP；代理实际连到 `142.251.151.119/153.119/154.119:443`。
- 客户端的 TLS 是真实端到端 TLS；CA 校验通过，Google 返回 HTTP/2 200/204。

本次测试中“人工”的部分：

- SYN-ACK 的丢弃发生在客户端本机 nft，不是云安全组、运营商或中间设备。
- 因此它只能证明：**在客户端确实收不到 SYN-ACK 的条件下，UDP 救援能让原生 TCP 完成握手**；不能证明：真实中间设备丢 SYN-ACK 时也能穿透。
- 真实中间设备测试需要单独构造：在客户端上游/云 ACL 丢弃来自服务端的 SYN-ACK，同时放行 UDP 39091，再重复本测试；这是下一阶段最重要的验证。

## 5. 遗留问题

1. **真实中间设备未测**：本地 nft 阻断不等于真实公网封锁；需要在云安全组/上游防火墙/真实链路上复现。
2. **代理是实验性最小实现**：仅 HTTP CONNECT；无认证、无 ACL、无连接数/速率限制、无 idle timeout；每连接一个线程；仅 IPv4；日志未做脱敏。不能直接当生产代理。
3. **MSS/PMTU 差异**：真实 SYN-ACK MSS 1332，注入 SYN-ACK MSS 1460；客户端有效发送 MSS 1448，而正常路径是 1320。V5 的 18 MiB 救援测试通过，但这是潜在 PMTU 风险；本次 V6 数据量小，没有触发。建议在注入时 clamp 到路径 MSS，并在 MTU < 1500 的路径复测。
4. **HELLO 日志噪声**：客户端 sidecar 反复打印 `HELLO_ACK verified`，服务端也反复记录 `HELLO verified`；不影响功能，但日志量偏大，建议确认 HELLO 重发周期并降噪。
5. **服务端出口依赖**：代理能否访问 Google 取决于服务端网络；若服务端出口被限制，本方案无法访问目标。测试环境可访问，但不是普遍保证。
6. **多客户端/NAT**：V4 的 peer map 只按目的 IP 匹配；同一 NAT 后多个客户端会冲突；V6 只有一个客户端。
7. **仅 IPv4**：代理和 sidecar 都只处理 IPv4；没有 IPv6 路径。
8. **密钥管理**：V6 使用独立 PSK，测试后已删除，未写入日志；V4 的旧 PSK 曾在会话记录中暴露，仍建议轮换。V6 日志已检查不含 `LXT_LAB_KEY`。
9. **客户端阻断脚本是 nft 版**：`client_drop_nft.sh` 适用于 Debian/nft；CentOS 客户端需要 iptables 版本。
10. **不是完整浏览器**：用 curl 验证了真实 HTTPS/TLS 和 HTTP/2，但没有验证浏览器子资源、并行连接、cookie、长连接复用等。

## 6. 是否具备扩展为 LXT 传输模块的条件

**具备“握手救援/bootstrap”条件，不具备“通用传输”条件。**

适合扩展的部分：

- 客户端到服务端的原生 TCP 被 SYN-ACK 阻断时，UDP 救援可以把真实 SYN-ACK 送回，随后业务数据直接走原生 TCP；这对 HTTP(S)、代理、SSH 等基于 TCP 的协议都适用。
- 服务端代理/业务进程不需要感知救援；救援只发生在握手阶段。
- 本次已经验证了真实 TLS 到 Google 的端到端数据，不只是 echo。

扩展前必须补的：

- 真实中间设备/云 ACL 阻断实验，而不是本地 nft。
- 代理安全加固（认证、ACL、限流、超时、IPv6、日志脱敏）。
- MSS clamp / PMTU 处理，覆盖 MTU < 1500 的路径。
- 多客户端 peer 映射、密钥轮换、健康检查、失败回退。
- 浏览器级测试（Chrome/Firefox）和大文件/长连接测试。

## 7. 清理与安全

已执行并验证：

- 客户端：停止 client-sidecar；删除 `lxtrescue0`；`client_drop_nft.sh stop` 删除 `table inet lxt_v4`；39090/39091 无监听；无 lxt 进程；临时日志与 key 文件删除。
- 服务端：停止 proxy 与 server-sidecar；39090/39091 无监听；无 lxt 进程；删除 `/root/lxt_v6` 与 `/tmp/lxt-v6-*`；未改动 iptables、默认路由、SSH、SELinux；`fuckipv6` 规则保持原样。
- 客户端既有 nginx（443/8880）与 sing-box（53543/8881）正常；服务端既有规则正常。
- V6 PSK 仅存在于本地 0600 临时文件，测试后删除；日志中未出现 `LXT_LAB_KEY`。
- 服务端 `firewall-cmd` 未安装；SELinux Disabled；SSH 仍为 22 端口、root 登录方式未改动。

清理后检查：

```text
client: no_lxt_procs; nft list tables -> empty; lxtrescue0 -> no_tun; 39090/39091 free
server: no_lxt_procs; 39090/39091 free; /root/lxt_v6 gone; /tmp/lxt-v6-* clean
server iptables: -P INPUT ACCEPT; -N FKV6F-99E15423; -A FORWARD -j FKV6F-99E15423; ... 未变
```

## 8. 复现命令（不含密钥）

服务端：

```bash
# 1) 启动最小 HTTP CONNECT 代理
python3.9 /root/lxt_v6/lxt_http_proxy.py --bind 0.0.0.0 --port 39090

# 2) 启动 V4 server-sidecar（PSK 从 stdin 读入，不写入 argv/日志）
cd /root/lxt_public_synack_v4
read -r K
( exec env LXT_LAB_KEY="$K" python3.9 lxt_public_lab.py server-sidecar \
    --interface eth0 --tcp-port 39090 --udp-port 39091 ) \
  >/tmp/lxt-v6-server-sidecar.log 2>&1 </dev/null &
```

客户端：

```bash
cd /root/lxt_public_synack_v4
./client_drop_nft.sh start 185.217.110.15 eth0 600

read -r K
( exec env LXT_LAB_KEY="$K" python3 lxt_public_lab.py client-sidecar \
    --interface eth0 --server 185.217.110.15 \
    --tcp-port 39090 --udp-port 39091 ) \
  >/tmp/lxt-v6-client-sidecar.log 2>&1 </dev/null &

curl --proxy http://185.217.110.15:39090 --noproxy "" \
  -sS -D /tmp/lxt-v6-google.headers -o /tmp/lxt-v6-google.html \
  -w 'HTTP=%{http_code} bytes=%{size_download} time=%{time_total}\n' \
  https://www.google.com/

./client_drop_nft.sh stop
```

## 9. 证据文件

- [最小 HTTP CONNECT 代理](../code/lxt_v6_google_proxy/lxt_http_proxy.py)
- [客户端 sidecar 日志](../evidence/v6/logs/lxt-v6-client-sidecar.log)
- [服务端 sidecar 日志](../evidence/v6/logs/lxt-v6-server-sidecar.log)
- [代理日志](../evidence/v6/logs/lxt-v6-proxy.log)
- [客户端 eth0 真实 SYN-ACK 抓包](../evidence/v6/logs/lxt-v6-eth0-synack.txt)
- [客户端 lxtrescue0 注入 SYN-ACK 抓包](../evidence/v6/logs/lxt-v6-tun-synack.txt)
- [Google 响应头](../evidence/v6/logs/lxt-v6-google.headers)
- [Google 首页 HTML](../evidence/v6/logs/lxt-v6-google.html)
- [nft 最终计数](../evidence/v6/logs/lxt-v6-nft-final.txt)

---

**最终判断**：V6 在真实公网 TCP/UDP 路径上，以“客户端本地丢弃 SYN-ACK”为人工故障，成功用 UDP 救援完成了到服务端 HTTP CONNECT 代理的原生 TCP 握手，并通过该连接真实访问 Google（HTTP/2 200 / 204）。三次连接、三次注入、三次真实 SYN-ACK 丢弃均有日志/抓包对应。该能力可以作为 LXT 的握手救援/bootstrap 模块；在真实中间设备验证、代理加固、MSS/PMTU 和多客户端处理完成前，不应把它当作通用传输或生产级翻墙方案。
