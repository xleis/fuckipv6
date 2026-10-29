# Changelog

All notable changes are documented here. The project follows semantic versioning.

## Unreleased

### Changed
- **WebUI 全面重构**:
  - 客户端控制台与协调信令中心统一采用侧边栏 + 顶栏 + 分层卡片设计系统，新增共享样式表 `crates/common/assets/webui.css`（通过 `/assets/webui.css` 分发）。
  - 重做指标卡、拓扑舞台、数据表格、日志控制台、表单、弹窗、右键菜单与空状态，统一深色极光 / 明亮双主题与响应式断点。
  - 客户端标签页与主题支持 `?tab=` / `?theme=` 深链，切换标签不再触发页面滚动；协调端新增侧栏滚动定位。

### Added
- 发布流程为每个 Linux、Windows 和 macOS 归档生成 CycloneDX 1.5 与 SPDX 2.3 SBOM、逐产物 SHA256、汇总 `SHA256SUMS` 和未签名来源元数据；这些材料随 GitHub Release 发布，但不构成签名证明、SLSA 证明或独立安全审计。
- 增加供应链门禁：固定 `cargo-deny 0.20.2`，同时审计主 workspace 与独立 fuzz workspace 的已知漏洞、许可证、重复版本、通配版本要求和依赖来源；报告写入 `target/supply-chain/` 并由 CI 上传。该检查不生成 SBOM，也不替代对上游 crate 的人工安全审计。
- 增加独立 `cargo fuzz` workspace 与三个崩溃可复现 harness：通用 v2/PQ 解码器、握手输入与签名验证路径、加密数据帧接收路径；提供 `scripts/fuzz-smoke.sh`，并在 secure-session 相关 PR、`main` 推送和每周定时任务中运行 2000 次短时 smoke。该入口仅证明当前语料范围内未发现 panic/越界/认证绕过，不代表密码学审计或长期 fuzz 覆盖率已经充分。
- 增加可执行发布门禁 `scripts/release-gate.py`：默认拒绝脏工作区，执行格式、辅助脚本测试、workspace check/Clippy/test、release build、Linux musl/Windows GNU 交叉检查及 Criterion quick 基准，输出包含 commit、工具链、硬件、命令、退出码、耗时、日志路径和基准估计值的 JSON 报告；GitHub Actions 对 Linux/Windows/macOS 报告执行同 commit 聚合。`--allow-dirty` 仅供诊断且 `release_ready=false`。该门禁不证明 WireGuard 性能对等、生产安全或独立审计完成。
- 增加 Secure Session v2 可选 `pq` feature 的 FIPS 203 ML-KEM-768 与 FIPS 204 ML-DSA-65 原语适配：锁定 `ml-kem 0.3.3` / `ml-dsa 0.1.1`（Apache-2.0 OR MIT，MSRV 1.85），提供严格定长编解码、私钥/临时随机数/shared secret 清零和脱敏 Debug，并通过 NIST ACVP keygen、encapsulation 与 sigGen KAT。两个上游 crate 均明确标注未经独立安全审计；当前仅完成原语适配，未接入 X-Wing、Ed25519 + ML-DSA-65 复合签名、`HELLO/ACCEPT` 或真实数据面，不得据此宣称已具备后量子安全或超越 WireGuard。
- 增加 Secure Session v2 无状态源可达性 cookie 原语：41 字节 cookie 使用 HMAC-SHA-256 绑定协议版本、源 IP/端口、地址族、双方身份、ephemeral 公钥哈希和签发时间；43 字节 challenge 使用独立控制帧类型，拒绝截断、尾随、错误版本、篡改、过期/未来时间、跨源和跨身份使用，并在响应前阻止反射放大。默认有效期 30 秒、未来偏差 2 秒、可配置上限 300 秒；cookie 仅证明源可达，仍须签名验证、有界重放缓存和限速。当前尚未接入控制通道或真实 UDP 数据面。
- 增加 Secure Session v2 连续 stride 缓冲批量 seal/open 快路径：调用方预分配整批数据、明文切片表和包长度表，布局在写入/解密前完整校验；批量 send counter 与 epoch 字节预算一次性检查，receive 采用 fail-fast 且只提交成功前缀的 replay 状态，热路径不逐包分配。新增 32 包 1280/1420 字节 Criterion 对照基线；当前仍为内存原语基准，尚未接入真实 UDP/TUN 数据面。
- 增加 Secure Session v2 运行时数据密码套件抽象：AES-256-GCM（`0x0201`）与 ChaCha20-Poly1305（`0x0202`）共用固定 16 字节头、方向 Nonce、长度绑定 AAD、16 字节标签和 4096 包重放窗口；握手安装数据 epoch 时只使用已认证 transcript 中的 suite ID，数据包不能自行改选算法。新增双套件往返、跨套件拒绝、pair suite 不一致、AAD/tag 篡改、方向反射和失败不消费重放计数测试；当前仍为实验性 classic profile，尚未接入真实 UDP/TUN 数据面。
- 增加 Secure Session v2 原子在线重协商桥接：`RekeySession` 独占组合 `RekeyStateMachine` 与 `EpochSession`，校验新密钥的 SID、epoch、方向和单调性；Responder 在 `REKEY_ACCEPT` 阶段仅暂存密钥，验证 `REKEY_CONFIRM` 后才安装，Initiator 在认证 ACCEPT 后安装。切换同时替换双向密钥，并通过 `EpochSession::rotate` 保留旧接收 epoch 的有界在途宽限；错误确认/材料、超时和重复消息均有回归测试。当前仍未接入真实控制通道或 UDP/TUN 数据面。
- 增加 Secure Session v2 握手到数据 epoch 的认证桥接：双方验证对端方向的 HMAC-SHA-256 confirmation 后，才将 HKDF-SHA-512 派生密钥、SID、epoch 和本地/对端 receiver index 装入 `EpochSession`；错误角色、交换方向、单比特篡改和其他 transcript 的确认均拒绝安装，并验证初始化双方可完成双向往返数据收发。当前尚未接入 `HELLO/ACCEPT` wire 字段、原子重协商或真实 UDP/TUN 数据面。
- 增加 Secure Session v2 HKDF-SHA-512 会话密钥派生：从完整签名握手绑定生成独立的双向数据密钥和双向确认密钥，支持可选 32-byte PSK，并只使用固定标签和长度前缀上下文；共享秘密、PSK、IKM、PRK 和输出密钥均纳入清零生命周期。
- 增加 Secure Session v2 双向 HMAC-SHA-256 key confirmation：Initiator/Responder 使用独立派生密钥确认同一 transcript，固定 32-byte 编码并使用常量时间验证；错误角色、交换方向、不同密钥调度、错误 transcript、截断/超长和单比特篡改均失败关闭。当前为密码学原语和测试，尚未接入 wire 握手或数据面激活。
- 增加 Secure Session v2 classic 握手负向回归：覆盖跨会话 `ACCEPT` 重放/乱序、双向临时密钥 MITM 替换、有效签名下的套件降级/版本/profile 篡改、错误 epoch、零 SID、单比特签名字段篡改和失败后 responder 状态恢复；所有路径均在数据密钥激活前失败关闭。
- 增加实验性 Secure Session v2 classic 握手 profile：使用 X25519 临时密钥与 Ed25519 长期身份生成签名 `HELLO/ACCEPT`，绑定双方身份、临时公钥、套件、SID、epoch 和 endpoint context，并在双方派生一致的 X25519 共享秘密与 SHA-512 transcript；60 秒时间窗、4096 项有界重放缓存和非 contributory 密钥交换均失败关闭。该 profile 明确不包含 X-Wing/ML-DSA-65，不可作为生产后量子套件宣传。
- 增加 Secure Session v2 canonical handshake transcript 构造器：使用独立协议域、两字节长度前缀和固定角色顺序绑定套件、双方身份/临时公钥、SID、epoch 及 endpoint context；未知套件、空字段、重复身份和超限输入在 SHA-512 前拒绝。
- 增加 Secure Session v2 `REKEY_INIT/ACCEPT/CONFIRM` 有界、版本化控制帧编解码与固定字节向量；解析失败关闭截断、超长、尾随字节、非法长度、未知版本/类型和非零保留字段。帧本身仍需由已认证控制通道保护。
- 增加 Secure Session v2 数据面 Criterion 基准，覆盖 64/512/1280/1420 字节的 seal/open、重放拒绝及 `EpochSession` 包装开销；发布 2026-10-30 开发基线并明确其结果不构成发布性能门禁。
- 增加 Secure Session Protocol v2 的固定 16 字节数据头、方向分离 Nonce、长度绑定 AAD、原地 ChaCha20-Poly1305 封装、4096 包重放窗口、发送/接收索引分离、带时间/字节上限和旧密钥宽限的 epoch 生命周期状态机，以及 `REKEY_INIT/ACCEPT/CONFIRM` 每会话在线轮换协调状态机；后者覆盖 5 秒超时、重复消息幂等、半开事务上限、SID 防复用和确认失败关闭，但尚未接入密码学握手、控制通道或真实数据面。完整 X-Wing 握手、PQC 身份和数据面接入仍未实现。
- 客户端重连时通过 v2 签名请求恢复原虚拟 IPv4/IPv6 租约；协调端内存分配器重启后，只要地址仍空闲即可恢复。
- WebUI 静态资源路由与 `text/css` 响应，新增浏览器页签图标与移动端布局适配。

