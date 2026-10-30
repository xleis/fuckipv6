# LXT 实验：UDP 辅助 TCP 握手（V4–V8）

本目录收录 LingXiTunnel（LXT）实验性传输机制的**科目说明、实验代码、实现细节、测试报告与证据日志**。

> 实验目的：验证在客户端正常 TCP 路径收不到 SYN-ACK 时，能否由服务端内核产生的真实 SYN-ACK 经 HMAC 认证的 UDP 通道送到客户端，注入 Linux TUN/TCP 栈，让**后续业务数据继续走原生 TCP**。
>
> 这不是 FakeTCP，也不是把 TCP 数据封装进 UDP。

---

## 1. 八个测试科目

这八个科目来自 V4 公网验收表，是整条实验链路的基础检查项。V5–V8 是在此之上的扩展验证，不是另外八个科目。

| # | 科目 | 验证目标 | V4 结果 | 详细 |
|---|---|---|---|---|
| 01 | 公网正常 TCP | 无阻断时 TCP 能正常建连、收发 | PASS：6/6 消息，3 连接 | [01_public_tcp](subjects/01_public_tcp/README.md) |
| 02 | UDP HMAC 认证 | HELLO/HELLO_ACK/SYNACK 的 HMAC、时间戳、nonce 校验 | PASS（正向） | [02_udp_hmac](subjects/02_udp_hmac/README.md) |
| 03 | SYN-ACK 阻断对照组 | 客户端临时 DROP SYN-ACK 后标准 TCP 必须超时 | PASS：timeout，nft 3 包/180 B | [03_synack_block_control](subjects/03_synack_block_control/README.md) |
| 04 | UDP 辅助 TCP 建连 | 阻断保持时，UDP 转送 SYN-ACK 并注入 TUN，TCP ESTABLISHED | PASS：4 次救援 | [04_udp_assisted_connect](subjects/04_udp_assisted_connect/README.md) |
| 05 | TCP 数据双向传输 | 建连后数据走原生 TCP，不经过 UDP 封装 | PASS：18/18 消息 | [05_bidirectional_data](subjects/05_bidirectional_data/README.md) |
| 06 | TCP 长连接复用 | 同一连接连续多条消息，不重新建连 | PASS：6/6，同一连接 | [06_long_connection_reuse](subjects/06_long_connection_reuse/README.md) |
| 07 | 断线后再次救援 | 服务端关闭后客户端重连，再次完成 UDP 救援 | PASS：3 次重连救援 | [07_reconnect_rescue](subjects/07_reconnect_rescue/README.md) |
| 08 | 临时防火墙规则清理 | 阻断规则、TUN、sidecar、定时器全部清理 | PASS | [08_firewall_cleanup](subjects/08_firewall_cleanup/README.md) |

### 版本扩展

| 版本 | 扩展内容 | 结论 |
|---|---|---|
| V4 | 两台真实公网主机；8 项验收 | 8/8 PASS |
| V5 | 18 MiB 双向分块负载 + MSS 抓包诊断 | 正常/救援均 PASS；MSS 1332 vs 1460 为潜在风险 |
| V6 | 服务端 HTTP CONNECT 代理，客户端经代理真实访问 Google | 3 次救援全部 PASS；HTTP/2 200、204 |
| V7 | 宿主机 netns 模拟中间路由器；A=FORWARD 丢 SYN-ACK，B=PREROUTING raw 丢 SYN-ACK | A PASS；B FAIL（NAT/SNAT 未执行） |
| V8 | 三 netns、完全无 NAT；A/B/C 三组 | A PASS；B PASS；C FAIL（严格 `ct state invalid drop`） |

---

## 2. 目录结构

