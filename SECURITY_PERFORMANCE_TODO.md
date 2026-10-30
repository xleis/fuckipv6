# 安全与性能 TODO（对标并超越 WireGuard）

> 目标：在可验证的安全模型中超过 WireGuard 的协议能力，并在同一硬件、同一网络条件下超过其数据面性能。  
> 原则：没有威胁模型、测试、基准数据和外部审计之前，不得声称“超越 WireGuard”。

当前逐项执行清单与验收条件见 [`SECURITY_SPRINT_TODO.md`](SECURITY_SPRINT_TODO.md)。

威胁、代码控制、测试证据和生产缺口的逐项映射见 [`SECURITY_TRACEABILITY.md`](SECURITY_TRACEABILITY.md)。

## 当前基线（2026-10-30，基于 `55b53c5` 及后续文档对齐）

### 已有能力

- Ed25519 长期身份、注册消息签名验证和公钥指纹。
- Secure Session v2 classic 已接入生产 `P2pEngine`：临时 X25519 + Ed25519 握手、双向 key confirmation、按长期公钥确定性选角色、认证 endpoint 绑定、计数 Nonce、4096 包重放窗口及 AEAD 失败关闭；该接入尚未完成独立密码学审计。
- QUIC Mesh、QUIC relay 与信令 relay 只运输同一 v2 密文；静态 X25519 与 legacy data 帧不会进入生产接收队列。
- HKDF-SHA-512 会话密钥派生、HMAC-SHA-256 confirmation，以及 ChaCha20-Poly1305 / AES-256-GCM 数据套件。
- QUIC/TLS 1.3 通道、服务端 SPKI pin、强制 Ed25519 客户端证书、证书/应用身份/TLS exporter 绑定；生产中继禁用 0-RTT，并已验证恢复会话回退到 1-RTT。
- ACL、准入黑名单、配置加密和 Exit Node 源地址校验。

### 必须补齐的差距

- [x] **前向保密**：生产 UDP/TUN 路径已使用每次会话独立的临时 X25519 握手；长期 Ed25519 私钥泄漏不再直接解密历史会话。该结论仍需独立协议与密码学审计。
- [x] **重放保护**：生产 v2 数据帧使用方向分离的计数 Nonce 和 4096 包重放窗口，未认证或重复数据在进入 TUN/ACL 前失败关闭；真实 UDP 双引擎测试覆盖重放与篡改。
- [ ] **密钥生命周期**：初始 epoch 已有 120 秒/2^60 字节硬上限、回滚拒绝和过期擦除，达到上限后失败关闭；在线 `REKEY_INIT/ACCEPT/CONFIRM` 尚未接入生产控制通道，因此持续流量达到时限后不能无缝换钥。
- [x] **握手认证**：生产 UDP 路径已使用绑定双方身份、临时公钥、算法套件、SID、epoch 和 endpoint context 的签名 transcript 与双向 key confirmation；不安全或未完成的会话无法发送数据。
- [ ] **端到端抗量子握手与身份**：生产数据面仍只使用 X25519 和 Ed25519，因此尚不具备生产后量子机密性或后量子身份认证。可选 `pq` feature 的本地进展分项记录如下：
  - [x] X-Wing draft-11 固定向量、封装/解封装、严格长度和非 contributory 失败关闭。
  - [x] Ed25519 + ML-DSA-65 复合身份、双签名强制验证、IdentityId 与签名域绑定。
  - [x] 实验性固定长度 `HELLO/ACCEPT`、TH1/TH2 固定向量、suite/profile/PSK 降级失败关闭、时间窗和有界重放缓存；6755 + 6635 字节 body 均在 16 KiB 单帧预算内。
  - [ ] 接入生产控制通道、Initiator `CONFIRM`、Cookie/限速、能力协商，以及 QUIC/relay/UDP/TUN 数据面；完成端到端容量、性能和独立安全审计。
- [x] **QUIC 双向身份**：中继服务端已启用强制客户端证书认证；Register 签名绑定双 pin 与 TLS exporter，证书 SPKI 和 Register 应用公钥逐字节核对，错绑、篡改和缺失绑定均在 relay 状态变更前失败关闭。CA/allowlist/撤销策略与独立安全审计仍未完成。
- [ ] **抗 DoS**：v2 已实现绑定源地址、身份、ephemeral 公钥和时间窗的无状态 cookie，但尚未接入握手控制通道；生产路径仍缺少握手速率控制和无效包成本上限。
- [ ] **数据面效率**：每包执行异步锁、哈希查找、序列化、随机 Nonce 和新 `Vec` 分配，不可能稳定超过内核 WireGuard。
- [ ] **可验证性能**：已有 v2 crypto/batch Criterion 基准、同机基线回归比较器和高负载端到端初测；只有显式提供匹配硬件基线时才执行相对回退门禁。尚无空闲机 CPU pinning 复测、自动维护的三平台性能基线或达到 WireGuard 对等的结果。

