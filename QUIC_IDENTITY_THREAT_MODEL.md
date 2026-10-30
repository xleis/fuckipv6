# QUIC 身份威胁模型与版本边界

> 状态：EQ-14/QI-16 状态对齐，对应 2026-10-30 的 QUIC 身份实现快照。
> 结论口径：本文定义必须满足的安全性质与当前缺口。存在实现和测试不代表已完成独立审计，也不构成“安全性或性能超过 WireGuard”的结论。

## 1. 范围

本文覆盖：

- QUIC Relay 的 TLS 1.3 传输、服务端公钥 pin 与客户端证书认证。
- P2P QUIC Mesh 的传输认证。
- 应用层 `RelayMessage::Register` 的身份签名与 relay 注册状态。
- QUIC connection ticket、0-RTT 与会话恢复策略。
- 证书、长期 Ed25519 身份和虚拟 IP 之间的绑定关系。

不覆盖 Secure Session v2 的 UDP 数据面密码学细节、Exit Node 防火墙实现、匿名性或跨平台系统安全。

## 2. 资产与信任边界

| 资产 | 要求 |
|---|---|
| 长期 Ed25519 私钥 | 只用于本地签名和证书 key proof；不落盘为 QUIC 私钥文件、不写日志、生命周期结束后清零 |
| 服务端 QUIC 私钥 | 只存在于 relay 进程；通过已认证通道分发公钥 pin |
| 客户端证书 | 公钥必须逐字节对应应用层 Ed25519 身份 |
| `RelayMessage::Register` | 必须绑定 client id、虚拟 IP、应用公钥、时间戳、服务端身份和当前 TLS 会话 |
| relay 注册状态与流量计数 | 只有完整验证成功且源、目的身份未被撤销后才能修改 |
| connection ticket / 0-RTT 数据 | 不得让未完成当前会话认证的 Register/Packet 产生状态副作用 |

信任边界：

1. 本地进程、系统 RNG、时钟和 TUN 属于客户端 TCB。终端被攻陷后，密码协议不能保护已暴露私钥或明文。
2. Relay 可以观察连接元数据并主动丢包、延迟、重排、复制或篡改密文，但不应能冒用客户端应用身份或静默替换服务端。
3. Coordinator 可以拒绝服务或返回错误候选端点；服务端 pin 和应用身份绑定必须让错误端点无法静默成为可信控制面。
4. 单台 relay 被攻陷不自动等于端到端载荷被解密；该结论依赖 Secure Session v2 数据面的认证与重放保护。

## 3. TLS 层安全不变量

生产 QUIC 服务端必须满足：

1. 使用 TLS 1.3 和 `fuckipv6-quic-v1` ALPN。
2. 客户端必须提供终端证书；无证书、不能完成 CertificateVerify 私钥持有证明的客户端不能建立可用会话。
3. 客户端证书使用 Ed25519 公钥，具有 `clientAuth` 用途，且在验证时刻有效。
4. 服务端 pin 使用证书 SPKI 的 BLAKE3 摘要；客户端必须拒绝错误 pin 或错误证书编码。
5. 自定义 verifier 不能跳过 TLS CertificateVerify；证书内容解析和握手签名验证必须同时成功。
6. 证书调试输出必须隐藏私钥字节。

当前实现边界：

- Relay 生产 TLS 使用动态客户端证书 allowlist：只有当前完成签名注册的节点 Ed25519 公钥能通过握手；allowlist 在注册/换钥时更新，在注销和按节点 ID 封禁时撤销，空名单默认拒绝。授权查询使用同步会话快照，避免在 TLS 回调中等待异步锁。
- 该 allowlist 是当前在线会话准入，不是 CA 信任链、CRL/OCSP、持久撤销列表或网络级最终授权。Relay 仍通过应用层 Register 绑定和运行期黑名单做纵深校验；CA、CRL/OCSP 和持久撤销仍未实现。
- P2P QUIC Mesh 使用独立随机节点证书与客户端身份证书；它尚未接入等同于 Relay 的动态长期身份 allowlist 或授权策略。
- 无客户端证书的客户端/服务端配置工厂只用于显式 legacy/负向测试，不得被生产路径调用。

## 4. 应用层身份绑定

QI-05～QI-07、QI-09、QI-15 已完成，当前实现满足以下不变量：

1. TLS 对端证书的 Ed25519 公钥与 Register `public_key` 逐字节相等。
2. Register 签名覆盖 client id、虚拟 IP、应用公钥、时间戳、服务端 pin、客户端证书 pin 和 TLS exporter。
3. exporter 使用版本化 label `fuckipv6-quic-client-cert-binding-v1` 和 context `relay-register`，并在两端调用同一 helper；静态常量不能替代会话绑定。
4. 任一字段缺失、重复出现、编码非法或校验失败时，在写入 `RelayState` 和流量计数之前关闭连接。
5. 来自一个 TLS 会话的签名不能重放到另一个服务端、客户端、连接或虚拟 IP。
6. Relay 注册前检查 Coordinator 运行期黑名单；每个转发包在读取目标状态和增加转发计数前，同时检查认证源身份与目标身份是否已撤销。

