# 证据文件说明

本目录保存 V5–V8 的原始日志/抓包文本。V4 的关键证据直接内嵌在报告中（seq/ack、nft counter、sidecar 日志摘录），因此没有单独复制 V4 日志目录。

```text
evidence/
├── v5/logs/   # 18 MiB 正常/救援、MSS、nstat、SYN 抓包文本
├── v6/logs/   # Google 代理、client/server sidecar、eth0/tun SYN-ACK、nft
├── v7/logs/   # A/B 两组、client/server、pcap、nft、nstat
└── v8/logs/   # normal/A/B/C、sidecar、proxy、bulk
```

## V5

| 文件 | 内容 |
|---|---|
| `lxt-v5-bulk-normal.log` | 正常 18 MiB，9/9 |
| `lxt-v5-bulk-rescue-nomss.log` | 救援 18 MiB（未加 MSS clamp），9/9 |
| `lxt-v5-bulk-server.log` | 服务端 accepted/validated |
| `lxt-v5-client-sidecar.log` | 客户端 pending/injected |
| `lxt-v5-server-sidecar.log` | 服务端 relayed SYN-ACK |
| `lxt-v5-normal-syn.txt` | 正常路径 SYN/SYN-ACK MSS |
| `lxt-v5-rescue-syn-eth0.txt` | 救援路径 eth0 真实 SYN-ACK（MSS 1332） |
| `lxt-v5-rescue-syn-tun.txt` | TUN 注入 SYN-ACK（MSS 1460） |
| `lxt-v5-rescue-ss.log` | `ss -ti`：`mss:1448 pmtu:1500 rcvmss:1320` |
| `lxt-v5-nstat-before.txt` / `-after.txt` | 重传/超时计数 |
| `lxt-v5-mss-server.txt` / `lxt-v5-mss-client.log` | 2 MiB MSS 诊断 |

## V6

| 文件 | 内容 |
|---|---|
| `lxt-v6-client-sidecar.log` | HELLO_ACK、pending/injected |
| `lxt-v6-server-sidecar.log` | HELLO verified、relayed |
| `lxt-v6-proxy.log` | CONNECT、upstream、tunnel closed |
| `lxt-v6-eth0-synack.txt` | eth0 真实 SYN-ACK（MSS 1332） |
| `lxt-v6-tun-synack.txt` | TUN 注入 SYN-ACK（MSS 1460） |
| `lxt-v6-google.headers` / `.html` | HTTP/2 200、server: gws、`<title>Google` |
| `lxt-v6-nft-final.txt` | nft 最终计数 |

## V7

`v7/logs/client/` 与 `v7/logs/server/` 保存 A/B 两组的：

- client/server sidecar、proxy、bulk 日志；
- `a-control-*.pcap`、`a-rescue-*.pcap`、`b-control-*.pcap`、`b-rescue-*.pcap`；
- nft 规则、nstat before/after、Google headers/html；
- 最终清理检查。

注意：V7-B 的失败是 NAT/SNAT 未执行；证据见 `b-rescue-eth0.pcap` 与报告第 6 节。

## V8

`v8/logs/` 保存：

- `bulk-normal.log`、`bulk-a.log`、`bulk-b.log`；
- `client-sidecar-baseline.log`、`client-sidecar-a.log`、`client-sidecar-b.log`；
- `client-c3.log`（有效 C 救援）、`client-c.log` / `client-c2.log`（harness 错误，保留以说明排除过程）；
- `server-sidecar.log`、`proxy-a/b/c.log`；
- `v5-normal.log`、`v5-a.log`、`v5-b.log`。

V8 的关键 nft/conntrack 计数（如 C 的 `10 packets/1192 B ct state invalid drop`）记录在报告第 4–5 节；日志本身不包含 nft 输出。

## 安全

- 这些日志不应包含 PSK；如发现任何 `LXT_LAB_KEY=<值>`，不要提交，先脱敏。
- 日志中包含实验机 IP、端口、序列号；仅用于实验归档，不要当作生产配置。
- V7/V8 的 pcap 可能包含测试 HTTP 内容；不包含真实用户数据。