```text
experimental/
├── README.md                 # 本文件：总览、八科目、版本、导航
├── subjects/                 # 八个科目的内容/实现/证据/报告索引
│   ├── 01_public_tcp/
│   ├── 02_udp_hmac/
│   ├── 03_synack_block_control/
│   ├── 04_udp_assisted_connect/
│   ├── 05_bidirectional_data/
│   ├── 06_long_connection_reuse/
│   ├── 07_reconnect_rescue/
│   └── 08_firewall_cleanup/
├── code/
│   ├── README.md             # 代码入口与模式说明
│   ├── lxt_public_synack_v4/ # 核心 sidecar / echo / probe / 阻断脚本
│   ├── lxt_v5_bulk_test/     # 18 MiB 分块负载工具
│   └── lxt_v6_google_proxy/  # HTTP CONNECT 代理
├── reports/                  # V4–V8 五份独立报告
│   ├── LXT_V4_PUBLIC_TEST_REPORT.md
│   ├── LXT_V5_BULK_TEST_REPORT.md
│   ├── LXT_V6_PROXY_GOOGLE_REPORT.md
│   ├── LXT_V7_MIDDLEBOX_REPORT.md
│   └── LXT_V8_NO_NAT_REPORT.md
└── evidence/
    ├── README.md             # 证据文件说明
    ├── v5/logs/
    ├── v6/logs/
    ├── v7/logs/
    └── v8/logs/
```

V4 的证据主要内嵌在 `reports/LXT_V4_PUBLIC_TEST_REPORT.md`；V5–V8 的日志在 `evidence/`。V7/V8 的 netns/nft 规则以报告中的命令为准，没有额外包装脚本。

---

## 3. 代码入口

| 文件 | 作用 |
|---|---|
| `code/lxt_public_synack_v4/lxt_public_lab.py` | 四种模式：`server-sidecar`、`client-sidecar`、`echo-server`、`probe`；AF_PACKET + TUN；HMAC-SHA256 |
| `code/lxt_public_synack_v4/client_drop_nft.sh` | 客户端 nft 临时阻断：`start SERVER_IP NIC [TTL]` / `stop` / `status` |
| `code/lxt_public_synack_v4/client_drop_once.sh` | 原始 iptables 一次性阻断脚本（V4 ZIP 自带） |
| `code/lxt_public_synack_v4/phase4_client.sh` | 对照 + 救援的客户端编排；stdin 读 key；trap 清理 |
| `code/lxt_public_synack_v4/test_offline.py` | 无网络特权的离线静态/单元检查 |
| `code/lxt_v5_bulk_test/lxt_bulk_probe.py` | 18 MiB 双向随机分块 + SHA-256；`server` / `client` |
| `code/lxt_v6_google_proxy/lxt_http_proxy.py` | 最小 HTTP CONNECT 代理；线程/连接；双向隧道 |

常用命令：

```bash
# 服务端 echo（每 2 条关闭，用于重连测试）
python3 lxt_public_lab.py echo-server --bind 0.0.0.0 --tcp-port 39090 --close-after 2

# 服务端 sidecar（需要 root/RAW/TUN 权限；key 只从环境变量读）
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py server-sidecar \
  --interface eth0 --tcp-port 39090 --udp-port 39091

# 客户端 sidecar
sudo env LXT_LAB_KEY="$LXT_LAB_KEY" python3 lxt_public_lab.py client-sidecar \
  --interface eth0 --server 198.51.100.10 --tcp-port 39090 --udp-port 39091

# 正常探测 / 阻断对照
python3 lxt_public_lab.py probe --server 198.51.100.10 --count 6 --timeout 4

# 18 MiB 双向
python3 lxt_bulk_probe.py client --server 198.51.100.10 --port 39090 \
  --up-mib 1 --down-mib 1 --connections 3 --rounds 3 --mode cycle --rate-mbps 10
```

---

## 4. 核心实现要点

