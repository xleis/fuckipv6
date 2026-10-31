# fuckipv6 - IPv6虚拟局域网组网软件

基于 IPv4/IPv6 双栈虚拟地址的组网软件，当前可直接用于受控环境组网。洋葱路由、Kademlia DHT 与分块传输已作为 P2P 基础设施接入，但尚未形成完整的用户产品流程。

产品层只有一种身份：**节点**。只发布一个 `fuckipv6` 程序；创建、加入、托管网络、中继和 Exit Node 都是节点在某网络中的能力，不是“客户端/服务端”两种程序。目标形态是桌面环境使用 GPUI 原生界面，无桌面环境使用 WebUI，两者共享同一命令层、状态模型、校验和错误语义。当前已完成单二进制入口收敛，并由 `NodeRuntimeHandler` 接通动态启动、创建、加入、按网络停止与关闭。顶层桌面入口已接入真实 GPUI 原生窗口；11 个 manifest 页面已投影真实 `NodeSnapshot` 字段，10 个 manifest 动作均映射为类型化 `NodeCommand` 并只经 `NodeCommandBus` 提交。当前仍是功能垂直切片：正式 IME/无障碍、跨平台实机、WebUI manifest 迁移、路由/角色后端闭环和 GPUI/WebUI 自动语义对等门禁尚未完成。

## ✨ 核心能力

- 🚀 **P2P 直连与 QUIC Mesh**: UDP/QUIC 双模自适应直连，端到端 TLS 1.3 Peer Pinning 与数据流多路复用，低延迟
- 🔄 **Multi-Relay 与连接时择优**: 并发探测多个候选中继并按 RTT/握手耗时选择；目前不是持续迁移或动态负载均衡
- 🛡️ **安全与 ACL 策略**: Secure Session v2 classic 使用临时 X25519 + Ed25519、双向 key confirmation、计数 Nonce 与 4096 包重放窗口保护 UDP/TUN 数据；QUIC 与中继仅承载同一 v2 密文。QUIC 中继当前强制 Ed25519 客户端证书，并将证书 SPKI、Relay Register 应用身份、双 pin 与 TLS exporter 绑定，生产配置禁用 0-RTT；Coordinator 运行期黑名单已在 relay 注册和双向转发路径强制执行，但证书层 CA/allowlist/CRL/OCSP 与网络级授权仍未实现。该 profile 尚未完成独立密码学审计；可选 `pq` feature 已实现 X-Wing draft-11、Ed25519 + ML-DSA-65 复合身份和实验性两消息混合 `HELLO/ACCEPT`，但尚未接入生产控制通道或数据面，且上游后量子 crate 未独立审计，不能据此宣称已具备后量子安全或超越 WireGuard。另有声明式入出站 ACL 与 Coordinator 准入黑名单
- 🔌 **进阶 NAT 穿透**: RFC 5780 NAT 类型检测 + RFC 6886 NAT-PMP + UPnP IGD 自动端口映射
- 🔒 **身份与鉴权**: Ed25519 签名验证；网络服务 Web/API 可配置 JWT；节点本地 WebUI/API 默认只监听回环，远程监听必须显式开启危险开关并配置认证，无认证的远程 IPC 始终拒绝启动
- 🌐 **跨平台 TUN**: Linux TUN、macOS utun、Windows 内嵌 WinTun + 纯 Rust IP Helper API（仍需目标平台实机验证）
- 📊 **可观测性**: 节点 WebUI、网络服务管理面板、实时拓扑、流量监控、本地 IPC 控制台，以及带 11 个 manifest 页面和类型化动作接线的 GPUI 原生应用（IME、无障碍与完整对等仍在完善）

## 📌 能力状态

