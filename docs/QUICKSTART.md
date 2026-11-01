# 快速开始指南

## 先理解产品模型

`fuckipv6` 只有一种产品身份：**节点**。只发布一个 `fuckipv6` 程序；没有需要分别安装或启动的“客户端程序”和“服务端程序”。

同一个节点可以：

- 创建网络，并承担信令、地址分配和 QUIC 中继等网络服务角色；
- 通过 `fk://` 邀请票据加入网络，成为普通成员；
- 同时在多个网络中承担不同职责；
- 作为 Exit Node 为指定网络提供出口转发。

协议和 TLS 代码中的 client/server、Initiator/Responder 只是会话术语，不是产品身份。

界面策略是严格二选一：有桌面环境时只启动 GPUI，无桌面环境时才启动 headless WebUI；两套 UI 必须共享命令层、状态模型、校验和错误语义。桌面上的 `--webui`、`--headless` 会直接失败，GPUI 不可用也会失败关闭，不会自动回退 WebUI。GPUI 信任当前操作系统登录用户，不要求应用内登录或 Passkey；所有者认证只用于 WebUI。NX-08 已接通可启动的真实 GPUI 原生外壳；NX-09 垂直切片已让 11 个 manifest 页面显示真实快照并接通类型化动作，但正式 IME/无障碍、双 UI 自动对等测试与跨平台实机验证尚未完成。

## 前置要求

- Linux、macOS 或 Windows；
- Linux 需要 root 或 `CAP_NET_ADMIN` 创建 TUN、配置地址和路由；macOS 需要 sudo/root；Windows 需要管理员权限和 WinTun；
- Rust 1.75+（从源码构建时）；
- 节点之间至少有一条可达的 IPv4 或 IPv6 传输路径。

托管网络服务时通常需要允许：

- TCP `6666`：WebSocket 信令；
- UDP `6667`：QUIC 中继；
- TCP `8080`：WebUI；公网访问必须使用内置 TLS 或受信认证反向代理。

## 编译

```bash
git clone https://github.com/yourusername/fuckipv6.git
cd fuckipv6
cargo build --release
```

构建产物只有：

```text
./target/release/fuckipv6
```

## 第一步：在一个可达节点上创建网络

在具有可达地址的节点上运行：

```bash
sudo ./target/release/fuckipv6 host \
  --name home \
  --port 6666 \
  --relay-port 6667 \
  --external-host 2001:db8::10 \
  --headless \
  --webui-port 8080
```

说明：

- `host` 是节点的网络服务角色，不是独立服务端程序；
- 该节点默认作为成员自动加入自己创建的网络；
- `--external-host` 应填写其他节点可达的域名、IPv4 或 IPv6 地址，用于生成邀请票据；
- 程序会输出 `fk://...` 邀请票据，后续节点使用同一票据加入；
- `--headless` 仅适用于无桌面服务器的 WebUI 表面，不改变节点身份或权限；在检测到桌面会话的主机上会被拒绝。

如果只在受控本地环境验证，可以省略 `--external-host`，并使用本机默认地址生成票据。

## 第二步：其他节点加入网络

在另一台节点上执行：

```bash
sudo ./target/release/fuckipv6 join "fk://..." \
  --name home \
  --start \
  --headless \
  --webui-port 8080
```

也可以把“保存配置”和“启动”拆开：

```bash
./target/release/fuckipv6 join "fk://..." --name home
sudo ./target/release/fuckipv6 connect home --headless --webui-port 8080
```

常用网络操作：

```bash
fuckipv6 list                 # 查看已保存网络
fuckipv6 default home         # 设置默认网络
fuckipv6 connect home         # 连接指定网络
fuckipv6 connect --all        # 连接所有已保存网络
fuckipv6 leave home           # 删除网络配置和该网络使用的身份材料
```

## 第三步：访问节点界面

无桌面节点的 WebUI 默认只在本机回环地址监听。回环地址不是认证捷径：完成所有者初始化后，普通受控 API 仍要求有效的节点所有者 Passkey 会话。本机访问：

```text
http://127.0.0.1:8080
```

本机管理优先直接访问回环；远程管理可使用 SSH 隧道：

```bash
ssh -L 8080:127.0.0.1:8080 user@node
```

随后在操作机打开 `http://127.0.0.1:8080`。

需要公网访问时，只能在以下两种互斥方案中选择一种：

### 内置直接 HTTPS

