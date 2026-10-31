# fuckipv6 Secure Session Protocol v2

状态：SP-01 设计草案。固定数据帧、AAD、AES-256-GCM/ChaCha20-Poly1305 运行时数据套件、重放窗口、本地 epoch 生命周期、在线重协商协调状态机、原子重协商桥接和实验性 X25519 + Ed25519 classic 握手 profile 已实现并有单元测试；`HELLO/ACCEPT/CONFIRM` 与“确认成功后安装 `EpochSession`”的桥接已接入生产 UDP/TUN 与 relay 数据路径。classic 共享 UDP 握手现要求初次 `HELLO` 先取得无状态 Cookie challenge，只有验证通过的 retry 才进入 admission、签名和 responder crypto；生产 UDP initiator 会在会话建立后按时间/字节/包计数发起在线 REKEY，使用 staged/active/retired epoch 和接收 grace 完成切换，并实际执行 CONFIRM 重传。Cookie key rotation、负载触发 Cookie 的自适应策略、产品指标/UI、跨节点压测和独立 DoS 审计，以及 QUIC/relay 认证 REKEY 通道仍未完成。可选 `pq` feature 已实现 X-Wing draft-11、Ed25519 + ML-DSA-65 复合身份以及实验性混合 `HELLO/ACCEPT/CONFIRM` profile，严格 wire 层用固定向量覆盖 transcript、截断/超长、版本与 kind 别名、降级、PSK 模式、重放和保留字段失败关闭。该混合 profile 的 wire 层尚未接入 `SessionDriver`、生产控制通道或数据面；混合 profile 的生产 Cookie/限速、容量/端到端性能验证和独立密码学评审也未完成。本文不是生产协议规范。

## 1. 设计目标

Secure Session Protocol v2 为 Peer 之间的 IP 载荷建立经过双向认证、具备前向保密和混合后量子能力的会话。它独立于承载握手消息的可靠控制通道，并可作为 UDP 快速数据面的密钥来源。

设计原则：

- 使用标准密码原语和经过维护的实现，不自行实现 ML-KEM、ML-DSA、X25519 或 AEAD。
- 混合模式要求经典与后量子组件都成功；不支持静默降级。
- 握手 transcript 绑定身份、临时密钥、算法、角色、会话和端点上下文。
- 数据面固定帧、无逐包随机数、无逐包内存分配、无逐包序列化。
- 先用可靠且已认证的控制通道传输大握手包，避免后量子握手造成 IP 分片。
- 在实现前完成外部评审；草案阶段不得宣传为“已超越 WireGuard”。

## 2. 版本与算法套件

协议版本为 `2`。线格式使用网络字节序，除非字段明确标为 little-endian。

### 2.1 生产候选套件

| ID | KEM | 身份签名 | KDF | AEAD |
|---|---|---|---|---|
| `0x0201` | X-Wing = X25519 + ML-KEM-768 | Ed25519 + ML-DSA-65，双签名均必需 | HKDF-SHA-512 | AES-256-GCM |
| `0x0202` | X-Wing = X25519 + ML-KEM-768 | Ed25519 + ML-DSA-65，双签名均必需 | HKDF-SHA-512 | ChaCha20-Poly1305 |

选择规则：

1. 双方交换的是**已签名**的套件能力集合。
2.  Initiator 选择双方共同支持集合中本地优先级最高的套件，并在 `HELLO` 中固定该套件。
3. Responder 必须接受该套件或中止；不得在同一 transcript 内改选更低套件。
4. 若没有共同套件，连接失败并要求显式策略变更；不得回退到 v1 静态密钥协议。
5. 自动选择只用于 AES-GCM/ChaCha20 的性能路由，两者安全等级相同，且都绑定在同一 transcript。
6. `0x0201` 仅在 CPU 提供 AES 硬件加速时优先；其他平台使用 `0x0202`。

### 2.2 X-Wing 实现状态与依赖门槛

目标构造是 `draft-connolly-cfrg-xwing-kem-11`（X25519 + ML-KEM-768 + SHA3-256）。`pq` feature 已按该 revision 实现固定长度组合 KEM、封装/解封装、shared-secret 组合和 draft-11 固定向量；X-Wing 公钥与密文长度分别为 1216 和 1120 字节。实现继续遵守：

- RustCrypto 的 `ml-kem 0.3.3` 与 `ml-dsa 0.1.1` 版本和许可证已锁定，且不得混用早期 Kyber round-3 格式与 FIPS 203 ML-KEM 格式。
- X-Wing 与复合身份只存在于 `pq` feature；关闭该 feature 时不得静默构造后量子能力。
- 若草案或依赖线格式变化，必须发布新的 suite/profile ID 并更新固定向量，不能静默改变已有线格式。
- 两个上游 crate 尚未经过独立安全审计；固定向量通过只证明实现一致，不证明协议安全。

生产接入前仍需完成外部密码学评审、依赖审计、控制通道集成、容量/端到端性能和跨平台验证。

### 2.3 后量子身份

长期身份是一个复合公钥：

```text
IdentityPublicKey = Ed25519PublicKey || ML-DSA-65PublicKey
IdentityId = BLAKE3("fuckipv6-v2 identity" || len(EdPk) || EdPk || len(PqPk) || PqPk)
```

