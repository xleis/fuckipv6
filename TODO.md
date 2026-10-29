# fuckipv6 项目 TODO 清单

## 功能闭环执行清单（2026-10-30）

> 本轮不以“页面已有入口”作为完成标准，而以“操作真实作用于运行中的客户端、状态来自真实运行时、失败不会伪装成功”为完成标准。

- [x] **FC-01 运行时控制通道**：让 WebUI/API 的断开、路由更新等控制命令真正到达 `ClientRuntime`，而不是只修改 `AppController` 快照。
- [x] **FC-02 路由热更新闭环**：`POST /api/routing` 更新运行中的 `SmartRouteEngine`、出口节点配置与持久化配置；接口失败时前端不得提示成功。
- [x] **FC-03 真实节点探测**：移除 WebUI 中伪造的 Ping 延迟，增加真实 P2P 探针接口与超时结果。
- [x] **FC-04 真实运行时快照**：WebUI 状态、流量和链路数据优先来自 IPC 运行时快照，不再用全零或模拟值代替。
- [x] **FC-05 Linux 策略路由生命周期**：在 Linux 启动、路由热切换和退出时安装/更新/回滚 `LinuxRouteManager`。
- [x] **FC-06 UI 真实性**：对后端未支持的操作禁用或明确标识“仅预览”，成功提示只在后端确认后显示。
- [x] **FC-07 文档对齐**：区分“可直接使用”“基础设施已接入（待产品化）”“尚未产品化”，同步 README、NEXTGEN_TODO、API 与架构文档。
  - [x] README：标注核心能力、能力边界与各阶段成熟度。
  - [x] CHANGELOG：纠正 Tauri、JWT、Exit Node 与 Overlay 基础设施的历史描述。
  - [x] NEXTGEN_TODO：修正用户态 NAT、Tauri 原生桌面和 Agent 5 “已完成”口径。
  - [x] API：补充客户端 WebUI/API、免密边界、状态字段与控制接口。
  - [x] ARCHITECTURE：同步当前实现边界与“基础设施已接入”状态。
- [ ] **FC-08 回归验证**：补充单元/集成测试，执行格式、Clippy、全工作区测试及 Linux/Windows 编译检查。

## 当前状态总结 ✅

### Phase 1 MVP - 已完成的功能

- ✅ **协调服务器（Coordinator）**
  - WebSocket 信令服务（IPv4/IPv6 双栈）
  - 客户端注册和会话管理
  - Ed25519 注册签名验证
  - IPv6 地址池管理（fd00::1000/120）
  - 客户端上线/下线广播
  - 信令通道 RelayPacket 中继 fallback
  - Web 管理界面/API（/api/status, /api/peers）
  - 心跳机制
  - 重复连接处理（死锁修复）

- ✅ **客户端（Client）**
  - TUN 设备创建和配置（Linux，MTU 1280）
  - IPv6 地址和路由自动配置
  - 基础 P2P 引擎（UDP 监听）
  - P2P 端点探测和 ping/pong 基础打洞
  - P2P 失败时通过信令通道中继
  - WebSocket 信令客户端
  - 本地 IPC 状态接口（默认 127.0.0.1:6668）
  - 配置文件自动管理（~/.config/fuckipv6/）
  - CLI 命令：join, connect, list, leave, default, status, peers, route, logs, stats, version, init-config
  - 性能统计收集器框架

- ✅ **通用库（Common）**
  - 协议定义（SignalingMessage, DataPacket, PeerInfo）
  - 配置结构体
  - 类型定义（ClientId, NatType）
  - Ed25519 身份、签名和指纹校验

### 关键 Bug 修复记录

1. ✅ Bincode 序列化兼容性（移除 `#[serde(tag)]` 等属性）
2. ✅ TUN 设备 IPv6 支持（改用 `ip tuntap` 命令创建）
3. ✅ IPv6 最小 MTU 要求（1200 → 1280）
4. ✅ 服务器端死锁修复（RwLock read/write 锁顺序）
5. ✅ 客户端注册消息发送时序（异步任务启动延迟）
6. ✅ P2P endpoint 上报错误（改为上报真实 UDP 监听端口）
7. ✅ `status` / `peers` 占位命令（改为本地 IPC 运行时快照）
8. ✅ TUN 创建错误提示和退出清理改进