已接入生产 `P2pEngine` 的 Secure Session v2 能力包括：临时 X25519 + Ed25519 的 HELLO/ACCEPT 握手、绑定 SID 与完整 transcript 的 CONFIRM 帧、SHA-512 transcript、HKDF-SHA-512 双向密钥派生、双向 HMAC-SHA-256 key confirmation、AES-256-GCM/ChaCha20-Poly1305 数据套件、固定 16 字节头、方向分离 Nonce、长度绑定 AAD、32-bit 包计数器、认证成功后才推进的 4096 包重放窗口，以及共享 UDP socket 上的握手/控制/数据多路复用。`send_to`、QUIC Mesh、QUIC relay 与信令 relay 调用同一 v2 seal/open 路径；未握手、未认证、legacy data 和静态 X25519 输入均失败关闭。双引擎真实 UDP 测试覆盖建链、双向数据、篡改、重放和未建链拒绝。批量快路径、无状态 cookie、`REKEY_INIT/ACCEPT/CONFIRM` 编解码与原子 epoch 轮换仍主要是 crate 内原语，尚未全部接入生产控制通道。可选 `pq` feature 已提供 ML-KEM-768/ML-DSA-65 适配、X-Wing draft-11、复合身份和实验性混合 `HELLO/ACCEPT`；固定向量和失败关闭测试已通过，但该 profile 尚未参与生产控制通道或数据面，上游实现也未经独立安全审计。

### 快速数据面基线（非发布结论）

#### 端到端对照（2026-10-30）

已提交可复现数据集：`benchmarks/data/2026-10-30-dataplane-baseline/`。3 次 × 10 秒中位数显示：

| 用例 | fuckipv6 | 内核 WireGuard | wireguard-go |
|---|---:|---:|---:|
| TCP 1 flow | 0.0551 Gbit/s | 1.4788 Gbit/s | 2.6436 Gbit/s |
| TCP 8 flows | 0.0497 Gbit/s | 1.4799 Gbit/s | 2.4687 Gbit/s |
| UDP 1420 controlled | 0.0405 Gbit/s，丢包 15.0% | 0.0500 Gbit/s，丢包 0% | 0.0500 Gbit/s，丢包 0% |
| UDP 64 controlled | 0.00352 Gbit/s，丢包 27.8% | 0.00500 Gbit/s，丢包 0% | 0.00500 Gbit/s，丢包 0% |

测试机同时运行桌面和编译任务，`load average` 约为 6.5-9.6；指标是噪声较大的基线，不是发布门禁。即使考虑噪声，TCP 和 64 字节小包性能仍存在数量级或显著差距，UI 不是主要瓶颈。

#### Criterion 微基准

2026-10-30 在 AMD Ryzen 7 5800H、Linux 7.2.11-zen1-1-zen、rustc 1.94.1 上执行：

```bash
cargo bench -p fuckipv6-common --bench protocol -- --quick --noplot
```

单次 AEAD 结果：

| 载荷 | 加密 | 解密 |
|---:|---:|---:|
| 64 B | 1.475 µs / 41.4 MiB/s | 1.461 µs / 41.8 MiB/s |
| 512 B | 1.659 µs / 294.3 MiB/s | 1.640 µs / 297.8 MiB/s |
| 1280 B | 2.151 µs / 567.5 MiB/s | 2.110 µs / 578.7 MiB/s |
| 1420 B | 2.530 µs / 535.3 MiB/s | 2.390 µs / 566.5 MiB/s |

其他结果：

- 静态 X25519 + BLAKE3 密钥协商：55.36 µs。
- 1280 B P2P datagram 反序列化：1.775 µs。
- Relay packet 解码：1.766 µs。

该结果只覆盖单次、内存中、单线程的密码学与编解码函数；不包含 syscall、TUN、UDP、调度、批处理、锁竞争或端到端网络开销，因此不能用于宣称性能超过 WireGuard。固定小包约 1.46 µs 的固定成本表明，当前逐包随机数、分配和异步状态管理需要优先优化。

## 安全目标

### S0：协议规范与威胁模型

- [x] 明确防护目标：被动窃听、主动 MITM、重放、篡改、身份冒用、密钥泄漏、DoS、元数据关联和量子攻击者。见 `SECURITY_MODEL.md`。
- [x] 定义信任边界：Coordinator、Relay、Exit Node、普通 Peer 分别能看到和不能看到什么。见 `SECURITY_MODEL.md`。
- [x] 固定算法套件、版本号、域名分离字符串、密钥推导输入和 transcript 定义（草案见 `PROTOCOL_V2.md`，待独立评审）。
- [ ] 完成独立协议评审，确认已实现状态机、消息格式、失败路径和迁移策略；当前草案见 `PROTOCOL_V2.md`，不能以代码存在替代评审。

