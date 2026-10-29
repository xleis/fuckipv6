# 快速开始指南

## 前置要求

- Linux、macOS 或 Windows
- Linux 需要 root 权限或 `CAP_NET_ADMIN` 创建 TUN 设备；macOS 需要 sudo/root 创建 utun 并配置路由；Windows 需要管理员权限和 WinTun
- Rust 1.75+
- IPv6网络环境

## 编译

```bash
cd /home/xlei/project/fuckipv6
cargo build --release
```

编译完成后，二进制文件位于：
- 客户端：`./target/release/fuckipv6`
- 协调服务器：`./target/release/fuckipv6-coordinator`

## 第一步：启动协调服务器

在你的公网服务器上运行：

```bash
# 使用默认端口：信令 0.0.0.0:6666，WebUI 0.0.0.0:8080
sudo ./target/release/fuckipv6-coordinator

# 或指定自定义端口
sudo ./target/release/fuckipv6-coordinator --bind "0.0.0.0:7777" --webui-bind "0.0.0.0:8080"

# 查看帮助
./target/release/fuckipv6-coordinator --help
```

**注意事项**：
- 确保防火墙开放了6666端口（TCP，用于WebSocket信令）
- 如需查看Web管理界面，开放8080端口或通过SSH隧道访问
- 服务器需要客户端可达；虚拟网络承载IPv6，传输endpoint可为IPv4或IPv6

## 第二步：配置客户端

在每台需要组网的机器上：

```bash
# 生成配置文件
./target/release/fuckipv6 init-config

# 编辑配置文件
nano client.toml
```

修改`coordinator_url`为你的服务器地址：

```toml
coordinator_url = "ws://[your-server-ipv6]:6666"
```

**配置说明**：

```toml
# 协调服务器地址
coordinator_url = "ws://[2001:db8::1]:6666"

# 私钥文件（自动生成）
private_key_path = "client_key.pem"

[network]
# 虚拟网络（由服务器分配）
virtual_network = "fd00::/64"

[tun]
# TUN设备名称
name = "fuckipv6"
# MTU大小
mtu = 1280

[p2p]
# P2P监听端口（0=随机）
listen_port = 0
# 连接超时（秒）
connect_timeout = 10
# 心跳间隔（秒）
heartbeat_interval = 30

[p2p.fallback_threshold]
# 最大RTT（毫秒）
max_rtt_ms = 200
# 最大丢包率
max_loss_rate = 0.05
# 连接超时（秒）
timeout_secs = 30

[log]
# 日志级别: trace, debug, info, warn, error
# 客户端和协调端都会读取该配置；RUST_LOG 环境变量会覆盖它
level = "info"
```

配置文件加载时会校验 WebSocket URL、IPv6 CIDR、监听地址、MTU、超时、丢包率阈值和日志级别；非法配置会在启动时直接报错。

客户端运行时会轮询配置文件变更并热重载 `[p2p.fallback_threshold]`，用于调整自动 fallback 的 RTT、丢包率和超时阈值。`log.level` 变更会被检测并写入日志，但当前日志订阅器需要重启客户端后生效。

如需加密保存配置文件，可使用 `FUCKIPV6_CONFIG_PASSPHRASE` 提供口令：

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' fuckipv6 encrypt-config client.toml client.toml.enc
FUCKIPV6_CONFIG_PASSPHRASE='change-me' sudo -E fuckipv6 start --config client.toml.enc
```

解密导出：

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' fuckipv6 decrypt-config client.toml.enc --output client.toml
```

## 第三步：启动客户端

**重要**：Linux/macOS 需要 sudo/root 权限创建虚拟网卡并配置路由；Windows 需要管理员权限。

```bash
# 启动客户端
sudo ./target/release/fuckipv6 start --config client.toml

# 或使用环境变量调整日志级别
sudo RUST_LOG=debug ./target/release/fuckipv6 start
```

成功启动后，你会看到类似输出：

```
INFO  Starting client with config: client.toml
INFO  Client ID: rBz8Xk3l...
INFO  Connecting to coordinator: ws://[2001:db8::1]:6666
INFO  WebSocket connected
INFO  Assigned virtual IP: fd00::1001
INFO  P2P engine listening on: [::]:54321
INFO  TUN device created: fuckipv6
INFO  Client started successfully, entering main loop
```

macOS 上配置中的 `tun.name = "fuckipv6"` 会自动分配实际 `utunN` 接口；如果要固定接口，可以改成 `utun0`、`utun1` 等系统允许的名称。

## 第四步：测试连通性

在客户端A上：

```bash
# 查看TUN设备（Linux）
ip addr show fuckipv6

# 查看路由（Linux）
ip -6 route | grep fuckipv6

# macOS 可查看实际 utun 接口和 IPv6 路由
ifconfig | grep -A4 utun
netstat -rn -f inet6 | grep utun

# 使用内置命令查看虚拟网络路由
sudo ./target/release/fuckipv6 route

# 查看客户端运行状态
sudo ./target/release/fuckipv6 status

# 查看已发现节点
sudo ./target/release/fuckipv6 peers

# 查看 systemd 日志
sudo ./target/release/fuckipv6 logs --service client --lines 100
```

