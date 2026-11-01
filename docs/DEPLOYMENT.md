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
- WebUI 默认只监听回环地址 `127.0.0.1:8080`；公网管理必须使用内置 HTTPS 或受信 TLS 反向代理；
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

脚本只调用统一二进制的 `service install`，由它原子安装二进制、创建默认配置并生成
systemd unit。安装内容包括：

- 唯一二进制 `/opt/fuckipv6/fuckipv6`；
- 命令入口 `/usr/local/bin/fuckipv6`；
- 系统服务 `fuckipv6.service`；
- 配置 `/etc/fuckipv6/service.json`。

安装后检查：

```bash
fuckipv6 --version
sudo fuckipv6 service status
```

升级时从新版本包运行 `sudo ./bin/fuckipv6 service upgrade`，不要用已安装的旧二进制原地
覆盖。卸载使用 `sudo /opt/fuckipv6/fuckipv6 service uninstall`；卸载会保留配置、身份和
状态数据。

### macOS 安装脚本

```bash
sudo ./scripts/install-macos-node.sh
sudo fuckipv6 service status
```

launchd 标签为 `cc.xlei.fuckipv6`，由 `service install --scope system` 生成。安装脚本不再
直接复制旧 plist，因此 stop/start、升级和卸载共享同一套服务生命周期语义。

### Windows

在管理员终端运行：

```powershell
fuckipv6.exe service install --scope system --webui-port 8080
fuckipv6.exe service status
```

服务进程会连接 Windows SCM 并上报 running/stopping/stopped 状态，Stop/Shutdown 控制会
触发与 Unix SIGTERM 相同的优雅关闭路径。当前只有交叉编译与单元测试证据，仍需在
Windows 实机完成安装、停止、崩溃恢复和 TUN 权限验证；不能把交叉编译等同于实机通过。

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

这只是前台交互入口。需要持久运行时先保存配置，再使用 `fuckipv6 service install`；
不要把 `--headless` 命令塞进服务 unit，否则会绕过 control 监督和 SCM/launchd 生命周期。

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

本机管理仍优先使用回环访问或 SSH 隧道：

```bash
ssh -L 8080:127.0.0.1:8080 user@node
```

生产环境如需公网访问，只能在以下两种互斥方案中选择一种：

1. WebUI 使用内置 TLS 直接提供 HTTPS；
2. WebUI 仅监听回环，由受信 TLS 反向代理终止 HTTPS。

禁止把明文 HTTP WebUI 直接暴露到公网。

### 直接 HTTPS

直接 HTTPS 时必须同时设置证书、私钥和公网 origin。需要远程管理时还应配置认证变量：

```bash
export FUCKIPV6_WEBUI_BIND=0.0.0.0       # IPv6 可用 ::
export FUCKIPV6_WEBUI_ALLOW_REMOTE=1
export FUCKIPV6_WEBUI_PUBLIC_ORIGIN=https://node.example.com
export FUCKIPV6_WEBUI_TLS_CERT=/etc/fuckipv6/tls/fullchain.pem
export FUCKIPV6_WEBUI_TLS_KEY=/etc/fuckipv6/tls/privkey.pem
export FUCKIPV6_WEBUI_USERNAME=admin
export FUCKIPV6_WEBUI_PASSWORD='使用足够长的随机密码'
export FUCKIPV6_WEBUI_JWT_SECRET='使用足够长的随机签名密钥'
```

- `FUCKIPV6_WEBUI_TLS_CERT` 与 `FUCKIPV6_WEBUI_TLS_KEY` 必须同时设置；任一缺失、PEM 损坏或私钥不匹配都会使节点拒绝启动；
- 证书必须当前有效，叶证书 SAN 必须包含公网 origin 的 DNS 主机名；存在 EKU 时必须允许 `serverAuth`；
- 公网 origin 必须是 `https://` DNS 主机名，不能包含用户信息、路径、query、fragment、`localhost` 或 IP 地址；
- 证书/私钥应只允许 `root` 或专用服务用户读取，例如 `root:root`、目录 `0700`、文件 `0600`；运行用户必须保留可读权限；
- 直接 TLS 与 `FUCKIPV6_WEBUI_TRUSTED_PROXY_CIDRS` 互斥；直接 TLS 会拒绝 `Forwarded`、`X-Forwarded-For`、`X-Forwarded-Host`、`X-Forwarded-Proto` 和 `X-Real-IP`；
- 证书续期失败或新证书不匹配时，下次启动会失败关闭，不会回退到 HTTP；
- 未配置密码时，非回环直接 TLS 只开放一次性所有者认领页与认领 API，普通管理 API 返回 HTTP 401。需要完成初始化后的远程管理时必须配置密码或等价的正式所有者会话。

非回环监听必须显式设置 `FUCKIPV6_WEBUI_ALLOW_REMOTE=1`。除一次性直接 TLS 所有者认领外，还必须配置非空密码；否则节点拒绝启动。

### 受信反向代理

反向代理方案要求 WebUI 仍只监听回环，由代理连接 `127.0.0.1:8080`：

```bash
export FUCKIPV6_WEBUI_PUBLIC_ORIGIN=https://relay.example.com
export FUCKIPV6_WEBUI_TRUSTED_PROXY_CIDRS=127.0.0.1/32,::1/128
```

