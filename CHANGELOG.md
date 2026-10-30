# Changelog

All notable changes are documented here. The project follows semantic versioning.

## Unreleased

### Security
- **节点私钥文件权限**: Unix 上创建 `node_identity.pem` 时直接使用 `0600` 模式，不再依赖进程 umask；覆盖已有身份文件时会在写入私钥前收紧权限，并新增新文件与宽松旧权限两类回归测试。
- **QUIC Relay 运行期身份撤销**: Coordinator 黑名单现在在 QUIC relay 注册前以及每个转发包的源、目的身份检查中强制执行；已撤销节点不能新建 relay 状态，也不能经现有连接发包，拒绝路径不增加 relay 转发计数。新增注册拒绝、撤销源、撤销目的和无状态/计数副作用回归测试。证书层 CA/allowlist/CRL/OCSP、网络级授权与独立安全审计仍未完成。
- **QUIC Relay 客户端证书 allowlist**: Relay 的 QUIC TLS 现在只接受当前已完成签名注册的节点 Ed25519 公钥；allowlist 随会话注册、换钥、注销和按节点 ID 封禁动态更新，空名单默认拒绝。TLS 授权查询使用同步快照，不在握手回调中等待异步会话锁。新增授权、未列入、空名单、未注册证书握手拒绝和注销后撤销回归测试。该控制是当前在线会话准入，不等同于 CA、CRL/OCSP、持久撤销列表或网络级最终授权，独立安全审计仍未完成。

