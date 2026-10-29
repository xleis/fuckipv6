# fuckipv6 Secure Session Protocol v2

状态：SP-01 设计草案。固定数据帧、AAD、AEAD、重放窗口和本地 epoch 生命周期状态机已实现并有单元测试；握手、PQC、在线轮换消息与数据面接入尚未实现，且尚未经过独立密码学评审。本文不是生产协议规范。

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

### 2.2 X-Wing 依赖门槛

目标构造是 `draft-connolly-cfrg-xwing-kem-11`（X25519 + ML-KEM-768 + SHA3-256）。在实现前必须满足：

- 所选 Rust crate 与草案 revision 完全一致，并有公开测试向量。
- 不混用早期 Kyber round-3 格式与 FIPS 203 ML-KEM 格式。
- crate 的维护状态、许可证、审计和 `zeroize` 行为通过依赖审查。
- 若草案在发布前变化，必须发布新的 suite ID，不能静默改变线格式。

当前 crates.io 的 `x-wing 0.1.x` 只标称早期草案。实现不得直接沿用，除非升级核验到相同 revision 或由审计过的薄封装提供固定测试向量。

### 2.3 后 Quantum 身份

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

## 3. 握手承载

后量子握手包远大于常见路径 MTU。v2 首版要求握手消息经可靠、有序、限长的控制通道传输，例如：

- 已存在的 QUIC 控制流。
- 经认证的 Coordinator/Relay 控制帧。
- 未来的独立可靠握手通道。

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

采用三消息 signed-KEM 流程，并在负载条件下插入无状态 cookie 挑战。握手成功需要 1.5 RTT；正常路径不额外增加 cookie RTT。

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
version, suite_id, CI, CR, HE_I, HP_I,
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
  L(version) || L(suite_id) || L(CI) || L(CR) ||
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
version, suite_id, SID,
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
  L(SID) || L(CR) || L(CI) || L(HE_R) || L(HP_R) ||
  L(CT_R) || L(NR) || L(T)
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

Responder 在全局或每源握手速率超过阈值时，不验证昂贵的双签名和 KEM，而是返回：

```text
RETRY { cookie = MAC(K_cookie, peer_context || TH1_without_signatures || time_window) }
```

规则：

- `K_cookie` 至少 256-bit，由 CSPRNG 生成，每 60 秒轮换并接受前一代。
- cookie 绑定控制通道认证身份、源地址/中继路径、`NI`、`EPK_I` hash、`TH1_without_signatures` 和时间窗口。
- cookie 不创建会话、不入全局表；验证失败返回统一错误或静默丢弃。
- Initiator 重发 `HELLO` 时携带 cookie；Responder 重新验证所有字段。
- 无状态挑战包响应不得大于触发包，防止反射放大。
- 符合速率预算的首次握手可以直接进入验证，但仍受全局 CPU、内存和半开状态上限约束。

## 5. 密钥派生

记 `L(x)` 为两字节大端长度加字段字节。所有 KDF 输入使用固定标签和长度编码，禁止直接字符串拼接。

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

要求：

- `direction_prefix` 固定为 `0x00000001`（I→R）或 `0x00000002`（R→I）；方向密钥本身也不同，两个方向不能共用密钥空间。
- `counter` 严格递增，重协商后新 epoch 使用新的密钥和计数器零点。每个 epoch 只允许 `0..2^32-1`，达到上限必须重新握手，不能回绕。
- `receiver_index` 是接收端本地 32-bit 索引，仅在当前 epoch 有效；发送端必须写入对端公布的索引，接收端必须与自己的本地索引匹配。该字段不能作为会话身份或认证凭据。
- `flags` 保留位必须为零；未知位直接拒绝。
- `epoch` 为 16-bit。达到 `0xffff` 后必须建立新会话，不能回绕；按 120 秒重协商上限可覆盖约 91 天连续会话。
- 不使用逐包随机 nonce、逐包 JSON/postcard、逐包 `Vec` 和逐包哈希表加锁。
- 解密前完成最小长度检查；AEAD 失败不产生错误响应。

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

后续优化顺序：

1. 固定帧和预计算 AEAD 状态。
2. 每核 session table 与缓冲池。
3. UDP `recvmmsg`/`sendmmsg` 或跨平台等价批量 I/O。
4. AES-NI + PCLMUL 的 AES-GCM、ARM Crypto Extensions 后端，其他平台 ChaCha20-Poly1305。
5. 端到端 benchmark 证明优化有效，而不是只测密码学函数。

## 12. 迁移策略

1. 保留现有 v1 协议作为**显式兼容模式**，UI 和日志标记“不具备前向保密/重放窗口”。
2. 新增 v2 协议与测试向量，不修改 v1 的线路语义。
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
