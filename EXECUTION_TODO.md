# fuckipv6 可执行 TODO

> 本清单把路线图拆成可独立提交、可验证的短任务。状态约定：`[ ]` 未开始，`[>]` 进行中，`[x]` 已完成，`[!]` 被外部修改阻塞。
>
> 执行规则：每项先完成代码与测试，再同步本清单和 `CHANGELOG.md`；验证通过后单独提交。禁止使用 `git commit -a`；只能暂存本任务文件，不得覆盖工作区中用户尚未提交的改动。并行任务按文件范围隔离，合并时如遇冲突必须人工核对。

## P0：共享契约与运行时底座

- [x] **EX-01 UI manifest 命令通道**：让 UI 通过统一节点命令总线获取版本化 manifest，而不是在 WebUI/GPUI 各复制页面和动作定义。验收：新增 `GetUiManifest` 命令与类型化结果；JSON 往返可用；返回 11 个页面和当前 schema 版本。已通过 `GetUiManifest` 与类型化结果接通统一命令总线。涉及 `crates/client/src/node/{command,runtime_handler,ui_manifest}.rs`。
- [x] **EX-02 内嵌服务状态快照**：把每个网络的内嵌信令/中继实例状态、监听地址和生命周期纳入 `NodeSnapshot` 与事件流。已完成 `Starting/Running/Stopping/Stopped/Failed` 状态、`EmbeddedServiceChanged` 事件及无票据/私钥/路径的共享 DTO；创建、启动失败、停止与全量关闭均有状态和事件测试。涉及 `crates/client/src/node/{state,event,mod,runtime_control}.rs`。
- [x] **EX-03 节点状态不变量**：为网络名称唯一、配置/运行网络对应、停止后 Peer 清理、失败回滚和 revision 单调性增加集中式不变量测试。已完成 `NodeSnapshot::invariant_violations()`，可诊断网络/配置/内嵌服务重名、运行网络缺配置、未知/离线网络 Peer 及 `peer_count` 不一致；停止、自然退出和失败回滚会清理 Peer，revision 在 `u64::MAX` 饱和而不是回绕。涉及 `crates/client/src/node/{state,runtime_control}.rs`。
- [x] **EX-04 命令错误契约**：补齐所有 `NodeCommand` 的验证、稳定错误码、可重试标记和序列化往返测试；路由/角色未实现时必须返回 `Unavailable`。已完成 10 个命令的 JSON 往返与拒绝路径覆盖，`StopNetworks { all: true }` 与显式网络名冲突会被拒绝，网络名校验与 `ConfigManager` 的 ASCII 字符集对齐；`NodeProblem` 固定 `invalid_request/not_found/conflict/unavailable/permission_denied/cancelled/internal` 线值及 `retryable`，运行时错误映射和未实现命令均有 handler 测试。`UpdateRouting` 已由 NX-04 接入运行中网络；`SetNetworkRole` 仍未实现并明确返回可重试的 `Unavailable`。涉及 `crates/client/src/node/{command,runtime_handler,state}.rs`。
- [x] **EX-05 零网络节点身份**：零网络启动时也建立并发布节点公开身份、公钥指纹和配置中的显示名。已完成配置根目录唯一的 `node_identity.pem` 与可选 `node_identity.json` 元数据；supervisor 构造即发布身份，所有网络启动计划复用同一节点密钥。测试覆盖空网络快照、跨重启稳定、显示名来源、私钥/种子/配置路径不进入 DTO。涉及 `crates/client/src/{config_manager,multi_network}.rs` 与 `crates/client/src/node/{plan,runtime_control,runtime_metadata,state}.rs`。

## P1：后端功能闭环