### Added
- **UI 运行模式 CLI 真值接线**: 顶层及 `host`/`join`/`connect`/`up`/`start` 支持互斥的 `--headless`、`--webui`、`--gpui`；`--gui` 保留为 WebUI 兼容别名。无显示环境、冲突参数和当前不可用的原生 GPUI 均明确失败，不再静默降级或把浏览器窗口标记为 GPUI。
- **零网络节点身份**: 节点控制面启动时即加载或生成配置根目录下唯一的 `node_identity.pem`，并从可选 `node_identity.json` 读取 `display_name`；即使没有启动任何网络，共享 `NodeSnapshot` 也会包含节点 ID、公钥指纹和显示名。网络启动计划统一复用节点级密钥，测试确认身份跨重启稳定，且私钥、种子和本机路径不会进入 DTO。README 已补充配置根目录、元数据格式和私钥文件权限注意事项。
- **统一节点命令错误契约**: 10 个 `NodeCommand` 均覆盖 JSON 序列化往返与至少一条拒绝路径；重复/非法网络名和无效载荷统一返回 `InvalidRequest`，网络名规则与 `ConfigManager` 的 ASCII 白名单对齐。`NodeProblem` 的 7 个错误码、`network_name` 和 `retryable` 已锁定线协议并可往返；handler 测试覆盖无运行时创建/加入、未知网络探测及 `RuntimeControlError` 映射，`UpdateRouting`/`SetNetworkRole` 在 EX-06/EX-07 完成前继续返回可重试的 `Unavailable`，不会伪装成功。
- **节点快照一致性诊断**: `NodeSnapshot::invariant_violations()` 集中报告网络、配置和内嵌服务重名、运行网络缺少配置、Peer 引用未知/离线网络及 `peer_count` 不一致。运行网络停止、自然退出和创建失败回滚均清理对应 Peer；共享 revision 在 `u64::MAX` 饱和而不回绕。UI 只可展示诊断结果，不得在本地修复或维护平行状态机。
- **内嵌服务状态快照**: `NodeSnapshot` 新增无敏感信息的 `embedded_services`，按网络发布内嵌信令/中继服务的 `Starting/Running/Stopping/Stopped/Failed` 状态及运行中监听地址；服务生命周期变化通过 `EmbeddedServiceChanged` 事件同步，测试覆盖创建、启动失败、按网络停止、全量关闭以及票据/私钥/配置路径不入快照。
- **UI 契约命令通道**: 新增 `NodeCommand::GetUiManifest` 与 `NodeCommandOutcome::UiManifest`，让 CLI、IPC、WebUI 和后续 GPUI 通过同一个节点命令总线获取版本化 manifest；测试覆盖 JSON 往返、11 个页面和当前 schema 版本。该命令只传输 UI 契约，不代表 WebUI/GPUI 已完成适配。
- 完成 QUIC 中继统一身份控制：生产服务端强制 Ed25519 客户端证书并验证私钥持有证明；Relay Register 签名绑定 client id、虚拟 IP、应用公钥、时间戳、服务端 pin、客户端证书 pin 和 TLS exporter；服务端在写入 relay 状态前逐字节核对证书 SPKI 与应用公钥。生产中继禁用 0-RTT，恢复会话回退到 1-RTT；负向测试覆盖缺失/错误证书、错误 pin、合法证书错绑应用身份、Register 篡改/缺失绑定及无 relay 状态副作用。客户端证书层 CA/allowlist/CRL/OCSP、网络级授权和独立安全审计仍未完成。
- 增加可选 `pq` feature 的 X-Wing draft-11、Ed25519 + ML-DSA-65 复合身份和实验性两消息混合 `HELLO/ACCEPT` profile：固定 X-Wing、复合身份、TH1、TH2 向量，规范 `HELLO` 6755 字节与 `ACCEPT` 6635 字节，并覆盖双签名、PSK 模式、有效签名降级、时间窗、重放、截断和非 contributory 输入失败关闭。该 profile 尚未接入生产控制通道、QUIC/relay 或数据面，尚无 Initiator hybrid `CONFIRM`、Cookie/限速和端到端容量测试；上游 ML-KEM/ML-DSA crate 未经独立审计，不得据此宣称已具备后量子安全或超越 WireGuard。
- 将 Secure Session v2 classic 接入生产 `P2pEngine`：使用长期 Ed25519 公钥字典序选择 Initiator/Responder，在共享 UDP socket 上完成 `HELLO -> ACCEPT -> CONFIRM`，仅接受注册 endpoint 的握手与数据，并在 TUN/ACL 前执行 AEAD 认证和重放检查；QUIC Mesh、QUIC relay 与信令 relay 统一运输 v2 密文，未建链、legacy data 和静态 X25519 输入失败关闭。双引擎真实 UDP 测试覆盖双向收发、篡改、relay 重放与未建链拒绝。生产在线重协商、Cookie/限速、后量子组合和独立密码学审计仍未完成，也不构成 WireGuard 安全或性能超越结论。
- 增加 Secure Session v2 独立 UDP 适配器及本地真实 socket 集成测试：完整执行 `HELLO -> ACCEPT -> CONFIRM`，固定对端地址与有序套件策略，设置握手/数据超时，复用 HELLO 重放缓存，并提供运行时会话与拒绝计数；测试覆盖双向数据、AEAD 篡改、重放、错误确认、不支持套件和旧版本拒绝。该适配器当前保留用于独立 socket 集成测试；生产 `P2pEngine` 已由后续条目接入共享 socket v2 数据面。该独立适配器本身不构成 WireGuard 安全或性能结论。
- 增加 `common::Identity` 到 Secure Session Ed25519 身份的受控适配：复用同一长期 Ed25519 种子，但不向调用方暴露种子或私钥字节，并用签名互验测试确认注册公钥与 v2 握手身份一致。该适配为客户端 v2 数据面接线提供身份基础；生产数据面接入已由后续条目完成。
- 增加 Secure Session v2 classic 握手 UDP 控制帧编解码：`HELLO`、`ACCEPT + responder confirmation` 和 `CONFIRM` 使用独立 `FKV2` magic、协议版本、类型和精确 body 长度，拒绝截断、超长、尾随字节、错误 magic/版本/类型及非零 flags/reserved；同时提供双方公钥顺序无关的 endpoint context 派生和随机输入不 panic 测试。该层只完成线格式；生产数据面接入已由后续条目完成。