剩余授权边界：Relay TLS allowlist 只覆盖当前活动会话，Coordinator 运行期黑名单覆盖注册和现有连接上的双向转发；二者都不是 CA 信任链、CRL/OCSP 或持久撤销机制。P2P QUIC Mesh 尚未接入同等动态 allowlist。TLS 层验证证书格式、用途、私钥持有证明与 relay 当前准入，应用层再验证证书与 Register 身份及当前通道一致；指定网络的最终授权策略仍须补齐。该实现未经独立安全审计。

## 5. 0-RTT 与重放边界

QUIC 0-RTT 数据不由当前连接重新证明客户端私钥持有，可能被攻击者重放。当前生产策略是直接禁用：

- 生产中继服务端将 `max_early_data_size` 设为 0。
- 生产认证客户端和 legacy 自检客户端均设置 `enable_early_data = false`。
- 恢复会话不能升级为 0-RTT，只能回退到 1-RTT；因此 Register 和 Packet 流不会进入 early data。
- 自检保留测试专用正向对照以证明恢复会话原本可提供 0-RTT，并验证生产配置下 `into_0rtt()` 失败且不会提交早期 Register/Packet。

若未来重新启用 0-RTT，必须另行发布策略，并具备幂等处理、有界重放缓存、时间窗和明确降级规则；当前版本不提供这些生产保证。

## 6. 威胁与预期行为

| 威胁 | 当前行为 | 剩余边界 |
|---|---|---|
| 无客户端证书连接 | mandatory client auth 使 TLS 握手失败或连接随即关闭 | 保持失败关闭 |
| 非 Ed25519 或缺少 clientAuth 的证书 | TLS verifier 拒绝 | 保持失败关闭 |
| 客户端使用错误服务端 pin | 客户端拒绝 | 保持失败关闭 |
| 合法证书搭配另一应用身份 | Register 在写入 relay 状态前被拒绝 | 证书层 CA/allowlist/CRL/OCSP 与网络级授权仍需补齐 |
| 已撤销身份注册或经现有连接转发 | Relay 注册与每个转发包的源/目的检查返回 `Forbidden`，不写 relay 状态、不增加转发计数 | 证书层 CA/allowlist/CRL/OCSP 与网络级授权仍待实现 |
| 篡改 Register 虚拟 IP、公钥、时间、pin 或签名 | 全字段签名与 TLS 证书/通道绑定校验失败，且无 relay 状态副作用 | 继续保持副作用前置拒绝 |
| 重放其它连接或其它 relay 的 Register | 服务端 pin、客户端证书 pin 与 TLS exporter 使跨连接重放失败 | exporter 或签名域变更必须升级版本并更新向量 |
| 重放 0-RTT Register/Packet | 生产配置不接受 early data，恢复连接回退到 1-RTT | 未来启用前必须完成独立重放设计 |
| 错误/恶意 relay 返回自签证书 | SPKI pin 不匹配则拒绝 | 保持失败关闭 |
| 证书或私钥进入日志 | 证书 Debug 已脱敏私钥，错误路径只记录原因分类 | 仍需系统验证 panic/core dump 等运行时边界 |

## 7. 版本与兼容边界

- 当前 QUIC 线格式版本标识为 ALPN `fuckipv6-quic-v1`；channel-binding exporter 使用独立版本化 label/context，不复用 ALPN 字符串。
- 生产路径不提供“自动退回无客户端证书”兼容开关。旧节点需要显式迁移或独立版本，不得静默降级。
- 证书只从长期 Ed25519 身份确定性派生；不得把 QUIC 私钥持久化成新的长期身份。
- 修改 exporter、证书身份绑定或 Register 签名域时必须提升相应版本/用途标签，并更新固定测试和文档。
- 本文不批准 `pq` feature 的混合身份接入 QUIC。QUIC 当前仍是经典 Ed25519 身份与 TLS 1.3 密码套件。

## 8. 非目标与发布门槛

本文不承诺：

- 抗量子身份认证或抗量子密钥交换已经接入 QUIC。
- 对 Relay/Coordinator/ISP 隐藏连接图、精确时序或流量规模。
- 抵御被攻陷终端、恶意 TUN、系统 RNG/时钟故障或供应链投毒。
- 抵抗大规模分布式拒绝服务；限速、半开连接上限和资源配额需独立验证。
- 已完成密码学审计、形式化验证或性能超过 WireGuard。

只有 QI-01～QI-15 全部完成、错误绑定与撤销身份的副作用测试通过、全量门禁通过且文档如实记录剩余风险后，才能在发布说明中描述 QUIC 统一身份层的完成状态。独立安全审计仍是更强的发布门槛。