| 状态 | 当前范围 |
|------|----------|
| **可直接使用** | 单一 `fuckipv6` 节点程序、节点级稳定身份、双栈 TUN 组网、UDP/QUIC P2P、QUIC/信令中继 fallback、智能分流、Linux Exit Node 内核转发/NAT、节点 CLI/WebUI、`host`/`join` 与多网络配置；Linux 桌面可启动 GPUI 原生界面并提交共享命令 |
| **基础设施已接入（待产品化）** | GPUI 的 11 个页面、真实快照投影和 10 个 manifest 动作映射已形成垂直切片，但 WebUI manifest 迁移、GPUI/WebUI 自动语义对等、正式 IME/无障碍和跨平台实机仍待完成；路由/角色动作已接线，后端仍明确返回 `Unavailable`；统一节点动态运行时命令（启动、创建、加入、按网络停止）已接通后端处理器，但 CLI/WebUI 尚未全部迁移；多中继连接时择优、3-Hop 洋葱电路、Kademlia DHT、Blake3 分块传输已有引擎接入与单元测试，但缺少完整 UI 用户流程 |
| **尚未产品化** | GPUI/WebUI 完整语义对等与端到端交互测试、正式 IME/无障碍、跨平台 GPUI 实机验证、路由/角色后端闭环、用户态 NAT、洋葱/DHT/分块传输的端到端产品体验，以及 Windows/macOS Exit Node 与 Linux 同等级验证 |
| **Linux 握手救援（实验性）** | `fuckipv6 lxt-rescue server|client` 已在正式 Rust 二进制内提供 Linux 可选子命令；它只转送真实 SYN-ACK 完成 TCP 握手，不代理后续 TCP 数据。`experimental/` 继续保存 V4–V8 的 Python 参考实现、报告与证据；真实第三方中间盒、生产可靠性和独立安全审计仍未完成 |

## 📖 快速开始

### 1. 编译

```bash
git clone https://github.com/yourusername/fuckipv6
cd fuckipv6
cargo build --release
```

### 2. 在可达节点上创建网络

```bash
# 该节点同时提供信令与 QUIC 中继，并默认作为成员加入网络
sudo ./target/release/fuckipv6 host \
  --name home \
  --port 6666 \
  --relay-port 6667 \
  --headless \
  --webui-port 8080
```

节点启动后会输出 `fk://` 邀请票据。信令默认监听 TCP `6666`，QUIC 中继默认监听 UDP `6667`，WebUI 默认仅监听 TCP `127.0.0.1:8080`；可用 `FUCKIPV6_WEBUI_BIND=::1` 显式选择 IPv6 回环。托管网络是节点角色，不是另一个服务端程序。

### 3. 在其他节点加入网络

```bash
# 保存邀请票据并直接启动；该节点也会运行 WebUI
sudo ./target/release/fuckipv6 join "fk://..." \
  --name home \
  --start \
  --headless \
  --webui-port 8080
```

也可以先用 `fuckipv6 join "fk://..." --name home` 保存配置，再运行 `sudo fuckipv6 connect home --headless`。所有加入者使用同一个 `fuckipv6` 程序。

### 4. 测试连通性

```bash
# 在节点 A 上 ping 节点 B 的虚拟 IP
ping6 fd00::1002
```

本机 Linux 端到端验证可使用 network namespace 脚本启动 1 个托管节点和 3 个隔离成员节点，覆盖互 ping、路由和接近 MTU 的大包 ping：

```bash
sudo ./scripts/test-netns.sh
```

**详细文档**:
- [快速开始指南](docs/QUICKSTART.md)
- [架构说明](ARCHITECTURE.md)
- [API/协议参考](API.md)
- [用户手册（中文）](docs/USER_MANUAL.zh-CN.md)
- [User Manual](docs/USER_MANUAL.en.md)
- [部署指南](docs/DEPLOYMENT.md)

## 🏗️ 架构

```
┌──────────────────────────┐
│ Node A（同一 fuckipv6）  │  ← 承担网络服务角色
│ - WebSocket 信令         │     监听: [::]:6666
│ - QUIC 中继              │     监听: [::]:6667
│ - IP 地址分配            │
│ - 普通成员               │
└────────────┬─────────────┘
             │ 信令 / relay
      ┌──────┴──────┐
      │             │
┌─────▼─────┐ ┌─────▼─────┐
│  Node B   │◄┤  Node C   │  ← UDP/QUIC P2P
│ TUN fd00  │ │ TUN fd00  │
└───────────┘ └───────────┘
```

**工作流程**：