### Fixed
- 修复供应链审计发现的 RustSec 安全公告：升级 `anyhow`、`crossbeam-epoch`、`quinn`/`quinn-proto`、`rustls`/`rustls-webpki`、`tokio` 等锁定版本，并统一主 workspace 的 `socket2`、`tower` 版本以消除可避免的重复依赖。
- 修复 `VirtualIps` 单栈地址因 Postcard 条件字段省略而无法自解码的问题；所有 Option 字段现在始终编码，并增加单栈/双栈往返回归测试。
- 修复客户端重连后 TUN/路由仍使用旧虚拟地址、但协调端分配新地址造成流量黑洞的问题；现在地址变化会记录错误并失败关闭。
- 修复 Linux Global Proxy / BypassMainland 策略表的默认路由未绑定虚拟源地址，导致 Exit Node 将合法的 IPv4 出站包误判为源地址伪造的问题。
- 修复 Exit Node 返回的公网响应被客户端统一内层源地址校验丢弃的问题；仅已配置且已认证的 Exit Peer 可向本机虚拟 IP 返回公网单播源，普通 Mesh 流量仍保持严格的虚拟源地址绑定。
- 修复 QUIC 中继发送无上限等待导致 UDP 被阻断时 TUN 数据路径卡死的问题；单包 2 秒未确认后摘除故障中继，并通过同一数据包的 TCP 信令中继完成降级。
- 将稳定期 P2P 探针收紧为 2 秒一次、1 秒响应宽限，使 UDP 黑洞更快被判定为不可用并把流量切换到 QUIC/TCP 信令回退。

### Documentation
- 发布 2026-10-30 数据面对照基线：当前项目在单流 TCP、受控 UDP 和小包路径上均落后内核 WireGuard，尚不能宣称性能超越；原始数据见 `benchmarks/data/2026-10-30-dataplane-baseline/`。
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