签名编码固定为 `Ed25519Signature (64) || ML-DSA-65Signature (3309)`。验证方必须：

1. 从公钥重新计算并核对 `IdentityId`。
2. 使用该 `IdentityId` 在 Coordinator 目录或已固定凭据中找到完整公钥。
3. 验证两个签名；任一失败都视为认证失败。
4. 不接受空、截断或可选的后量子签名。

迁移期可以存在“仅经典身份”的显式配置，但它不得使用 `0x0201`/`0x0202` 套件，也不得被宣传为 v2 安全会话。

### 2.4 后量子原语适配状态

`fuckipv6-secure-session` 的默认构建不启用后量子依赖；只有显式启用 `pq` feature 时才编译 ML-KEM-768/ML-DSA-65 适配层。当前锁定的 RustCrypto crate 为 `ml-kem 0.3.3` 与 `ml-dsa 0.1.1`，许可证为 `Apache-2.0 OR MIT`，MSRV 为 1.85。

适配层只暴露 FIPS 203/204 固定尺寸编解码、密钥生成、封装/解封装、签名和验证，并提供 NIST ACVP known-answer 测试。ML-KEM 公钥、密文和 shared secret 分别为 1184、1088、32 字节；ML-DSA-65 公钥和签名分别为 1952、3309 字节。私钥 seed、临时随机数和 shared secret 使用清零容器，私钥 Debug 输出脱敏。

当前实现已包括：

- X-Wing draft-11 组合 KEM、固定长度编解码和 draft-11 向量。
- Ed25519 + ML-DSA-65 复合身份、IdentityId 绑定和双签名强制验证。
- 实验性固定长度混合 `HELLO`/`ACCEPT` profile、TH1/TH2 固定向量、降级与 PSK 模式失败关闭。

仍未包括：

- 生产控制通道、QUIC/relay 或 UDP/TUN 数据面接入。
- 混合 Initiator `CONFIRM` 线格式、混合 profile 的生产 Cookie/限速接入。
- 完整能力协商、端到端容量测试、跨平台运行证据和独立密码学审计。

两个上游 crate 均明确声明尚未经过独立安全审计。SS-13 的测试通过只证明适配和标准向量一致，不构成安全认证；在完成组合协议评审、依赖审计、跨平台验证和端到端基准前，不得宣称后量子安全或性能超越 WireGuard。

## 3. 握手承载

后量子握手包远大于常见路径 MTU。v2 首版要求握手消息经可靠、有序、限长的控制通道传输，例如：

- 已存在的 QUIC 控制流。
- 经认证的 Coordinator/Relay 控制帧。
- 未来的独立可靠握手通道。

当前 QUIC 中继控制通道的边界：

- TLS 1.3 服务端通过预配置 SPKI pin 认证；生产服务端要求客户端提供 Ed25519 终端证书并完成 CertificateVerify，缺失证书或错误用途失败关闭。
- Relay `Register` 签名覆盖 client id、虚拟 IP、应用公钥、时间戳、服务端 pin、客户端证书 pin 和 TLS exporter；服务端在写 relay 状态前核对证书 SPKI 与 Register 应用公钥。
- 生产中继 `max_early_data_size = 0`，客户端不启用 early data；恢复会话只能回退到 1-RTT，Register/Packet 不能提交到 0-RTT。
- 这些控制通道认证不替代 v2 Peer 身份认证，也不构成客户端证书的 CA/allowlist/撤销策略。

控制通道只负责传输和粗粒度限速，**不构成** Peer 身份认证。每个握手 frame 为：

```text
u8  protocol_version = 2
u8  handshake_type
u16 payload_length
u8  payload[payload_length]
```

约束：

- `payload_length <= 16 KiB`；超过上限在分配前拒绝。
- 控制通道必须提供端到端长度和完整性；若经过 Relay，Relay 只能转发二进制 frame。
- 握手完成后，用户数据只走 v2 数据帧，不把控制通道的 TLS/QUIC 认证当作 Peer 认证的替代品。
- 若未来支持通过 UDP 直接承载握手，必须实现独立的分片、重传、拥塞和 DoS 防护，不能简单地发送 IP 分片。

## 4. 握手流程

后量子 profile 设计采用三消息 signed-KEM 流程，并允许在负载条件下插入无状态 cookie 挑战；其成功路径按无 cookie 的正常情况计为 1.5 RTT。当前 classic 生产共享 UDP 路径不同：Responder 对每个初次 `HELLO` 都先返回 challenge，只有 `HELLO_WITH_COOKIE` 才进入昂贵验证，因此不能把“正常路径不增加 cookie RTT”套用于当前 classic 接入。

### 4.1 符号

```text
CI, CR        Initiator / Responder IdentityId
HE_I, HE_R    Initiator / Responder 的 Ed25519 公钥
HP_I, HP_R    Initiator / Responder 的 ML-DSA-65 公钥
EPK_I         Initiator 的一次性 X-Wing 公钥
CT_R          Responder 返回的 X-Wing 密文
NI, NR        256-bit 随机 nonce
SID           128-bit session id
T             Unix 时间，秒，64-bit
P             Suite ID
```

### 4.2 `HELLO`（Initiator -> Responder）

字段：

```text
version, profile, suite_id, CI, CR, HE_I, HP_I,
EPK_I, NI, T,
endpoint_context_hash,
psk_mode,
cookie_or_zero,
signature_ed_i, signature_pq_i
```