1. 成员节点连接网络服务，获取虚拟 IP（如 `fd00::1001`）
2. 网络服务通知同一网络中的成员节点有新节点上线
3. 节点之间通过 UDP 探测和多中继测速验证可达端点
4. 应用流量先封装为 v2 密文，再按路径选择 QUIC Stream Mesh 或 UDP 直连；两种传输均不会接受静态密钥或明文降级
5. 如果 P2P 不可用，节点优先通过 QUIC 中继转发；QUIC 中继不可用时降级到 WebSocket 信令中继

### 节点身份

节点身份独立于网络配置。首次启动节点控制面时，程序会在配置根目录创建 `node_identity.pem`，并在零网络状态下直接发布节点 ID 与公钥指纹；后续创建或加入的网络都复用这一身份，不会为每个网络生成不同的节点密钥。

配置根目录按以下顺序确定：

1. `$XDG_CONFIG_HOME/fuckipv6`
2. `$HOME/.config/fuckipv6`

可选文件 `node_identity.json` 用于设置节点显示名：

```json
{
  "display_name": "edge-gateway"
}
```

显示名首尾空白会被去除，空值表示不设置显示名，控制字符会被拒绝。`node_identity.pem` 包含节点私有密钥材料，不得提交到仓库、放入邀请票据或发送给其他节点。在 Unix 上，程序创建或重写该文件时会将其权限收紧为 `0600`；其他平台仍需按本地账户与 ACL 策略保护配置目录。

## 📦 项目结构

```
fuckipv6/
├── crates/
│   ├── common/          # 共享库
│   │   ├── protocol.rs  # 信令协议
│   │   ├── types.rs     # 数据类型
│   │   ├── config.rs    # 配置管理
│   │   └── crypto.rs    # Ed25519加密
│   │
│   ├── client/          # 节点进程、CLI 与界面适配器（历史 crate 名）
│   │   ├── tun/         # TUN设备管理
│   │   ├── p2p/         # P2P连接引擎
│   │   ├── signaling/   # 节点侧信令会话
│   │   └── cli/         # 命令行界面
│   │
│   └── coordinator/     # 内嵌网络服务库（无独立用户程序）
│       ├── signaling/   # WebSocket信令服务
│       ├── relay/       # QUIC中继服务
│       └── webui/       # Web管理界面和API
│
├── experimental/        # LXT UDP辅助TCP握手实验归档（非生产）
└── docs/
    └── QUICKSTART.md    # 快速开始指南
```

## 🧪 Linux 握手救援（实验性，已接入正式程序）

LingXiTunnel（LXT）UDP 辅助 TCP 握手救援已作为可选 Linux 子命令接入正式 `fuckipv6` Rust 二进制。服务端仍需单独运行目标 TCP 服务；该子命令只捕获其 SYN-ACK。当客户端正常路径收不到 SYN-ACK 时，服务端捕获内核产生的真实 SYN-ACK，经 HMAC-SHA256 认证的 UDP 通道送达客户端，再注入独立 TUN。握手完成后，业务数据仍走原生 TCP；这不是 FakeTCP，也不是把 TCP 数据封装进 UDP，更不是通用代理或穿透保证。

### 准备共享密钥

服务端和客户端必须使用同一个密钥文件；密钥只从 `0600` 文件读取，不通过命令行传入：

```bash
sudo install -d -m 0700 /etc/fuckipv6
sudo sh -c 'umask 077 && openssl rand -hex 32 > /etc/fuckipv6/lxt-rescue.key'
sudo chmod 600 /etc/fuckipv6/lxt-rescue.key
```

### 启动服务端和客户端

```bash
# 服务端：捕获目标 TCP 端口产生的真实 SYN-ACK
sudo ./target/release/fuckipv6 lxt-rescue server \
  --interface eth0 \
  --tcp-port 39090 \
  --bind 0.0.0.0 \
  --udp-port 39091 \
  --key-file /etc/fuckipv6/lxt-rescue.key

# 客户端：记录发往服务端的 SYN，并把认证 UDP 收到的 SYN-ACK 注入 TUN
sudo ./target/release/fuckipv6 lxt-rescue client \
  --interface eth0 \
  --server 198.51.100.10 \
  --tcp-port 39090 \
  --udp-port 39091 \
  --hello-interval-ms 1500 \
  --tun lxtrescue0 \
  --key-file /etc/fuckipv6/lxt-rescue.key
```