- [x] **EX-06 路由热更新后端**：把 `UpdateRouting` 接入 `NetworkRuntimeManager`，同步 Smart Route、Exit Node 配置、持久化和共享快照。验收：成功与失败均回滚一致，网络运行中切换不重启节点；完成后解除 CLI/WebUI 迁移的 `Unavailable`。已由 NX-04 完成统一命令总线、运行态与控制通道接线；CLI/WebUI 自身迁移仍分别由 EX-08/EX-09 跟踪。
- [ ] **EX-07 网络角色热更新后端**：把 `SetNetworkRole` 接到持久化与运行时重配置，覆盖成员、发现、中继和 Exit Node 角色。验收：非法组合明确拒绝，停止后可释放服务，角色变化发布状态与事件。
- [ ] **EX-08 CLI 统一命令迁移**：CLI 的网络创建、加入、启停、探测和关闭只通过 `NodeCommandBus`。验收：不保留第二套业务规则，输出消费统一结果和错误码。
- [ ] **EX-09 WebUI 统一命令迁移**：WebUI 的节点操作与状态读取只通过共享命令总线和 `NodeStateStore`。验收：移除 controller 内部独立网络/状态变更逻辑，HTTP 适配层只转换 DTO。

## P1：双 UI 与模式选择

- [ ] **EX-10 WebUI manifest/快照适配**：WebUI 获取并校验 manifest，页面、动作、字段、可用条件和错误映射由共享契约驱动。验收：manifest 版本不匹配时明确失败，不静默降级。
- [x] **EX-11 GPUI 应用外壳**：引入 GPUI，完成窗口、主题、导航、状态订阅、错误提示和退出清理；桌面模式不依赖浏览器。已完成真实 `gpui::Application::run`/`open_window` 外壳、manifest 导航、`NodeStateStore` 订阅、`NodeCommandBus` 命令、错误提示、无显示/未编译能力门禁和退出清理；Linux 桌面与 Xvfb 冒烟均可保持原生窗口进程运行。业务页面与自动对等测试归 EX-12/EX-14。
- [ ] **EX-12 GPUI 功能页面**：按 manifest 实现网络、Peer、拓扑、流量、路由、Exit Node、中继、日志、身份和设置页面。验收：不实现第二套校验、权限或状态机。
- [x] **EX-13 UI 模式选择与回退**：实现 `--gpui`、`--headless` 和自动检测；无显示环境使用 WebUI，GPUI 初始化失败给出显式策略。验收：三种模式及失败回退可自动化测试。完成：共享 `Gpui/WebUi/Headless` 模式契约、冲突/无显示/GPUI 不可用错误、CLI 参数接线、旧 `--gui` 到 WebUI 的兼容别名及 `NodeSnapshot.ui_mode` 均已落地；NX-08 已把顶层 `--gpui` 接到真实原生窗口，未编译或无显示时明确失败，不会伪装成 WebUI。
- [>] **EX-14 UI 对等契约测试**：依据 manifest 自动比较 WebUI/GPUI 的页面、动作、字段、校验、错误映射和权限。验收：缺少任一动作或语义不一致时 CI 失败，覆盖无网络、单网络和双网络。

## P1：安全、性能与发布门禁

- [x] **EX-15 WebUI 暴露面收紧**：默认只绑定回环或受信地址，并提供显式危险覆盖；在没有逐主体鉴权前拒绝公开监听。验收：启动参数、警告、文档和测试一致。完成：WebUI/API 默认 `127.0.0.1`、支持 `::1`，非回环必须显式危险开关和非空密码；IPC 默认 `127.0.0.1` 且无认证时远程 fail-closed；日志、401 响应与文档已对齐。
- [>] **EX-16 安全验收补齐**：推进 `SECURITY_PERFORMANCE_TODO.md` 中的威胁模型、密钥轮换、混合后量子生产接线和证书授权策略。验收：每项有负向测试和明确未审计边界，不宣称超越 WireGuard。已收紧节点身份私钥权限：Unix 创建时固定 `0600`，覆盖前也先收紧旧权限，并有新文件/旧宽权限回归测试；已把 Coordinator 运行期黑名单接入 QUIC relay 注册和双向转发，撤销身份在产生 relay 状态或转发计数前失败关闭；Relay TLS 已使用随在线会话动态更新的客户端证书 Ed25519 公钥 allowlist，空名单默认拒绝，注册/换钥更新，注销/按节点 ID 封禁撤销。密钥轮换、混合后量子生产接线、CA/CRL/OCSP、网络级最终授权、稳定身份与公钥解耦、多 Coordinator 同步、P2P QUIC Mesh 撤销及独立审计仍未完成；持久撤销目录、Coordinator 撤销 API 和 P2P UDP 会话撤销已落地。
- [ ] **EX-17 数据面性能快路径**：完成 Peer 热状态、缓冲池、批量 I/O 和端到端 profiling，在 CPU 固定、安静主机上与 WireGuard/wireguard-go 复测。验收：提交原始数据与复现命令，只有达到既定阈值才更新结论。
- [ ] **EX-18 最终发布门禁**：验证唯一 `fuckipv6` 入口、三平台构建、UI 冒烟、安全边界和文档一致；清理遗留 coordinator 产物与失效口径。验收：干净工作区执行发布门禁通过，README/TODO/CHANGELOG 不夸大成熟度。

