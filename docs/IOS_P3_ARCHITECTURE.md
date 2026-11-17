# iOS P3 架构说明：Packet Tunnel 中的嵌入式节点运行时

本文描述 P3 阶段 iOS 侧的结构：系统 Packet Tunnel VPN 里跑真实的 Rust 节点运行时
（P2 的 `EmbeddedNodeRuntime`），Swift 只负责系统集成、数据面搬运与控制通道。
所有协议、加密、P2P、路由与状态机仍然在 Rust 侧，Swift 不维护第二套业务状态机。

## 1. 范围

- 目标平台：iPhone / iPad 真机与 arm64 模拟器，iOS 17.0+。
- 在范围内：NetworkExtension Packet Tunnel 宿主、FFI 桥、双向数据面、IPC 控制通道、
  App Group / Keychain 存储、启动停止与失败恢复、调试界面。
- 不在范围内：正式产品 UI、Exit Node、全局代理、分流规则 UI、macOS/Android 适配。

## 2. 分层结构

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 系统集成 | `ios/FuckIpv6Tunnel/PacketTunnelProvider.swift` | `NEPacketTunnelProvider` 子类，实现 `PacketTunnelHost`，持有 node / endpoint / pump / 网络设置 |
| 数据面 | `ios/FuckIpv6Tunnel/Data/` | `PacketFlow`（系统 flow 抽象）、`PacketPump`（双向泵）、`MessageRouter`（IPC） |
| 桥 | `ios/FuckIpv6Tunnel/Bridge/` | `Fv6Node`（C ABI 封装）、`PacketEndpoint`、`TunnelSettings`（JSON → NE 设置） |
| 共享 | `ios/Shared/` | `AppGroup`、`TunnelConfiguration`、`TunnelStatus`、`IdentityVault` |
| 主 App | `ios/FuckIpv6/App/` | `NETunnelProviderManager` 控制、配置读写、状态展示，无数据面 |
| C ABI | `crates/ios-bridge/` | `fv6_runtime_*` / `fv6_endpoint_*` / `fv6_prepare_network` |
| 运行时 | `crates/client/src/embedded/` | P2 的 `EmbeddedNodeRuntime`，复用 P2P/QUIC/Relay/Signaling/Secure Session |

依赖方向单向：Swift 只依赖 C ABI；Rust 通过 `HostCallbacks`（open_network / close_network）
回调 Swift，不反向链接任何 Swift 符号。

## 3. 进程与所有权

主 App 与 Extension 是两个进程，App Group 是它们唯一的共享面。

| 资源 | 所有者 | 说明 |
| --- | --- | --- |
| `Fv6Runtime` 句柄 | Extension | `fv6_runtime_create` 创建，`destroy()` 释放一次 |
| `Fv6Endpoint` 句柄 | Extension | 由 Rust 在 `open_network` 里移交；成功后所有权归 Swift，必须 `release()` |
| 系统 packet flow | Extension | `NEPacketTunnelFlow`，只在 Extension 内 |
| 网络设置 | Extension | `setTunnelNetworkSettings` |
| 配置 `config.toml` / `networks.json` | App Group | 非秘密；App 与 Extension 都可读写 |
| 节点身份 `node_identity.pem` | Keychain（运行期短暂落盘） | 见 §7 |
| 状态 `status.json` | App Group | Extension 写，App 读 |
| `NETunnelProviderManager` | App | 只被 App 使用 |

主 App 全程不加载 Rust 运行时，也不跑数据面：它只写配置、启停隧道、读状态文件，
以及通过 `NETunnelProviderSession.sendProviderMessage` 发控制消息。

## 4. 启动调用流

```
App: VPNController.connect()
  └─ NETunnelProviderManager.connection.startVPNTunnel()
       └─ [Extension] PacketTunnelProvider.startTunnel(options:completionHandler:)
            └─ Thread.detachNewThread { startNode() }        // 不阻塞系统回调线程
                 ├─ AppGroup.storageRoot()                    // App Group 目录
                 ├─ TunnelConfiguration.load()                // 缺配置 → 明确失败
                 ├─ IdentityVault.restore(...)                // 先恢复身份，避免生成新身份
                 ├─ Fv6Node.prepareNetwork(...)               // 仅当 config.toml 不存在
                 ├─ fv6_runtime_create  → Fv6Runtime
                 ├─ fv6_runtime_start   → EmbeddedNodeRuntime::start(config, IosPlatform)
                 │     └─ signaling 就绪 → IosPlatform.open_network(...)
                 │           └─ [Swift] openNetwork(settingsJSON:endpoint:)
                 │                 ├─ setTunnelNetworkSettings(...)  // 有界等待
                 │                 ├─ PacketEndpoint(pointer)        // 接管句柄
                 │                 └─ PacketPump.start()             // 双向泵启动
                 ├─ fv6_runtime_snapshot → NodeSnapshot
                 └─ publish(.running, snapshotJSON:)           // 写 status.json
```

