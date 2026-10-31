# 实验代码说明

本目录是 LXT UDP 辅助 TCP 握手实验的 Python 归档参考，不是 `fuckipv6` 主工程的运行时代码。正式实现已位于 `crates/client/src/lxt_rescue/`，通过 `fuckipv6 lxt-rescue server|client` 提供；本目录用于复现和对照实验，Rust 构建不依赖它。所有实验只在受控 Linux 实验机上进行。

```text
code/
├── lxt_public_synack_v4/   # 核心 sidecar + echo + probe + 阻断脚本
├── lxt_v5_bulk_test/       # 18 MiB 双向分块负载
└── lxt_v6_google_proxy/    # 最小 HTTP CONNECT 代理
```

---

## 1. `lxt_public_synack_v4/lxt_public_lab.py`

四种模式：

| 模式 | 作用 | 关键参数 |
|---|---|---|
| `server-sidecar` | 抓本机出站 SYN-ACK，UDP 转发给已认证 peer | `--interface --bind --tcp-port --udp-port` |
| `client-sidecar` | 抓出站 SYN，收 UDP SYN-ACK，注入 TUN | `--interface --tun --server --tcp-port --udp-port --hello-interval` |
| `echo-server` | 普通 TCP echo，按 `--close-after` 关闭 | `--bind --tcp-port --close-after` |
| `probe` | 客户端消息探测/重连 | `--server --tcp-port --count --interval --timeout --retries` |

### 协议常量

- `MAGIC = b"LXTPUB4"`
- `HELLO=1`, `HELLO_ACK=2`, `SYNACK=3`
- `TAG_LEN=16`
- `MAX_UDP=4096`

### 关键函数

| 函数 | 行号 | 说明 |
|---|---:|---|
| `load_key()` | 37 | 从 `LXT_LAB_KEY` 读 hex；至少 32 字节 |
| `mac_msg()` | 48 | 构造 `MAGIC/type/timestamp/nonce/body_len/body/tag` |
| `check_msg()` | 55 | 校验长度、MAGIC、±30s、HMAC、nonce 重放 |
| `checksum()` | 78 | 16-bit 反码和 |
| `fix_checksums()` | 87 | 重算 IPv4 头 + TCP 伪头校验和 |
| `tcp_ipv4()` | 101 | 解析 14B Ethernet + IPv4 + TCP |
| `packet_socket()` | 117 | AF_PACKET RAW，绑定网卡 |
| `open_tun()` | 123 | `/dev/net/tun`，`IFF_TUN \| IFF_NO_PI` |
| `run_server_sidecar()` | 136 | 只处理 `PACKET_OUTGOING`，`sport==tcp_port`，`flags & 0x17 == 0x12` |
| `run_client_sidecar()` | 185 | 捕获出站 SYN，`ack == isn+1` 匹配，改写 + 注入 |
| `run_echo_server()` | 261 | TCP echo |
| `run_probe()` | 294 | 消息探测/重连 |
| `parser()` | 343 | 子命令/参数 |

### 已修复的关键 Bug

`tcp_ipv4()` 需要 `frame[12:14] == b"\x08\x00"`。TUN 读取的是裸 IP，客户端注入前必须包装：

```python
# 错误：EtherType = 0x0000，tcp_ipv4() 返回 None
packet = b"\0" * 14 + payload

# 正确
packet = b"\0" * 12 + b"\x08\x00" + payload
```

没有这个修复时，日志会显示 `pending SYN`、服务端 `relayed SYN-ACK`，但客户端没有 `injected`，救援静默失败。

### 安全

- key 只从环境变量读取；不要写进 argv、README、日志或 Git。
- 服务端 sidecar 需要 RAW 权限；客户端还需要 `/dev/net/tun` + `CAP_NET_ADMIN`。
- 不在生产服务器上改 SSH、默认路由、iptables/firewalld、SELinux。

---

## 2. `lxt_public_synack_v4/client_drop_nft.sh`

```bash
./client_drop_nft.sh start SERVER_IP NIC [TTL]
./client_drop_nft.sh stop
./client_drop_nft.sh status
```

只创建 `table inet lxt_v4` + `input` 链 + 单条：

```text
iifname "<NIC>" ip saddr <SERVER_IP> tcp sport <PORT> \
  tcp flags syn,ack / syn,ack counter drop
```

`TTL` 到期自动删表；`stop` 删表并杀定时器。原始 `client_drop_once.sh` 是 iptables 版本，保留作对照。

`phase4_client.sh`：

- `full`：对照 + 救援；
- `rescue`：只救援；
- 从 stdin 读 key；
- `trap` 清理 sidecar / nft / 定时器。

---

## 3. `lxt_v5_bulk_test/lxt_bulk_probe.py`

```bash
# 服务端
python3 lxt_bulk_probe.py server --bind 0.0.0.0 --port 39090 --rate-mbps 10

# 客户端
python3 lxt_bulk_probe.py client --server SERVER_IP --port 39090 \
  --up-mib 1 --down-mib 1 --connections 3 --rounds 3 \
  --mode cycle --rate-mbps 10
```

- `MAGIC = b"LXTBULK1"`
- `MAX_BYTES_PER_DIRECTION = 64 MiB`
- 模式：`tiny` / `mixed` / `large` / `cycle`
- 每轮双向 SHA-256；`--connections` 新建 socket，`--rounds` 在同一连接复用
- `--rate-mbps` 是发送限速，不是压力测试

---

## 4. `lxt_v6_google_proxy/lxt_http_proxy.py`

- 最小 stdlib HTTP CONNECT 代理；`socketserver.ThreadingTCPServer`。
- 默认 `--bind 0.0.0.0 --port 39090`；`--connect-timeout 10`；`--read-timeout 15`。
- 解析 `CONNECT host:port`，IPv4 upstream，返回 `HTTP/1.1 200 Connection Established` + `Proxy-Agent: lxt-v6`。
- 双向 `pump()`，每连接一个线程；非 CONNECT 返回 405。
- 只用于实验；无认证、无 ACL、无限流、无 IPv6。

---

## 5. V7/V8

V7/V8 没有额外 Python 包装；使用：

- V4 sidecar；
- V6 代理；
- V5 bulk；
- 报告中的 nft/netns 命令。

V7/V8 的规则、拓扑、证据见：

- [`../reports/LXT_V7_MIDDLEBOX_REPORT.md`](../reports/LXT_V7_MIDDLEBOX_REPORT.md)
- [`../reports/LXT_V8_NO_NAT_REPORT.md`](../reports/LXT_V8_NO_NAT_REPORT.md)

---

## 6. 离线检查

```bash
cd lxt_public_synack_v4
python3 -m py_compile lxt_public_lab.py
python3 test_offline.py
```

`test_offline.py` 只做无网络特权的静态/单元检查；实际 AF_PACKET/TUN 行为必须在 Linux root 环境测试。
