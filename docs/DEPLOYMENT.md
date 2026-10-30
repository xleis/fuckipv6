# fuckipv6 部署指南

本文档说明如何以**节点**为单位部署 `fuckipv6`。产品层不再区分客户端和服务端，也不存在第二个用户需要安装或启动的协调程序。

## 部署模型

```text
一个 fuckipv6 二进制
├── 普通成员节点：加入一个或多个网络
├── 托管节点：为网络提供信令、地址分配、QUIC 中继
├── Exit Node：为指定网络提供出口转发
└── 同一节点可同时承担以上多个角色
```

角色属于“节点在某个网络中的成员状态”，不属于二进制类型、系统用户或权限等级。

桌面环境可运行顶层 `fuckipv6 --gpui` 启动真实 GPUI 原生外壳，无桌面环境使用 `--headless` WebUI。GPUI 已接通共享 manifest、状态和命令总线，并完成 11 页真实快照投影与 manifest 动作接线；完整对等测试、IME/无障碍和跨平台实机仍待完成；`--webui` 仍是浏览器托管路径，不能与 GPUI 混称。

## 目录

- [系统要求](#系统要求)
- [构建与安装](#构建与安装)
- [创建网络](#创建网络)
- [加入网络](#加入网络)
- [运行 WebUI](#运行-webui)
- [systemd 持久运行](#systemd-持久运行)
- [Docker 部署](#docker-部署)
- [安全边界](#安全边界)
- [监控与维护](#监控与维护)
- [故障排除](#故障排除)
- [发布与升级](#发布与升级)

---

## 系统要求

### 通用节点

- CPU：1 核及以上；
- 内存：128 MB 及以上；托管大型网络时建议 512 MB 及以上；
- 操作系统：Linux、macOS 或 Windows；
- IPv4 或 IPv6 传输路径至少一条可达；
- Linux 需要 root 或 `CAP_NET_ADMIN`；macOS 需要 sudo/root；Windows 需要管理员权限和 WinTun；
- 构建源码需要 Rust 1.75+。

### 托管网络服务的节点

- 建议具有稳定的公网 IPv4/IPv6 地址或可转发端口；
- 开放 TCP `6666`（WebSocket 信令）；
- 开放 UDP `6667`（QUIC 中继）；
- WebUI 默认只监听回环地址 `127.0.0.1:8080`，不应为远程管理直接开放到公网；
- 如果网络服务位于 NAT 后，应配置 TCP/UDP 端口映射。

IPv6 普及后，普通成员之间可以优先使用公网 IPv6 直连；但信令、NAT 穿透和中继仍可能需要可达端点。不要把“每个设备都有公网 IPv6”当作所有网络环境都已成立的假设。

---

## 构建与安装

### 从源码构建

```bash
git clone https://github.com/yourusername/fuckipv6.git
cd fuckipv6
cargo build --release
```

唯一用户程序为：

```text
target/release/fuckipv6
```

### Linux 安装脚本

在包含 `fuckipv6` 二进制和安装脚本的源码或 release 包目录运行：

```bash
sudo ./scripts/install-node.sh
```

安装内容包括：

- 唯一二进制 `/opt/fuckipv6/fuckipv6`；
- 命令入口 `/usr/local/bin/fuckipv6`；
- 系统服务 `fuckipv6.service`。

安装后检查：

```bash
fuckipv6 --version
systemctl cat fuckipv6
```

### 手动安装

```bash
sudo install -d /opt/fuckipv6 /etc/fuckipv6 /var/lib/fuckipv6 /var/log/fuckipv6
sudo install -m 0755 target/release/fuckipv6 /opt/fuckipv6/fuckipv6
sudo ln -sf /opt/fuckipv6/fuckipv6 /usr/local/bin/fuckipv6
```

身份私钥和运行状态应保存在 `/var/lib/fuckipv6`，配置应保存在 `/etc/fuckipv6`。不要将私钥、节点配置或构建产物提交到 Git。

### macOS 安装脚本

```bash
sudo ./scripts/install-macos-node.sh
sudo launchctl kickstart -k system/cc.xlei.fuckipv6
```

launchd 标签为 `cc.xlei.fuckipv6`，二进制、配置、日志和 WebUI 端口均使用同一节点安装布局。

### Windows

使用 release 包中的 `setup_windows.ps1`，或在管理员终端直接运行 `fuckipv6.exe`。Windows 上的 GPUI 原生外壳尚待目标平台构建和实机验证；未验证的桌面路径应显式使用 WebUI。

---

## 创建网络

先在可达节点上启动网络服务角色。以下命令直接创建网络、输出 `fk://` 票据，并让本节点加入该网络：

```bash
sudo fuckipv6 host \
  --name home \
  --port 6666 \
  --relay-port 6667 \
  --external-host 2001:db8::10 \
  --headless \
  --webui-port 8080
```

参数说明：

| 参数 | 作用 |
|------|------|
| `--name home` | 网络名称；不是进程级“服务端身份” |
| `--port 6666` | WebSocket 信令监听端口 |
| `--relay-port 6667` | QUIC 中继 UDP 端口 |
| `--external-host` | 写入邀请票据、其他节点可达的域名或地址 |
| `--headless` | 使用 WebUI，不尝试桌面界面 |
| `--gpui` | 在支持的桌面显示环境中启动真实 GPUI 原生外壳 |
| `--webui-port 8080` | 本地 WebUI 监听端口 |

当前 `host` 默认让本节点自动加入所创建网络。CLI 尚不能通过 `--auto-connect=false` 关闭这一行为；需要纯网络服务角色时，应等待运行时监督器完成该配置语义，不要把当前限制描述成最终设计。

保存输出的 `fk://...` 票据。它是加入网络所需凭证，不应发布到公共日志或工单系统。

### 使用全局 host 参数

也可以不进入子命令，直接启动默认应用并内嵌网络服务：

```bash
sudo fuckipv6 \
  --headless \
  --webui-port 8080 \
  --host \
  --host-name home \
  --host-port 6666 \
  --host-relay-port 6667 \
  --external-host 2001:db8::10
```

这条路径适合 systemd 持久运行，但同样受当前自动加入语义影响。

---

## 加入网络

在其他节点上执行：

```bash
sudo fuckipv6 join "fk://..." \
  --name home \
  --start \
  --headless \
  --webui-port 8080
```

或分开保存与启动：

```bash
fuckipv6 join "fk://..." --name home
sudo fuckipv6 connect home --headless --webui-port 8080
```

连接多个网络：

```bash
sudo fuckipv6 connect home office --headless
sudo fuckipv6 connect --all --headless
```

查看和修改已保存网络：

```bash
fuckipv6 list
fuckipv6 default home
fuckipv6 leave home
```

网络配置的兼容字段仍为 `coordinator_url`，但该字段只表示网络服务端点，不表示独立产品程序。

---

## 运行 WebUI

headless 节点默认只在本机回环地址提供 WebUI：

```text
http://127.0.0.1:8080
```

远程管理优先使用 SSH 隧道：

```bash
ssh -L 8080:127.0.0.1:8080 user@node
```

需要在受控网络中直接监听时，必须显式设置全部危险开关与认证变量：

```bash
export FUCKIPV6_WEBUI_BIND=0.0.0.0       # IPv6 可用 ::
export FUCKIPV6_WEBUI_ALLOW_REMOTE=1
export FUCKIPV6_WEBUI_USERNAME=admin
export FUCKIPV6_WEBUI_PASSWORD='使用足够长的随机密码'
export FUCKIPV6_WEBUI_JWT_SECRET='使用足够长的随机签名密钥'
```

- 非回环监听缺少 `FUCKIPV6_WEBUI_ALLOW_REMOTE=1` 或非空密码时，节点拒绝启动；
- 设置 `FUCKIPV6_WEBUI_BIND=::1` 可显式使用 IPv6 回环；设为 `::` 属于非回环危险监听；
- `/`、`/assets/webui.css` 和 `/api/auth/login` 可匿名访问，其余 REST API 与 `/ws` 需要有效 JWT；
- REST API 使用 `Authorization: Bearer <token>`，浏览器 WebSocket 使用 `/ws?token=<token>`；认证失败返回 HTTP 401、`WWW-Authenticate: Bearer realm="fuckipv6-webui"` 和 JSON 错误；
- `/api/auth/login` 按直连源 IP 和用户名独立计数；连续失败会触发指数退避并返回 HTTP 429 与 `Retry-After`，成功登录会清除该键的失败状态。进程内状态表最多保留 4096 项、15 分钟未活动后清理，不信任可伪造的 `X-Forwarded-For`；
- 认证开启时，显式配置的 `FUCKIPV6_WEBUI_JWT_SECRET` 必须至少 32 字节且估计熵不低于 128 bit；应使用密码学随机源生成，不要使用可记忆短语。未显式配置时节点会生成进程级随机密钥，重启后既有 Token 失效；
- WebUI/API 没有内置 TLS，远程使用时必须置于 TLS 反向代理或受控管理网之后；
- 回环模式默认免密，本机同权限进程仍可访问控制面；需要本机认证时也可仅设置 `FUCKIPV6_WEBUI_PASSWORD`。

本地 IPC 默认监听 `127.0.0.1:6668`，可用 `FUCKIPV6_IPC_ADDR=[::1]:6668` 选择 IPv6 回环。由于 IPC 尚无认证握手，`FUCKIPV6_IPC_ALLOW_REMOTE=1` 只表示显式确认危险意图，非回环地址仍会拒绝启动；远程操作请使用 SSH 隧道。

当前 WebUI 是功能较完整的无头界面。GPUI 已实现真实原生外壳、11 页真实快照投影和 manifest 动作接线，并消费同一命令和状态契约；完整交互、IME/无障碍、跨平台实机与自动对等测试仍未完成。

---

## systemd 持久运行

安装脚本提供基础服务文件：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now fuckipv6
sudo systemctl status fuckipv6
sudo journalctl -u fuckipv6 -f
```

默认服务以 headless 模式启动 WebUI。要让同一服务持久承担网络服务角色，可使用 drop-in 替换 `ExecStart`：

```ini
# sudo systemctl edit fuckipv6
[Service]
ExecStart=
ExecStart=/opt/fuckipv6/fuckipv6 --headless --webui-port 8080 --host --host-name home --host-port 6666 --host-relay-port 6667 --external-host 2001:db8::10
```

成员节点可替换为：

```ini
[Service]
ExecStart=
ExecStart=/opt/fuckipv6/fuckipv6 connect home --headless --webui-port 8080
```

应用修改：

```bash
sudo systemctl daemon-reload
sudo systemctl restart fuckipv6
sudo journalctl -u fuckipv6 -n 100 --no-pager
```

服务名只有 `fuckipv6.service`。旧 `fuckipv6-client.service` 和 `fuckipv6-coordinator.service` 已不再随发布物提供。

---

## Docker 部署

仓库只提供一个节点镜像：

```bash
docker build -f Dockerfile -t fuckipv6:local .
```

由于 TUN、路由和主机网络能力依赖宿主机，Docker 运行需要 `network_mode: host`、`NET_ADMIN` 和 `/dev/net/tun`：

```bash
docker run -d \
  --name fuckipv6 \
  --restart unless-stopped \
  --network host \
  --cap-add NET_ADMIN \
  --device /dev/net/tun:/dev/net/tun \
  -v fuckipv6-state:/var/lib/fuckipv6 \
  -v fuckipv6-config:/etc/fuckipv6 \
  fuckipv6:local
```

Compose 入口只有一个服务：

```bash
docker compose up -d node
docker compose logs -f node
```

容器默认启动 headless WebUI。托管网络服务或加入成员网络时，通过 WebUI、compose `command` 或挂载一致的节点配置选择角色。容器化部署不应复制出第二个协调镜像。

---

## 安全边界

### 身份与配置

- 节点身份由 Ed25519 私钥表示；私钥属于节点，不属于“客户端/服务端身份”；
- 配置文件可能包含网络端点、路径、策略和身份引用，应限制为 root 或专用服务用户可读；
- 使用 `FUCKIPV6_CONFIG_PASSPHRASE` 可加密兼容配置文件；
- 定期备份身份与配置，但不要把私钥放入镜像、制品或公开备份。

示例备份：

```bash
sudo install -d -m 0700 /var/backups/fuckipv6
sudo tar -C / -czf "/var/backups/fuckipv6/node-$(date +%F).tar.gz" etc/fuckipv6 var/lib/fuckipv6
sudo find /var/backups/fuckipv6 -type f -mtime +30 -delete
```

### 网络暴露面

- WebUI/API 默认只监听回环；远程监听必须显式开启危险开关并配置管理员密码，否则拒绝启动；
- WebUI/API 没有内置 TLS，生产环境应使用 SSH 隧道、管理网或 TLS 终止反向代理，不应直接暴露到公网；
- IPC 默认只监听回环且没有认证协议，远程 IPC 始终 fail-closed；
- 网络服务需开放的端口应只对预期成员地址开放；
- TLS client/server、握手 Initiator/Responder 是协议角色，不是节点权限等级；
- Exit Node 会改变转发和 NAT 行为，应只对受信网络启用并审计策略。

### 密码学状态

Secure Session v2 classic 已接入 UDP/TUN 生产数据面，但尚未完成独立密码学审计；后量子混合 profile 尚未接入生产控制通道和数据面，不能宣称已具备后量子安全。当前性能基线也未证明超越 WireGuard。具体边界见 `SECURITY_PERFORMANCE_TODO.md` 和 `SECURITY_TRACEABILITY.md`。

---

## 监控与维护

### 日志

```bash
sudo journalctl -u fuckipv6 -f
sudo journalctl -u fuckipv6 -n 200 --no-pager
sudo journalctl -u fuckipv6 --since "1 hour ago"
```

调整日志级别：

```bash
sudo systemctl edit fuckipv6
```

```ini
[Service]
Environment="RUST_LOG=debug"
```

修改后重启 `fuckipv6`。

### 运行状态

```bash
fuckipv6 status
fuckipv6 peers
fuckipv6 stats
fuckipv6 route
```

WebUI 提供在线成员、连接拓扑、流量、系统日志和网络操作。远程自动化应优先调用稳定的本地控制接口；不要在脚本中抓取 HTML。

### 资源检查

```bash
systemctl status fuckipv6
ps -o pid,%cpu,%mem,rss,cmd -C fuckipv6
ss -tlnp | grep -E ':(6666|8080)'
ss -ulnp | grep 6667
ip -6 addr show fuckipv6
ip -6 route show
```

---

## 故障排除

### 服务未启动

```bash
sudo systemctl status fuckipv6 --no-pager
sudo journalctl -u fuckipv6 -n 200 --no-pager
```

检查配置路径、私钥权限、端口占用和 TUN 权限。

### 无法加入网络

检查：

```bash
ping -6 <托管节点地址>
ss -tlnp | grep 6666
sudo systemctl status fuckipv6
```

确认邀请票据完整、`--external-host` 可从成员节点解析或访问、TCP `6666` 未被防火墙阻断。

### 无法创建 TUN

```bash
lsmod | grep tun
ls -l /dev/net/tun
ip link show type tun
ip link delete fuckipv6 2>/dev/null || true
```

确认进程具备 root 或 `CAP_NET_ADMIN`。服务文件中应保留 `CAP_NET_ADMIN` 和 `CAP_NET_RAW`。

### 能信令但无法 P2P

```bash
fuckipv6 peers
sudo ss -ulnp | grep 6667
```

检查 UDP 中继端口、NAT 映射、IPv6 防火墙和 Peer 的 Path/RTT/Loss。Relay 路径可用不代表直连失败。

### WebUI 无法访问

```bash
ss -tlnp | grep 8080
curl -v http://127.0.0.1:8080/api/status
```

确认节点以 `--headless` 启动、端口未被占用，并检查监听地址和反向代理设置。

### 旧安装残留

旧版本可能遗留独立服务或配置：

```bash
sudo systemctl disable --now fuckipv6-client.service fuckipv6-coordinator.service 2>/dev/null || true
sudo rm -f /etc/systemd/system/fuckipv6-client.service /etc/systemd/system/fuckipv6-coordinator.service
sudo systemctl daemon-reload
```

迁移时保留节点身份和网络凭证，合并为 `fuckipv6.service` 与统一节点配置。

---

## 发布与升级

发布物只包含 `fuckipv6` 节点程序及对应示例、安装脚本和服务文件，不再提供独立协调二进制。

升级前：

1. 备份 `/etc/fuckipv6` 与 `/var/lib/fuckipv6`；
2. 阅读 `CHANGELOG.md` 的兼容性说明；
3. 在非生产网络验证票据、配置和身份保持可用；
4. 替换唯一二进制并重启 `fuckipv6.service`。

```bash
sudo install -m 0755 target/release/fuckipv6 /opt/fuckipv6/fuckipv6
sudo systemctl restart fuckipv6
sudo systemctl status fuckipv6
```

发布门禁：

```bash
python3 scripts/release-gate.py run --platform linux --print-plan
python3 scripts/release-gate.py run --platform linux --toolchain stable
```

门禁通过不替代独立密码学审计、真实跨公网/跨 NAT 验证、目标平台实机验证或 WireGuard 性能对等证明。

---

## 典型场景

### 家庭网络

- NAS、软路由或树莓派充当托管节点；
- 其他设备使用同一个 `fk://` 票据加入；
- 优先使用公网 IPv6 直连，NAT 环境使用 QUIC/信令中继 fallback；
- 所有设备运行同一个 `fuckipv6` 二进制。

### 云上可达节点

- 云主机承担信令和 QUIC 中继服务角色；
- 家庭和办公节点作为普通成员加入；
- 通过防火墙限制信令和 WebUI 访问范围；
- TLS 终止、反向代理和访问控制独立部署并审计。

### 多网络节点

同一个节点可并发加入家庭、办公室和测试网络；每个网络拥有独立 TUN、虚拟地址、Peer、路由和策略。网络服务角色和 Exit Node 角色按网络分别授予，不是进程级“客户端/服务端模式”。

---

## 最终检查清单

- [ ] 仅安装了 `fuckipv6` 一个用户程序；
- [ ] systemd/launchd 中只有 `fuckipv6.service` 或 `cc.xlei.fuckipv6`；
- [ ] 网络通过 `fk://` 票据或统一节点配置加入；
- [ ] WebUI/IPC 默认仅监听回环；远程 WebUI 使用显式危险开关、认证和 TLS 边界；
- [ ] 身份与配置已纳入加密备份；
- [ ] 日志、Peer 路径、路由和 Exit Node 策略已检查；
- [ ] 没有把 WebUI 浏览器窗口称为 GPUI；
- [ ] 没有宣称未经审计或基准证明的“超越 WireGuard”结论。