### S1：达到并超过 WireGuard 的基础安全属性

- [ ] 每次生产会话使用临时 X25519 密钥完成带身份认证的握手，提供前向保密；本地 v2 原语已实现，待数据面接入。
- [ ] 生产握手 transcript 绑定双方长期身份、临时公钥、会话 ID、算法套件和 PSK 状态；本地绑定与负向测试已实现，待接入。
- [ ] 生产数据面为每个方向派生独立密钥，使用显式 64 位计数器 Nonce 和滑动重放窗口；本地实现已存在，待接入。
- [ ] 在达到时间、包数或字节数阈值时强制重协商，并安全擦除旧密钥；本地重协商与 epoch 轮换已实现，待控制通道和数据面接入。
- [ ] 将版本、会话 ID、方向和计数器纳入生产 AEAD AAD，拒绝跨会话或跨方向重放；本地 v2 帧已满足，生产静态路径仍不满足。
- [ ] 对生产握手和无效数据包实施 cookie/速率限制，避免反射放大和 CPU 耗尽；cookie 原语已实现，待握手接入和限速策略。

### S2：混合后量子安全

- [ ] 握手同时执行 X25519 与 ML-KEM-768，组合结果只有在两者都安全时才能建立会话。
- [ ] 使用 Ed25519 + ML-DSA-65 混合身份签名；迁移期支持“经典/混合”能力协商和明确降级策略。
- [ ] 后量子算法只采用经过审计的标准实现，不自行实现数学原语；当前锁定的 RustCrypto 原语仍明确标注未经独立安全审计。
- [ ] 记录后量子握手额外带宽、CPU 和连接延迟，并加入容量测试。

### S3：工程与密码学保障

- [ ] 所有生产密钥材料使用 `zeroize`，禁止进入日志、panic 信息和调试输出；v2 原语已有清理和脱敏测试，接入后仍需运行时验证。
- [ ] 所有认证比较使用恒定时间实现；继续补齐非对齐、截断、超大帧、随机输入和状态机乱序测试。
- [x] 为握手输入、v2/PQ 解析器和数据帧接收路径提供 `cargo fuzz` harness、短时 smoke 脚本及 PR/周定时 CI；长期持续 fuzz 和 panic=abort 生产路径仍待补充。
- [ ] 增加依赖审计、SBOM、许可证检查和已知漏洞扫描。
  - [x] 使用固定版本 `cargo-deny` 审计主 workspace 与 fuzz workspace 的已知漏洞、许可证、重复版本和依赖来源，并接入 CI。
  - [x] 为发布产物生成 CycloneDX/SPDX SBOM、SHA256 清单和未签名来源元数据；签名证明与 SLSA 证明仍未实现。
- [ ] 邀请独立密码学/协议团队审计并公开审计范围与结论。
- [ ] 提供可复现的安全测试向量和协议兼容性测试套件。

## 性能目标

性能比较必须在相同机器、相同 MTU、相同流数量和相同 CPU 配额下进行，并同时记录：

- [ ] 1420 字节载荷的单流/八流吞吐量（Gbit/s）。
- [ ] 64 字节载荷的包转发率（Mpps）。
- [ ] CPU cycles/byte、instructions/byte 和每核吞吐量。
- [ ] 新建连接、重连、重协商的 p50/p95/p99 延迟。
- [ ] 每 Peer 内存占用、队列深度、丢包率和尾延迟。
- [ ] 与 Linux 内核 WireGuard 及 `wireguard-go` 分别对照。

### 基线优先级

1. [x] 先建立当前项目、Linux 内核 WireGuard、`wireguard-go` 的可重复基准（初版，仍需空闲机器和 CPU pinning 复测）。
2. [ ] 数据面目标：1420 字节吞吐和 64 字节 PPS 均不低于 WireGuard 的 105%，CPU/byte 不高于 WireGuard。
3. [ ] 混合后量子握手允许一次性成本增加，但重连和 0-RTT 路径不得因此出现可感知退化。
4. [ ] 连续运行 24 小时验证无内存增长、密钥泄漏或性能衰减。

## 实施顺序

- [x] **SP-00 初始安全审计**：记录现有密码学与数据面差距。
- [ ] **SP-01 威胁模型与 v2 协议规范**：先完成设计评审，再写握手代码。
- [x] **SP-02 性能基准框架**：已加入 crypto、wire format、端到端和 WireGuard 对照基准；脚本支持控制面、客户端和服务端 CPU 绑定及静默宿主复测。
- [x] **SP-03 临时密钥握手**：Ed25519 身份 + X25519 临时密钥 + transcript 绑定。
  - [x] 实现 HELLO/ACCEPT 线格式、签名 transcript、临时密钥握手、HKDF 密钥派生和双向 key confirmation，并覆盖负向测试。
  - [x] 接入真实客户端共享 UDP socket、身份来源、经典数据套件选择、认证 endpoint 路由和 UDP/TUN 数据面；legacy/static 数据失败关闭。