服务端需要 root 或 `CAP_NET_RAW`；客户端需要 root，或 `CAP_NET_RAW` + `CAP_NET_ADMIN`，并需要可用的 `/dev/net/tun` 与 `ip` 命令。客户端还依赖 UDP 可达以及 NAT/conntrack 保留后续 TCP 状态。

### 实验验证与边界

| 版本 | 验证内容 | 结果 |
|---|---|---|
| V4 | 两台真实公网主机、8 项基础验收 | 8/8 PASS |
| V5 | 18 MiB 双向分块负载与 MSS 诊断 | 正常/救援均 PASS；MSS 1332 vs 1460 仍需 clamp/验证 |
| V6 | HTTP CONNECT 代理访问 Google | 3 次救援 PASS；HTTP/2 200/204 |
| V7 | netns 模拟中间路由器的两种 SYN-ACK 丢弃位置 | FORWARD 丢弃 PASS；PREROUTING raw 丢弃因 NAT/conntrack 状态缺失 FAIL |
| V8 | 无 NAT 三 netns 验证 invalid 流量处理 | 默认放行 invalid PASS；严格 `ct state invalid drop` FAIL |

当前正式接入范围是 Linux、IPv4、单个 TCP 端口和单客户端场景。它不能保证穿透真实第三方中间设备，也不处理已建立 TCP 流的 UDP 代理、限速、ACL 或跨平台运行；MSS 处理、真实运营商/防火墙测试、生产可靠性和独立安全评审仍属于后续工作。历史 Python 参考实现、报告和证据见 [`experimental/`](experimental/README.md)。

## 🎯 开发状态

### ✅ Phase 1 - 核心组网（可直接使用）

- [x] 网络服务信令 (WebSocket，内嵌节点)
- [x] IPv6地址分配器
- [x] TUN虚拟网卡 (Linux/macOS/Windows)
- [x] UDP P2P连接
- [x] 成员节点注册和发现
- [x] 基本数据转发
- [x] CLI工具
- [x] Ed25519注册签名验证
- [x] UDP端点探测/基础打洞
- [x] 信令通道中继fallback
- [x] 本地IPC status/peers
- [x] 网络服务 Web UI/API

**当前可用功能**：
- 成员节点注册到网络服务
- 自动分配虚拟 IPv4/IPv6 双栈地址
- 节点间 UDP/QUIC P2P 直连，失败时 QUIC/信令中继
- 通过虚拟IP进行通信（ping、SSH、文件传输）
- WebSocket 信令断线自动重连并重新注册
- `fuckipv6 status` / `fuckipv6 peers` 查看运行时状态、RTT、丢包率、带宽估计和 P2P 健康状态
- `fuckipv6 route` / `fuckipv6 logs` 查看路由和服务日志
- `fuckipv6 logs --service memory` 查看运行中节点聚合的最近 warn/error；网络服务 API 支持 `/api/logs?service=memory`
- `fuckipv6 join/list/default/connect/leave` 管理并切换多个已保存网络配置
- `http://127.0.0.1:8080` 通过 Web UI/API/WebSocket 查看在线节点、连接拓扑、实时流量和系统日志

### ✅ Phase 2 - 传输与穿透（可直接使用）

- [x] 基础UDP打洞探测
- [x] 完整NAT类型检测（STUN/RFC5780）
- [x] QUIC 协议能力（证书/pin、双向流、多路复用、0-RTT 自检与 QUIC P2P 直连）
- [x] 信令通道中继服务（P2P失败时）
- [x] 独立 QUIC 中继服务（注册、转发、限速、并发控制和 loopback 集成测试；更大规模真实 WAN 压测仍建议补充）
- [x] 连接质量监控（RTT、丢包率、健康阈值）
- [x] 自动Fallback机制（P2P未验证/发送失败/健康阈值不达标）
- [x] Windows WinTun 支持
- [x] macOS utun 支持