```bash
export FUCKIPV6_WEBUI_BIND=0.0.0.0
export FUCKIPV6_WEBUI_ALLOW_REMOTE=1
export FUCKIPV6_WEBUI_PUBLIC_ORIGIN=https://node.example.com
export FUCKIPV6_WEBUI_TLS_CERT=/etc/fuckipv6/tls/fullchain.pem
export FUCKIPV6_WEBUI_TLS_KEY=/etc/fuckipv6/tls/privkey.pem
export FUCKIPV6_WEBUI_USERNAME=admin
export FUCKIPV6_WEBUI_PASSWORD='使用足够长的随机密码'
export FUCKIPV6_WEBUI_JWT_SECRET='使用足够长的随机签名密钥'
```

证书必须当前有效、SAN 匹配公网域名、可供 `serverAuth` 使用，并与私钥匹配；配置错误时节点拒绝启动，不会回退到 HTTP。建议证书/私钥由 `root` 或专用服务用户独占读取，例如目录 `0700`、文件 `0600`，同时保证运行用户可读。

### 受信 TLS 反向代理

```bash
export FUCKIPV6_WEBUI_PUBLIC_ORIGIN=https://relay.example.com
export FUCKIPV6_WEBUI_TRUSTED_PROXY_CIDRS=127.0.0.1/32,::1/128
```

WebUI 必须继续监听回环，代理应连接 `127.0.0.1:8080`。代理必须覆盖 `Host`、`X-Forwarded-Host`、`X-Forwarded-Proto: https` 和单个 `X-Forwarded-For`；POST 必须保留精确匹配的 `Origin`。`Forwarded`、`X-Real-IP`、重复转发头和 HTTP 降级都会被拒绝。直接 TLS 与可信代理 CIDR 不能同时配置。