---

## Phase 1 剩余工作 🚧

### 1. P2P 数据包转发测试

**优先级：高 🔴**

**目标：** 实现两个客户端之间的实际数据包转发

**待完成任务：**
- [x] 实现完整的 TUN → P2P → TUN 数据转发链路
- [x] 测试两个客户端互 ping（`sudo ./scripts/test-netns.sh` 已验证）
- [x] 验证路由表正确性（`sudo ./scripts/test-netns.sh` 已验证）
- [x] 测试大包传输（接近 MTU；`sudo ./scripts/test-netns.sh` 已验证）
- [x] 测试并发多客户端场景（3+ 节点；`sudo ./scripts/test-netns.sh` 已验证）

**当前问题：**
- 本地单测已覆盖 P2P 引擎间 UDP 数据转发
- `sudo ./scripts/test-netns.sh` 已在单机 Linux network namespace 中通过，覆盖 3 个隔离客户端、路由、互 ping 和接近 MTU 的 ping
- 仍建议补充真实双主机跨 NAT/跨公网环境验证

**验证步骤：**
```bash
# 客户端 A
sudo ./fuckipv6 connect
# 获得 IP: fd00::1001

# 客户端 B（另一台机器）
sudo ./fuckipv6 connect
# 获得 IP: fd00::1002

# 客户端 A ping 客户端 B
ping6 fd00::1002
```

---

### 2. 错误处理改进

**优先级：中 🟡**

**待完成任务：**
- [x] 优雅处理 TUN 设备创建失败（权限不足、设备已存在、iproute2 缺失、MTU 非法）
- [x] WebSocket 断线自动重连
- [x] 协调端心跳超时检测和客户端清理
- [x] 客户端 WebSocket 断线自动重连
- [x] 客户端退出时优雅关闭 TUN 设备（Ctrl+C / SIGTERM）

**代码位置：**
- `crates/client/src/tun/linux.rs` - TUN 错误处理
- `crates/client/src/signaling/client.rs` - 重连逻辑
- `crates/coordinator/src/signaling/server.rs` - 超时清理

---

### 3. 日志优化

**优先级：低 🟢**

**待完成任务：**
- [x] 移除调试用的 🔍 emoji 日志
- [x] 统一日志格式（client/coordinator 共用 common tracing 初始化）
- [x] 添加日志级别配置（默认 INFO，`RUST_LOG` 优先）
- [x] 减少正常运行时的日志输出（协调端高频信令消息降级到 DEBUG/TRACE）

---

## Phase 2: 核心功能 🎯

### 1. NAT 穿透（UDP 打洞）

**优先级：高 🔴**

**目标：** 实现点对点直连，无需中继

**待完成任务：**
- [x] NAT 类型检测（注册前 STUN/RFC5780 探测并上报协调端）
  - Full Cone NAT
  - Restricted Cone NAT
  - Port-Restricted Cone NAT
  - Symmetric NAT
- [x] 基础同步 UDP 打洞探测
  - 向对端所有 endpoint 发送 UDP 包
  - 启动阶段快速探测，之后每 30 秒健康探测
- [x] 多 endpoint 尝试
- [x] 打洞成功率统计（`fuckipv6 peers` Probe 列）

**文件：**
- `crates/client/src/p2p/punching.rs` - STUN/RFC5780 NAT 类型检测
- `crates/common/src/types.rs` - NatType
- `crates/common/src/protocol.rs` - 注册消息携带 NAT 类型