构造：

1. 生成一次性 X-Wing 密钥对，只保留私钥到握手结束。
2. 生成至少 256-bit 随机 `NI`。
3. 定义 `TH1`：

```text
TH1 = SHA-512(
  "fuckipv6-v2 hello" ||
  L(version) || L(profile) || L(suite_id) || L(CI) || L(CR) ||
  L(HE_I) || L(HP_I) || L(EPK_I) || L(NI) || L(T) ||
  L(endpoint_context_hash) || L(psk_mode) || L(cookie_or_zero)
)
```

4. 用 Ed25519 和 ML-DSA-65 分别签署 `TH1`，都放入 `HELLO`。
5. Responder 在验证 cookie/限速前不得执行 KEM 或创建完整会话状态。

`endpoint_context_hash` 至少绑定当前网络 ID、双方虚拟 IP 和预期的直接端点集合。它是上下文绑定，不是对端身份的唯一来源。

### 4.3 `ACCEPT`（Responder -> Initiator）

字段：

```text
version, profile, suite_id, SID, epoch,
CR, CI, HE_R, HP_R,
CT_R, NR, T,
signature_ed_r, signature_pq_r,
responder_confirm
```

构造：

1. 验证 `HELLO` 的版本、套件、时间窗、身份、双签名和 cookie。
2. 检查 `CI`/`CR` 与当前网络授权关系；拒绝未知、被撤销或被禁止的身份。
3. 对 `EPK_I` 执行 X-Wing 封装，得到 `(CT_R, ss)`。
4. 定义不含自身签名的 `TH2_input`：

```text
TH2_input = SHA-512(
  "fuckipv6-v2 accept" ||
  TH1 || L(signature_ed_i) || L(signature_pq_i) ||
  L(SID) || L(epoch) || L(CR) || L(CI) ||
  L(HE_R) || L(HP_R) || L(CT_R) || L(NR) || L(T)
)
```

5. Responder 用 Ed25519 和 ML-DSA-65 签署 `TH2_input`。
6. 计算最终 transcript：

```text
TH2 = SHA-512(
  "fuckipv6-v2 transcript" || TH2_input ||
  L(signature_ed_r) || L(signature_pq_r)
)
```

7. 按第 5 节派生握手密钥和确认密钥。
8. `responder_confirm = HMAC-SHA-256(K_confirm_r, TH2)`，放入 `ACCEPT`。

Initiator 验证 Responder 的两个签名、身份目录绑定、`SID` 唯一性和 `responder_confirm`。

#### 4.3.1 实验性 classic profile

`crates/secure-session/src/handshake.rs` 实现 `CLASSIC_PROFILE_ID = 1`，并已由 `P2pEngine` 接入生产 UDP/TUN 路径。它是当前可运行的迁移 profile，但不是本文最终混合后量子候选套件的替代品，也不得在能力声明中表示已经具备后量子安全：

- `HELLO` 使用 Ed25519 长期身份签名，包含双方 `IdentityId`、X25519 临时公钥、256-bit `NI`、Unix 时间、endpoint context hash、协议版本、profile 和候选数据套件；`ACCEPT` 包含 SID、epoch、双方临时公钥、`NI/NR`、时间和 Responder 签名。
- `IdentityId = SHA-512(L("fuckipv6-v2 classic ed25519 identity") || L(Ed25519PublicKey))[0..32]`。验证方必须同时校验公钥对应的 IdentityId 与消息字段。
- `HELLO` 与 `ACCEPT` 各自使用长度前缀、域分离的 SHA-512 签名输入；最终 transcript 使用第 8.2 节 canonical 构造。双方必须得到相同的 `TH2` hash 和 X25519 共享秘密。
- Responder 只接受与本地可信时钟相差不超过 60 秒的 `HELLO`，并保留最多 4096 个已认证 `HELLO` digest 直到时间窗结束；重复 `HELLO` 和缓存耗尽均失败关闭。进程重启后的持久重放状态尚未实现。
- all-zero 临时公钥、非 contributory X25519 结果、全零 nonce/SID、错误身份、错误签名、错误 endpoint context、错误 epoch 和错误套件均失败关闭。
- SS-07 负向测试额外覆盖跨会话 `ACCEPT` 重放/乱序、双向临时密钥 MITM 替换、有效签名下的套件降级/版本/profile 篡改、单比特签名字段篡改，以及失败握手不污染 responder 重放状态。
- 该 profile 已实现 SS-05 的 HKDF-SHA-512 会话密钥派生、SS-06 的双向 HMAC-SHA-256 key confirmation，以及 SS-08 的认证数据 epoch 桥接。canonical `ACCEPT` 携带 Responder confirmation，`CONFIRM` 携带 Initiator confirmation；只有验证对端方向 confirmation 后才安装双向 AEAD 状态，并绑定 SID、epoch 和 receiver index。SS-10 使桥接按已认证 transcript 中的 suite ID 选择 AES-256-GCM 或 ChaCha20-Poly1305，并拒绝混合 suite 的收发 pair。生产 `P2pEngine` 已使用共享 UDP socket 完成该握手、按认证 endpoint 路由数据并在 TUN/ACL 前认证解密；SS-09 的在线重协商已在该路径接入，classic 无状态 Cookie challenge/retry 也已接入，Cookie key rotation、产品指标/UI 和独立 DoS 审计仍未完成。

