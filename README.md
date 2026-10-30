# fuckipv6 - IPv6虚拟局域网组网软件

基于 IPv4/IPv6 双栈虚拟地址的组网软件，当前可直接用于受控环境组网。洋葱路由、Kademlia DHT 与分块传输已作为 P2P 基础设施接入，但尚未形成完整的用户产品流程。

## ✨ 核心能力

- 🚀 **P2P 直连与 QUIC Mesh**: UDP/QUIC 双模自适应直连，端到端 TLS 1.3 Peer Pinning 与数据流多路复用，低延迟
- 🔄 **Multi-Relay 与连接时择优**: 并发探测多个候选中继并按 RTT/握手耗时选择；目前不是持续迁移或动态负载均衡
- 🛡️ **安全与 ACL 策略**: Secure Session v2 classic 使用临时 X25519 + Ed25519、双向 key confirmation、计数 Nonce 与 4096 包重放窗口保护 UDP/TUN 数据；QUIC 与中继仅承载同一 v2 密文。QUIC 中继当前强制 Ed25519 客户端证书，并将证书 SPKI、Relay Register 应用身份、双 pin 与 TLS exporter 绑定，生产配置禁用 0-RTT；客户端证书的 CA/allowlist/撤销策略仍未实现。该 profile 尚未完成独立密码学审计；可选 `pq` feature 已实现 X-Wing draft-11、Ed25519 + ML-DSA-65 复合身份和实验性两消息混合 `HELLO/ACCEPT`，但尚未接入生产控制通道或数据面，且上游后量子 crate 未独立审计，不能据此宣称已具备后量子安全或超越 WireGuard。另有声明式入出站 ACL 与 Coordinator 准入黑名单
- 🔌 **进阶 NAT 穿透**: RFC 5780 NAT 类型检测 + RFC 6886 NAT-PMP + UPnP IGD 自动端口映射
- 🔒 **身份与鉴权**: Ed25519 签名验证；Coordinator Web/API 可配置 JWT，客户端本地 WebUI/API 当前免密并只应绑定本机或受信网络
- 🌐 **跨平台 TUN**: Linux TUN、macOS utun、Windows 内嵌 WinTun + 纯 Rust IP Helper API（仍需目标平台实机验证）
- 📊 **可观测性**: Coordinator Web 管理面板、客户端 WebUI/浏览器应用窗口、实时拓扑、流量监控与本地IPC控制台

## 📌 能力状态

| 状态 | 当前范围 |
|------|----------|
| **可直接使用** | 双栈 TUN 组网、UDP/QUIC P2P、QUIC/信令中继 fallback、智能分流、Linux Exit Node 内核转发/NAT、Coordinator WebUI、客户端 CLI/本地 WebUI、`host`/`join` 与多网络配置 |
| **基础设施已接入（待产品化）** | 多中继连接时择优、3-Hop 洋葱电路、Kademlia DHT、Blake3 分块传输；已有引擎接入与单元测试，但缺少完整 CLI/WebUI 用户流程 |
| **尚未产品化** | Tauri 原生桌面应用、用户态 NAT、洋葱/DHT/分块传输的端到端产品体验，以及 Windows/macOS Exit Node 与 Linux 同等级验证 |

## 📖 快速开始

### 1. 编译

```bash
git clone https://github.com/yourusername/fuckipv6
cd fuckipv6
cargo build --release
```

### 2. 启动协调服务器

```bash
# 在公网服务器上运行
./target/release/fuckipv6-coordinator
```

默认信令监听 `0.0.0.0:6666`，QUIC 中继监听 `0.0.0.0:6667`，Web管理界面监听 `0.0.0.0:8080`。

### 3. 配置并启动客户端

```bash
# 生成配置文件
./target/release/fuckipv6 init-config

# 编辑配置文件，修改coordinator_url
nano client.toml

# 启动客户端（需要root权限）
sudo ./target/release/fuckipv6 start
```

### 4. 测试连通性

```bash
# 在客户端A上ping客户端B的虚拟IP
ping6 fd00::1002
```

本机 Linux 端到端验证可使用 network namespace 脚本启动 1 个 coordinator 和 3 个隔离客户端，覆盖互 ping、路由和接近 MTU 的大包 ping：

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
┌──────────────────────┐
│  Coordinator Server  │  ← 公网服务器
│  - WebSocket信令     │     监听: [::]:6666
│  - QUIC中继          │     监听: [::]:6667
│  - IP地址分配        │
└──────────┬───────────┘
           │ WebSocket
    ┌──────┴──────┐
    │             │
