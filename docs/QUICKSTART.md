# 快速开始指南

## 先理解产品模型

`fuckipv6` 只有一种产品身份：**节点**。只发布一个 `fuckipv6` 程序；没有需要分别安装或启动的“客户端程序”和“服务端程序”。

同一个节点可以：

- 创建网络，并承担信令、地址分配和 QUIC 中继等网络服务角色；
- 通过 `fk://` 邀请票据加入网络，成为普通成员；
- 同时在多个网络中承担不同职责；
- 作为 Exit Node 为指定网络提供出口转发。

协议和 TLS 代码中的 client/server、Initiator/Responder 只是会话术语，不是产品身份。

界面目标是：有桌面环境时使用 GPUI 原生界面，无桌面环境时使用 WebUI；两套 UI 必须共享命令层、状态模型、校验和错误语义。NX-08 已接通可启动的真实 GPUI 原生外壳，但 11 个业务页面仍以 NX-09 占位为主，双 UI 自动对等测试尚未完成。`--webui` 始终是浏览器托管路径，不能称为 GPUI。

## 前置要求

- Linux、macOS 或 Windows；
- Linux 需要 root 或 `CAP_NET_ADMIN` 创建 TUN、配置地址和路由；macOS 需要 sudo/root；Windows 需要管理员权限和 WinTun；
- Rust 1.75+（从源码构建时）；
- 节点之间至少有一条可达的 IPv4 或 IPv6 传输路径。

托管网络服务时通常需要允许：

- TCP `6666`：WebSocket 信令；
- UDP `6667`：QUIC 中继；
- TCP `8080`：WebUI；生产环境应仅绑定可信网络或放在认证反向代理后。

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
- `--headless` 仅选择 WebUI 表面，不改变节点身份或权限。

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

节点默认只在本机回环地址提供 WebUI，本机访问：

```text
http://127.0.0.1:8080
```

远程管理优先使用 SSH 隧道，不要直接把控制面暴露到公网：

```bash
ssh -L 8080:127.0.0.1:8080 user@node
```

随后在操作机打开 `http://127.0.0.1:8080`。如确需让 WebUI/API 监听非回环地址，必须同时设置 `FUCKIPV6_WEBUI_BIND`、`FUCKIPV6_WEBUI_ALLOW_REMOTE=1` 和非空 `FUCKIPV6_WEBUI_PASSWORD`，否则节点拒绝启动；远程部署还应通过 TLS 反向代理或受控管理网提供传输保护。完整变量见[部署指南](DEPLOYMENT.md#运行-webui)。

IPC 默认监听 `127.0.0.1:6668`，也可用 `FUCKIPV6_IPC_ADDR=[::1]:6668` 选择 IPv6 回环。IPC 当前没有认证协议，因此即使设置 `FUCKIPV6_IPC_ALLOW_REMOTE=1` 也不会允许非回环监听。

有桌面显示时可运行 `fuckipv6 --gpui` 启动真实 GPUI 原生外壳；无显示环境使用 headless WebUI。当前 GPUI 已接通共享 manifest、状态和命令总线，但 NX-09 业务页面仍是占位，不能宣称 GPUI/WebUI 功能对等。

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
sudo systemctl start fuckipv6
sudo systemctl status fuckipv6
sudo journalctl -u fuckipv6 -f
```

服务名只有 `fuckipv6.service`。网络创建、加入或默认连接配置好以后，同一个服务会以 headless 模式提供 WebUI 和节点运行时。

macOS 使用：

```bash
sudo ./scripts/install-macos-node.sh
sudo launchctl kickstart -k system/cc.xlei.fuckipv6
```

Windows 请使用 release 包中的 `setup_windows.ps1`，或在管理员终端运行同一个 `fuckipv6.exe`。

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

不是。WebUI 是当前功能较完整的界面表面；GPUI 已是真实原生桌面外壳，但多数业务页面仍待 NX-09。二者已消费同一 manifest、状态和命令契约，完成自动对等测试后才算达到统一 UI 目标。

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
