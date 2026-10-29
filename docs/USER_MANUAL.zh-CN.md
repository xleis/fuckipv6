# 用户手册

本文面向需要部署和使用 fuckipv6 的操作者。当前版本已完成本机端到端验证、依赖审计和基础压力测试；生产上线前仍建议补充真实跨主机/跨 NAT 验证和外部安全审计。

## 工作方式

fuckipv6 创建一个只承载 IPv6 的虚拟局域网。客户端通过协调服务器发现彼此并获得虚拟 IPv6 地址。实际数据优先通过 UDP P2P 发送；如果端点未验证、发送失败或连接质量不达标，客户端会优先通过协调服务器的 QUIC 中继转发，QUIC 中继不可用时再降级到 WebSocket 信令通道中继。

## 安装

从源码构建：

```bash
cargo build --release
```

生成的二进制：

- `target/release/fuckipv6`
- `target/release/fuckipv6-coordinator`

Linux 客户端需要 root 或 `CAP_NET_ADMIN` 创建 TUN 设备。macOS 客户端需要 sudo/root 创建 utun 并配置 IPv6 路由，默认 `tun.name = "fuckipv6"` 会由系统自动分配实际 `utunN` 接口。Windows 客户端需要管理员权限和 WinTun。

## 启动协调服务器

```bash
./target/release/fuckipv6-coordinator
```

默认端口：

- TCP `6666`: WebSocket 信令。
- TCP `8080`: Web 管理界面和 HTTP API。

可用参数：

```bash
./target/release/fuckipv6-coordinator --bind 0.0.0.0:6666 --webui-bind 0.0.0.0:8080
```

Web UI:

```text
http://<coordinator>:8080
```

## 管理客户端网络配置

保存一个网络：

```bash
fuckipv6 join example.com:6666 --name office
```

查看已保存网络：

```bash
fuckipv6 list
```

设置默认网络：

```bash
fuckipv6 default office
```

连接默认网络：

```bash
sudo fuckipv6 connect
```

连接指定网络：

```bash
sudo fuckipv6 connect office
```

临时连接但不保存配置：

```bash
sudo fuckipv6 connect example.com:6666 --temp
```

删除网络配置：

```bash
fuckipv6 leave office
```

## 传统配置文件启动

生成配置：

```bash
fuckipv6 init-config client.toml
```

编辑 `coordinator_url`：

```toml
coordinator_url = "ws://example.com:6666"
```

启动：

```bash
sudo fuckipv6 start --config client.toml
```

## 日常操作

查看运行状态：

```bash
sudo fuckipv6 status
```

查看对等节点：

```bash
sudo fuckipv6 peers
```

测试虚拟 IPv6 连通性：

```bash
ping6 fd00::1002
```

或：

```bash
fuckipv6 ping fd00::1002 --count 4
```

查看路由：

```bash
fuckipv6 route
```

查看日志：

```bash
fuckipv6 logs --service client --lines 100
fuckipv6 logs --service coordinator --lines 100
```

查看 TUN 设备统计：

```bash
fuckipv6 stats
```

## 配置项

常用客户端配置：

```toml
coordinator_url = "ws://example.com:6666"
private_key_path = "identity.pem"

[network]
virtual_network = "fd00::/64"

[tun]
name = "fuckipv6"
mtu = 1280

[p2p]
listen_port = 0
connect_timeout = 10
heartbeat_interval = 30

[p2p.fallback_threshold]
max_rtt_ms = 200
max_loss_rate = 0.05
timeout_secs = 30

[log]
level = "info"
```

`RUST_LOG` 会覆盖配置中的日志级别。

客户端运行时会自动检测配置文件变化并热重载 `[p2p.fallback_threshold]`。`log.level` 变化会写入日志提示，但需要重启客户端后生效。

## 配置文件加密

使用环境变量 `FUCKIPV6_CONFIG_PASSPHRASE` 提供口令。加密格式使用 Argon2id 派生密钥和 ChaCha20-Poly1305 认证加密。

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' fuckipv6 encrypt-config client.toml client.toml.enc
```

用加密配置启动：

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' sudo -E fuckipv6 start --config client.toml.enc
```

解密导出：

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' fuckipv6 decrypt-config client.toml.enc --output client.toml
```

## 故障排查

权限不足：

```text
创建 TUN 设备失败
```

处理方式：Linux/macOS 使用 `sudo`，Linux 也可为二进制授予 `CAP_NET_ADMIN`；Windows 使用管理员终端运行。

TUN 设备残留：

```bash
sudo ip link delete fuckipv6
```

macOS 上 utun 会随进程关闭而释放；如果残留 IPv6 路由或地址，使用 `netstat -rn -f inet6` 和 `ifconfig utunN` 定位后手动清理。

无法连接协调服务器：

- 确认 `coordinator_url` 使用 `ws://` 或 `wss://`。
- 确认防火墙开放 TCP 6666。
- 确认客户端能访问协调服务器公网地址。

P2P 不通但虚拟 IP 可通：

- 这是 fallback 生效的表现。
- 在 `fuckipv6 peers` 查看 RTT、丢包率、Probe 成功率和活跃端点。
- 在 Web UI 查看 relay payload 是否增长。

## 当前限制

- NAT 类型检测依赖可访问的 STUN/RFC5780 服务器；检测证据不足时显示为 Unknown。
- P2P 直连数据面当前为 UDP；QUIC 已用于中继 fallback、证书 pin、多路复用和 0-RTT 自检。
- QUIC 中继已实现注册、转发、限速、并发控制和 loopback 集成测试。
- 单机 Linux network namespace 已覆盖 3 节点、路由、互 ping 和接近 MTU 的验证；Windows 和 macOS 构建由 CI 覆盖，生产上线前仍建议补充真实跨主机/跨 NAT 环境和更大规模压力测试。
