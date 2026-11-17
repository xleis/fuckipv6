# iOS compatibility P2: embedded Rust node

P2 提供 `fuckipv6_client::embedded::EmbeddedNodeRuntime`。它在宿主提供的 Tokio runtime 中运行现有节点内核，不启动 `__control` / `__node` 子进程，不解析 CLI 参数，不监听 Unix IPC，不注册进程信号，不创建原生 TUN，不启动 GUI。Apple NetworkExtension、FFI、Keychain 和正式 iOS UI 均不属于本阶段。

## 复用关系

```text
Desktop: CLI / GUI -> Control process -> Node process -> MultiNetworkSupervisor
Embedded: trusted host -> EmbeddedNodeRuntime       -> MultiNetworkSupervisor
                                                       |
                                      NodeCommandBus + NodeRuntimeHandler
                                      NodeStateStore + OperationService
                                      NetworkRuntimeManager
                                                       |
                                      LiveNetworkRuntimeLauncher
                                      prepare_client_runtime_ext
                                      existing P2P / Signaling / Relay / Secure Session
                                                       |
                                      RuntimeTunIo -> ExternalPacketDevice
```

桌面构造函数继续选择原生模式；嵌入式构造函数传入 `EmbeddedContext`。两种模式共用身份格式、业务命令、权限求值、拒绝优先、审计、operation 幂等记录及节点状态机。没有复制一套节点实现，也没有改变协议或密码算法。

`runtime_process.rs` 保留桌面进程管理职责。嵌入式入口不调用它，也不调用 `MultiNetworkSupervisor::run_plan`，因为后者还会启动旧式控制监听器。嵌入式入口直接通过已有命令总线执行 `StartNode`。

## 宿主 API

```rust,ignore
use std::sync::Arc;
use fuckipv6_client::embedded::{EmbeddedNodeConfig, EmbeddedNodeRuntime};
use fuckipv6_client::node::{NodeCommand, NodeCommandRequest};

// adapter implements EmbeddedPlatform and owns the OS VPN session callbacks.
let mut node = EmbeddedNodeRuntime::start(
    EmbeddedNodeConfig {
        storage_root: app_group_path,
        dns_servers: vec![],
    },
    Arc::new(adapter),
).await?;

let snapshot = node.snapshot().await?;
let outcome = node.execute(NodeCommandRequest::new(
    1, NodeCommand::GetSnapshot,
)).await?;
node.network_path_changed().await?;
node.stop().await?;
```

宿主必须提供仍在运行、启用网络 I/O 与计时器的 Tokio runtime。此 API 不另建 executor。`execute` 将调用者视为可信本机宿主，并从当前权威快照构造现有 owner context；它不是接收未经认证远端 JSON 的入口。`execute_authorized` 接受已经认证、由适配器构造的 `NodeCommandContext`，仍经过原有命令总线的授权和审计。

命令业务拒绝保留 `NodeCommandOutcome::Rejected(NodeProblem)`。请求校验、命令总线中断和宿主不可用等 API 错误返回 `NodeProblem`。平台返回的 `code`、`message_key`、`retryable` 等字段会穿过启动及重启汇总，不被压平为成功或普通字符串。

`submit(context, request, IdempotencyKey)` 和 `operation(id)` 复用 `OperationService`：幂等键冲突、operation 状态转换、审计关联及重启后中断恢复均保留。含敏感一次性材料的执行结果只留内存，不新增持久化格式。调用者需要区分 API 错误、命令拒绝和 operation 的终态。

## 平台接口

`EmbeddedPlatform::open_network(EmbeddedNetworkRequest)` 同时承担设备工厂与系统网络配置边界。请求包含：

- 实际网络名称、Coordinator 分配的 `VirtualIps`。
- 原有 `ClientConfig` 中的虚拟网段、路由策略与 MTU。
- 宿主指定的 DNS 地址；空列表表示保留宿主 DNS 策略。
- 实际 P2P transport 地址，供宿主关联该代会话。

适配器应用系统 IP、路由、DNS、MTU 后返回 `EmbeddedNetwork::new(ExternalPacketDevice, lease)`。构造函数要求 P1 的外部设备类型，不能把原生 `TunDevice` 误传进嵌入式宿主。设备 MTU 必须与网络配置一致。未来 NEPacketTunnelFlow 与 VpnService 都使用同一个原始 IP 包端点；本阶段没有对应平台实现。

`EmbeddedNetworkLease` 管理一代系统配置：`close` 异步确认清理；`cancel` 在启动取消、工厂失败或析构时立即作废该代会话。二者必须幂等。工厂须在产生系统副作用前建立回滚所有权，不能把取消后的配置回调应用到新一代会话。

`EmbeddedPlatform::shutdown` 必须等待适配器持有的异步回滚、取消中的 open 和平台回调完成，不能留下 detached work。Rust 宿主在释放存储所有权、报告停止成功之前等待该 hook。一个适配器实例应由一个活动宿主独占；重新使用前必须完成上一代停止。

P1 队列仍按包数有界，保留包边界、所有权复制、背压、MTU / IP 头长度校验、批处理成功前缀和关闭语义。P2 的全局取消会中断阻塞中的写入，所以宿主不消费输出导致队列满时，`stop` 仍能关闭数据面。

## 路径变化与恢复