---

## 下一阶段执行队列（NX-01 至 NX-18，2026-10-30）

> NX-01 至 NX-18 是对上方 EX-06 至 EX-18 的可执行细化。父项与 NX 项完成时必须同步更新；下一阶段状态只以本文件为准，`TODO.md` 仅保留入口和历史里程碑，避免出现第二份状态源。
>
> 产品模型固定为“只有节点”，不区分服务端与客户端。桌面环境使用原生 GPUI；无桌面环境使用 WebUI。两套 UI 必须共享 manifest、命令、快照、事件、权限、校验与错误语义，只允许渲染层不同。
>
> 工作区隔离：`crates/client/src/cli/commands.rs`、`crates/client/src/cli/mod.rs`、`crates/client/src/webui/controller.rs` 当前可能包含用户未提交的 Exit Node 持久化改动。实施相关任务时必须 rebase 到用户改动落地后的 HEAD；不得覆盖、回退或混入用户改动。禁止通过复制业务逻辑、重启网络、伪造成功或浏览器 `--app` 窗口绕过验收。

### P0-A：统一控制面与节点语义

- [x] **NX-01 UI 模式真值契约（细化 EX-13）**
  - 工作：定义唯一的 `Gpui | WebUi | Headless` 运行模式及其检测来源，明确自动检测、显式参数、冲突参数和失败回退规则；状态快照必须报告实际生效模式。
  - 依赖：EX-01、EX-05；CLI 参数接线涉及受保护的 `crates/client/src/cli/mod.rs`。
  - 验收：三种模式均有类型化和序列化测试；浏览器独立窗口只能标记为 WebUI，不能冒充 GPUI；冲突参数拒绝启动；自动检测结果可注入并覆盖 Wayland、X11、无显示和 Windows 服务场景。
  - 完成（2026-10-30）：共享模式/请求/显示环境模型、稳定 wire 值、冲突与无显示拒绝、GPUI 能力门禁和 `NodeSnapshot.ui_mode` 已落地；顶层及节点运行命令支持 `--headless`、`--webui`、`--gpui`，`--gui` 保留为 WebUI 可见别名，冲突参数由 Clap 拒绝，原生 GPUI 不可用时显式失败。

- [x] **NX-02 本地控制面暴露面收紧（细化 EX-15）**
  - 工作：WebUI/API 和本地 IPC 默认只绑定回环地址；远程监听必须显式开启、产生醒目警告并要求已配置认证，禁止匿名公网暴露。
  - 依赖：EX-03、EX-15；受保护 CLI 文件保持不修改，危险开关通过显式环境变量接入现有启动路径；合并时必须保留用户未提交改动。
  - 验收：默认 IPv4/IPv6 监听均为回环；`0.0.0.0`/`::` 仅能通过显式危险参数开启且缺少认证时启动失败；参数、日志、HTTP 响应和文档一致；覆盖拒绝启动、认证成功/失败和回环回归测试。
  - 完成（2026-10-30）：WebUI/API 默认 `127.0.0.1:8080`，支持 `FUCKIPV6_WEBUI_BIND=::1`；非回环必须 `FUCKIPV6_WEBUI_ALLOW_REMOTE=1` 且配置密码，否则拒绝启动。REST 使用 Bearer JWT，WebSocket 查询 Token 仅限 `/ws`；401/`WWW-Authenticate`/日志/文档一致。IPC 默认 `127.0.0.1:6668`，支持 `[::1]:6668`，危险开关存在但因无认证协议仍拒绝远程监听。后续加固已为登录增加按源 IP+用户名的原子失败计数、指数退避与 429/`Retry-After`，成功登录清除状态，状态表有容量和 TTL 上限；显式 JWT secret 校验最小长度与估计熵，并锁定认证 DTO/配置的 Debug 脱敏。