假设客户端A分配到 `fd00::1001`，客户端B分配到 `fd00::1002`

在客户端A上ping客户端B：

```bash
ping6 fd00::1002
```

成功输出：

```
PING fd00::1002(fd00::1002) 56 data bytes
64 bytes from fd00::1002: icmp_seq=1 ttl=64 time=12.3 ms
64 bytes from fd00::1002: icmp_seq=2 ttl=64 time=11.8 ms
```

## 本机端到端验证

在 Linux 开发机上，可以用 network namespace 脚本启动 1 个协调服务器和 3 个隔离客户端，验证路由、双向 ping、接近 MTU 的大包 ping 和三节点连通性：

```bash
sudo ./scripts/test-netns.sh
```

脚本需要 root、`ip`、`ping6` 或 `ping -6`、TUN 内核模块和 network namespace 支持。它会自动构建 release 二进制并在退出时清理测试 namespace。

## 常见问题

### 1. 权限错误

```
Error: 创建 TUN 设备失败：权限不足，请使用 root 运行或授予 CAP_NET_ADMIN
```

**解决**：必须使用足够权限运行客户端
```bash
sudo ./target/release/fuckipv6 start
```

如果提示设备已存在，说明上次异常退出留下了残留设备：

```bash
sudo ip link delete fuckipv6
```

macOS 残留路由或地址可通过 `netstat -rn -f inet6` 和 `ifconfig utunN` 定位，必要时手动删除对应 IPv6 路由或地址。

### 2. 连接失败

```
Error: Failed to connect to coordinator
```

**检查**：
- 协调服务器是否运行
- 服务器地址和端口是否正确
- 防火墙是否开放
- IPv6网络是否可达

### 3. 查看协调端状态

浏览器访问：

```text
http://your-server:8080
```

或直接调用API：

```bash
curl http://your-server:8080/api/status
curl http://your-server:8080/api/peers
```

Web 管理页面会通过 `/ws` WebSocket 自动刷新；如果浏览器无法建立 WebSocket，会退回到定时 REST 轮询。页面的 System Logs 面板会调用 `/api/logs` 读取 systemd journal 最近日志。

### 4. 无法ping通

**检查**：
- 两个客户端是否都成功连接到服务器
- 查看日志中是否有"Peer online"消息
- 确认虚拟IP地址正确
- 运行 `fuckipv6 peers` 查看 Path、RTT、Loss、Probe 成功率和 BW 带宽估计；如果 Path 显示 Relay，说明P2P尚未验证或健康阈值不达标，但fallback可用

### 5. TUN设备已存在

如果上次异常退出，TUN设备可能没有清理：

```bash
# 删除旧设备
sudo ip link delete fuckipv6
```

## 高级用法

### 查看详细日志

```bash
sudo RUST_LOG=trace ./target/release/fuckipv6 start
```

### 后台运行

```bash
# 使用systemd
sudo nano /etc/systemd/system/fuckipv6.service
```

```ini
[Unit]
Description=fuckipv6 Client
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=/home/user/fuckipv6
ExecStart=/home/user/fuckipv6/target/release/fuckipv6 start --config /etc/fuckipv6/client.toml
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable fuckipv6
sudo systemctl start fuckipv6
sudo systemctl status fuckipv6
```

### SSH通过虚拟网络

假设客户端B的虚拟IP是 `fd00::1002`：

```bash
ssh user@fd00::1002
```

### 文件传输

```bash
scp file.txt user@[fd00::1002]:/tmp/
```

## 架构说明

```
┌─────────────────┐
│  Coordinator    │  WebSocket信令 + IPv6地址分配
│  (公网服务器)    │
└────────┬────────┘
         │
    ┌────┴────┐
    │         │
┌───▼──┐  ┌──▼───┐
│Client│◄─┤Client│  UDP P2P直连
│  A   │  │  B   │
└──────┘  └──────┘
fd00::1001  fd00::1002
```

- **协调服务器**：负责客户端注册、地址分配、对等节点发现
- **P2P连接**：客户端之间通过UDP直接通信，数据不经过服务器
- **TUN设备**：虚拟网卡，提供IPv6虚拟局域网接口

## 下一步

当前实现了Phase 1 MVP，后续可以扩展：

- **Phase 2**: NAT穿透、QUIC协议、中继服务
- **Phase 3**: Web管理界面、性能优化
- **Phase 4**: Windows支持、移动端

## 故障排查

启用调试日志：

```bash
sudo RUST_LOG=fuckipv6=debug,fuckipv6_client=debug ./target/release/fuckipv6 start
```

查看网络连接：

```bash
# 查看UDP监听端口
sudo ss -ulnp | grep fuckipv6

# 查看TUN设备状态
ip link show fuckipv6
ip -6 addr show fuckipv6
```

抓包分析：

```bash
# 抓取TUN设备流量
sudo tcpdump -i fuckipv6 -n

# 抓取UDP流量
sudo tcpdump -i any 'udp and port 54321' -n
```