#### 4.3.2 实验性 hybrid profile

`crates/secure-session/src/hybrid/handshake.rs` 实现 `HYBRID_PROFILE_ID = 2`，覆盖 suite `0x0201`/`0x0202`。它是 crate 内的两消息 `HELLO -> ACCEPT` 原语，不是已接入生产控制通道的协议实现：

- canonical `HELLO` body 为 6755 字节，canonical `ACCEPT` body 为 6635 字节；两者合计 13390 字节，低于 16 KiB 控制帧 payload 上限。由于单个消息远大于安全 UDP MTU，生产接入必须先提供可靠、有序、限长的已认证控制通道。
- `HELLO` 固定绑定 `version`、`profile`、`suite_id`、双方 `IdentityId`、Initiator 复合公钥的 Ed25519/ML-DSA-65 分量、X-Wing 临时公钥、32 字节 nonce、时间、endpoint context hash、`psk_mode` 和 41 字节 cookie。`TH1` 使用域分离的 SHA-512 覆盖这些未签名字段，复合身份必须同时提供有效 Ed25519 和 ML-DSA-65 签名。
- `TH2_input` 使用域分离的 SHA-512 覆盖 `TH1`、`HELLO` 双签名的两个分量、`SID`、`epoch`、Responder/Initiator `IdentityId`、Responder 复合公钥的两个分量、X-Wing 密文、Responder nonce 和时间。最终 `TH2` 再绑定 Responder 的两个签名分量。`suite`、`profile`、endpoint context 和 `psk_mode` 通过 `TH1` 间接绑定；双方还必须显式核对 `ACCEPT.profile`/`suite_id` 与 `HELLO` 一致。
- `ACCEPT` 当前不含未经认证的额外字段：`responder_confirmation` 是由 `TH2` 和派生密钥计算的 HMAC-SHA-256 确认值。`PSK mode 0/1` 必须与双方本地配置一致，经典-only 身份、缺失任一后量子签名、有效签名下的 suite/profile 降级、非 contributory X-Wing 结果和篡改均失败关闭。
- Responder 在完成身份、时间窗、endpoint、签名、KEM 和确认验证后才把 `TH1` 插入最多 4096 项的有界 replay cache；重复 `HELLO`、缓存耗尽或过期项重新出现均失败关闭。当前缓存只在进程内，尚未实现持久化重放状态。
- 固定向量覆盖 X-Wing draft-11、复合身份、TH1、TH2_input、TH2 和消息容量。生产缺口仍包括 Initiator hybrid `CONFIRM`、cookie 校验/生产限速、能力协商、控制通道与 relay/UDP/TUN 接入，以及端到端容量和独立安全审计。

### 4.4 `CONFIRM`（Initiator -> Responder）

字段：

```text
version, SID, TH2_hash,
initiator_confirm
```

其中：

```text
initiator_confirm = HMAC-SHA-256(K_confirm_i, TH2)
```

Responder 验证 `TH2_hash` 和确认值后才允许该 epoch 接收 Initiator 数据。Initiator 在验证 `ACCEPT` 后可以发送数据，但必须保留重传能力，直到收到控制通道确认。

### 4.5 Cookie/Retry

classic 生产共享 UDP 路径当前对每个初次 `HELLO` 都返回无状态 Cookie challenge，而不是等待全局或每源速率超过阈值后再挑战；负载触发 Cookie 的自适应策略尚未实现。Responder 只在验证 Cookie 后执行 admission、签名验证与 KEM；初次 `HELLO` 不分配 admission、不创建 pending responder，也不执行 responder crypto。线上使用独立的 `FKV2` 控制帧，而不是旧版 43 字节 standalone `CHALLENGE`：

```text
COOKIE_CHALLENGE body {
  responder_identity: [u8; 32],
  cookie: [u8; 41]
}
```

`COOKIE_CHALLENGE` 的 body 固定为 73 字节，加上 12 字节 `FKV2` 控制头后总长为 **85** 字节。Initiator 的重试使用独立 `HELLO_WITH_COOKIE` 帧，其 body 为原 classic `HELLO` body 后追加 41 字节 cookie：

```text
HELLO_WITH_COOKIE body {
  hello: canonical classic HELLO body [236],
  cookie: [u8; 41]
}
```

该 body 固定为 277 字节，控制帧总长为 **289** 字节。固定向量覆盖 challenge 和 retry 编码；Initiator 不能只回送 challenge 头部或 `HELLO.cookie_or_zero` 形式的数据。

Initiator 只接受来自该 peer 已注册端点或当前已认证端点的 `COOKIE_CHALLENGE`。由于 challenge 本身未认证，其他来源不得替换 pending retry，也不得把 Initiator 的后续较大 retry 引导到攻击地址；真实 NAT 端点迁移必须由后续可认证控制帧或数据帧完成。

MAC 输入使用独立域 `L("fuckipv6-v2 stateless cookie mac")`，随后按顺序写入以下每个字段，字段均编码为 `L(x)`，其中 `L(x) = len(x):u16be || x`：

```text
protocol_version, cookie_version,
source_address, initiator_identity, responder_identity,
sha256_ephemeral_public_key, issued_at
```