启动是否成功只看真实初始化结果：配置缺失、身份恢复失败（非致命则记录）、
`fv6_runtime_create` 失败、`fv6_runtime_start` 失败都会让 `startTunnel` 以错误结束，
并在返回前完成清理。不存在“先报成功再看情况”的路径。

## 5. 数据面

两个方向各一条通道，不按包建任务：

- **Host → Node**：`PacketFlow.startReading` 一次只登记一个读请求，系统回调到达后转发整批。
  `PacketPumpPacket(data:)` 从版本 nibble 判定 `AF_INET` / `AF_INET6`，非 IP 数据报直接丢弃。
  每个包调用 `PacketEndpoint.send`；`queueFull` / `busy` 最多重试 64 次、每次 1 ms，
  仍失败则计为丢弃（有界背压，绝不无界排队）。
- **Node → Host**：单线程以 250 ms 超时 `receive`，收到后按包边界写回
  `flow.writePackets`；`bufferTooSmall` 触发重新分配缓冲区，其他错误 10 ms 退避，
  `closed` / `invalidHandle` 停止并计为故障。

包完整性由 Rust 侧 `crate::packet::validate`（`crates/ios-bridge/src/packet.rs`）二次校验：
非空、不超过 MTU、版本为 4/6、IPv4 的 IHL/total_len、IPv6 的 payload_len 必须与缓冲区一致，
不合法返回 `InvalidPacket`，不会进入数据面。

**结束顺序**：`stop()` 先把 `running` 置 false 并推进 generation，等在途发送计数归零，
再等 node→host 线程退出，返回“是否排空”。只有返回 true 才允许释放 endpoint；
未排空时保留句柄并记录日志，避免悬挂回调访问已释放内存。

## 6. 网络配置与路由

`TunnelSettings`（Rust 生成的 JSON）映射到 `NEPacketTunnelNetworkSettings`：

- 地址与前缀：`ipv4 {address, prefix}` → `NEIPv4Settings(addresses:subnetMasks:)`；
  `ipv6 {address, prefix}` → `NEIPv6Settings(addresses:networkPrefixLengths:)`。
- MTU：小于 1280 直接拒绝启动（IPv6 最低要求）。
- 路由：只把 Mesh 地址空间写进 `includedRoutes`；默认路由不捕获，
  避免把 QUIC/Signaling 传输再导回隧道形成环。
- DNS：`dns` 非空时设置 `NEDNSSettings`。
- 远端地址：`transport` 解析为 `[v6]:port` 或 `v4:port`，作为 `NEPacketTunnelNetworkSettings` 的对端提示。

首轮只做 Mesh-only Split Tunnel。地址冲突或路由无法安全生成时，`openNetwork` 返回错误码，
Rust 侧按 `NodeProblem` 处理并使会话失败，不会带着错误配置继续跑。

## 7. 身份与存储

- 配置文件、网络列表、状态文件位于 App Group `group.com.lingxifox.fuckipv6` 下，
  目录为 `<container>/fuckipv6/node`；不含私钥。
- 节点身份沿用桌面格式（`node_identity.pem`，由桌面 `ConfigManager` 生成与解析），
  但常态存放于 Keychain：`kSecClassGenericPassword`，
  `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`，既不随备份迁移，也能在锁屏首次解锁后启动。
- 运行期由 `IdentityVault.restore` 写回磁盘（文件保护
  `.completeFileProtectionUntilFirstUserAuthentication`），停止时 `capture` 写回 Keychain 并删除明文。
  `capture` 失败只记录日志，不影响停机。
- 恢复顺序很关键：先恢复身份再检查配置，否则 `prepareNetwork` 会生成新身份，导致与既有节点身份不一致。

## 8. 控制通道（IPC）

App 通过 `sendProviderMessage` → `handleAppMessage` 访问 `MessageRouter`：

- 限制：长度 1..64 KiB；必须是 JSON 对象；`v` 必须为 1；`cmd` 必须是
  `status` / `snapshot` / `execute` / `pathChanged`；节点未运行时直接返回 `state` 错误。
- 响应：`{ok: true, v, cmd, result}` 或 `{ok: false, v, cmd?, error: {code, message}}`，
  `code` 沿用 `Fv6Status`（与 C ABI 同一套错误语义）。