- `network_path_changed` 是可信平台通知，仅请求已有 Signaling session 重连并跳过等待中的退避；已有重连事件继续驱动原有 P2P、Relay 恢复逻辑。不替换身份或 Packet I/O，也不执行系统网络命令。返回表示通知已处理，不代表连接已恢复健康。
- `RebuildTun` / `RestartNetworks` 使用原有 actor 的停止、清理、重新启动流程；嵌入式模式由工厂建立新外部设备和新租约。不会调用 `TunDevice::create_dual_stack()`。
- 外部端点关闭使 reader 退出时，嵌入式事件循环明确失败，网络快照进入 Failed；通过原有类型化生命周期命令恢复，不直接改快照冒充恢复。
- 路径通知不会替代 STUN / ICE 候选重新发现或系统 VPN 配置变更。需要刷新整个 transport 配置时，宿主应显式重启相应网络，并由工厂重新配置该代 VPN 会话。

## 存储与所有权

`storage_root` 必须是显式绝对路径。身份仍为该根目录下现有 `node_identity.pem` / 元数据格式；网络配置、策略、审计和 File 续传状态也使用该根目录。嵌入式数据面不再隐式调用 `ConfigManager::new()` 选择 HOME。后续 App Group 可提供此路径；Keychain 身份后端仍未接入。

`embedded-operations.json` 使用既有 operation 存储格式。`embedded-node.lock` 防止两个嵌入式宿主同时占用同一根目录；启动冲突返回 Conflict。桌面进程有自己的实例锁，宿主不得与仍在运行的桌面节点共享同一个存储根目录。

启动先建立任务与清理所有权，再通过原有 `GetSnapshot` 加载策略、成员关系等权威控制状态，然后执行 `StartNode`。该命令保持原来的默认网络选择行为；空配置启动控制内核，其他网络由类型化命令启用。重复 `StartNode` 保持现有幂等行为，不重复打开设备。

停止顺序为：

1. 发布宿主取消信号，打断启动中的工厂和阻塞的数据面 I/O。
2. 关闭命令准入并等待已接收的命令；等待 operation 任务。
3. 复用原有网络关闭流程，回收 reader、出站任务、Signaling、P2P / Secure Session、Relay、Packet I/O 与平台租约。
4. 关闭并 join 网络 actor、服务 actor；内嵌 Coordinator join 维护任务和连接任务，Relay client join 接收任务。
5. join 部分初始化的 P2P 清理任务，等待平台 shutdown，成功后释放存储锁。

成功停止可重复调用；停止后可读取最终权威快照。关闭失败返回错误并保留 Failed 状态、资源所有者与存储锁，调用者可再次 `stop` 重试，不能把失败忽略后另起一个宿主。取消 `stop` 的等待不会丢失其 cleanup task。直接 Drop 只请求异步清理；必须在销毁 Tokio runtime 前 await stop，不能把析构当作同步停止确认。

## 当前边界

- 本阶段验收平台为 macOS arm64。Linux 完整运行时仍绑定原生路由 / 防火墙管理，外部设备注入会被明确拒绝，未伪造系统配置成功。现有原生 Linux 路径保留。
- 嵌入式模式不启动配置文件轮询器、Unix IPC、系统信号监听器或 Linux 自动恢复监听器。配置变更经已有类型化命令提交。
- 通用宿主诊断和安装宿主流量策略尚无移动适配器，嵌入式 `GetDiagnostics`、`ApplyTrafficPolicy`、`RollbackTrafficPolicy` 返回 Unsupported。只读策略检查与原有权限检查没有绕过。
- `NodeSnapshot.interfaces` 不伪造操作系统虚拟接口；外部设备分配地址由实际活动网络快照提供。
- Secure Session v2 完成密钥确认前，发往该 peer 的包不被缓存：数据面记录发送错误后丢弃该包（有可用中继时才走 sealed fallback）。宿主应按正常丢包处理握手窗口，不要在设备刚打开时假设数据面已可用。
- 数据面加密 worker 池是进程级单例，worker 数量只在首次初始化时生效；同一宿主内重复 "start → 运行 → stop" 会重复同一配置请求，视为幂等空操作。协议与密钥状态不受影响，只有 worker 数量属于进程级 sizing。若同一进程里请求不同的 worker 数量，仍返回原有错误（`crypto worker pool was initialized before configuration`），不会悄悄改变已建立的 worker 状态；宿主应在第一次 start 前固定 worker 配置。
- 未声称 iOS / Android 整仓交叉编译通过；原生 TUN cfg、部分依赖及平台接口还需移动目标验证。没有为通过编译删除原模块或屏蔽旧测试。

## P3 所需 FFI 边界清单

1. 创建、停止和释放不透明节点句柄，绑定 Tokio executor 的寿命。
2. 配置路径 / DNS 输入及稳定 `NodeProblem` 输出。
3. 平台 open / close / cancel / shutdown 回调及代次所有权。
4. ExternalPacketEndpoint 输入、输出、关闭、有界队列背压与缓冲区释放。
5. 类型化命令、授权上下文映射、权威快照读取；operation 提交、查询与取消适配。
6. 网络路径通知、明确的设备重建入口与线程 / callback 规则。

这些是后续接口清单，本轮没有实现 Apple FFI、NetworkExtension、VPN 配置或 Swift 页面。