- 不要设置 `FUCKIPV6_WEBUI_BIND=0.0.0.0` 或 `::`；反向代理公网模式禁止明文监听公网；
- `FUCKIPV6_WEBUI_TRUSTED_PROXY_CIDRS` 只能填写实际代理地址，不能使用 `0.0.0.0/0` 或 `::/0`；
- 代理必须覆盖而不是追加 `Host`、`X-Forwarded-Host`、`X-Forwarded-Proto` 和 `X-Forwarded-For`；`Host` 与 `X-Forwarded-Host` 必须等于配置 origin 的主机，`X-Forwarded-Proto` 必须为 `https`；
- `X-Forwarded-For` 必须只包含一个由代理覆盖的真实客户端 IP；`Forwarded`、`X-Real-IP` 和重复转发头都会被拒绝；
- POST 必须带与配置 origin 完全匹配的 `Origin`；来源不明的跨站请求失败关闭；
- 不接受 HTTP 降级；代理到 WebUI 的链路只能位于受信回环或受控管理网。

### 所有者认领与安全响应头

- 一次性初始化码只能放在 JSON 请求体中，禁止放入 URL、query、Referer 或日志；成功后公网认领入口永久返回 `410 Gone`；
- 所有者初始化请求的最终 HTTP 状态写入配置目录下的 `owner-security-audit.jsonl`。文件权限为 `0600`，仅记录通道、来源 IP、方法、路径、状态和固定 outcome，不记录请求体、请求头、初始化码、恢复码或 Passkey；审计写入失败时控制面失败关闭；
- 已验证的 HTTPS 边界返回 `Strict-Transport-Security: max-age=31536000; includeSubDomains`；本机 HTTP 或未通过来源校验的请求不会误发 HSTS；
- 生产 node surface 使用每次响应随机生成的 CSP nonce；脚本仅允许同源与对应 nonce，样式仅允许同源、对应 nonce 和运行时 `style` 属性；同时返回 `X-Frame-Options: DENY`、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`Permissions-Policy` 与同源隔离响应头；
- 旧的内嵌主界面仅作为迁移兼容路径，使用较宽松的 CSP；正式部署必须走 manifest 驱动 node surface，安全验收不以旧兼容页为准。

本地 IPC 默认监听 `127.0.0.1:6668`，可用 `FUCKIPV6_IPC_ADDR=[::1]:6668` 选择 IPv6 回环。由于 IPC 尚无认证握手，`FUCKIPV6_IPC_ALLOW_REMOTE=1` 只表示显式确认危险意图，非回环地址仍会拒绝启动；远程操作请使用 SSH 隧道。

当前 WebUI 是功能较完整的无头界面。GPUI 已实现真实原生外壳、11 页真实快照投影和 manifest 动作接线，并消费同一命令和状态契约；完整交互、IME/无障碍、跨平台实机与自动对等测试仍未完成。

---

## systemd 持久运行

推荐通过统一管理命令操作服务：

```bash
sudo fuckipv6 service install --scope system --webui-port 8080
sudo fuckipv6 service status
sudo fuckipv6 service restart
sudo fuckipv6 service stop
sudo fuckipv6 service start
sudo journalctl -u fuckipv6 -f
```

生成的 unit 执行：

```text
/opt/fuckipv6/fuckipv6 __service   --options-file /etc/fuckipv6/service.json   --config-dir /etc/fuckipv6
```

`__service` 持有 control 锁，监督 node 和 WebUI；不要改成 `--headless`、`host` 或其他
交互式子命令。网络创建、加入和默认连接应先用 CLI 写入配置，再重启服务。

用户级服务使用：

```bash
fuckipv6 service install --scope user --webui-port 8080
fuckipv6 service status --scope user
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
- WebUI/API 支持内置 TLS；默认回环模式是 HTTP，公网暴露必须使用内置 HTTPS 或受信 TLS 终止反向代理，禁止明文直接暴露；
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

检查服务状态和生成的 service.json：

```bash
sudo fuckipv6 service status
sudo sed -n '1,80p' /etc/fuckipv6/service.json
sudo journalctl -u fuckipv6 -n 100 --no-pager
```

再确认端口未被占用，并检查监听地址和反向代理设置。

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
4. 从新版本二进制执行带二进制备份和失败回滚的升级；
5. 检查服务状态、WebUI 和节点数据面。

```bash
sudo ./target/release/fuckipv6 service upgrade --webui-port 8080
sudo fuckipv6 service status
```

`service upgrade` 不覆盖已有 `service.json`，并会保持升级前的运行/停止状态。

发布门禁：

```bash
python3 scripts/release-gate.py run --platform linux --print-plan
python3 scripts/release-gate.py run --platform linux --toolchain stable
```

发布报告会把 `lifecycle-cleanup` 作为必需检查；Linux 还必须通过策略路由/路由表、
DoH DNS、TUN 和 Exit 规则的有界清理回归。缺少测试或任一清理回归失败都会阻断平台
报告和跨平台聚合，不能通过修改摘要或文案绕过。

Linux 变更涉及 TUN、策略路由或 DNS 接管时，还应在具备 `/dev/net/tun` 的主机执行特权
命名空间验收：

```bash
sudo ./scripts/test-lifecycle-cleanup-netns.sh
```

脚本会先构建测试，再进入 private network/mount namespace。实际 TUN、规则、路由表和
临时 `resolv.conf` 均只存在于该命名空间；退出后由内核自动销毁，不修改宿主网络配置。
该验收需要 root 和 namespace 权限，不属于无特权发布门禁。

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
- 可直接启用内置 TLS，或由受信反向代理终止 TLS；两种边界都需独立配置和审计。

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