### ✅ Phase 3 - 管理与可观测性（可直接使用）

- [x] Web管理界面（在线节点/API）
- [x] 性能优化
- [x] 多路复用
- [x] `status` / `peers` 运行时IPC
- [x] `route` / `logs` 等扩展CLI命令

### 🚧 Phase 4 - 生产就绪（持续验证）

- [ ] 独立安全审计（仅完成依赖漏洞扫描；人工协议与密码学审计尚未执行）
- [x] 压力测试（QUIC relay 100 个已注册客户端并发连接）
- [x] 监控和日志
- [x] 部署工具（systemd、安装脚本、Docker、CI、Release 自动发布）

### ✅ Phase 5 - 安全与策略（基础能力可用）

- [x] Secure Session v2 classic（临时 X25519 + Ed25519、确认帧、计数 Nonce、重放窗口）已接入生产 `P2pEngine` 的 UDP/TUN 数据面；旧版静态密钥数据包失败关闭，QUIC 与 relay 只承载同一 v2 密文
- [x] 在 UDP/TUN 数据面接入 v2 在线 REKEY：常驻 initiator 在会话建立后按时间/字节/包计数发起换钥，通过 staged/active/retired epoch 和 CONFIRM 重传完成切换，失败时保留旧会话并拒绝 suite 降级或 epoch 回滚
- [ ] 将认证 REKEY 接入 QUIC/relay 控制通道，补齐无状态 Cookie、生产限速和跨真实网络切换测试，并完成独立密码学审计；当前 QUIC/relay 仍未完成在线换钥
- [x] 实现 X-Wing draft-11、Ed25519 + ML-DSA-65 复合身份和实验性混合 `HELLO/ACCEPT/CONFIRM`，固定向量覆盖 transcript、截断/超长、版本与 kind 别名、降级、PSK、重放和非 contributory 输入失败关闭；严格 wire 层已通过 transport-independent `SessionDriver` 适配接入双向数据面
- [ ] 将混合 profile 的严格 wire 层接入 SessionDriver 与生产控制通道、QUIC/relay 和 UDP/TUN 数据面，并完成互操作、容量与独立审计；当前生产路径仍不具备端到端后量子机密性或后量子身份认证
- [x] QUIC 中继双向身份与通道绑定：强制客户端证书，TLS 层只接受当前已完成签名注册的节点公钥 allowlist，Register 签名绑定证书/应用身份与 TLS exporter，错绑在 relay 状态写入前失败关闭，生产配置禁用 0-RTT；运行期黑名单已在 relay 注册和双向转发路径强制执行（CA/CRL/OCSP、稳定节点身份与当前公钥解耦、多 Coordinator 即时同步、P2P QUIC Mesh 完整撤销/重连策略、授权 epoch/租约绑定与独立审计仍待完成；持久撤销目录和 Coordinator 撤销 API 已落地）
- [x] ICE 候选收集、STUN、UPnP IGD、RFC 6886 NAT-PMP
- [x] Windows 内嵌 WinTun 与纯 Rust IP Helper
- [x] IPv4/IPv6 双栈虚拟地址与网络服务 JWT 鉴权
- [x] 声明式 ACL、准入黑名单与多中继智能选路

### 🚧 Phase 6 - 下一代能力（基础设施已接入，待产品化）

- [x] 智能分流（MeshOnly / BypassMainland / GlobalProxy）与 Linux Exit Node 内核转发/NAT
- [x] `fuckipv6 host` 一键建网与 `fk://` 邀请票据
- [x] 多网络并发、独立 TUN 与路由隔离
- [x] 自适应 WebUI；`--webui` 仍显式启动浏览器托管界面
- [x] 真实 GPUI 原生外壳：窗口、基础主题/导航、共享 manifest、`NodeStateStore` 订阅、`NodeCommandBus` 命令、错误提示和退出清理
- [x] GPUI 的 11 个 manifest 页面、真实快照投影、空态/错误态与类型化动作映射垂直切片
- [ ] GPUI/WebUI 完整语义对等测试、正式 IME/无障碍与跨平台实机验证
- [x] 3-Hop 洋葱路由、Kademlia DHT 与 Blake3 分块传输基础设施（引擎与测试已接入，尚无完整用户操作流程）