- `source_address` 为 19 字节：地址族（IPv4=`4`，IPv6=`6`）、端口、IP 地址。必须绑定观察到的 IP 和端口；“只绑定 IP”或忽略地址族都不合规。
- `sha256_ephemeral_public_key = SHA-256(L("fuckipv6-v2 ephemeral key hash") || L(EPK_I))`。哈希接口接受变长公钥字节，以便未来接入 X-Wing 公钥，不自行解释密钥组编码。
- 默认 cookie 有效期为 30 秒，允许 2 秒未来时钟偏差；可配置有效期上限为 300 秒，未来偏差不得超过有效期。边界语义为 `issued_at <= now + skew` 且 `now - issued_at <= lifetime`。
- 验证端必须先以常量时间校验完整 MAC，再检查时间窗口；截断、尾随字节、版本错误、时间无效、跨源、跨身份或跨 `EPK_I` 均失败关闭。
- cookie 不创建 responder 会话，也不进入全局会话表；初次 `HELLO` 的 challenge 路径不消耗 admission。`COOKIE_CHALLENGE` 总长小于触发它的完整初次 `HELLO`，防止反射放大。
- Initiator 发送 `HELLO_WITH_COOKIE` 时，Responder 必须重新验证 cookie 和全部握手字段；只有成功后才分配 admission、创建 pending responder 并执行签名验证和 KEM。相同 retry 必须幂等，不能重复分配 admission。
- endpoint 迁移和 health 入站记录只允许由成功处理的认证控制帧触发；初次未认证 `HELLO`、未受信 `COOKIE_CHALLENGE` 和失败 Cookie 校验不得改变 peer endpoint。
- cookie 只证明源地址可达性，不认证 peer。精确的 `HELLO` 重放在 cookie 生命周期内仍可能被转发，必须由签名验证后的有界握手重放缓存拒绝；调用方仍须执行每源/每身份速率限制。

当前 classic 共享 UDP 路径已实现上述 challenge/retry、真实发包和失败关闭测试；`P2pEngine` 每个实例生成一次 256-bit `CookieKey` 并注入该实例的所有 peer，密钥不进入 DTO、UI 或日志。Cookie key rotation、负载触发 Cookie 的自适应策略、指标 API/UI、准入前 Peer 扫描消除、统一身份目录、跨节点压力测试和独立 DoS 审计仍未完成，不能把当前接入等同于全量 DoS 防护或独立安全验收。

规则：

- `K_cookie` 至少 256-bit，由 CSPRNG 生成，每 60 秒轮换并接受前一代。
- cookie 密钥不得全部为零，并必须经过 CSPRNG 生成；轮换期间只能同时接受当前和前一代密钥。
- 验证失败返回统一错误或静默丢弃，不得通过不同的错误内容和时序泄露身份是否存在。

## 5. 密钥派生

记 `L(x)` 为两字节大端长度加字段字节。所有 KDF 输入使用固定标签和长度编码，禁止直接字符串拼接。

经典 profile 中，`TH2` 为第 8.2 节 canonical transcript hash；密钥调度的 transcript binding 为 `TH_KS = SHA-512(L("fuckipv6-v2 classic key schedule") || L(accept_hash) || L(ACCEPT.signature))`，因此签名后的完整 `ACCEPT` 也绑定到派生密钥。HKDF info 严格为 `L(label) || L(SID) || L(epoch)`，不重复写入协议版本；协议版本已由 `HELLO/ACCEPT` 和 `TH2` 绑定。

```text
PSK_or_zero = psk_mode ? configured_psk : [0u8; 32]

PRK = HKDF-Extract(
  salt = SHA-512("fuckipv6-v2 extract" || TH2),
  IKM  = L(psk_mode) || L(PSK_or_zero) || L(ss)
)

K_i2r = HKDF-Expand(PRK, "fuckipv6-v2 i2r key" || L(SID) || L(epoch), 32)
K_r2i = HKDF-Expand(PRK, "fuckipv6-v2 r2i key" || L(SID) || L(epoch), 32)
K_confirm_i = HKDF-Expand(PRK, "fuckipv6-v2 confirm i" || L(SID) || L(epoch), 32)
K_confirm_r = HKDF-Expand(PRK, "fuckipv6-v2 confirm r" || L(SID) || L(epoch), 32)
```

要求：

- 若网络策略配置了 PSK，`psk_mode` 必须为 1，双方使用同一个 32-byte 高熵 PSK；一方缺失时握手必须失败。
- PSK 不是密码或低熵口令；不得用登录密码代替。
- `ss`、`PRK`、方向密钥和确认密钥在不再需要时 `zeroize`。
- 每次握手必须生成新的 `SID`、`NI`、`NR` 和 X-Wing 临时密钥。
- 不复用 v1 的静态 X25519 密钥派生公式。
- 不记录 `ss`、方向密钥、确认密钥或未加密的握手明文。
- 测试向量覆盖密钥交换双方、所有套件、错误 suite、错误 transcript 和确认失败。

## 6. 数据帧

UDP 数据帧固定 16 字节头，后接 AEAD 密文和 16 字节认证标签：

```text
offset  size  field
0       1     version_and_kind  (v2 data = 0x20)
1       1     flags
2       2     epoch
4       4     receiver_index
8       8     counter
16      ...   AEAD ciphertext || tag
```