已验证 HTTPS 边界返回一年期 HSTS；生产 node surface 使用动态 CSP nonce，并返回 `X-Frame-Options: DENY`、`nosniff`、`no-referrer` 和同源隔离响应头。本机 HTTP 不返回 HSTS。完整变量、证书限制和代理配置见[部署指南](DEPLOYMENT.md#运行-webui)。

IPC 默认监听 `127.0.0.1:6668`，也可用 `FUCKIPV6_IPC_ADDR=[::1]:6668` 选择 IPv6 回环。IPC 当前没有认证协议，因此即使设置 `FUCKIPV6_IPC_ALLOW_REMOTE=1` 也不会允许非回环监听。

有桌面显示时可运行 `fuckipv6 --gpui` 启动真实 GPUI 原生外壳；该路径不启动 WebUI，也不显示 Passkey、WebUI 会话或恢复入口。无显示环境才使用 headless WebUI，WebUI 的公网与回环受控 API 都要求所有者会话。当前 GPUI 已接通共享 manifest、状态和命令总线，11 个页面会展示真实快照并映射 manifest 动作；但完整交互、正式 IME/无障碍和 GPUI/WebUI 自动对等仍未完成，不能宣称功能对等。

## 使用传统配置文件启动

从一个节点生成配置：

```bash
./target/release/fuckipv6 init-config node.toml
```

编辑网络服务端点。当前配置字段为了兼容仍名为 `coordinator_url`：

```toml
coordinator_url = "wss://network.example.com:6666"
private_key_path = "node_key.pem"

[network]
virtual_network = "fd00::/64"

[tun]
name = "fuckipv6"
mtu = 1400

[p2p]
listen_port = 0
heartbeat_interval = 30
connect_timeout = 10

[log]
level = "info"
```

启动同一个节点程序：

```bash
sudo ./target/release/fuckipv6 start --config node.toml --headless
```

`coordinator_url` 是内部兼容名称，不代表用户需要选择或运行一个独立协调程序。

配置文件加载时会校验 WebSocket URL、IPv6 CIDR、监听地址、MTU、超时、丢包率阈值和日志级别；非法配置会直接报错。

如需加密配置文件，可使用 `FUCKIPV6_CONFIG_PASSPHRASE`：

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' \
  fuckipv6 encrypt-config node.toml node.toml.enc

FUCKIPV6_CONFIG_PASSPHRASE='change-me' \
  sudo -E fuckipv6 start --config node.toml.enc --headless
```

解密导出：

```bash
FUCKIPV6_CONFIG_PASSPHRASE='change-me' \
  fuckipv6 decrypt-config node.toml.enc --output node.toml
```

## 验证连通性

查看 TUN、IPv6 地址和路由：

```bash
ip addr show fuckipv6
ip -6 route | grep fuckipv6
```

查看节点运行状态和 Peer：

```bash
sudo ./target/release/fuckipv6 status
sudo ./target/release/fuckipv6 peers
sudo ./target/release/fuckipv6 route
```

假设另一个成员节点的虚拟 IPv6 是 `fd00::1002`：

```bash
ping6 fd00::1002
```

`peers` 输出中的 Path、RTT、Loss、Probe 成功率和带宽估计可用于判断当前是直连还是中继。Path 显示 Relay 时，说明 P2P 尚未验证或质量低于阈值，但 fallback 可用。

## 本机端到端验证

Linux 开发机可使用 network namespace 脚本启动 1 个托管节点和 3 个隔离成员节点：

```bash
sudo ./scripts/test-netns.sh
```

脚本覆盖双向 ping、路由和接近 MTU 的大包 ping。它需要 root、`ip`、`ping`、TUN 内核模块和 network namespace 支持。

## 安装为系统服务

Linux release 包和源码仓库提供统一的节点安装脚本：

```bash
sudo ./scripts/install-node.sh
sudo fuckipv6 service status
sudo journalctl -u fuckipv6 -f
```

脚本调用 `fuckipv6 service install --scope system`。生成的 `fuckipv6.service` 执行
`__service --options-file /etc/fuckipv6/service.json --config-dir /etc/fuckipv6`，
由同一进程持有 control 锁并监督 node 和 WebUI。网络创建、加入或默认连接配置好以后，
重启服务即可沿用已保存配置。

macOS 使用：

```bash
sudo ./scripts/install-macos-node.sh
sudo fuckipv6 service status
```

Windows 在管理员终端运行同一个二进制：

```powershell
fuckipv6.exe service install --scope system --webui-port 8080
fuckipv6.exe service status
```

Windows 已接入 SCM 控制处理器，但当前只有交叉编译与单元测试证据，仍需实机验证；
macOS 同样需要目标平台实机证据。服务名只有 `fuckipv6.service` 或 launchd 标签
`cc.xlei.fuckipv6`。

## 常见问题

### 权限错误

```text
Error: 创建 TUN 设备失败：权限不足
```

使用 root、`CAP_NET_ADMIN` 或平台等价权限。如果上次异常退出留下设备，可清理：

```bash
sudo ip link delete fuckipv6
```

### 无法加入网络

检查：

- 邀请票据是否完整；
- `--external-host` 是否填写为其他节点可达地址；
- TCP `6666` 是否可达；
- 网络服务节点的防火墙和 NAT 端口映射是否正确；
- 使用 `sudo journalctl -u fuckipv6 -n 100` 查看错误。

### 能完成信令但无法 P2P

这是正常降级路径。节点会优先尝试 QUIC 中继，必要时再走 WebSocket 信令中继。检查 UDP `6667`、NAT 映射和中继健康状态。

### WebUI 是否等价于 GPUI？

不是。WebUI 是当前功能较完整的界面表面；GPUI 已实现 11 页真实快照投影和 manifest 动作接线的垂直切片，但完整交互、IME/无障碍、跨平台实机与自动语义对等仍未完成。二者已消费同一 manifest、状态和命令契约，完成自动对等测试后才算达到统一 UI 目标。

## 架构边界

```text
                 一个 fuckipv6 进程（本地节点）
┌─────────────────────────────────────────────────────┐
│ UI：GPUI（原生外壳） | WebUI（headless） | CLI/IPC   │
├─────────────────────────────────────────────────────┤
│ 共享命令、状态、校验、确认和错误契约                │
├─────────────────────────────────────────────────────┤
│ 节点监督器：身份、配置、日志、生命周期、多网络管理  │
├─────────────────────────────────────────────────────┤
│ 网络 A：成员、地址、Peer、路由、策略、服务角色      │
│ 网络 B：成员、地址、Peer、路由、策略、服务角色      │
└─────────────────────────────────────────────────────┘
```

- **节点**是唯一产品身份；
- **网络**是隔离的虚拟覆盖网；
- **成员**是某节点在一个网络中的参与状态；
- **网络服务角色**包括信令、地址分配、中继和 Exit Node；
- **GPUI/WebUI** 都只能做展示与输入适配，不得各自复制业务状态机。

详细迁移项见 [`UNIFIED_NODE_TODO.md`](../UNIFIED_NODE_TODO.md)。