- [>] **NX-03 共享节点 HTTP/WS 适配器（细化 EX-10）**
  - 工作：提供版本化的快照、manifest 和事件 WebSocket 适配层，把 HTTP/WS DTO 转换为 `NodeCommandBus` 与 `NodeStateStore` 调用；不拥有业务状态。
  - 依赖：EX-01、EX-02、EX-04、NX-01。
  - 验收：快照只来自 `NodeStateStore`，动作只发往命令总线；manifest schema 版本不匹配时明确失败而非降级；WS 断线、重复事件、revision 跳跃和非授权请求均有契约测试；不存在第二套页面/动作定义。
  - 进展（2026-10-30）：新增版本化 `/api/node/snapshot`、`/api/node/manifest`、WebSocket `/api/node/events` 和 `POST /api/node/commands`；读路径经命令总线取得共享状态，动作请求原样提交 `NodeCommandBus`。覆盖 schema 不匹配、非法请求、后端拒绝、`Unavailable`、重复/回退 revision、跳号与广播滞后重同步、断线。manifest 现在在 HTTP 适配器交付前检查 schema、固定页面集合与顺序、命令覆盖、动作/字段唯一性、必填字段校验、命令权限与后端状态一致性；格式错误返回 `ContractViolation` 而不是交给 UI 降级渲染。剩余：挂载到实际 WebUI/GPUI（NX-07/NX-08），并与 NX-02 的认证/暴露面策略接通后才算完整验收。

- [x] **NX-04 路由热更新后端（细化 EX-06）**
  - 工作：让 `UpdateRouting` 原子更新运行中的 Smart Route、Exit Node 配置、持久化配置、共享快照和事件；任何阶段失败都恢复旧运行态。
  - 依赖：EX-03、EX-04；必须基于用户 Exit Node 持久化改动后的最新 HEAD 实现运行控制接线，不得覆盖工作区改动。
  - 验收：运行中切换不重启网络、不清空 Peer；持久化失败、运行时应用失败和快照发布失败均可回滚；并发更新串行化；成功/失败均有集成测试，`Unavailable` 仅在未实现时保留。
  - 完成（2026-10-30）：`NodeCommandBus::UpdateRouting` 经 `NetworkRuntimeManager` actor 串行进入现有多网络运行循环，由 `apply_runtime_routing` 同时更新 Smart Route、Exit Node 运行配置和磁盘配置，再发布共享配置快照与 `RoutingChanged` 事件。actor 在运行时拒绝、响应超时、响应通道关闭和快照发布失败时重新应用旧配置；超时后的补偿命令排在同一控制通道中，晚到的新配置会被随后恢复。测试覆盖命令总线成功路径、Peer 保留、无重启、运行时失败、晚到超时回滚、并发串行后写胜出和快照失败回滚。manifest 已将 `update_routing` 标为 `Implemented`；`SetNetworkRole` 仍为 `Unavailable`。

- [ ] **NX-05 网络角色热更新后端（细化 EX-07）**
  - 工作：把 `SetNetworkRole` 接到成员、发现、中继和 Exit Node 角色的运行时服务生命周期与持久化，保持“节点内角色”而非服务端/客户端进程划分。
  - 依赖：EX-02、EX-04、NX-04；复用统一事务与回滚边界。
  - 验收：非法角色组合返回稳定错误码；角色启用只启动所需内嵌服务，禁用后释放端口/任务；失败回滚持久化与运行态；角色变化产生快照和事件；覆盖零网络、停止网络和多网络隔离。

