# Changelog

All notable changes are documented here. The project follows semantic versioning.

## Unreleased

### Changed
- **WebUI 全面重构**:
  - 客户端控制台与协调信令中心统一采用侧边栏 + 顶栏 + 分层卡片设计系统，新增共享样式表 `crates/common/assets/webui.css`（通过 `/assets/webui.css` 分发）。
  - 重做指标卡、拓扑舞台、数据表格、日志控制台、表单、弹窗、右键菜单与空状态，统一深色极光 / 明亮双主题与响应式断点。
  - 客户端标签页与主题支持 `?tab=` / `?theme=` 深链，切换标签不再触发页面滚动；协调端新增侧栏滚动定位。

### Added
- WebUI 静态资源路由与 `text/css` 响应，新增浏览器页签图标与移动端布局适配。

### Fixed
- 修复 Linux Global Proxy / BypassMainland 策略表的默认路由未绑定虚拟源地址，导致 Exit Node 将合法的 IPv4 出站包误判为源地址伪造的问题。
- 修复 Exit Node 返回的公网响应被客户端统一内层源地址校验丢弃的问题；仅已配置且已认证的 Exit Peer 可向本机虚拟 IP 返回公网单播源，普通 Mesh 流量仍保持严格的虚拟源地址绑定。
- 修复 QUIC 中继发送无上限等待导致 UDP 被阻断时 TUN 数据路径卡死的问题；单包 2 秒未确认后摘除故障中继，并通过同一数据包的 TCP 信令中继完成降级。
- 将稳定期 P2P 探针收紧为 2 秒一次、1 秒响应宽限，使 UDP 黑洞更快被判定为不可用并把流量切换到 QUIC/TCP 信令回退。

### Documentation
- 修正当前加密能力口径：现有 UDP 数据面是静态 X25519 会话密钥 + 随机 Nonce 的 ChaCha20-Poly1305，尚未实现前向保密、重放窗口和密钥轮换，不能等同于 WireGuard；完整差距与验收门槛见 `SECURITY_PERFORMANCE_TODO.md`。
- 明确能力成熟度口径：区分“可直接使用”“基础设施已接入（待产品化）”与“尚未产品化”。
- 修正桌面入口描述：当前使用系统浏览器应用窗口或默认浏览器打开 WebUI，尚未接入 Tauri 原生桌面。
- 明确 JWT 仅用于 Coordinator Web/API；客户端本地 WebUI/API 当前免密，只适合本机或受信网络。
- 明确 Exit Node 当前依赖 Linux 内核转发与 NAT 规则，不是用户态 NAT；其他平台尚未证明同等级可用。
- 明确 3-Hop 洋葱路由、Kademlia DHT 与 Blake3 分块传输是已接入且具备测试的基础设施，不是完整用户产品。

## 0.3.0 - 2026-10-29

### Added
- **Apple / Linear 现代无边框高档拓扑引擎**:
  - 全面移除网格十字标尺、雷达扫描虚线环、径向渐变雾斑底盘、卡片描边线条及 HUD 分割线。
  - 纯粹深色通透舞台，搭配极简胶囊节点卡片 (`roundRect` + 翠绿状态指示点 + 等宽字体)、中心协调中枢柔和发光蓝点。
  - 画布节点、在线列表及日志终端全量支持纯 SVG 矢量图标高档右键快捷菜单。
- **下一代核心架构演进 (Phase 6)**:
  - 纯 Rust CN-CIDR / GeoIP 基数树 Trie 智能分流与规则引擎 (`MeshOnly`, `BypassMainland`, `GlobalProxy`)。
  - Exit Node 出口网关管理，支持白名单鉴权与 Linux 内核转发/NAT；其他平台的同等级产品化仍未验证。
  - 单一二进制自适应双模：无头服务器端自动挂载 Axum WebUI，桌面端使用系统浏览器应用窗口，不包含 Tauri 原生窗口。
  - 节点完全对称 (Symmetric Mesh)：客户端内嵌 Coordinator Actor，提供一键建网 (`fuckipv6 host`) 与 Ticket 凭证解析 (`fuckipv6 join fk://...`)。
  - 多网络隔离并发 (`fuckipv6 up`, `fk-<net>` 独立 TUN 与路由隔离)。
  - 3-Hop 洋葱多跳路由、Kademlia DHT 与 Blake3 分块传输基础设施（已接入 P2P 引擎，完整用户流程待产品化）。
- **全系统日志中文本地化**:
  - 协调端与客户端运行日志、信令交互、TUN 创建与路由事件全量翻译为规范易读中文。
  - WebUI 日志解析器内置中文字典智能归一化系统级日志。

### Fixed
- **QUIC Relay 空闲超时重连循环修复**:
  - 在 Quinn 传输配置中增加 10s `keep_alive_interval` 与 120s `max_idle_timeout`，彻底消除 30 秒空闲掉线死循环。
  - 优化 `QuicRelayClient::close()`，避免 active ping 下 `wait_idle()` 阻塞。
- **PeerSnapshot 多网络字段同步**:
  - 修复 `crates/client/src/webui/controller.rs` 缺少 `network_name` 与 `networks` 初始化导致的编译报错。
- **无外部依赖纯静态分发**:
  - 提供基于 `x86_64-unknown-linux-musl` 的 Linux 单一可执行静态二进制与基于 `x86_64-pc-windows-gnu` 的免外部 DLL 纯 Rust Windows 发布包。

---

## 0.2.0 - 2026-10-28

### Added
- WireGuard 级 E2EE 端到端数据面加密（X25519 + ChaCha20-Poly1305 + Blake3 KDF）。
- 高级 ICE 候选端点收集（本地网卡、STUN WAN、UPnP IGD、RFC 6886 NAT-PMP）。
- Windows 平台纯 Rust IP Helper 替代外部 netsh，集成内嵌自解压 WinTun 驱动。
- 双栈 IPv4/IPv6 虚拟网络协同分配。
- Coordinator Web 管理界面/API 的 HMAC-SHA256 JWT Token 鉴权；客户端本地 API 不在此范围内。
- 声明式入站/出站 ACL 访问控制与黑名单实时踢出机制。
- 多中继拓扑并发测速与最优选路。
- 历史记录：公网 WAN 节点 (`relay.xlei.cc`) 曾完成双向连通性验证；该结果不是本仓库可重复自动验证的测试。

---

## 0.1.0 - 2026-06-08

### Added
- Coordinator WebSocket 信令服务与客户端注册。
- Linux TUN IPv6 路由与虚拟局域网通信。
- 基础 UDP 打洞与信令 Relay fallback。
- 本地 IPC 接口与基础 CLI。