### Changed
- **动态网络运行时已接入统一节点命令层**: `NodeRuntimeHandler` 现在把动态启动、创建、票据/URL/主机加入、按网络停止与关闭转发给 `NetworkRuntimeManager` actor；初始计划支持部分失败后只重试未运行网络，创建失败会回滚配置、内嵌服务与节点状态，`auto_connect=false` 会保留服务直到显式停止。网络上下线、配置与失败状态会同步到共享快照和事件。路由热更新与网络角色变更仍返回稳定 `Unavailable`；CLI/WebUI 仍主要使用旧入口，GPUI 尚未实现，双 UI 对等测试未完成。权限仍是 `TrustedControlPlane` 且无逐主体鉴权，WebUI 仍监听全部网卡。
- **内嵌网络服务生命周期 Actor**: 新增由节点侧统一持有的 `EmbeddedServiceManager`，在同一 actor 中管理多个网络的内嵌信令与 QUIC 中继实例，提供启动去重、排序列表、按网络停止和全量关闭；服务实例在停止或 actor 关闭时释放，关闭后的新请求返回稳定错误。该基座随后已接入共享命令处理器、节点状态/事件和动态网络运行时；服务级状态已进入共享快照，但 WebUI/GPUI 展示适配仍未完成。
- **节点配置与权限快照**: 共享 `NodeSnapshot` 现在发布启动覆盖后的有效网络配置和统一权限类别；配置投影包含路由、Exit Node、TUN/P2P、ACL 摘要与日志级别，但刻意省略私钥和日志文件路径。多网络监督器同时补上 `is_exit_node` 启动覆盖。权限快照明确标记当前为 `TrustedControlPlane`（连接控制面的调用方可全权操作），不代表已实现逐用户或远程鉴权；WebUI 仍监听全部网卡，CLI/IPC/WebUI/GPUI 尚未完整渲染这些字段。
- **共享 UI 能力清单基座**: 新增版本化、传输无关的节点 UI manifest，声明 11 个页面及动作、字段、校验、确认、权限、可用条件和后端成熟度；必填角色字段使用显式 `Role` 校验，测试校验页面/动作唯一性、破坏性确认，以及动作成熟度与当前真实命令处理器一致。动态启动、创建、加入和停止标记为 `Implemented`，路由与角色变更标记为 `Unavailable`；WebUI/GPUI 尚未消费该 manifest，双 UI 对等和端到端测试仍未完成。
- **节点 IPC 接入统一命令总线**: 多网络运行时的本地 IPC 服务现在持有共享 `NodeCommandBus`，新增可提交 `NodeCommandRequest` 并返回类型化 `NodeCommandResult` 的传输请求，同时保留旧 IPC 请求以兼容现有 CLI/WebUI 调用。真实 TCP 测试覆盖 `GetSnapshot` 往返；CLI 和 WebUI 尚未迁移到该入口，动态启动、创建/加入后端已由后续动态运行时条目接通，路由和角色变更仍不可用。
- **统一节点发布真实身份与有界日志**: 多网络监督器从首个成功建立的真实网络运行时发布节点 ID 与公钥指纹，不把签名私钥或种子写入共享快照；检测到不同网络身份时保留首次身份并发出警告。监督器还会在启动就绪时及每秒刷新转换为类型化 DTO 的有界 WARN/ERROR 聚合日志，未知级别不会伪装成其他严重度。当前日志缓冲不是完整 INFO/DEBUG 流，零网络启动尚无节点身份，CLI/IPC/WebUI/GPUI 也尚未消费该共享快照。
- **统一节点生产命令处理器基座**: 多网络监督器创建共享 `NodeCommandBus`，新增真实 `NodeRuntimeHandler`，已接通共享快照读取、动态启动、创建、加入、按网络停止、P2P Peer 探测和全局关闭，并为探测返回传输无关的结构化结果。路由热更新和网络角色变更尚未接入运行时 actor，当前明确返回稳定 `Unavailable` 错误；CLI/IPC/WebUI 仍未迁移到该命令入口，因此不代表统一 UI 已完成。
- **统一节点共享状态填充真实网络遥测**: 多网络监督器按秒聚合真实运行时中的 Peer 身份、虚拟地址、端点、P2P 质量、TUN/P2P 流量和错误计数到 `NodeStateStore`；停止期间的生命周期状态不会被竞态刷新覆盖，全部网络退出后会清理残留 Peer。身份与日志随后由真实运行时生产填充；配置、权限快照以及 CLI/IPC/WebUI/GPUI 的状态适配仍未完成。
- **统一节点命令与状态契约基座**: 增加传输无关的 `NodeCommand`、请求/异步结果、稳定错误码、网络角色与节点快照 DTO，并实现并发命令总线和单调事件流；新增带修订号、watch 快照和事件通知的 `NodeStateStore`，多网络监督器现会发布生命周期及网络 Connecting/Online/Stopping/Offline/Failed 状态。真实命令处理器及 Peer/流量生产填充随后接入，身份/日志随后由运行时发布；CLI/IPC/WebUI/GPUI 适配器迁移仍未完成。
- **统一节点运行时基座**: 增加节点级 `Stopped/Starting/Running/Stopping/Failed` 生命周期、共享 watch 事件流和 `NodeRuntimePlan`；多网络监督器现在统一消费零网络、默认、全部或具名网络启动计划，空计划仍保留节点控制面和 WebUI，全部所选网络初始化失败时进入 `Failed`。身份、日志、本地控制面及 GPUI/WebUI 尚未全部订阅同一运行时和命令契约，因此不表示统一 UI 已完成。
- **统一节点发布入口**: workspace 只发布 `fuckipv6` 一个用户程序，移除独立 `fuckipv6-coordinator` 二进制；Docker、Compose、systemd、launchd、安装脚本、CI/Release、测试和基准统一使用节点入口。`host`、`join`、中继和 Exit Node 均保留为同一节点的网络角色，不再代表两种产品身份。新增单二进制发布门禁。
- **WebUI 全面重构**:
  - 客户端控制台与协调信令中心统一采用侧边栏 + 顶栏 + 分层卡片设计系统，新增共享样式表 `crates/common/assets/webui.css`（通过 `/assets/webui.css` 分发）。
  - 重做指标卡、拓扑舞台、数据表格、日志控制台、表单、弹窗、右键菜单与空状态，统一深色极光 / 明亮双主题与响应式断点。
  - 客户端标签页与主题支持 `?tab=` / `?theme=` 深链，切换标签不再触发页面滚动；协调端新增侧栏滚动定位。

