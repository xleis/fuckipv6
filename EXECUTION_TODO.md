# fuckipv6 可执行 TODO

> 本清单把路线图拆成可独立提交、可验证的短任务。状态约定：`[ ]` 未开始，`[>]` 进行中，`[x]` 已完成，`[!]` 被外部修改阻塞。
>
> 执行规则：一次只推进一个 `[>]`；每项先完成代码与测试，再同步本清单和 `CHANGELOG.md`；验证通过后单独提交。不得修改或暂存用户声明保护的 `crates/client/src/cli/commands.rs`、`crates/client/src/cli/mod.rs`、`crates/client/src/webui/controller.rs`。

## P0：共享契约与运行时底座

- [x] **EX-01 UI manifest 命令通道**：让 UI 通过统一节点命令总线获取版本化 manifest，而不是在 WebUI/GPUI 各复制页面和动作定义。验收：新增 `GetUiManifest` 命令与类型化结果；JSON 往返可用；返回 11 个页面和当前 schema 版本。已通过 `GetUiManifest` 与类型化结果接通统一命令总线。涉及 `crates/client/src/node/{command,runtime_handler,ui_manifest}.rs`。
- [x] **EX-02 内嵌服务状态快照**：把每个网络的内嵌信令/中继实例状态、监听地址和生命周期纳入 `NodeSnapshot` 与事件流。已完成 `Starting/Running/Stopping/Stopped/Failed` 状态、`EmbeddedServiceChanged` 事件及无票据/私钥/路径的共享 DTO；创建、启动失败、停止与全量关闭均有状态和事件测试。涉及 `crates/client/src/node/{state,event,mod,runtime_control}.rs`。
- [x] **EX-03 节点状态不变量**：为网络名称唯一、配置/运行网络对应、停止后 Peer 清理、失败回滚和 revision 单调性增加集中式不变量测试。已完成 `NodeSnapshot::invariant_violations()`，可诊断网络/配置/内嵌服务重名、运行网络缺配置、未知/离线网络 Peer 及 `peer_count` 不一致；停止、自然退出和失败回滚会清理 Peer，revision 在 `u64::MAX` 饱和而不是回绕。涉及 `crates/client/src/node/{state,runtime_control}.rs`。
- [x] **EX-04 命令错误契约**：补齐所有 `NodeCommand` 的验证、稳定错误码、可重试标记和序列化往返测试；路由/角色未实现必须持续返回 `Unavailable`。已完成 10 个命令的 JSON 往返与拒绝路径覆盖，`StopNetworks { all: true }` 与显式网络名冲突会被拒绝，网络名校验与 `ConfigManager` 的 ASCII 字符集对齐；`NodeProblem` 固定 `invalid_request/not_found/conflict/unavailable/permission_denied/cancelled/internal` 线值及 `retryable`，运行时错误映射和未实现命令均有 handler 测试，`UpdateRouting`/`SetNetworkRole` 仍明确返回可重试的 `Unavailable`。涉及 `crates/client/src/node/{command,runtime_handler,state}.rs`。
- [x] **EX-05 零网络节点身份**：零网络启动时也建立并发布节点公开身份、公钥指纹和配置中的显示名。已完成配置根目录唯一的 `node_identity.pem` 与可选 `node_identity.json` 元数据；supervisor 构造即发布身份，所有网络启动计划复用同一节点密钥。测试覆盖空网络快照、跨重启稳定、显示名来源、私钥/种子/配置路径不进入 DTO。涉及 `crates/client/src/{config_manager,multi_network}.rs` 与 `crates/client/src/node/{plan,runtime_control,runtime_metadata,state}.rs`。

## P1：后端功能闭环

- [ ] **EX-06 路由热更新后端**：把 `UpdateRouting` 接入 `NetworkRuntimeManager`，同步 Smart Route、Exit Node 配置、持久化和共享快照。验收：成功与失败均回滚一致，网络运行中切换不重启节点；完成后解除 CLI/WebUI 迁移的 `Unavailable`。
- [ ] **EX-07 网络角色热更新后端**：把 `SetNetworkRole` 接到持久化与运行时重配置，覆盖成员、发现、中继和 Exit Node 角色。验收：非法组合明确拒绝，停止后可释放服务，角色变化发布状态与事件。
- [ ] **EX-08 CLI 统一命令迁移**：CLI 的网络创建、加入、启停、探测和关闭只通过 `NodeCommandBus`。验收：不保留第二套业务规则，输出消费统一结果和错误码。受保护文件解除前为 `[!]`。
- [ ] **EX-09 WebUI 统一命令迁移**：WebUI 的节点操作与状态读取只通过共享命令总线和 `NodeStateStore`。验收：移除 controller 内部独立网络/状态变更逻辑，HTTP 适配层只转换 DTO。受保护文件解除前为 `[!]`。

## P1：双 UI 与模式选择

- [ ] **EX-10 WebUI manifest/快照适配**：WebUI 获取并校验 manifest，页面、动作、字段、可用条件和错误映射由共享契约驱动。验收：manifest 版本不匹配时明确失败，不静默降级。
- [ ] **EX-11 GPUI 应用外壳**：引入 GPUI，完成窗口、主题、导航、状态订阅、错误提示和退出清理；桌面模式不依赖浏览器。验收：可在 Linux 桌面启动并连接真实节点命令总线。
- [ ] **EX-12 GPUI 功能页面**：按 manifest 实现网络、Peer、拓扑、流量、路由、Exit Node、中继、日志、身份和设置页面。验收：不实现第二套校验、权限或状态机。
- [ ] **EX-13 UI 模式选择与回退**：实现 `--gpui`、`--headless` 和自动检测；无显示环境使用 WebUI，GPUI 初始化失败给出显式策略。验收：三种模式及失败回退可自动化测试。
- [ ] **EX-14 UI 对等契约测试**：依据 manifest 自动比较 WebUI/GPUI 的页面、动作、字段、校验、错误映射和权限。验收：缺少任一动作或语义不一致时 CI 失败，覆盖无网络、单网络和双网络。

## P1：安全、性能与发布门禁

- [ ] **EX-15 WebUI 暴露面收紧**：默认只绑定回环或受信地址，并提供显式危险覆盖；在没有逐主体鉴权前拒绝公开监听。验收：启动参数、警告、文档和测试一致。
- [ ] **EX-16 安全验收补齐**：推进 `SECURITY_PERFORMANCE_TODO.md` 中的威胁模型、密钥轮换、混合后量子生产接线和证书授权策略。验收：每项有负向测试和明确未审计边界，不宣称超越 WireGuard。
- [ ] **EX-17 数据面性能快路径**：完成 Peer 热状态、缓冲池、批量 I/O 和端到端 profiling，在 CPU 固定、安静主机上与 WireGuard/wireguard-go 复测。验收：提交原始数据与复现命令，只有达到既定阈值才更新结论。
- [ ] **EX-18 最终发布门禁**：验证唯一 `fuckipv6` 入口、三平台构建、UI 冒烟、安全边界和文档一致；清理遗留 coordinator 产物与失效口径。验收：干净工作区执行发布门禁通过，README/TODO/CHANGELOG 不夸大成熟度。