- [ ] **NX-06 CLI 统一命令迁移（细化 EX-08）**
  - 工作：CLI 的建网、加入、启停、探测、状态、路由、Exit Node 和关闭全部通过 `NodeCommandBus`，CLI 只负责参数解析、确认和结果渲染。
  - 依赖：NX-01、NX-03、NX-04、NX-05；涉及 `crates/client/src/cli/commands.rs` 与 `crates/client/src/cli/mod.rs`，合并时必须保留用户未提交改动。
  - 验收：删除或封存 CLI 内独立运行时状态机和直接配置写入；帮助文本、退出码和错误输出消费统一 `NodeProblem`；无网络、运行网络、失败回滚和多网络场景有端到端测试；不改变节点唯一产品模型。

- [ ] **NX-07 WebUI 统一命令迁移（细化 EX-09）**
  - 工作：WebUI 节点操作和状态读取只通过共享命令总线、状态存储和事件流；controller 只保留连接/展示适配，不直接改业务状态。
  - 依赖：NX-03、NX-04、NX-05；涉及 `crates/client/src/webui/controller.rs`，合并时必须保留用户未提交改动。
  - 验收：移除 controller 内重复的网络/路由/Exit Node 状态机及直接运行时写入；HTTP 层只做 DTO 和鉴权转换；所有失败透传错误码与可重试标记；无网络、双网络、断线重连和权限拒绝均有集成测试。

### P0-B：GPUI 与 UI 对等

- [x] **NX-08 GPUI 应用外壳（细化 EX-11）**
  - 工作：接入真正的 GPUI 原生窗口，完成主题、导航、`NodeStateStore` 订阅、命令提交、错误提示和退出清理；不得依赖浏览器、WebView 或 `--app` 窗口。
  - 依赖：NX-01、NX-03；命令总线与状态存储保持现有共享契约。
  - 验收：Linux 桌面可启动独立 GPUI 进程并连接真实节点；窗口关闭/崩溃路径不遗留运行时任务；无显示环境不会错误启动 GPUI；截图或自动窗口冒烟测试纳入 CI 可运行环境。
  - 完成（2026-10-30）：新增 `gpui_shell`，顶层 `--gpui` 与桌面自动模式在 OS 主线程启动真实 `gpui::Application` 窗口；原生会话使用独立 Tokio runtime、真实 `MultiNetworkSupervisor`、共享 `NodeStateStore`/`NodeCommandBus`，11 页 manifest 全部有导航项，非 overview 页面显示 NX-09 占位且不伪造动作。无显示和未编译 shell 在创建窗口前失败；正常关闭、窗口创建失败和 panic unwind 均通过带超时的 `Shutdown`/runtime 回收清理。Linux Wayland 实机与 Xvfb 均验证窗口进程可启动；`scripts/gpui-smoke.sh` 同时验证 headless 能力门禁和强制 X11 原生窗口启动，并已加入 Verify CI。

- [ ] **NX-09 GPUI 功能页面（细化 EX-12）**
  - 工作：严格按 11 页 manifest 实现 `overview`、`networks`、`peers`、`topology`、`traffic`、`routing`、`exit_node`、`relay`、`logs`、`identity`、`settings`。
  - 依赖：NX-03、NX-08；后端未实现动作继续显示 manifest 的 `Unavailable`，不得前端伪造可用。
  - 验收：每页动作、字段、必填项、校验、确认、权限和不可用原因均来自共享契约；GPUI 不复制业务校验或状态机；覆盖空节点、单网络、双网络、离线、权限不足和命令失败状态。

- [>] **NX-10 UI 对等自动门禁（细化 EX-14）**
  - 工作：建立 WebUI/GPUI 共用的页面与动作覆盖检查，自动比较页面集合、动作、字段、可用条件、权限、错误映射和命令 DTO。
  - 依赖：NX-03、NX-07、NX-09。
  - 验收：任一 UI 缺少页面、动作或字段时 CI 失败；manifest 版本漂移导致失败；至少覆盖无网络、单网络和双网络；允许渲染差异但不允许语义差异。
  - 进展（2026-10-30）：manifest 自带 fail-closed 契约校验，HTTP 适配器在交付前拒绝页面顺序/集合、action/field 重复、必填字段无校验、命令权限/后端状态不一致及 Shutdown 确认缺失。该基础门禁不能替代真实 WebUI/GPUI 渲染层对等测试，NX-10 仍未完成。