**参考资料：**
- [RFC 5389 - STUN](https://datatracker.ietf.org/doc/html/rfc5389)
- [P2P 通信原理与实现](https://tailscale.com/blog/how-nat-traversal-works/)

---

### 2. QUIC 协议升级

**优先级：高 🔴**

**目标：** 将 UDP 升级为 QUIC，获得加密和可靠传输

**待完成任务：**
- [x] 集成 quinn crate（基础 QUIC 配置和 loopback 自检）
- [x] 生成自签名证书
- [x] 客户端证书验证（公钥 pin）
- [x] 双向 QUIC 流（frame send/recv）
- [x] 连接多路复用（并发 bidirectional streams 测试）
- [x] 0-RTT 重连优化（loopback 重连自检验证 resumed session 早期数据被接受）

**依赖：**
```toml
quinn = "0.11"
rustls = "0.23"
rcgen = "0.13"  # 证书生成
```

**文件：**
- `crates/client/src/p2p/quic.rs` - QUIC 配置、pin verifier、双向流 frame 和 loopback 自检
- `crates/common/src/quic.rs` - 自签名证书、公钥 pin、QUIC frame 和 relay wire format

---

### 3. 中继服务（Relay）

**优先级：中 🟡**

**目标：** P2P 失败时通过服务器中转

**待完成任务：**
- [x] 实现信令通道中继 fallback
- [x] 实现 QUIC 中继服务器
  - 监听 0.0.0.0:6667
  - 数据包转发
  - 带宽限制（1 Mbps/客户端）
- [x] 客户端自动切换到 QUIC 中继 fallback
  - [x] 连接到中继服务器（`QuicRelayClient`）
  - [x] Fallback 触发逻辑（P2P `send_to` 失败后优先 QUIC relay，失败再降级信令 relay）
  - [x] 自动切回 P2P（每个 TUN 包仍先尝试 P2P，成功即不走 relay）
- [x] 中继流量统计（信令 RelayPacket 数、payload 字节、错误数，Web UI/API/WebSocket 展示）
- [x] QUIC 中继 loopback 集成测试（两个已注册客户端经 coordinator relay 转发）

**Fallback 触发条件：**
- P2P 连接失败超过 10 秒
- RTT > 200ms
- 丢包率 > 5%

**文件：**
- `crates/coordinator/src/relay/server.rs` - QUIC relay 监听、注册、转发和 1 Mbps/客户端 token bucket
- `crates/client/src/p2p/relay.rs` - pinned QUIC relay 客户端、注册、收发包 API

---

### 4. 加密和认证

**优先级：高 🔴**

**目标：** 防止未授权客户端加入网络

**待完成任务：**
- [x] Ed25519 密钥对生成
- [x] 客户端注册时签名验证
- [x] QUIC 证书公钥验证
- [x] 密钥持久化

**依赖：**
```toml
ed25519-dalek = "2"
```

**文件：**
- `crates/common/src/crypto.rs` - 扩展
- `crates/common/src/identity.rs` - 已存在，需完善

---

### 5. Windows TUN 支持 ✅ **已完成**

**优先级：中 🟡**

**目标：** 支持 Windows 客户端

**已完成任务：**
- [x] 独立自主化：内嵌 WHQL 驱动自解压，消除外部下载与外部 wintun.dll 依赖
- [x] 纯 Rust 网络配置：移除外部 netsh 进程调用，全量复刻为 Windows IP Helper API (`CreateUnicastIpAddressEntry` / `CreateIpForwardEntry2`)
- [x] 实现 `crates/client/src/tun/windows.rs`
- [x] 管理员权限检查（使用 windows-sys API）
- [x] 构建脚本（setup_windows.ps1 + setup_windows.bat 无外部依赖构建）
- [x] 文档同步：README_WINDOWS.md 和 QUICKSTART_WINDOWS.md

**依赖：**
```toml
wintun = "0.5"
windows-sys = { version = "0.59", features = [
    "Win32_Foundation",
    "Win32_System_Threading",
    "Win32_Security",
    "Win32_NetworkManagement_IpHelper",
    "Win32_NetworkManagement_Ndis",
    "Win32_Networking_WinSock",
] }
```

**实现细节：**
- 使用 WinTun 内核驱动（轻量级、高性能）
- 支持管理员权限自动检测
- 自动清理残留的网络适配器
- 实现 Drop trait 确保资源正确释放

**文件位置：**
- 实现：`crates/client/src/tun/windows.rs`
- 安装脚本：`setup_windows.ps1`, `setup_windows.bat`
- 文档：`README_WINDOWS.md`, `QUICKSTART_WINDOWS.md`

**参考：**
- [WinTun 官方文档](https://www.wintun.net/)
- [wintun crate](https://crates.io/crates/wintun)

---

### 5.1 macOS utun 支持 ✅ **已完成**

**优先级：中 🟡**

**目标：** 支持 macOS 客户端

**已完成任务：**
- [x] 接入 `tun` crate 的 macOS utun 后端
- [x] 支持系统自动分配 `utunN`，也支持配置固定 `utunN`
- [x] 处理 macOS utun 4 字节 packet information header
- [x] 使用 `ifconfig` 配置 IPv6 地址
- [x] 使用 `route` 配置 `fd00::/64` IPv6 路由
- [x] 扩展 `route`、`logs`、`stats` CLI 的 macOS 平台实现
- [x] launchd plist 和 macOS 安装脚本
- [x] CI 和 Release 增加 macOS x86_64/arm64 覆盖

**文件位置：**
- 实现：`crates/client/src/tun/macos.rs`
- 平台抽象：`crates/client/src/tun/mod.rs`
- CLI：`crates/client/src/cli/commands.rs`
- 安装脚本：`scripts/install-macos-client.sh`, `scripts/install-macos-coordinator.sh`
- launchd：`scripts/fuckipv6-client.plist`, `scripts/fuckipv6-coordinator.plist`
- CI/Release：`.github/workflows/ci.yml`, `.github/workflows/release.yml`

---

### 6. 连接健康检查

**优先级：中 🟡**

**目标：** 自动检测连接质量并切换

**待完成任务：**
- [x] RTT 测量（每 30 秒 ping）
- [x] 丢包率统计
- [x] 带宽估计
- [x] 自动 P2P ↔ 中继切换（健康不达标走信令中继，健康恢复后切回 P2P）
- [x] 连接质量报告（ControlMessage::QualityReport）

**文件：**
- `crates/client/src/p2p/health.rs`
- `crates/common/src/protocol.rs` - 扩展 ControlMessage

---

## Phase 3: 优化和 UI 🎨

### 1. 性能优化

**优先级：中 🟡**

**待完成任务：**
- [x] 减少数据面分配（TUN read 独立任务复用缓冲区；P2P/QUIC relay 发送接口改为借用 payload，避免 fallback 前多次复制）
- [x] 并发控制（QUIC relay 连接/数据流 tokio task 数量限制）
- [x] 内存池（TUN reader 使用固定容量 buffer pool 复用入站数据包缓冲区）
- [x] CPU 性能分析（`PROFILE_SECONDS=1 ./scripts/profile-cpu.sh`，生成 `target/profiling/fuckipv6-common-protocol.perf-report.txt`；未安装 `cargo-flamegraph` 时自动降级为 `perf report`）
- [x] 压力测试（QUIC relay 100 个已注册客户端并发连接）

**性能目标：**
- P2P 延迟：<10ms (LAN) / <50ms (WAN)
- 吞吐量：>100 Mbps (单连接)
- 内存：<50MB (客户端) / <200MB (服务器)
- CPU：<5% (空闲) / <20% (满载)

---

### 2. Web 管理界面

**优先级：低 🟢**

**目标：** 可视化监控网络状态

**待完成任务：**
- [x] Axum Web 服务器（0.0.0.0:8080）
  - [x] 前端页面（HTML）
  - [x] 客户端列表
  - [x] 连接拓扑图（内置 SVG，随 WebSocket 快照实时更新）
  - [x] 实时流量统计（信令收发、RelayPacket payload、WebSocket 速率展示）
  - [x] 系统日志查看
- [x] WebSocket 实时更新
- [x] REST API

**文件：**
- `crates/coordinator/src/webui/` - 新建
- `crates/coordinator/src/webui/static/` - 静态资源

**依赖：**
```toml
axum = "0.7"
tower-http = "0.5"  # CORS、静态文件
```

---

### 3. 完善 CLI

**优先级：中 🟡**

**待完成任务：**
- [x] `fuckipv6 peers` - 查看所有节点（IPC）
- [x] `fuckipv6 ping <peer>` - 测试延迟（调用系统 ping6）
- [x] `fuckipv6 route` - 查看路由表（Linux/macOS/Windows）
- [x] `fuckipv6 logs` - 查看日志
- [x] `fuckipv6 status` - 实时状态（IPC）
- [x] IPC 通信机制（本地 TCP 控制接口，默认 127.0.0.1:6668）

**文件：**
- `crates/client/src/cli/commands.rs` - 扩展
- `crates/client/src/ipc/` - 新建

---

### 4. 配置文件完善

**优先级：低 🟢**

**待完成任务：**
- [x] 支持多网络配置切换（join/list/default/connect/leave，多配置目录隔离）
- [x] 配置验证和默认值
- [x] 热重载配置（运行时应用 `p2p.fallback_threshold`；`log.level` 变更提示需重启）
- [x] 配置文件加密（Argon2id + ChaCha20-Poly1305，`encrypt-config`/`decrypt-config`，`.enc` 启动解密）

**示例配置：**
```toml
[network]
coordinator_url = "wss://your-server.com:6666"
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

[log]
level = "info"
file = "/var/log/fuckipv6.log"
```

---

## Phase 4: 生产就绪 🚀

### 1. 错误处理

**优先级：高 🔴**

**待完成任务：**
- [x] 全量审计用户可见错误路径的 `anyhow::Context`（核心信令/TUN/QUIC relay/配置/IPC/日志路径已补上下文；测试辅助保留简洁 `?`）
- [x] 优雅降级（NAT 检测失败降级 Unknown；QUIC relay 不可用降级信令 relay；Web/API 日志无 journalctl 时降级内存错误聚合；Linux 路由已存在/不可加只告警）
- [x] 自动重连机制
- [x] 详细错误码和用户友好的错误消息（信令/QUIC relay 使用命名错误码和一致消息构造）
- [x] 错误日志聚合（common tracing layer 环形收集 warn/error；coordinator `/api/logs?service=memory` 和客户端 IPC `fuckipv6 logs --service memory` 可读取）

---

### 2. 测试

**优先级：高 🔴**

**待完成任务：**
- [x] 单元测试（`cargo llvm-cov --workspace --summary-only` 行覆盖率 68.01%）
  - IP 分配器测试
  - 协议序列化测试
  - 路由表测试
- [x] 集成测试
  - [x] 协调端 WebSocket 注册流程
  - [x] 信令 RelayPacket 中继转发
  - [x] P2P 引擎间 UDP 数据包转发
  - [x] 真实客户端进程注册流程（`register-check` 命令 + coordinator/client 进程集成测试）
  - [x] TUN 级数据包转发（`sudo ./scripts/test-netns.sh` 覆盖 3 节点、路由、互 ping、近 MTU ping）
- [x] 网络故障测试
  - [x] 断线重连（信令连接管理器重连并重新注册）
  - [x] NAT 重新映射（重复注册保留虚拟 IP，刷新 endpoint/NAT 并广播给 peers）
  - [x] 服务器崩溃/恢复（连接断开后自动重连并重新注册）
- [x] 性能基准测试（criterion，覆盖协议序列化、relay wire format 和 QUIC cert pin）

**文件：**
- `crates/*/tests/` - 集成测试
- `benches/` - 性能测试

---

### 3. 文档

**优先级：中 🟡**

**待完成任务：**
- [x] README.md
  - 项目介绍
  - 快速开始
  - 架构图
  - FAQ
- [x] CONTRIBUTING.md - 贡献指南
- [x] ARCHITECTURE.md - 架构详细说明
- [x] API.md - 协议文档
- [x] CHANGELOG.md - 版本变更记录
- [x] 用户手册（中文/英文）
- [x] API 文档（cargo doc）

---

### 4. 部署工具

**优先级：中 🟡**

**待完成任务：**
- [x] systemd 服务文件
  ```ini
  [Unit]
  Description=fuckipv6 Coordinator
  After=network.target

  [Service]
  Type=simple
  ExecStart=/usr/local/bin/fuckipv6-coordinator
  Restart=on-failure

  [Install]
  WantedBy=multi-user.target
  ```
- [x] Docker 镜像
  - Coordinator Dockerfile
  - Client Dockerfile
  - docker-compose.yml
- [x] 一键安装脚本
  - Linux install-client/install-coordinator
  - macOS launchd install-macos-client/install-macos-coordinator
  - Windows setup.ps1/setup.bat
- [x] GitHub Actions CI
  - Linux/macOS/Windows 自动编译
  - Linux/macOS/Windows 自动测试
- [x] GitHub Actions Release 发布
  - 自动打包 Linux/macOS/Windows Release
  - 自动上传产物

---

## 技术债务 🔧

### 代码质量

- [x] 移除所有 `unwrap()`，改用 `?` 或 `expect()`（`rg "unwrap\\(" crates --glob "*.rs"` 无结果）
- [x] 修复所有 Clippy 警告（`cargo clippy --workspace --all-targets -- -D warnings`）
- [x] 移除未使用的代码（dead_code；无 `#[allow(dead_code)]` 残留，clippy 通过）
- [x] 添加类型注释（启动/IPC/日志聚合/P2P receiver 路径补充显式 channel、NAT、snapshot 等类型；`cargo check --workspace` 通过）
- [x] 重构过长函数（核心启动路径、P2P receiver、signaling connection、coordinator 注册、QUIC relay connection/packet stream、CLI route/logs 已拆分；剩余 >50 行主要是展示函数、平台初始化和测试/自检）

### 依赖管理

- [x] 更新关键风险依赖到当前可用版本（postcard 1.1.3、axum 0.8、tokio-tungstenite 0.29、tower-http 0.6）
- [x] 移除未使用的依赖（cargo machete）
- [x] 审计安全漏洞（cargo audit；无漏洞/告警）

---

## 已知问题 🐛

### 1. 客户端退出异常

**现象：** 客户端 Ctrl+C 退出时，服务器显示 `ResetWithoutClosingHandshake`

**影响：** 日志有 ERROR 级别消息，但功能正常

**优先级：** 低 🟢

**修复方案：** 捕获 SIGINT 信号，优雅关闭 WebSocket 连接

---

### 2. TUN 设备清理

**现象：** 客户端崩溃时 TUN 设备可能残留

**影响：** 需要手动 `sudo ip link delete fuckipv6`

**优先级：** 中 🟡

**当前状态：** 正常 Ctrl+C / SIGTERM 退出会显式删除 TUN 设备；Drop 仍作为异常退出后的兜底清理。进程被 SIGKILL 或宿主机崩溃时仍可能残留，需要手动 `sudo ip link delete fuckipv6`。

---

## 下一步行动建议 📋

### 立即执行（本周）

1. **测试 P2P 数据转发** 🔴
   - 先运行 `sudo ./scripts/test-netns.sh` 做本机 3 节点验证
   - 再部署两个真实主机客户端验证 `ping6` 是否工作
   - 这是 MVP 的最后一块拼图

2. **真实主机 MTU/多节点验证** 🟢
   - 验证接近 MTU 的 `ping6 -s`
   - 验证 3+ 真实节点互通

3. **优雅退出处理** 🟡
   - 捕获 Ctrl+C
   - 关闭 WebSocket
   - 删除 TUN 设备

### 短期目标（本月）

1. **NAT 穿透** 🔴
2. **QUIC 升级** 🔴
3. **Windows 支持** 🟡

### 中期目标（下月）

1. **中继服务** 🟡
2. **Web 管理界面** 🟢
3. **性能优化** 🟡

### 长期目标（季度）

1. **生产部署** 🚀
2. **完整文档** 📚
3. **社区推广** 🌟

---

## 资源和参考

### 技术文档

- [Rust 异步编程](https://rust-lang.github.io/async-book/)
- [Tokio 文档](https://tokio.rs/)
- [Quinn QUIC 库](https://github.com/quinn-rs/quinn)
- [TUN/TAP 原理](https://www.kernel.org/doc/Documentation/networking/tuntap.txt)

### 类似项目参考

- [Tailscale](https://github.com/tailscale/tailscale) - WireGuard 之上的网状网络
- [Nebula](https://github.com/slackhq/nebula) - 可扩展的覆盖网络
- [ZeroTier](https://github.com/zerotier/ZeroTierOne) - 虚拟以太网交换机

---

## Phase 5: 企业级特性与自主化升级 ✅ **全部完成**

### 1. WireGuard 级数据平面端到端加密（E2EE） ✅ **已完成**
- [x] Curve25519 X25519 Diffie-Hellman 会话密钥协商
- [x] ChaCha20-Poly1305 AEAD 载荷数据加密与完整性认证
- [x] 密钥衍生 Blake3 KDF
- [x] P2P 直连与 Relay 中继全路径端到端加密覆盖

### 2. 高级 ICE 候选端点收集与 UDP 打洞协同 ✅ **已完成**
- [x] 本地 LAN IPv4/IPv6 网卡端点发现与探测
- [x] STUN WAN 公网映射端点收集
- [x] 信令通道候选端点同步与打洞协调
- [x] NAT-PMP / UPnP 端口映射探测支持

### 3. Windows 纯 Rust 独立自主与免外部依赖 ✅ **已完成**
- [x] 内嵌 WHQL 驱动自解压，零运行时外部 DLL 下载
- [x] 纯 Rust 替代 netsh.exe：全量复刻 Windows IP Helper API (`CreateUnicastIpAddressEntry`, `CreateIpForwardEntry2`)
- [x] 纯静态交叉编译发布包 `release/fuckipv6-windows/`

### 4. 双栈网络模式与 Coordinator JWT 鉴权 ✅ **已完成**
- [x] IPv4/IPv6 双栈网络虚拟 IP 协同分配
- [x] RFC 2104 HMAC-SHA256 JWT Token 签发与生命周期校验
- [x] WebUI 与 API 强制鉴权中间件及 Bearer Token 机制
- [x] 运行时配置热加载与 Web 界面配置管理

### 5. 端到端 QUIC P2P 直连数据链路 (QuicP2pMesh) ✅ **已完成**
- [x] 双向 TLS 1.3 证书公钥指纹 Pinning 校验
- [x] QUIC 数据流多路复用连接池与 Direct Mesh 传输
- [x] 原生 UDP 直连与 QUIC 直连自适应降级回退双模架构

### 6. UPnP IGD 与 RFC 6886 NAT-PMP 自动端口映射 ✅ **已完成**
- [x] 纯 Rust 实现 NAT-PMP 网关探测与端口映射请求解析
- [x] 纯 Rust 实现 UPnP SSDP 组播发现与 SOAP WANIPConnection 端口映射
- [x] 端点收集期并发自动尝试映射并作为高优先级候选端点上报

### 7. 声明式 ACL 访问控制与准入黑名单 ✅ **已完成**
- [x] 客户端入站/出站声明式规则过滤 (协议、端口、IPv4/IPv6 CIDR)
- [x] Coordinator 注册准入黑名单 (基于 ClientId / BLAKE3 公钥指纹)
- [x] 协调端运行时 Ban/Unban API 与活动连接强制踢出机制

### 8. 分布式多中继拓扑 (Multi-Relay Mesh) 与最优选路 ✅ **已完成**
- [x] 信令协议与协调服务器分布式下发 Multi-Relay 集群列表
- [x] 客户端多中继并发探测与握手 RTT 实时测速
- [x] 最优低延迟中继自动选路与故障平滑切换

### 9. 公网真实 WAN 节点双向连通性测试 ✅ **已完成**
- [x] 部署至云服务器 `relay.xlei.cc` (`185.217.110.15`)
- [x] 本机与云端客户端基于公网互通测试通过（双向 0% 丢包，平均延迟 159ms）
- [x] 跨平台 CLI `fuckipv6 ping` 与 IPC 状态监控 (`status` / `peers`) 验证通过

---

## Phase 6: 下一代核心演进（智能分流、Exit Node、单二进制双模、对称网络、洋葱基础设施） 🚧

> 详细子 Agent 并行开发规划与实施技术规范见专项文档：[NEXTGEN_TODO.md](file:///home/xlei/project/fuckipv6/NEXTGEN_TODO.md)

### 1. Agent 1: 智能分流与规则引擎 (GeoIP / CN-CIDR) ✅ **已完成**
- [x] 纯 Rust 高性能 CN-CIDR / GeoIP 基数树 Trie 匹配 (三大运营商 IPv4/IPv6 聚合网段零外部依赖)
- [x] 规则引擎实现（Direct 直连 / Proxy 走出口 / Mesh 走对等内网 / Reject 阻断）
- [x] TUN 流量捕获与路由分流

### 2. Agent 2: Exit Node 出口网关与公网代理 ✅ **已完成**
- [x] 客户端配置与 CLI 声明 `exit_node` 支持
- [x] 出口节点访问控制白名单与 `--exit-node` / `--routing-mode` CLI 参数全打通
- [x] 境外服务器 `relay.xlei.cc` 出口网关、内核 IP 转发与 MASQUERADE NAT 规则实测上线

### 3. Agent 3: 单一二进制自适应双模 (浏览器应用窗口 + Axum WebUI) ✅ **可直接使用**
- [x] 启动时环境智能检测（Wayland / X11 / Headless / Windows 服务）
- [x] 内嵌现代可视化响应式控制台，无头环境自动挂载 WebUI (`0.0.0.0:8088`)
- [x] 零停机热切换分流模式与出口节点的 REST API (`/api/status`, `/api/peers`, `/api/routing`)
- [x] 桌面环境通过系统浏览器应用窗口或默认浏览器打开 WebUI；Tauri 原生桌面尚未接入

### 4. Agent 4: 对称对等节点网络与一键建网 (Symmetric Mesh) ✅ **已完成**
- [x] 协调端与 QUIC 中继 Actor 嵌入客户端主程序，节点对称化
- [x] 一键邀请码 Ticket (`fk://<pubkey>@<host:port>?name=<NAME>&v6=...`) 协议生成与解析加入
- [x] 一键建网命令 `fuckipv6 host --name <NAME>` 与加入命令 `fuckipv6 join <Ticket>`
- [x] 多网络 (Multi-Network) 并发共存与路由隔离 (`fuckipv6 up [NETWORKS...]`, `fk-<net>` 隔离 TUN 与独立路由)

### 5. Agent 5: 洋葱多跳路由与海量 P2P 基础设施 (暗网级) 🚧 **基础设施已接入，待产品化**
- [x] 3-Hop 电路协商协议与多层嵌套洋葱解密（洋葱剥皮）
- [x] Kademlia DHT 无中心分布式寻址与 TTL 键值存储
- [x] 基于 Blake3 分块的传输原语、分块组装与公钥寻址 E2EE 消息基础设施（尚无完整文件浏览/发布/拉取产品流程）
- [x] 客户端统一 Overlay 调度引擎集成与全协议单元测试通过

---

## 总结

**可直接使用：** Phase 1 核心组网、UDP/QUIC P2P、QUIC/信令中继 fallback、Linux/macOS/Windows TUN、E2EE、ICE/NAT 穿透、Coordinator JWT、Web/CLI 观测、智能分流、Linux Exit Node 内核转发/NAT、浏览器 WebUI 与对称建网。

**基础设施已接入（待产品化）：** 多中继连接时择优、3-Hop 洋葱路由、Kademlia DHT、Blake3 分块传输。已有引擎接入和单测覆盖，但缺少完整 CLI/WebUI 用户流程。

**尚未产品化：** Tauri 原生桌面、用户态 NAT、其他平台与 Linux 同等级的 Exit Node 验证。生产上线前仍需真实跨公网/跨 NAT、Windows/macOS 实机、更大规模 WAN 压测和外部安全审计。

---

**更新日期：** 2026-10-29
**版本：** v0.3.0
**维护者：** fuckipv6 团队