总认证开销为 32 字节，与 WireGuard 数据包开销相同。UDP datagram 长度即密文长度，不增加长度字段。

AEAD 参数：

```text
nonce = direction_prefix[4] || counter_be[8]
aad   = header[0..16] || L(SID) || L(payload_length)
```

`payload_length` 为明文/密文载荷长度，不包含 16 字节固定头和 16 字节认证标签；长度编码为两字节大端整数。

`suite_id` 不写入数据头，而是由已认证握手/重协商消息绑定到收发 cipher 状态。AES-256-GCM 与 ChaCha20-Poly1305 必须共用上述 header、Nonce、AAD、标签和重放语义；接收端只使用协商套件解密，不能用单个数据包切换算法。混合套件的 `EpochSession` 收发 pair 必须在安装前拒绝。

要求：

- `direction_prefix` 固定为 `0x00000001`（I→R）或 `0x00000002`（R→I）；方向密钥本身也不同，两个方向不能共用密钥空间。
- `counter` 严格递增，重协商后新 epoch 使用新的密钥和计数器零点。每个 epoch 只允许 `0..2^32-1`，达到上限必须重新握手，不能回绕。
- `receiver_index` 是接收端本地 32-bit 索引，仅在当前 epoch 有效；发送端必须写入对端公布的索引，接收端必须与自己的本地索引匹配。该字段不能作为会话身份或认证凭据。
- `flags` 保留位必须为零；未知位直接拒绝。
- `epoch` 为 16-bit。达到 `0xffff` 后必须建立新会话，不能回绕；按 120 秒重协商上限可覆盖约 91 天连续会话。
- 不使用逐包随机 nonce、逐包 JSON/postcard、逐包 `Vec` 和逐包哈希表加锁。
- 解密前完成最小长度检查；AEAD 失败不产生错误响应。
- 批量接口使用调用方预分配的连续缓冲：第 `i` 包从 `i * stride` 开始，`packet_lengths[i]` 给出实际 datagram 长度；不允许在热路径逐包分配 `Vec`。
- 批量 seal 在写入第一个包前校验全部明文长度、stride、缓冲边界和 `2^32` send counter 范围；校验失败不得产生部分输出或推进 counter。
- 批量 open 在解密前校验完整布局；认证采用 fail-fast，失败包不消费 replay counter，但失败前已成功认证的前缀保持已提交。调用方只有在整批成功时才能把整批视为原子完成。
- 上述批次语义只消除逐包分配和重复边界检查，不改变每个 packet 的独立 AEAD、AAD、nonce 和 replay 规则。

## 7. 重放与重排序

每个 `(session, epoch, direction)` 维护独立的 4096-bit 滑动窗口：

- `max_counter` 记录已接受最大计数器。
- 小于等于 `max_counter` 的包按窗口位图检查。
- 重复包、过旧包和超出窗口下界的包立即丢弃。
- 只有当认证成功后窗口位图才更新。
- 非法 `epoch`、未知 `receiver_index` 和零长度密文在解密前限流丢弃。
- `epoch` 达到 `0xffff` 后禁止继续重协商并要求建立新会话，防止 16-bit 字段回绕。

计数器不得回绕。达到 `2^32` 包、`2^60` 字节或 120 秒任一上限时进入重协商。

## 8. 重协商与密钥退役

v2 的会话重协商是一次新的 X-Wing 临时握手，不是从旧密钥无限派生：

```text
REKEY_INIT -> REKEY_ACCEPT -> REKEY_CONFIRM
```

- `REKEY_*` 使用现有已认证会话的固定控制帧传输；大字段仍可通过可靠控制通道承载。
- 新 epoch 必须包含新的 `SID`、临时 X-Wing 密钥、签名和确认值。
- 旧 epoch 在收到新 epoch 确认后进入有限接收宽限期，防止在途数据丢包。
- 新 epoch 稳定后立即擦除旧方向密钥和旧窗口。
- 重协商失败时旧 epoch 只能在其硬上限内继续；达到硬上限后失败关闭。
- 重协商期间必须限制半开 epoch 数量和握手 CPU 预算。

不能只通过 `HKDF(old_key, counter)` 做无限重钥，因为这会让当前密钥泄漏反向解密旧 epoch。

### 8.1 在线重协商协调约束

`fuckipv6-secure-session` 已实现与密码学原语解耦的每会话重协商状态机；它只处理已经由控制通道认证、且由完整 X-Wing/双签名/确认流程验证过的消息。该状态机本身不构成握手认证，并遵守以下约束：

- 每个会话最多同时存在 1 个半开重协商事务，超时为 5 秒；超时只清除事务，不延长当前数据 epoch 的硬上限。
- `REKEY_INIT` 必须绑定当前 `base_epoch`、严格递增的 `new_epoch`、全新的 128-bit `SID` 和 256-bit Initiator nonce。
- `REKEY_ACCEPT` 必须逐字段绑定 `REKEY_INIT`，并携带 Responder nonce、最终 transcript hash、Responder confirmation 和本地推导出的 Initiator confirmation。
- `REKEY_CONFIRM` 必须逐字段绑定 `REKEY_ACCEPT`；只有确认值与 Responder 预先推导的期望值完全一致时才激活新 epoch。
- 重传的完全相同的 `INIT`/`ACCEPT`/`CONFIRM` 在 5 秒窗口内按幂等处理，不创建第二个 epoch；字段不完全相同的重复消息不得覆盖已有事务。
- 已尝试的 `SID` 在会话内永久保留，超时或失败的 `SID` 也不得复用。
- epoch `0xffff` 是最后一个 epoch；之后必须建立全新会话，不能回绕 `epoch` 或复用旧 `SID`。
- 消息解析、签名、KEM、KDF 或 key-confirmation 失败必须在进入状态机前拒绝；状态机不把非零占位值视为密码学有效性证明。