┌───▼────┐   ┌───▼────┐
│Client A│◄──┤Client B│  ← UDP/QUIC P2P
│        │   │        │
│TUN设备 │   │TUN设备 │
│fd00::1 │   │fd00::2 │
└────────┘   └────────┘
```

**工作流程**：

1. 客户端连接协调服务器，获取虚拟IP（如`fd00::1001`）
2. 协调服务器通知所有客户端新节点上线
3. 客户端之间通过 UDP 探测和多中继测速验证可达端点
4. 应用流量先封装为 v2 密文，再按路径选择 QUIC Stream Mesh 或 UDP 直连；两种传输均不会接受静态密钥或明文降级
5. 如果 P2P 不可用，客户端优先通过 QUIC 中继转发；QUIC 中继不可用时降级到 WebSocket 信令中继

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
│   ├── client/          # 客户端
│   │   ├── tun/         # TUN设备管理
│   │   ├── p2p/         # P2P连接引擎
│   │   ├── signaling/   # 信令客户端
│   │   └── cli/         # 命令行界面
│   │
│   └── coordinator/     # 协调服务器
│       ├── signaling/   # WebSocket信令服务
│       ├── relay/       # QUIC中继服务
│       └── webui/       # Web管理界面和API
│
└── docs/
    └── QUICKSTART.md    # 快速开始指南
```

## 🎯 开发状态

### ✅ Phase 1 - 核心组网（可直接使用）

- [x] 信令服务器 (WebSocket)
- [x] IPv6地址分配器
- [x] TUN虚拟网卡 (Linux/macOS/Windows)
- [x] UDP P2P连接
- [x] 客户端注册和发现
- [x] 基本数据转发
- [x] CLI工具
- [x] Ed25519注册签名验证
- [x] UDP端点探测/基础打洞
- [x] 信令通道中继fallback
- [x] 本地IPC status/peers
- [x] 协调端Web UI/API

**当前可用功能**：
- 客户端注册到协调服务器
- 自动分配虚拟 IPv4/IPv6 双栈地址
- 客户端间 UDP/QUIC P2P 直连，失败时 QUIC/信令中继
- 通过虚拟IP进行通信（ping、SSH、文件传输）
- WebSocket 信令断线自动重连并重新注册
- `fuckipv6 status` / `fuckipv6 peers` 查看运行时状态、RTT、丢包率、带宽估计和 P2P 健康状态
- `fuckipv6 route` / `fuckipv6 logs` 查看路由和服务日志
- `fuckipv6 logs --service memory` 查看运行中客户端聚合的最近 warn/error；协调端支持 `/api/logs?service=memory`
- `fuckipv6 join/list/default/connect/leave` 管理并切换多个已保存网络配置
- `http://<coordinator>:8080` 通过 Web UI/API/WebSocket 查看在线节点、连接拓扑、实时流量和系统日志

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
- [ ] 接入 v2 在线重协商控制通道、无状态 Cookie 与限速，并完成独立密码学审计；当前 epoch 达到时限/字节上限后失败关闭而不是在线换钥
- [x] 实现 X-Wing draft-11、Ed25519 + ML-DSA-65 复合身份和实验性混合 `HELLO/ACCEPT`，固定 X-Wing/TH1/TH2 向量并覆盖降级、PSK、重放与非 contributory 输入失败关闭
- [ ] 将混合 profile 接入生产控制通道、QUIC/relay 与 UDP/TUN 数据面并完成独立审计；当前生产路径仍不具备端到端后量子机密性或后量子身份认证
- [x] QUIC 中继双向身份与通道绑定：强制客户端证书，Register 签名绑定证书/应用身份与 TLS exporter，错绑在 relay 状态写入前失败关闭，生产配置禁用 0-RTT（CA/allowlist/撤销与独立审计仍待完成）
- [x] ICE 候选收集、STUN、UPnP IGD、RFC 6886 NAT-PMP
- [x] Windows 内嵌 WinTun 与纯 Rust IP Helper
- [x] IPv4/IPv6 双栈虚拟地址与 Coordinator JWT 鉴权
- [x] 声明式 ACL、准入黑名单与多中继智能选路

### 🚧 Phase 6 - 下一代能力（基础设施已接入，待产品化）

- [x] 智能分流（MeshOnly / BypassMainland / GlobalProxy）与 Linux Exit Node 内核转发/NAT
- [x] `fuckipv6 host` 一键建网与 `fk://` 邀请票据
- [x] 多网络并发、独立 TUN 与路由隔离
- [x] 自适应 WebUI 与浏览器应用窗口（不是 Tauri 原生桌面）
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

## FAQ

**现在能用于真实组网吗？**

可以用于原型验证和受控环境测试。当前已实现 Linux/macOS/Windows TUN、UDP P2P、QUIC/信令中继 fallback、身份认证、NAT 类型检测和基础观测；单机 Linux network namespace 已覆盖 3 节点、路由、互 ping 和接近 MTU 的验证。生产上线前仍建议补充真实跨主机/跨 NAT 环境验证、外部安全审计和更大规模压力测试。

**必须有公网 IPv6 吗？**

不必须。虚拟网络承载 IPv6 包，但协调服务器和 P2P endpoint 可以使用 IPv4 或 IPv6 传输。协调服务器需要被客户端访问到。

**P2P 失败会怎样？**

客户端会在端点未验证、发送失败或健康阈值不达标时优先切换到 QUIC 中继；QUIC 中继不可用时再通过协调服务器 WebSocket 信令通道发送 `RelayPacket`。

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
**最后更新**: 2026-10-30