1. **协议**：`MAGIC="LXTPUB4"`，类型 `HELLO=1`、`HELLO_ACK=2`、`SYNACK=3`；报文 = `MAGIC | type | u64 timestamp | 12B nonce | u16 body_len | body | 16B tag`；tag 为 HMAC-SHA256 前 16 字节；时间戳 ±30 秒；nonce 重放表。
2. **服务端捕获**：AF_PACKET `ETH_P_ALL`，只处理 `PACKET_OUTGOING`；`sport == tcp_port` 且 `flags & 0x17 == 0x12`；按目的 IP 匹配 8 秒内认证过的 HELLO peer；0.25 秒去重；UDP 39091 转发。
3. **客户端注入**：捕获出站 SYN（`flags & 0x17 == 0x02`），记录 `(src, sport, isn)`；收到 SYNACK 后要求 `(isn + 1) & 0xffffffff == ack`；把目的 IP/端口改成本地连接，重算校验和，写入 TUN `lxtrescue0`。
4. **关键修复**：`tcp_ipv4()` 需要 14 字节 Ethernet 头且 `frame[12:14] == b"\x08\x00"`；TUN 读到的是裸 IP，必须用 `b"\0" * 12 + b"\x08\x00" + payload` 包装。V4 早期用 `b"\0" * 14` 导致 EtherType=0，救援包被静默丢弃。
5. **数据面**：UDP 只承载 HELLO/HELLO_ACK/SYNACK；握手完成后，echo / bulk / HTTP 数据全部走原生 TCP。

---

## 5. 报告与证据

| 报告 | 内容 |
|---|---|
| [V4 公网报告](reports/LXT_V4_PUBLIC_TEST_REPORT.md) | 八科目验收、Phase 3/4、seq/ack、清理 |
| [V5 bulk 报告](reports/LXT_V5_BULK_TEST_REPORT.md) | 18 MiB、MSS 1332/1460、nstat、SHA |
| [V6 代理报告](reports/LXT_V6_PROXY_GOOGLE_REPORT.md) | HTTP CONNECT、Google HTTP/2 200、204 |
| [V7 中间盒报告](reports/LXT_V7_MIDDLEBOX_REPORT.md) | A/B 两组、NAT 失败机制 |
| [V8 无 NAT 报告](reports/LXT_V8_NO_NAT_REPORT.md) | A/B/C、conntrack invalid、C 失败 |

证据日志：

- `evidence/v5/logs/`：normal / rescue / MSS / nstat / pcap 文本
- `evidence/v6/logs/`：client/server sidecar、proxy、eth0/tun SYN-ACK、Google headers/html、nft
- `evidence/v7/logs/`：A/B 的 client/server 日志、pcap、nft、nstat
- `evidence/v8/logs/`：normal/A/B/C 的 sidecar、proxy、bulk 日志

---

## 6. 结论摘要

- **V4**：8/8 PASS，机制在真实公网路径上成立；UDP 只负责握手救援，数据仍走原生 TCP。
- **V5**：18 MiB 正常/救援均 PASS；MSS 1332 vs 1460 是潜在 PMTU 风险，不是本次失败。
- **V6**：通过服务端 HTTP CONNECT 代理真实访问 Google；3 次救援全部成功。
- **V7**：A（FORWARD 丢 SYN-ACK）PASS；B（PREROUTING raw 丢 SYN-ACK）FAIL，原因是 NAT/SNAT 未执行，不是 conntrack 本身。
- **V8**：去掉 NAT 后，B PASS（conntrack 停在 `SYN_SENT`，invalid ACK/DATA 仍被默认 forward 放行，18 MiB PASS）；C（严格 `ct state invalid drop`）FAIL。

**LXT 可作为握手救援/bootstrap 模块；不是通用传输，也不是生产级代理。真实第三方中间盒仍需单独测试。**

---

## 7. 安全与边界

- 本目录不包含任何真实预共享密钥（PSK）。所有命令使用 `LXT_LAB_KEY` 环境变量占位；测试密钥应使用 `openssl rand -hex 32` 生成，且不要提交 Git/写入公开日志。
- 阻断规则只应在**客户端实验机**上临时创建，必须带 TTL/trap 自动清理；不要对朋友的服务器改 SSH、默认路由、iptables/firewalld 或既有业务。
- 仅 IPv4；sidecar 需要 root/RAW/TUN；peer map 目前只按源 IP；MSS 仍建议做路径 clamp；代理无认证/ACL/限流。
- 本目录是实验归档，不是生产代码；`fuckipv6` 主工程未修改。