生产发布前仍建议完成真实跨公网/跨 NAT、Windows/macOS 实机、更大规模 WAN 压测和外部安全审计。

## 🛠️ 技术栈

- **语言**: Rust 2021
- **异步运行时**: Tokio
- **网络**:
  - QUIC Stream Mesh 与加密 UDP 双模 P2P 直连
  - QUIC 中继与 WebSocket 信令 fallback
  - TUN/TAP (虚拟网卡)
- **序列化**: postcard
- **加密**: Secure Session v2 classic（临时 X25519、Ed25519、HKDF-SHA-512、HMAC-SHA-256、ChaCha20-Poly1305 / AES-256-GCM）；后量子迁移与审计边界见 `SECURITY_PERFORMANCE_TODO.md`
- **GUI**: GPUI 0.2（Linux 原生窗口已实机/Xvfb 冒烟；Windows/macOS 仍待目标平台验证）
- **CLI**: clap

## 📊 性能目标

| 指标 | Phase 1 (MVP) | Phase 2 (优化后) |
|------|---------------|------------------|
| P2P延迟 | <50ms | <10ms (局域网) |
| 吞吐量 | 100+ Mbps | 500+ Mbps |
| 连接建立 | <3秒 | <1秒 |
| 内存占用 | <100MB | <50MB |
| CPU占用 | <20% | <5% (空闲) |

2026-10-30 的同机端到端基线尚未达标：单流 TCP 约 `0.055 Gbit/s`，内核 WireGuard 为 `1.479 Gbit/s`；受控 1420 字节 UDP 为 `0.040 Gbit/s`，WireGuard 为 `0.050 Gbit/s`。当前结果是高负载环境中的噪声较大的基线，不是超越 WireGuard 的证明。原始数据、限制和复现命令见 [`benchmarks/data/2026-10-30-dataplane-baseline`](benchmarks/data/2026-10-30-dataplane-baseline/README.md)。

CPU profiling 可使用 `PROFILE_SECONDS=10 ./scripts/profile-cpu.sh` 对协议/relay 基准采样；脚本优先生成 flamegraph，未安装 `cargo-flamegraph` 时降级输出 `target/profiling/*.perf-report.txt`。

## ✅ 发布门禁

发布门禁要求工作区干净，并依次执行格式检查、辅助脚本测试、workspace check/Clippy/test、release build、平台交叉编译检查和 Criterion quick 快速基准。Linux 默认额外检查 `x86_64-unknown-linux-musl` 与 `x86_64-pc-windows-gnu`。若显式提供匹配硬件的 Criterion 基线，门禁还会按回退阈值、噪声余量、标准误和最小样本数拒绝相对性能回退。

```bash
# 先查看解析后的命令计划，不执行编译
python3 scripts/release-gate.py run --platform linux --print-plan

# 正式门禁；结果写入 target/release-gate/linux.json，日志写入同级 artifacts 目录
python3 scripts/release-gate.py run --platform linux --toolchain stable

# 保存匹配硬件的 full 基线，并在后续 full 门禁中启用回归判定
python3 scripts/bench-compare.py snapshot \
  --criterion target/criterion \
  --output benchmarks/baselines/linux-host-criterion.json
python3 scripts/release-gate.py run --platform linux --toolchain stable \
  --benchmark full \
  --benchmark-baseline benchmarks/baselines/linux-host-criterion.json
```

基线必须来自同一平台和可比较的硬件/负载条件；`--quick` 默认只有 2 个 Criterion 样本，低于比较器默认的 10 样本门槛，正式回归判定应使用 `--benchmark full`。未提供 `--benchmark-baseline` 时报告明确记录比较状态为 `not_run`，不会把缺少基线算作通过。诊断脏工作区时可使用 `--allow-dirty`，但报告的 `release_ready` 必定为 `false`。CI 会用 `verify.yml` 汇总 Linux、Windows、macOS 报告，只有三平台同 commit、干净且至少一个平台产生机器可读基准结果时才允许发布。门禁只证明工程检查可执行和同机基准未无声回退，不证明已达到 WireGuard 性能对等，也不替代真实跨平台网络测试、协议审查或独立密码学审计。