### P1-C：安全能力生产化

- [ ] **NX-11 生产数据面在线重协商（细化 EX-16）**
  - 工作：把 REKEY 接入真实 UDP、QUIC 和 relay 数据面，支持密钥世代切换、旧世代短暂重叠和失败保留旧会话。
  - 依赖：EX-16、现有 Secure Session 握手与数据面。
  - 验收：会话到期前触发 REKEY；切换期间不中断有效流量；重放/乱序/旧世代包有明确窗口；失败不降级为明文或旧弱参数；敏感旧密钥及时清零；UDP、QUIC、relay 均有集成测试。

- [ ] **NX-12 握手 DoS 与限速（细化 EX-16）**
  - 工作：实现握手 Cookie/挑战、每源地址与每身份配额、全局并发上限、有限握手状态、TTL 清理、背压和可观测指标。
  - 依赖：NX-11、统一身份目录。
  - 验收：伪造源地址泛洪不能无限分配内存或 CPU；达到配额返回可识别错误而非崩溃；有效节点不被短时突发永久饿死；状态表有硬上限和淘汰测试；日志不泄露密钥或完整敏感票据。

- [ ] **NX-13 后量子 profile 生产接入（细化 EX-16）**
  - 工作：把已选定的混合后量子 profile 接入能力协商、CONFIRM、握手 transcript、数据面密钥派生、版本/算法绑定和容量预算；禁止静默降级。
  - 依赖：NX-11；密码学原语已有测试但尚未完成生产接线。
  - 验收：双方支持时协商 PQ profile，不支持或确认失败时按策略拒绝而非弱降级；profile/算法版本进入 transcript 与审计事件；固定向量、篡改、降级和互操作测试通过；明确性能与流量开销；独立审计前不得宣称“后量子安全”或“超越 WireGuard”。

- [>] **NX-14 身份授权与撤销生命周期（细化 EX-16）**
  - 工作：建立网络级最终授权、持久撤销目录和一致的身份查找，使控制面、relay、P2P QUIC 和重连路径使用同一授权结论。
  - 依赖：NX-02、NX-12；复用现有黑名单持久化与证书 allowlist 边界。
  - 验收：重启后撤销仍生效；撤销会阻止新会话并使现有会话按策略失效；目录/存储失败时失败关闭；跨网络不能越权复用身份；覆盖崩溃恢复、并发撤销/注册和负向授权测试。
  - 进展（2026-10-30）：Coordinator 已加入持久撤销目录和稳定 `network_id`，旧路径哈希记录可迁移且歧义时拒绝改写；缺目录、读取失败或存储不支持撤销时注册失败关闭。撤销/注册经授权门串行，进程内互斥、跨进程文件锁和原子替换保证并发更新不丢记录；重启后撤销仍生效，跨网络写入被拒绝，撤销会断开在线会话并移出证书 allowlist，QUIC relay 已建立连接逐包重查持久撤销。覆盖独立子进程并发撤销、重启恢复和存储失败负向测试。剩余：稳定节点身份与当前公钥解耦、产品 CLI/WebUI/GPUI 撤销入口、多 Coordinator 即时同步、P2P QUIC Mesh 完整撤销/重连策略和授权 epoch/租约绑定。Coordinator 撤销 API 与 P2P UDP 会话撤销已完成。

- [ ] **NX-15 Exit Node 跨平台一致性（细化 EX-16/EX-18）**
  - 工作：补齐 Linux network namespace 自动化，并为 Windows/macOS 实现等价转发/NAT 能力或明确能力探测与禁用原因。
  - 依赖：NX-04、NX-05。
  - 验收：Linux 覆盖启动、路由、NAT、清理和故障回滚；Windows/macOS 要么通过实机集成测试，要么 UI/CLI/API 明确报告不支持且禁用动作；任何平台都不得显示虚假的 Exit Node 成功状态。