SS-09 已提供 `RekeySession` 作为 `RekeyStateMachine` 与 `EpochSession` 的唯一原子组合入口：

- 所有候选密钥必须同时匹配提议中的 SID、epoch，形成合法的相反方向 send/receive 对，并严格高于当前 epoch；任一项不符都不会推进状态机或替换在线密钥。
- Initiator 在认证并验证 ACCEPT 中的 Responder confirmation 后才安装新收发密钥；Responder 在 ACCEPT 阶段仅暂存密钥，只有验证匹配的 CONFIRM 后才安装。重复 ACCEPT/CONFIRM 保持幂等。
- 安装通过 `EpochSession::rotate` 一次替换 send 与 receive 状态，并将旧 receive cipher 放入有界宽限窗口；超过期限或淘汰上限后旧密钥被丢弃。
- 失败确认、材料不匹配和超时均保留当前 epoch；超时后立即开始的新事务会清除旧暂存材料，不能跨事务复用。
- 若状态机已推进但底层 epoch 安装返回内部错误，会话进入不可用状态并失败关闭，而不是继续使用不一致的 epoch。

该包装层仍要求调用方先认证控制帧、验证签名/KEM/KDF/confirmation，并正确推导数据密钥。生产 `P2pEngine` 已接入初始 v2 会话与 UDP/TUN 数据路径，但尚未接入在线重协商控制帧、快路径缓冲池或完整并发会话表。

重协商控制帧使用固定 8 字节头，整数均为大端序：

```text
offset  size  field
0       1     version (v2 = 0x02)
1       1     kind (INIT = 0x31, ACCEPT = 0x32, CONFIRM = 0x33)
2       1     flags/reserved (must be zero)
3       1     reserved (must be zero)
4       2     body length (exact value for kind)
6       2     reserved (must be zero)
8       ...   body
```

body 依次连接定宽字段，不带可变长整数、填充或尾随字节：

- `REKEY_INIT`（52 字节 body，60 字节 frame）：`base_epoch:u16 || new_epoch:u16 || SID:[16] || NI:[32]`。
- `REKEY_ACCEPT`（180 字节 body，188 字节 frame）：`INIT-body || NR:[32] || transcript_hash:[32] || responder_confirmation:[32] || expected_initiator_confirmation:[32]`。
- `REKEY_CONFIRM`（212 字节 body，220 字节 frame）：`ACCEPT-body || initiator_confirmation:[32]`。

解析器必须在读取 body 前限制总长不超过 220 字节，并拒绝版本、kind、body length、保留字节不符，以及截断和尾随字节。该编码只提供规范边界，不提供机密性、完整性和身份认证；控制帧仍必须由当前已认证会话保护，并在进入重协商状态机前完成全部密码学校验。

### 8.2 Canonical handshake transcript

经典握手和未来混合握手共用独立的 canonical transcript 构造。所有字段（包括版本、套件、角色和定宽标识）统一编码为两字节大端长度前缀 `L(x) = len(x):u16be || x`，不允许省略长度或使用分隔符。字段按以下固定顺序写入 SHA-512：

```text
SHA-512(
  L("fuckipv6-v2 handshake transcript") ||
  L(version) || L(suite_id) ||
  L(0x01) || L(CI) || L(EPK_I) ||
  L(0x02) || L(CR) || L(EPK_R) ||
  L(SID) || L(epoch) || L(endpoint_context_hash)
)
```

- `0x01` 和 `0x02` 分别是 Initiator/Responder 角色标记；顺序固定，调用方本地的运行角色不改变字段顺序，双方必须得到相同 transcript。
- 当前允许 `suite_id = 0x0201`（AES-256-GCM）或 `0x0202`（ChaCha20-Poly1305）；未知套件、全零身份、相同身份、空临时公钥和全零 endpoint context 在哈希前拒绝。
- `endpoint_context_hash` 由调用方对网络 ID、双方虚拟 IP 和预期 endpoint 集合做域分离哈希得到，长度为 32 字节。
- 单个字段最长 65535 字节，整个 canonical transcript 最多 512 KiB；超限在继续追加前拒绝。
- 该 transcript 只负责字段绑定和歧义消除，不代替签名、KEM 验证或 key confirmation。固定测试向量由 `transcript.rs` 单元测试维护。

## 9. 状态机与失败路径

```text
Idle
  -> HelloReceived
  -> RetrySent
  -> AcceptSent
  -> Established
  -> Rekeying
  -> Closing
  -> Closed
```

强约束：