- [ ] **SP-04 重放与重协商**：计数器 Nonce、滑动窗口、密钥过期和安全擦除。
  - [x] 实现数据帧重放窗口、发送/接收状态分离和可测试的 epoch 轮换生命周期。
  - [x] 实现 REKEY_INIT/ACCEPT/CONFIRM 每会话协调状态机、5 秒超时、重复消息幂等、半开事务上限、SID 防复用和失败关闭测试。
  - [ ] 将状态机接入握手产生的键、控制通道线格式与真实数据面轮换；当前状态机不执行密码学验证。
- [ ] **SP-05 混合后量子握手**：X25519 + ML-KEM-768，Ed25519 + ML-DSA-65。
  - [x] 锁定可选 ML-KEM-768/ML-DSA-65 原语适配，并通过 NIST ACVP KAT、固定长度负向测试和清零检查。
  - [x] 实现 X-Wing draft-11、Ed25519 + ML-DSA-65 复合身份及实验性 `HELLO/ACCEPT` transcript，固定 X-Wing、身份、TH1、TH2 向量，并覆盖降级、PSK 模式、时间窗、重放和非 contributory 输入失败关闭。
  - [ ] 将混合 profile 接入生产控制通道和数据面，补齐 Initiator `CONFIRM`、Cookie/限速、能力协商、端到端容量测试及独立审计；上游未审计前不得用于生产。
- [ ] **SP-06 数据面快路径**：固定帧格式、预计算 Peer 状态、缓冲池、批处理和零拷贝。
  - [x] 实现 v2 固定帧、预分配批量 seal/open 和禁止逐包分配的本地快路径基准。
  - [ ] v2 固定帧已进入真实 UDP/TUN 路径；仍需完成 Peer 热状态、缓冲池、批量 I/O 和端到端 profiling。
- [x] **SP-07 QUIC 统一身份**：双向认证、证书/公钥绑定、0-RTT 重放限制。
  - [x] 服务端 mandatory client auth，要求证书具备 clientAuth 用途并完成 TLS 私钥持有证明。
  - [x] Register 签名覆盖服务端 pin、客户端证书 pin 和 TLS exporter，并在写 relay 状态前核对证书与应用公钥。
  - [x] 生产配置 `max_early_data_size = 0`、客户端不启用 early data，恢复会话回归测试证明 Register/Packet 不能进入 0-RTT。
  - [ ] 增加客户端证书 CA/allowlist/撤销与网络级授权策略，并完成独立安全审计；当前任何满足格式与用途要求的自签 Ed25519 身份仍可能在 TLS 层完成握手，最终依赖应用层绑定和现有准入策略。
- [ ] **SP-08 审计与发布门禁**：模糊测试、外部审计、跨平台验证和性能回归门禁。
  - [x] 增加默认拒绝脏工作区的发布门禁、Linux musl/Windows GNU 编译检查和 Linux/Windows/macOS CI 报告聚合。
  - [x] 增加 Criterion 基线快照与回归比较器；样本不足、基线缺失或基准消失默认失败，并接入可选发布门禁参数。
  - [ ] 保留三平台同机基线、实跑证据，并完成独立安全审计；单次微基准和同机回退门禁均不能证明超过 WireGuard。

## 完成定义

只有同时满足以下条件，才能在 README/发布说明中宣称“安全等级和性能超过 WireGuard”：

1. 所有 S0-S3 安全项完成，并通过独立审计。
2. Linux x86_64 与 ARM64 的数据面基准均达到性能目标。
3. Linux musl、Windows GNU 编译和对应平台运行测试通过。
4. 基准脚本、原始数据、硬件信息和复现步骤提交到仓库。
5. CHANGELOG、ARCHITECTURE、API 和 README 与实际算法及限制一致。

## 非目标与风险

- 后量子握手一定会增加握手包大小和 CPU 成本；能够保证的是数据面性能目标，不能承诺所有路径零退化。
- 用户态 Rust 网络栈要和内核 WireGuard 比吞吐，必须采用批处理、零拷贝和每核状态；仅替换 AEAD 库不够。
- 自研密码协议风险极高。若标准协议或经过审计的库能满足目标，应优先采用，而不是发明新协议。
- Onion/DHT 等元数据保护能力会增加延迟，不应默认开启，也不能与 WireGuard 的单跳数据面性能直接混为一谈。