- [x] **NX-16 用户态 NAT 可行性原型（细化 EX-16）**
  - 工作：原型实现 TCP/UDP 连接跟踪、超时、端口复用、资源上限和失败关闭策略，评估作为无内核转发平台后备路径的可行性。
  - 依赖：NX-04、NX-15。
  - 验收：覆盖 TCP 建连/关闭、UDP 会话超时、端口耗尽、并发上限和畸形包；有内存/吞吐/延迟预算；原型通过 feature flag 隔离并可一键禁用；若无法满足安全或性能门槛，产出 no-go 结论而不是产品化半成品。
  - 进展（2026-10-30）：纯标准库原型已在非默认 `userspace-nat-prototype` feature 中完成，未接入节点产品路径。实现 IPv4/TCP/UDP 严格解析与校验和、TCP/UDP 有界状态与超时、确定性端口分配/复用、每源及全局流上限、容量拒绝不驱逐在线流、未知入站/畸形包/时钟回退失败关闭；12 个确定性测试通过。默认逻辑内存预算为 64 KiB + 512 B/流（4096 流约 2.06 MiB accounting），性能门禁为固定 CPU 下 1500 B 包单核 ≥1 Gbit/s 且 p99 校验+查表 ≤10 µs，但尚未测量。当前没有包重写/校验和更新、真实 I/O、RSS、吞吐/延迟、跨平台或独立安全证据，结论为 **NO-GO，禁止产品接入**。详见 [`docs/USERSPACE_NAT_PROTOTYPE.md`](docs/USERSPACE_NAT_PROTOTYPE.md)。

### P1-D：性能与发布证据

- [>] **NX-17 数据面性能快路径（细化 EX-17）**
  - 工作：完成 Peer 热状态、无锁/低锁快路径、缓冲池、批量 I/O、减少逐包分配和序列化，并用可重复 profiling 驱动优化。
  - 依赖：NX-11；先冻结基准脚本、流量模型、固定 CPU 和安静主机条件。
  - 验收：提交原始吞吐、p99 延迟、CPU/包和内存数据及复现命令；与固定版本内核 WireGuard 和 wireguard-go 对比；至少吞吐高于基线、p99 不劣化且 CPU/包不劣化，才可标记“性能超过”；未达标时保留原始结果并明确失败。
  - 进展（2026-10-30）：基准门禁已改为从 canonical raw iperf3/perf/ping 证据重算指标，校验完整 commit/tree、干净工作区、源码与三方二进制哈希、证据 manifest 完整性和执行参数；弱化 policy、篡改/逃逸/重复证据、缺失或非正指标、UDP 单 run 丢包超限均强制失败。25 个 benchmark 合成测试通过；尚未在固定 CPU 的安静主机执行真实对比，性能结论仍为“未达标/未证明”。

- [ ] **NX-18 三平台发布证据与最终门禁（细化 EX-18）**
  - 工作：验证唯一 `fuckipv6` 节点入口、Linux/Windows/macOS 构建与运行、WebUI/GPUI 冒烟、单二进制内嵌服务、安全边界、文档和性能证据；清理遗留 coordinator 产物与失效口径。
  - 依赖：NX-01 至 NX-17 中所有发布必需项；干净工作区。
  - 验收：三平台发布门禁通过；GPUI 与 WebUI 语义对等，无桌面环境自动使用 WebUI；单二进制不暴露独立服务端/客户端模式；README、TODO、CHANGELOG、API、ARCHITECTURE 不夸大成熟度；无独立密码学审计和可复现性能证据时，禁止发布“加密等级或性能超越 WireGuard”的结论。

### 推荐执行顺序

1. **先稳定当前 P2P/路由/安全改动**：基于用户 Exit Node 持久化改动后的 HEAD 完成 NX-04、NX-05，再处理 NX-06、NX-07；不得把用户改动混入本任务提交。
2. **控制面与 UI 地基**：NX-01 → NX-03 → NX-02 → NX-08 → NX-09 → NX-10，CLI/WebUI/GPUI 始终消费同一 manifest、命令和快照。
3. **安全生产化**：NX-11 → NX-12 → NX-14 → NX-13 → NX-15/NX-16，先保证会话切换和失败关闭，再扩大能力面。
4. **证据驱动的性能与发布**：NX-17 达标后才执行 NX-18；每个任务独立提交，并在提交前运行格式、Clippy、测试和文档一致性检查。