## 🤝 贡献

欢迎贡献！请查看 [CONTRIBUTING.md](CONTRIBUTING.md)。

## ⚠️ 注意事项

- **Linux需要 root 权限或 CAP_NET_ADMIN** 创建 TUN 设备；macOS 需要 sudo/root 创建 utun 并配置路由；Windows 需要管理员权限和 WinTun
- **虚拟网络支持 IPv4/IPv6 双栈地址**，信令与 P2P endpoint 也可使用 IPv4 或 IPv6 传输
- **NAT 类型检测依赖可访问的 STUN/RFC5780 服务器**；检测失败会标记为 `Unknown`，数据转发仍会在 P2P 失败时走 QUIC/信令中继

### 本地控制面安全

WebUI/API 与 IPC 默认都只监听回环地址，回环 WebUI 默认免密。远程 WebUI/API 必须同时显式设置危险开关和认证配置，否则节点拒绝启动：

```bash
export FUCKIPV6_WEBUI_BIND=0.0.0.0
export FUCKIPV6_WEBUI_ALLOW_REMOTE=1
export FUCKIPV6_WEBUI_USERNAME=admin
export FUCKIPV6_WEBUI_PASSWORD='使用足够长的随机密码'
export FUCKIPV6_WEBUI_JWT_SECRET='使用足够长的随机签名密钥'
```

远程控制面没有内置 TLS，只能部署在受控管理网或 TLS 反向代理之后，不应直接暴露到公网。REST API 使用 `Authorization: Bearer <token>`，浏览器 WebSocket 使用 `/ws?token=<token>`；其他静态页面和 `/api/auth/login` 可匿名访问。IPC 尚无认证协议，`FUCKIPV6_IPC_ALLOW_REMOTE=1` 也不会放行非回环监听，请使用 SSH 隧道。

## FAQ

**现在能用于真实组网吗？**

可以用于原型验证和受控环境测试。当前已实现 Linux/macOS/Windows TUN、UDP P2P、QUIC/信令中继 fallback、身份认证、NAT 类型检测和基础观测；单机 Linux network namespace 已覆盖 3 节点、路由、互 ping 和接近 MTU 的验证。生产上线前仍建议补充真实跨主机/跨 NAT 环境验证、外部安全审计和更大规模压力测试。

**必须有公网 IPv6 吗？**

不必须。虚拟网络承载 IPv6 包，但网络服务与 P2P endpoint 可以使用 IPv4 或 IPv6 传输。至少有一个成员节点或独立可达端点需要能被其他成员访问，用于信令和 fallback 中继。

**P2P 失败会怎样？**

节点会在端点未验证、发送失败或健康阈值不达标时优先切换到 QUIC 中继；QUIC 中继不可用时再通过网络服务的 WebSocket 信令通道发送 `RelayPacket`。

**为什么默认 MTU 是 1280？**

1280 是 IPv6 最小 MTU，能减少跨网络路径上的分片和不可达问题。

**如何切换多个网络配置？**

使用 `fuckipv6 join <server> --name <name>` 保存网络，`fuckipv6 list` 查看，`fuckipv6 default <name>` 设置默认网络，`fuckipv6 connect <name>` 连接指定网络。

## 📝 许可证

MIT License - 详见 [LICENSE](LICENSE) 文件

## 🔗 相关项目

- [Tailscale](https://tailscale.com/) - 商业VPN方案
- [WireGuard](https://www.wireguard.com/) - 现代VPN协议
- [ZeroTier](https://www.zerotier.com/) - SD-WAN解决方案
- [Nebula](https://github.com/slackhq/nebula) - 可扩展overlay网络

## 📧 联系方式

- Issues: [GitHub Issues](https://github.com/yourusername/fuckipv6/issues)
- Discussions: [GitHub Discussions](https://github.com/yourusername/fuckipv6/discussions)

---

**当前版本**: v0.3.0（Phase 6 功能阶段）
**最后更新**: 2026-10-31