- 未认证的 `HELLO` 不能创建 `Established` 状态。
- `ACCEPT` 不能在没有有效 `HELLO` transcript 的情况下生成。
- `CONFIRM` 失败不能把状态推进到 `Established`。
- 同一 `SID`、`NI`、`NR` 或 `EPK_I` 不得复用。
- 过期半开状态有固定上限和超时（建议 5 秒）。
- 任何解析错误、签名失败、AEAD 失败、计数器异常和状态乱序都失败关闭。
- 只有公开原因分类可进入日志；日志不包含秘密、完整签名或明文。

## 10. 抗 DoS 预算

实现必须支持可配置的默认值，并在测试中验证：

- 每身份、每源、每中继和全局握手速率限制。
- 半开会话上限、每身份会话上限和总 epoch 上限。
- 在 CPU/内存压力下强制 cookie retry，而不是继续分配状态。
- 双签名和 X-Wing 解封装使用独立的 CPU 预算；超额请求直接丢弃。
- 控制通道 frame 最大 16 KiB；UDP 包最大等于路径安全 MTU 加 32 字节。
- 错误日志按类别采样并限速，防止日志放大。
- 对超时、重复 `HELLO`、重复 cookie 和重复 `CONFIRM` 做幂等响应或静默丢弃。

## 11. 性能快路径

实现必须把控制面和数据面拆分。每包热路径只允许：

1. 一次按 `receiver_index` 的直接索引/分片查找。
2. 读取预计算 epoch 状态。
3. 检查固定头、epoch 和计数器。
4. 使用预初始化 AEAD 进行原地解密/加密。
5. 更新重放窗口和统计。
6. 将包交给有界缓冲池。

禁止在热路径中：

- 获取全局异步锁。
- 序列化/反序列化完整 `P2pDatagram`。
- 随机生成 nonce。
- 哈希 PeerId 或虚拟 IP。
- 分配新的 `Vec` 或克隆密钥。
- 为每个包等待控制通道。

已实现的内存内批量原语基准使用固定 32 包、1280/1420 字节和同一预分配连续缓冲，并分别与 32 次单包调用比较。该基准只能证明当前实现没有逐包分配及测量调用开销，不能替代 UDP/TUN 端到端或 WireGuard 同机对照。

后续优化顺序：

1. 固定帧和预计算 AEAD 状态。
2. 每核 session table 与缓冲池。
3. UDP `recvmmsg`/`sendmmsg` 或跨平台等价批量 I/O。
4. AES-NI + PCLMUL 的 AES-GCM、ARM Crypto Extensions 后端，其他平台 ChaCha20-Poly1305。
5. 端到端 benchmark 证明优化有效，而不是只测密码学函数。

## 12. 迁移策略

1. 当前生产 `P2pEngine` 已采用 **v2-only 失败关闭**：未完成 v2 确认、legacy data 和静态 X25519 输入均不会进入数据接收队列；可配置的 v1 兼容模式尚未提供。
2. 保留 v2 固定向量与线路语义；未来若增加显式 v1 兼容模式，UI 和日志必须标记“不具备前向保密/重放窗口”，且不得静默回退。
3. Coordinator 注册与发现增加 v2 复合身份和套件能力字段；旧客户端不得被当成 v2。
4. 网络策略可以选择 `v2-required`、`v2-preferred`、`v1-compat`。
5. `v2-required` 下没有共同 v2 套件时失败，不回退。
6. 完成审计、跨平台和基准门槛后，默认值才切换到 `v2-required`。
7. 在默认切换前，README 不得把 v1 加密描述为 WireGuard 等价。

## 13. 测试与评审要求

最低测试集：

- X-Wing、ML-KEM、ML-DSA、HKDF、AES-GCM、ChaCha20-Poly1305 标准测试向量。
- 双方密钥/确认值一致性和所有单比特翻转失败。
- 错误身份、错误 suite、错误 transcript、错误 endpoint context、错误签名。
- 重放、乱序、窗口边界、计数器回绕、epoch 混淆和方向反射。
- 重协商成功、失败、超时、旧 epoch 宽限和擦除。
- cookie 错误、过期、重放、跨源使用和负载下 retry。
- 截断、超长、未知 flags、非零 reserved、16 KiB 边界和随机模糊输入。
- 多线程并发握手、会话表竞争、资源上限和日志限速。
- 协议状态机模型检查；关键密钥材料使用后不可读测试。

发布前必须由独立团队评审：

- X-Wing、复合签名和 transcript 的组合安全性。
- 三消息认证流程和 key confirmation。
- cookie 构造、时间窗和重协商状态机。
- 固定数据帧 AAD/nonce 是否覆盖所有上下文。
- 实现是否与规范逐字节一致，以及错误路径是否存在降级或资源泄漏。

## 14. 尚未解决的问题

- X-Wing 仍为 Internet-Draft；生产依赖必须跟踪最终标准和实现 revision。
- ML-DSA-65 的 Rust 实现审计状态、侧信道表现和跨平台确定性需要单独评估。
- 复合签名是否采用新的标准组合封装，还是固定双签名；当前选择固定双签名并要求两者都验证。
- 大握手包在 Coordinator/Relay 控制通道上的分片和公平性协议尚未实现。
- QUIC 双向证书、混合身份目录和 0-RTT 重放策略需要与 SP-07 统一。
- 性能目标需要端到端数据证明；固定帧本身不等于性能已超过 WireGuard。

在上述问题关闭并经外部评审前，本文件只能作为实现计划和测试设计的输入。