### Added
- 增加 Secure Session v2 运输无关会话驱动适配层：把实验性 classic 握手、HKDF 密钥派生、双向 HMAC key confirmation、数据包收发、原子在线重协商、旧 epoch 短期宽限和本地材料失配后的失败关闭收束到一个 `SessionDriver` 接口，并提供建链、篡改/重放拒绝、错误确认拒绝、重协商和关闭后拒绝操作的集成测试。该驱动是后续集成的基础；`fuckipv6-client` 后续已依赖该 crate 并接入共享 socket v2 数据面。当前 profile 仍非后量子，测试结果也不构成 WireGuard 安全等级或性能超越结论。
- 增加文档一致性门禁：校验受跟踪 Markdown 的相对链接与标题锚点，拒绝 `file://`/越界路径，并持续检查 Secure Session v2 已接入生产数据面但未完成独立审计、未证明 WireGuard 性能对等及后量子边界等关键声明；同时校验执行队列依赖状态。独立 `docs` CI job 运行辅助脚本单元测试并上传 JSON 报告，补齐 Cargo 元数据声明的 MIT `LICENSE`。
- 增加 Criterion 基准回归门禁：`scripts/criterion_compare.py` 接受原始 Criterion 目录、`bench-compare` 快照和 release-gate 报告，支持多平台基线选择，并默认拒绝平台错配、缺失/新增基准、未知或不足样本、未知标准误、高噪声和超过阈值的平均延迟回退；`scripts/release-gate.py --benchmark-baseline` 可重复指定基线并将比较失败计入发布门禁。未提供基线时报告明确记录 `not_run`，该比较只检测匹配硬件上的相对回退，不构成 WireGuard 性能对等或超越结论。
- 发布流程为每个 Linux、Windows 和 macOS 归档生成 CycloneDX 1.5 与 SPDX 2.3 SBOM、逐产物 SHA256、汇总 `SHA256SUMS` 和未签名来源元数据；这些材料随 GitHub Release 发布，但不构成签名证明、SLSA 证明或独立安全审计。
- 增加供应链门禁：固定 `cargo-deny 0.20.2`，同时审计主 workspace 与独立 fuzz workspace 的已知漏洞、许可证、重复版本、通配版本要求和依赖来源；报告写入 `target/supply-chain/` 并由 CI 上传。该检查不生成 SBOM，也不替代对上游 crate 的人工安全审计。
- 增加独立 `cargo fuzz` workspace 与三个崩溃可复现 harness：通用 v2/PQ 解码器、握手输入与签名验证路径、加密数据帧接收路径；提供 `scripts/fuzz-smoke.sh`，并在 secure-session 相关 PR、`main` 推送和每周定时任务中运行 2000 次短时 smoke。该入口仅证明当前语料范围内未发现 panic/越界/认证绕过，不代表密码学审计或长期 fuzz 覆盖率已经充分。
- 增加可执行发布门禁 `scripts/release-gate.py`：默认拒绝脏工作区，执行格式、辅助脚本测试、workspace check/Clippy/test、release build、Linux musl/Windows GNU 交叉检查及 Criterion quick 基准，输出包含 commit、工具链、硬件、命令、退出码、耗时、日志路径和基准估计值的 JSON 报告；GitHub Actions 对 Linux/Windows/macOS 报告执行同 commit 聚合。`--allow-dirty` 仅供诊断且 `release_ready=false`。该门禁不证明 WireGuard 性能对等、生产安全或独立审计完成。
- 增加 Secure Session v2 可选 `pq` feature 的 FIPS 203 ML-KEM-768 与 FIPS 204 ML-DSA-65 原语适配：锁定 `ml-kem 0.3.3` / `ml-dsa 0.1.1`（Apache-2.0 OR MIT，MSRV 1.85），提供严格定长编解码、私钥/临时随机数/shared secret 清零和脱敏 Debug，并通过 NIST ACVP keygen、encapsulation 与 sigGen KAT。两个上游 crate 均明确标注未经独立安全审计；当前仅完成原语适配，未接入 X-Wing、Ed25519 + ML-DSA-65 复合签名、`HELLO/ACCEPT` 或真实数据面，不得据此宣称已具备后量子安全或超越 WireGuard。
- 增加 Secure Session v2 无状态源可达性 cookie 原语：41 字节 cookie 使用 HMAC-SHA-256 绑定协议版本、源 IP/端口、地址族、双方身份、ephemeral 公钥哈希和签发时间；43 字节 challenge 使用独立控制帧类型，拒绝截断、尾随、错误版本、篡改、过期/未来时间、跨源和跨身份使用，并在响应前阻止反射放大。默认有效期 30 秒、未来偏差 2 秒、可配置上限 300 秒；cookie 仅证明源可达，仍须签名验证、有界重放缓存和限速。当前尚未接入生产握手控制通道；生产 v2 数据帧路径已按后续条目接入。
- 增加 Secure Session v2 连续 stride 缓冲批量 seal/open 快路径：调用方预分配整批数据、明文切片表和包长度表，布局在写入/解密前完整校验；批量 send counter 与 epoch 字节预算一次性检查，receive 采用 fail-fast 且只提交成功前缀的 replay 状态，热路径不逐包分配。新增 32 包 1280/1420 字节 Criterion 对照基线；该项是内存原语基准；生产 v2 固定帧已接入真实 UDP/TUN 路径，但批量 I/O 与快路径优化仍未完成。
- 增加 Secure Session v2 运行时数据密码套件抽象：AES-256-GCM（`0x0201`）与 ChaCha20-Poly1305（`0x0202`）共用固定 16 字节头、方向 Nonce、长度绑定 AAD、16 字节标签和 4096 包重放窗口；握手安装数据 epoch 时只使用已认证 transcript 中的 suite ID，数据包不能自行改选算法。新增双套件往返、跨套件拒绝、pair suite 不一致、AAD/tag 篡改、方向反射和失败不消费重放计数测试；该 classic profile 已由后续条目接入真实 UDP/TUN 数据面，但仍不属于后量子 profile。
- 增加 Secure Session v2 原子在线重协商桥接：`RekeySession` 独占组合 `RekeyStateMachine` 与 `EpochSession`，校验新密钥的 SID、epoch、方向和单调性；Responder 在 `REKEY_ACCEPT` 阶段仅暂存密钥，验证 `REKEY_CONFIRM` 后才安装，Initiator 在认证 ACCEPT 后安装。切换同时替换双向密钥，并通过 `EpochSession::rotate` 保留旧接收 epoch 的有界在途宽限；错误确认/材料、超时和重复消息均有回归测试。当前仍未接入生产控制通道；初始 UDP/TUN 数据面已由后续条目使用同一 v2 认证帧接入。
- 增加 Secure Session v2 握手到数据 epoch 的认证桥接：双方验证对端方向的 HMAC-SHA-256 confirmation 后，才将 HKDF-SHA-512 派生密钥、SID、epoch 和本地/对端 receiver index 装入 `EpochSession`；错误角色、交换方向、单比特篡改和其他 transcript 的确认均拒绝安装，并验证初始化双方可完成双向往返数据收发。`HELLO/ACCEPT` wire 字段和初始数据面已由后续条目接入；生产在线原子重协商仍未接入。
- 增加 Secure Session v2 HKDF-SHA-512 会话密钥派生：从完整签名握手绑定生成独立的双向数据密钥和双向确认密钥，支持可选 32-byte PSK，并只使用固定标签和长度前缀上下文；共享秘密、PSK、IKM、PRK 和输出密钥均纳入清零生命周期。
- 增加 Secure Session v2 双向 HMAC-SHA-256 key confirmation：Initiator/Responder 使用独立派生密钥确认同一 transcript，固定 32-byte 编码并使用常量时间验证；错误角色、交换方向、不同密钥调度、错误 transcript、截断/超长和单比特篡改均失败关闭。该原语后续已接入 wire 握手和数据面激活。
- 增加 Secure Session v2 classic 握手负向回归：覆盖跨会话 `ACCEPT` 重放/乱序、双向临时密钥 MITM 替换、有效签名下的套件降级/版本/profile 篡改、错误 epoch、零 SID、单比特签名字段篡改和失败后 responder 状态恢复；所有路径均在数据密钥激活前失败关闭。
- 增加实验性 Secure Session v2 classic 握手 profile：使用 X25519 临时密钥与 Ed25519 长期身份生成签名 `HELLO/ACCEPT`，绑定双方身份、临时公钥、套件、SID、epoch 和 endpoint context，并在双方派生一致的 X25519 共享秘密与 SHA-512 transcript；60 秒时间窗、4096 项有界重放缓存和非 contributory 密钥交换均失败关闭。该 profile 已作为当前生产迁移 profile 使用，但明确不包含 X-Wing/ML-DSA-65，不可作为生产后量子套件宣传。
- 增加 Secure Session v2 canonical handshake transcript 构造器：使用独立协议域、两字节长度前缀和固定角色顺序绑定套件、双方身份/临时公钥、SID、epoch 及 endpoint context；未知套件、空字段、重复身份和超限输入在 SHA-512 前拒绝。
- 增加 Secure Session v2 `REKEY_INIT/ACCEPT/CONFIRM` 有界、版本化控制帧编解码与固定字节向量；解析失败关闭截断、超长、尾随字节、非法长度、未知版本/类型和非零保留字段。帧本身仍需由已认证控制通道保护。
- 增加 Secure Session v2 数据面 Criterion 基准，覆盖 64/512/1280/1420 字节的 seal/open、重放拒绝及 `EpochSession` 包装开销；发布 2026-10-30 开发基线并明确其结果不构成发布性能门禁。
- 增加 Secure Session Protocol v2 的固定 16 字节数据头、方向分离 Nonce、长度绑定 AAD、原地 ChaCha20-Poly1305 封装、4096 包重放窗口、发送/接收索引分离、带时间/字节上限和旧密钥宽限的 epoch 生命周期状态机，以及 `REKEY_INIT/ACCEPT/CONFIRM` 每会话在线轮换协调状态机；后者覆盖 5 秒超时、重复消息幂等、半开事务上限、SID 防复用和确认失败关闭；初始 v2 握手与数据面此后已接入生产路径，但在线重协商仍等待接入真实控制通道。X-Wing、复合身份和实验性 HELLO/ACCEPT 此后已实现，但仍未接入生产控制通道或数据面。
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
- 对齐 README、快速开始、部署、迁移清单与变更记录中的统一节点模型：产品层只有节点，`host`/`join` 是同一程序创建或加入网络的操作；明确当前桌面路径仍为浏览器 WebUI，GPUI 原生界面与两套 UI 的共享契约尚未完成，不能把目标状态描述为已交付。
- 固化统一节点目标架构：唯一产品身份为节点，协调/信令/中继/Exit Node 都是按网络承担的服务角色；明确 GPUI/WebUI 必须共享命令层、状态契约与能力清单，并列出独立协调二进制、浏览器伪桌面和重复 UI 业务逻辑的迁移/删除边界。
- 发布 2026-10-30 数据面对照基线：当前项目在单流 TCP、受控 UDP 和小包路径上均落后内核 WireGuard，尚不能宣称性能超越；原始数据见 `benchmarks/data/2026-10-30-dataplane-baseline/`。
- 记录 v2 接入前的加密能力口径：当时 UDP 数据面使用静态 X25519 会话密钥 + 随机 Nonce 的 ChaCha20-Poly1305，没有前向保密、重放窗口和密钥轮换；当前生产路径已切换为 v2 classic 临时会话，但独立审计、后量子组合和在线重协商仍未完成，完整差距与验收门槛见 `SECURITY_PERFORMANCE_TODO.md`。
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