- `execute` 的 `request` 必须是 JSON 对象字符串，先本地校验再转发 `fv6_runtime_execute`，
  即命令类型、参数与权限检查仍在 Rust 侧。
- 规范请求形状（即 `NodeCommandRequest` 的 serde 序列化结果，外层 `command` 内嵌同一 tag）：
  `{"request_id":1,"command":{"command":"get_snapshot"}}`；带参数命令为
  `{"request_id":2,"command":{"command":"stop_networks","payload":{"networks":[],"all":true}}}`。
- 失败时 `message` 直接透传 Rust 诊断（例如 `invalid node command request: ...`），不是本地通用文本。

## 9. 生命周期与恢复

- **重复 start**：运行中再 `start` 返回 Ok（Rust 侧幂等）；`starting` 期间返回 `busy`。
- **重复 stop**：`stopped` / `failed` / `created` 再次 stop 返回 Ok。
- **启动中取消**：Rust 侧的节点初始化不可中断，`stop()` 会置位 `stop_requested`、join 进行中的启动任务
  （受 stop 超时约束），再把状态落为 `stopped`；启动调用方拿到终态——`closed` 表示取消生效，
  `nodeProblem` 表示启动自身先失败。两条路径都不留残余任务或 socket。
- **部分初始化失败**：`teardownNode` 按 pump → node（stop + destroy）→ endpoint → 网络设置 → 身份 的顺序回收，
  每一步都幂等。
- **网络路径变化**：`networkPathChanged` → `fv6_runtime_network_path_changed` → 
  `EmbeddedNodeRuntime::network_path_changed`。分工明确：Swift 只重建 TUN 侧（
  `openNetwork` 新会话、旧 pump 先停后弃），传输重连与 Mesh 重选由 Rust 负责。
- **旧回调保护**：每次 `openNetwork` 递增会话代次；晚到的设置回调即使成功，也会因为代次过期
  而释放新 endpoint 并返回 `state`，不会覆盖新会话。
- **系统侧生命周期**：`stopTunnel(with:)` 在独立线程执行 teardown，之后再调用 `completionHandler`，
  避免阻塞系统回调；`handleAppMessage` 在调用线程同步处理并立即回包。

## 10. 平台注入点（P2 复用）

iOS 侧只实现 P2 定义的平台接口，不改动 `crates/client`：

| P2 接口 | iOS 实现 |
| --- | --- |
| `EmbeddedPlatform::open_network(request)` | `IosPlatform`（`crates/ios-bridge/src/platform.rs`）→ C ABI 回调 → `PacketTunnelProvider.openNetwork` |
| `EmbeddedPlatform::shutdown()` | `PacketTunnelProvider.closeNetwork` |
| `EmbeddedNetworkRequest.config` | `TunnelSettings` 的 `network` / `mtu` / `ipv4` / `ipv6` / `routes` / `dns` / `transport` |
| 存储路径 | `AppGroup.storageRoot()` |
| 包 I/O | `EndpointHandle` + `ExternalPacketDevice`（P1 抽象），Endpoint 句柄交给 Swift |

## 11. C ABI 约束

- 所有 `fv6_runtime_*` / `fv6_endpoint_*` 调用会阻塞等待 Rust 侧结果，**不得在主线程调用**；
  Extension 用独立线程，主 App 不直接调用。
- 句柄唯一所有权：`fv6_runtime_create` 与 `fv6_endpoint_*` 各释放一次；重复 `destroy` / `release`
  是空操作。
- 错误信息两段式读取：先给空缓冲区拿到 `bufferTooSmall` 与所需长度，再读实际内容。
- 返回值语义固定：`0 ok / 1 invalidArgument / 2 invalidHandle / 3 state / 4 io / 5 timeout /
  6 closed / 7 bufferTooSmall / 8 queueFull / 9 invalidPacket / 10 panic / 11 busy / 12 nodeProblem`。

## 12. 已知边界

- `handleAppMessage` 在当前线程同步执行（快照与命令都是毫秒级），未引入独立消息队列。
- Keychain 使用 Extension 默认访问组；跨 App 共享访问组需要在运行时拿到带 Team ID 前缀的组名
  （`$(AppIdentifierPrefix)` 只在签名时展开），当前未硬编码前缀。
- 未实现 Exit Node、全局代理与分流 UI；默认路由刻意不捕获。
- 网络切换只做 TUN 侧重建，不主动触发 Mesh 重选（交给 Rust 的路径变更逻辑）。
