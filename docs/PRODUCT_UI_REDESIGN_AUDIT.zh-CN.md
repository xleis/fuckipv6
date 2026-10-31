# fuckipv6 产品化 UI 与节点控制面现状审计

> 审计日期：2026-10-31
> 审计提交：`104ec8ef6c80d9461e282090d25ee319cbd010ec`
> 规划基线：`b1bb83723b53908341b763b5fd10b6de4bf34ac1`
> 上游方案：`docs/PRODUCT_UI_REDESIGN.zh-CN.md`
> 待办清单：`docs/PRODUCT_UI_REDESIGN_TODO.zh-CN.md`
> 本文定位：W0 现状冻结证据基线，不是功能完成声明，也不是发布验收报告

## 0. 审计范围与结论边界

本次审计覆盖：

- 单一 `fuckipv6` 二进制的 UI 选择与进程生命周期；
- GPUI 11 页、共享 node manifest 11 页与旧统一 WebUI 5 页；
- 旧 WebUI controller API、NodeCommandBus、HTTP/WebSocket node adapter；
- node、network、peer、Exit、Relay、Coordinator 与控制面的当前状态语义；
- 保留、重写、删除边界，以及工作区提交保护。

本次没有进行：

- 真实 TUN/路由/DNS 变更测试；
- Google、YouTube、DNS 泄漏或吞吐/延迟/丢包实测；
- 多机 Coordinator、Relay、Exit 故障注入；
- 跨平台实机测试；
- 对既有自动化测试套件的重新执行，因为本提交只修改文档。

因此：

- “代码存在”不等于“产品功能完成”；
- “页面能打开”不等于“动作已闭环”；
- “命令返回过成功”不等于“网络质量达标”；
- 翻墙、抗干扰和网速仍只能标记为 `目标` 或 `未知`，不能对外宣称合格。

## 1. 证据等级与状态标记

### 1.1 证据等级

| 等级 | 含义 |
|---|---|
| E1 | 当前提交中的代码路径、类型、路由或配置可直接确认 |
| E2 | 当前提交内存在单元/集成测试，但本次未重跑 |
| E3 | 本次实际运行命令或测试并记录结果 |
| E4 | 隔离环境中的多机端到端实测，带环境快照、清理证据和 commit |

本审计以 E1 为主，只引用现有测试源码说明“已有覆盖意图”，不把 E2 当作 E3/E4。后续能力声明必须绑定更高等级证据。

### 1.2 闭环状态

| 状态 | 定义 |
|---|---|
| `已验证` | 当前提交有可复现测试或实网证据，成功、失败与恢复路径均满足验收 |
| `已实现待验证` | 后端路径存在，但缺少当前提交上的端到端、权限或恢复证据 |
| `垂直切片` | 页面、命令或适配器的一部分存在，尚未形成完整产品闭环 |
| `目标` | 已确认要交付，但当前没有完整实现 |
| `未支持` | 当前后端明确返回不可用、拒绝或没有对应能力 |
| `未知` | 证据不足，不能确认行为 |

## 2. 审计基线与工作区保护

### 2.1 当前仓库事实

- 分支：`main`，相对 `origin/main` 没有额外提交状态变化；
- HEAD：`104ec8ef6c80d9461e282090d25ee319cbd010ec`；
- 规划基线：`b1bb83723b53908341b763b5fd10b6de4bf34ac1`；
- 产品 bin 只有 `crates/client/Cargo.toml` 中的 `fuckipv6`；
- 顶层入口为 `crates/client/src/main.rs::main`，调用 `cli::run_entrypoint()`。

### 2.2 审计开始时工作区

审计开始时已存在并发/在途改动，不属于 W0 文档提交：

```text
 M crates/client/src/cli/commands.rs
 M crates/client/src/cli/mod.rs
 M crates/client/src/ipc.rs
 M crates/client/src/multi_network.rs
A  crates/client/src/p2p/buffer_pool.rs
 M crates/client/src/p2p/connection.rs
MM crates/client/src/p2p/mod.rs
 M crates/client/src/p2p/overlay.rs
 M crates/client/src/p2p/secure_runtime.rs
A  crates/client/src/p2p/udp_batch.rs
 M crates/client/src/signaling/client.rs
 M crates/client/src/webui/controller.rs
 M scripts/bench-dataplane.sh
?? crates/client/src/p2p/inbound_queue.rs
?? docs/PRODUCT_UI_REDESIGN.zh-CN.md
?? docs/PRODUCT_UI_REDESIGN_TODO.zh-CN.md
?? release/fuckipv6-coordinator-linux-x86_64
```

其中主索引已有：

```text
A crates/client/src/p2p/buffer_pool.rs
M crates/client/src/p2p/mod.rs
A crates/client/src/p2p/udp_batch.rs
```

保护规则：

- 不使用 `git add -A`、`git add .` 或会覆盖现有索引的提交方式；
- W0 只提交本审计文档与 TODO 文档；
- 不删除 `release/fuckipv6-coordinator-linux-x86_64`；
- 不删除未跟踪的 `crates/client/src/p2p/inbound_queue.rs`；
- 不自动清理其他在途改动；
- secure-session 线格式/驱动与独立 UDP 适配器继续按依赖顺序单独提交。

### 2.3 绝对不可提交的文件

以下三个文件在本轮所有提交中都必须排除：

```text
crates/client/src/cli/commands.rs
crates/client/src/cli/mod.rs
crates/client/src/webui/controller.rs
```

提交前必须检查 `git diff --cached --name-only`，不能只依赖编辑器或提交消息判断。

## 3. 单二进制、进程角色与生命周期

### 3.1 单二进制事实

证据：

- `crates/client/Cargo.toml`：唯一产品 `[[bin]] name = "fuckipv6"`；
- `crates/client/src/main.rs`：只调用 `fuckipv6_client::cli::run_entrypoint()`；
- `NodeCommand` 注释明确没有产品级 client/server 命令变体；
- `NodeNetworkRole` 只有 `Member/Discovery/Relay/ExitNode`，它们是网络角色，不是第二种产品身份。

结论：

- 单二进制已经是代码事实，状态为 `已验证`（E1）；
- 目标中的 control 与 node 内部进程拆分尚未完成，状态为 `目标`；
- 当前 GPUI 在进程内持有 node runtime，状态为 `垂直切片`。

### 3.2 UI 选择

证据：

- `crates/client/src/ui/mode.rs::UiMode`：`Gpui/WebUi/Headless`；
- `UiModeRequest`：`Auto/Gpui/WebUi/Headless`；
- `resolve_ui_mode()`：
  - 无桌面环境 → `Headless`；
  - 有桌面且 GPUI 可用 → `Gpui`；
  - 有桌面但 GPUI 不可用 → `WebUi`；
  - 显式 `--gpui` 有 GPUI 可用性和显示器门禁；
  - 旧 `--gui` 只映射到 WebUI，不冒充 GPUI；
- `crates/client/src/cli/mod.rs::run_default_app()` 对 WebUi/Headless 都启动旧 WebUI server；GPUI 被要求走顶层 `--gpui` 路径。

结论：

- 自动选择逻辑有代码与单测路径，状态为 `已实现待验证`；
- “无公网 IP 即用 WebUI、个人 PC 即用 GPUI”不是当前实现，也不应成为目标；目标只看显式偏好与桌面环境；
- 顶层 `--host` 尚未接入 GPUI，当前提示使用 `--webui`，状态为 `垂直切片`。

### 3.3 关闭 UI 的生命周期

证据：

- `crates/client/src/gpui/session.rs::NativeNodeSession` 持有 Tokio runtime 与 `MultiNetworkSupervisor`；
- `crates/client/src/gpui/mod.rs` 在 `view::run_window(...)` 返回后调用 `session.shutdown()`；
- `NativeNodeSession::shutdown_inner()` 提交 `NodeCommand::Shutdown`，然后 `drop(supervisor)` 并关闭 Tokio runtime；
- `Drop for NativeNodeSession` 再次收敛到同一清理路径。

结论：

- 当前关闭 GPUI 会停止节点网络，违反“关闭 UI 不断网”，状态为 `未支持`；
- 当前进程生命周期与控制面、数据面未拆开，状态为 `垂直切片`；
- 目标 W2 必须把 node/control 生命周期从窗口生命周期解耦，并由服务监督器托管。

## 4. 旧页面到新五模块的统一映射

新一级导航固定为：

1. 概览
2. 节点
3. 网络
4. 活动
5. 设置

### 4.1 GPUI 11 页映射

证据：

- `crates/client/src/gpui/view_model.rs::PageKind::ALL` 固定 11 页；
- `crates/client/src/gpui/view.rs::page_title()` 提供对应中文标题；
- `project_*()` 从同一个 `NodeSnapshot` 生成各页模型。

| 旧页面 | 当前内容/动作 | 当前闭环 | 新归属 | 处置决定 |
|---|---|---|---|---|
| `overview` | 节点概览、刷新快照 | `已实现待验证` | 概览 | 重写为唯一主状态、待处理问题和网络卡片 |
| `networks` | 创建、加入、启动、停止网络 | 运行时有 runtime 时 `已实现待验证`，无 runtime 时 `未支持` | 网络 → 我的网络 | 保留命令语义，重写交互与对象详情 |
| `peers` | Peer 列表、探测 Peer | `已实现待验证` | 网络 → 成员与拓扑 | 保留探测能力，页面降为网络对象子视图 |
| `topology` | 拓扑摘要 | `垂直切片`，无真实拓扑动作 | 网络 → 成员与拓扑 | 重写为全局可交互拓扑，不保留为一级页 |
| `traffic` | 流量计数摘要 | `垂直切片` | 节点 → 运行状态/诊断 | 删除自动刷新的无行动价值大图，保留按需诊断数据 |
| `routing` | 路由模式、Exit 选择、本机 Exit 标记 | runtime 且 Exit 能力允许时 `已实现待验证` | 节点 → 流量路由 | 保留命令概念，重写为有序规则、测试和原子应用 |
| `exit_node` | Exit 角色启停 | Linux 且能力满足时 `已实现待验证`，其他平台 `未支持` | 网络 → 互联网出口 | 从全局身份页降为网络能力页 |
| `relay` | Relay 角色启停 | `未支持`，后端只能在创建/停止内嵌服务时整体切换 | 网络 → Relay | 不保留可点击伪开关，等待 W13 独立后端 |
| `logs` | 原始日志 | `垂直切片` | 节点 → 诊断 | 不进入一级“活动”，只在诊断中限时查看/导出 |
| `identity` | 节点公钥指纹与身份信息 | `垂直切片`，无轮换/恢复/Passkey | 节点 → 身份与密钥 | 重写所有者、Passkey 和密钥生命周期 |
| `settings` | 节点关闭动作 | `已实现待验证`，但生命周期错误 | 设置 + 节点运行状态 | 全局设置与节点启停拆开；“关闭 UI”不得触发关闭节点 |

结论：

- 11 个旧一级入口全部有明确归处，没有“暂不处置”项；
- `traffic`、`topology`、`logs`、`identity` 当前没有 manifest 动作，不能被视为功能页闭环；
- Relay 的 `backend_status=Unavailable` 与后端 `runtime_control.rs::set_network_role()` 一致，不能通过 UI 重试伪装为可用。

### 4.2 旧统一 WebUI 5 页映射

证据：

- `crates/client/src/webui/mod.rs::UNIFIED_MASTER_HTML`；
- 导航标签：`tab-overview/tab-peers/tab-routing/tab-settings/tab-logs`；
- 页面数据主要来自 `/api/status`、`/api/peers`、`/api/logs`、`/api/config` 和 `/ws`。

| 旧页面 | 当前状态来源 | 当前闭环 | 新归属 | 处置决定 |
|---|---|---|---|---|
| 全景拓扑大盘 `tab-overview` | `AppUiState.status`、Coordinator/运行时字段、Peer 列表 | `垂直切片` | 概览 + 网络 → 拓扑 | 删除双状态大盘，重写唯一主状态与全局拓扑 |
| 对等节点与终端 `tab-peers` | `/api/peers`、`/api/peers/probe` | 探测为 `已实现待验证`；旧页面状态来源不统一 | 网络 → 成员与拓扑 | 删除旧页面，迁移 Peer 探测命令 |
| 原生智能分流 `tab-routing` | `/api/routing`、controller 状态 | `垂直切片`；不是有序规则/测试/回滚闭环 | 节点 → 流量路由 | 废弃旧表单，按 W11 重写 |
| 专网中枢设置 `tab-settings` | `/api/config`、`/api/host`、`/api/stop_host`、黑名单 API | `垂直切片`；混合网络治理、Coordinator 与节点控制 | 网络治理 + 节点 + 设置 | 拆分对象，旧页面删除 |
| 系统运行日志 `tab-logs` | `/api/logs`、`/ws` | `垂直切片` | 节点 → 诊断 | 旧页面删除，原始日志不进入活动 |

### 4.3 Node manifest 11 页映射

证据：

- `crates/client/src/node/ui_manifest.rs::node_ui_manifest()`；
- manifest 强制 `REQUIRED_PAGES` 固定顺序并校验动作、权限、字段、确认和命令覆盖；
- `crates/client/src/webui/node_ui.html` 与 `crates/client/src/gpui/view_model.rs` 都消费该 manifest/命令契约。

| manifest 页面 | 动作 | 新归属 | 处置决定 |
|---|---|---|---|
| `overview` | `refresh_snapshot` | 概览 | 保留快照命令，页面重写 |
| `networks` | 创建、加入、启动、停止 | 网络 → 我的网络 | 保留命令，迁移到新对象模型 |
| `peers` | `probe_peer` | 网络 → 成员与拓扑 | 保留探测，迁入网络详情 |
| `topology` | 无 | 网络 → 成员与拓扑 | 重写为真实拓扑，不再一级导航 |
| `traffic` | 无 | 节点 → 运行状态/诊断 | 删除一级页，按需下钻 |
| `routing` | `update_routing` | 节点 → 流量路由 | 目标契约将替换为规则集合与原子应用 |
| `exit_node` | `set_exit_node_role` | 网络 → 互联网出口 | 保留能力约束，迁入网络详情 |
| `relay` | `set_relay_role`，但为 `Unavailable` | 网络 → Relay | 删除可点击伪动作，等待 W13 |
| `logs` | 无 | 节点 → 诊断 | 删除一级页 |
| `identity` | 无 | 节点 → 身份与密钥 | 删除一级页，迁入节点详情 |
| `settings` | `shutdown_node` | 设置 + 节点操作 | 拆分全局设置与显式停止节点 |

结论：

- manifest 是当前最接近共享契约的资产，状态为 `垂直切片`；
- 其 11 页信息架构不符合新五模块目标，不能继续扩展；
- manifest 校验原则、字段规则和动作后端状态声明应保留并演化到 W1/W4。

## 5. 双 WebUI 表面审计与处置

### 5.1 表面选择

证据：

- `create_app_web_router_with_auth_and_node()` 根据 `node_adapter.is_some()` 设置 `node_manifest_surface`；
- `handle_index()` 在 node adapter 存在时返回 `node_surface::NODE_MANIFEST_HTML`；
- 否则返回 `UNIFIED_MASTER_HTML`；
- 两个表面挂载在同一个 router 上，旧 controller API 在两种情况下都存在。

### 5.2 旧统一表面

页面：

```text
overview
peers
routing
settings
logs
```

状态来源：

- `AppController::AppUiState`；
- 运行时 IPC 状态合并；
- Coordinator 内嵌状态；
- Peer/日志/配置/黑名单缓存；
- WebSocket `/ws`。

主要 API：

```text
GET  /api/status
GET  /api/peers
POST /api/peers/probe
GET  /api/config
POST /api/config
GET  /api/blacklist
POST /api/blacklist/ban
POST /api/blacklist/unban
GET  /api/logs
GET  /ws
POST /api/connect
POST /api/disconnect
POST /api/host
POST /api/stop_host
POST /api/routing
POST /api/coordinator/ban
POST /api/coordinator/unban
```

问题：

- 页面按旧实现模块组织；
- 状态源由 controller、IPC 和 Coordinator 缓存拼接；
- 网络治理、网络成员、节点路由和全局设置混在一起；
- 没有 operation、幂等键、统一审计和统一事件游标；
- controller 生命周期与 node 生命周期耦合；
- 与 manifest/GPUI 的功能和状态语义分叉。

### 5.3 node manifest 表面

页面：与第 4.3 节相同的 11 页。

API：

```text
GET  /api/node/snapshot
GET  /api/node/manifest
GET  /api/node/events
POST /api/node/commands
```

优点：

- HTTP adapter 只投影 command bus、state store 和 event bus；
- 版本化 schema；
- WebSocket 首帧发送快照；
- 事件 sequence 去重；
- state revision 缺口或 broadcast lag 触发 `Resync`；
- revision 回退报 `ContractViolation`。

不足：

- 仍是 WebSocket + ephemeral broadcast，不是目标 SSE + cursor；
- 仍固定 11 页旧信息架构；
- 写操作没有完整 idempotency key、operation 状态机和审计；
- `NodeAccessModel` 当前为 `TrustedControlPlane`，不提供远程逐用户认证；
- WebUI 外层仍复用旧 controller API 与旧登录体系。

### 5.4 最终处置

决定：

- 新旧双 WebUI 都不作为最终产品表面；
- 第一版只保留一套新的 React + TypeScript + Vite 表面；
- GPUI 继续原生渲染，不嵌 WebView；
- 两者共享领域模型、设计令牌、状态合成、命令、错误、权限和文案；
- 旧表面只作为迁移证据，不再接收新功能；
- 删除顺序：
  1. 先冻结新的领域契约与 API；
  2. 在新表面完成对应垂直切片和契约测试；
  3. 切换单入口和认证；
  4. 删除 `UNIFIED_MASTER_HTML`、node 旧页面和旧 controller 路由；
  5. 清理兼容测试和旧 CLI 产品路径；
- 回退窗口只允许回到上一个已发布版本，不允许重新并行维护旧表面。

结论：双 WebUI 处置计划已冻结，当前代码仍为 `垂直切片`，删除工作归 W4/W6。

## 6. 命令、API 与动作闭环映射

### 6.1 NodeCommand 当前映射

证据：`crates/client/src/node/command.rs`、`crates/client/src/node/runtime_handler.rs`。

| 命令 | 后端现状 | 前置条件 | 权限 | 幂等/审计现状 | 结论 |
|---|---|---|---|---|---|
| `GetSnapshot` | 直接读取 `NodeStateStore` | 无 | `ViewState` | 只读；不产生写审计 | `已实现待验证` |
| `GetUiManifest` | 返回 canonical manifest | 无 | 无页面动作 | 只读；启动引导查询 | `已实现待验证` |
| `StartNetworks` | 有 runtime 时启动选择；无 runtime 拒绝 | runtime 可用，选择合法 | `ManageNetworks` | 未证明幂等；当前只有 `CommandCompleted{request_id,success}` | `已实现待验证` |
| `StopNetworks` | 有 runtime 走 runtime，否则走 registry | 指定网络或 `all=true` | `ManageNetworks` | 未证明幂等；无持久审计关联 | `已实现待验证` |
| `CreateNetwork` | 有 runtime 时创建并保存 | runtime 可用，名称/端口合法 | `ManageNetworks` | 重名冲突语义需后续冻结；无完整 operation 审计 | `已实现待验证` |
| `JoinNetwork` | 有 runtime 时加入并可自动启动 | runtime 可用，票据/目标合法 | `ManageNetworks` | 重复加入语义需后续冻结；无完整审批/申请 | `垂直切片` |
| `UpdateRouting` | 有 runtime 时热更新 | runtime 可用；启用本机 Exit 时平台能力须支持 | `ChangeRouting` | 当前整命令更新，未证明原子回滚和失败关闭 | `已实现待验证` |
| `SetNetworkRole(ExitNode)` | 有 runtime 时热更新 | 网络运行；Linux/主机能力满足 | `ManageRoles` | 当前无跨重启保留的完整审计 | `已实现待验证` |
| `SetNetworkRole(Discovery/Relay)` | 明确 `Unavailable` | 只能随内嵌 Discovery+Relay 服务整体创建/停止 | `ManageRoles` | 不修改状态 | `未支持` |
| `SetNetworkRole(Member)` | 明确 `Conflict`，成员是基线能力 | 不适用 | `ManageRoles` | 不修改状态 | `未支持` |
| `ProbePeer` | registry 执行探测 | 网络在线，目标非空，timeout 合法 | `ProbePeers` | 只读探测；当前不是持久审计事件 | `已实现待验证` |
| `Shutdown` | 停止所有网络并回收服务/监督器 | 无 | `StopNode` | 破坏性确认；当前会被关闭 GPUI 自动触发 | `未支持`（生命周期目标错误） |

### 6.2 Manifest 动作映射

| 动作 ID | 命令 | 权限 | 可用性 | 破坏性确认 | 闭环状态 |
|---|---|---|---|---|---|
| `refresh_snapshot` | `GetSnapshot` | `ViewState` | Always | 否 | `已实现待验证` |
| `create_network` | `CreateNetwork` | `ManageNetworks` | NodeRunning | 否 | `已实现待验证` |
| `join_network` | `JoinNetwork` | `ManageNetworks` | NodeRunning | 否 | `垂直切片` |
| `start_networks` | `StartNetworks` | `ManageNetworks` | NodeRunning | 否 | `已实现待验证` |
| `stop_networks` | `StopNetworks` | `ManageNetworks` | NetworkOnline | 有非破坏确认 | `已实现待验证` |
| `probe_peer` | `ProbePeer` | `ProbePeers` | NetworkOnline | 否 | `已实现待验证` |
| `update_routing` | `UpdateRouting` | `ChangeRouting` | NetworkSelected | 普通确认 | `已实现待验证` |
| `set_exit_node_role` | `SetNetworkRole(ExitNode)` | `ManageRoles` | NetworkSelected | 普通确认 | `已实现待验证` |
| `set_relay_role` | `SetNetworkRole(Relay)` | `ManageRoles` | NetworkSelected | 普通确认 | `未支持` |
| `shutdown_node` | `Shutdown` | `StopNode` | NodeRunning | 强制破坏性确认 | `未支持`（生命周期目标错误） |

共享 manifest 的优点：

- `NodeUiManifest::validate()` 校验页面与动作 ID 唯一、字段、权限一致性、后端状态一致性；
- `Shutdown` 必须带破坏性确认；
- 每个 `NodeUiCommandKind` 必须有动作；
- `SetNetworkRole` 允许角色级后端状态差异，以表达 Exit 已实现、Relay 不可用。

仍缺失：

- 动作幂等键；
- operation ID、阶段、取消、超时与恢复；
- actor、目标、结果和原因持久审计；
- 近期 Passkey 重新认证；
- 目标 SSE cursor 与 durable event；
- 新领域的启动/停止/重启、维护、审批、组、ACL、策略测试、File 等命令。

### 6.3 旧 WebUI API 到新领域的去向

| 旧 API | 当前语义 | 新去向 | 处置 |
|---|---|---|---|
| `GET /api/status` | controller/IPC/Coordinator 拼接快照 | 概览 + 节点 + 网络快照 | 重写为单一权威快照 |
| `GET /api/peers` | 旧 Peer UI DTO | 网络成员/拓扑 | 废弃旧 DTO，迁移领域模型 |
| `POST /api/peers/probe` | Peer 探测 | 网络成员诊断 | 保留能力，迁移命令 |
| `GET/POST /api/config` | 全局配置读写 | 设置 + 网络配置 | 拆分对象，增加验证/差异/回滚 |
| `GET /api/blacklist` | Coordinator 黑名单 | 网络安全策略 | 迁移为 ACL/封禁领域动作 |
| `POST /api/blacklist/ban` | 加入黑名单 | 网络安全策略 | 增加权限、理由、过期与审计 |
| `POST /api/blacklist/unban` | 移除黑名单 | 网络安全策略 | 增加权限与审计 |
| `GET /api/logs` | 原始日志 | 节点 → 诊断 | 保留受限导出，不进入活动 |
| `GET /ws` | 旧状态推送 | SSE + cursor 事件 | 替换 |
| `POST /api/connect` | 连接/启动旧网络 | 网络启动/加入 | 迁移为幂等 operation |
| `POST /api/disconnect` | 断开旧网络 | 网络停止/离开 | 迁移为幂等 operation |
| `POST /api/host` | 托管 Coordinator/Relay | 网络创建 + 能力启用 | 拆分网络与能力，禁止双 UI 伪动作 |
| `POST /api/stop_host` | 停止托管 | 能力停用/网络停止 | 拆分权威操作与清理结果 |
| `POST /api/routing` | 单体路由模式更新 | 节点 TrafficPolicy | 由 W11 替换 |
| `/api/coordinator/ban|unban` | 兼容旧入口 | 统一安全策略 | 废弃兼容入口 |

### 6.4 CLI 与 UI 的能力差

CLI 当前暴露 `Host/Join/Connect/Up/Down/List/Leave/Default/Start/Status/Peers/Ping/Stats/Route/Logs` 等路径；UI 只覆盖其中一部分，且部分 CLI 是旧产品路径。

需要后续判定：

- `Leave`、`Default`、`Ping`、`Route`、`Stats` 等是否成为新 UI 动作或仅保留诊断命令；
- 旧 `Host`、`Connect`、`Disconnect` 语义如何迁移到新网络/能力模型；
- 是否保留兼容 CLI。产品文档已决定第一版不要求旧配置、旧邀请票据和旧 CLI 兼容层；
- 在 W1 领域契约完成前，不把 CLI 能执行等同于 UI 已闭环。

### 6.5 目标中存在但当前缺失的能力

以下全部标记为 `目标`，不是现有产品能力：

- node 启动/停止/重启/维护，以及关闭 UI 不断网；
- 单一 owner、多 Passkey、恢复码和 HTTPS 公网初始化；
- 网络发现索引、申请理由、开放加入和管理员审批；
- 稳定签名网络 ID、签名发现记录、防冒名/回滚；
- 自定义组、动态组、拒绝优先 ACL；
- 有序 TrafficPolicy、规则测试、差异应用、失败关闭和回滚；
- Coordinator 候选组、多数接管、任期、fencing 和紧急权限冻结；
- Relay 独立启停、授权、配额、健康和回退；
- Exit 授权、DNS 泄漏保护、Google/YouTube 和性能报告；
- File ACL、配额、上传下载和续传；
- SSE durable cursor、operation、审计和统一错误模型；
- Android Compose/VpnService；
- GPUI/WebUI 新五模块外壳和像素级视觉回归。

## 7. 状态语义审计

### 7.1 当前状态源

| 状态域 | 权威来源 | 当前字段/枚举 | 结论 |
|---|---|---|---|
| Node 生命周期 | `NodeLifecycle` | `Stopped/Starting/Running/Stopping/Failed` | `已实现待验证` |
| Network 状态 | `NodeNetworkSnapshot` | `Offline/Connecting/Online/Degraded/Stopping/Failed` | `已实现待验证` |
| Embedded service | `NodeEmbeddedServiceSnapshot` | `Starting/Running/Stopping/Stopped/Failed` | `已实现待验证` |
| 网络角色 | `NodeNetworkRole` | `Member/Discovery/Relay/ExitNode` | 配置可表达，Relay 热切换 `未支持` |
| 路由模式 | `NodeRoutingMode` | `MeshOnly/BypassMainland/GlobalProxy` | `垂直切片`，目标将扩展为有序策略 |
| Peer | `NodePeerSnapshot` | online/p2p_active/p2p_healthy/rtt/loss/traffic | `已实现待验证` |
| Exit 能力 | `ExitNodeCapabilities` | platform/supported/forwarding/nat/cleanup/reasons | `已实现待验证` |
| Control 认证 | WebUI auth + `NodeAccessModel` | JWT 用户名密码；`TrustedControlPlane` | 与目标所有者安全模型不一致，`目标` |
| UI 模式 | `UiModeResolution` | requested/effective/display/gpui_available | `垂直切片` |

### 7.2 当前问题

- 旧统一 WebUI 可同时展示 `client_status`、Coordinator 状态和运行时可用性，用户可能看到并列状态；
- `NodeSnapshot` 本身没有 control 健康字段，无法直接表达“control 异常而 node 正常”；
- GPUI 与 WebUI 对状态的组织与文案并不完全一致；
- 网络能力开与主机能力成功不是同一件事；
- Relay 配置/内嵌服务存在不等于可独立热切换；
- Exit 的 `supported=true` 也不等于选定 Exit 节点在线、授权有效或 Google 可达；
- 事件流断开后旧快照必须视为陈旧，不能继续把旧在线状态当实时事实。

### 7.3 目标唯一主状态

UI 只显示一个主状态，优先级固定：

1. 用户意图或过渡状态：启动中、停止中、更新中、维护中；
2. 节点运行故障；
3. 运行降级；
4. 控制服务异常；
5. 运行正常。

约束：

- 存在需要处理的问题时，不显示“运行正常”；
- 需要处理区最多显示三条，按影响排序；
- network、peer、capability、Coordinator 状态是下钻事实，不是第二个全局主状态；
- “控制服务异常”只表示控制入口/会话/API 不可用，不自动推断 node 故障；
- “节点运行故障”表示数据面 node 明确故障，不因浏览器断网而误报；
- control 与 node 状态分别建模，但由一个合成器生成唯一主状态；
- 所有状态必须给出可行动说明，而不是只有颜色。

### 7.4 陈旧数据与事件缺口

当前 node adapter：

- subscribe 后再取 snapshot；
- 首帧发送 snapshot；
- 用 event sequence 去重；
- `StateChanged.revision` 缺口触发 `Resync`；
- broadcast `Lagged` 触发 `Resync`；
- revision 回退报 `ContractViolation` 并关闭连接。

目标：

- 改为 SSE + durable cursor；
- UI 重连先取快照，再从 cursor 继续；
- 任何缺口都整体 resync，禁止局部猜测修复；
- 超过新鲜度阈值的连接、Peer、Exit 健康数据必须标为陈旧；
- 旧数据不得继续显示为绿色在线或“运行正常”。

### 7.5 翻墙、抗干扰与网速

当前结论：

- 翻墙完整闭环：`目标`；
- Google/YouTube 稳定访问：`未知`，不得宣称可用；
- 抗干扰等级：`未知`；
- 吞吐、延迟、丢包具体指标：`未知`；
- DNS 泄漏、IPv4/IPv6 分流和失败关闭：`目标`；
- Exit 实网测试必须使用隔离客户端、独立管理通道和带外清理；
- 没有绑定 commit、环境快照、时间、失败清理和结果证据的报告，不得作为发布依据。

## 8. 保留、重写、删除清单

### 8.1 保留并演进

- 单一 `fuckipv6` 二进制事实；
- 节点身份、TUN、P2P、QUIC、Relay、Exit、Coordinator、secure-session 基础和现有测试资产；
- `NodeSnapshot` 作为统一状态投影的起点；
- `NodeCommandBus` 作为适配器与后端之间的边界；
- `NodeUiManifest::validate()` 的 fail-closed 契约校验原则；
- `NodeEventBus` 的 sequence/revision/resync 设计；
- `NodeHttpAdapter` 只投影、不持有业务状态的分层原则；
- 端口、字段、破坏性确认等可复用验证规则；
- 成熟 UI 组件和行为原语。

### 8.2 重写

- 五模块信息架构；
- 唯一主状态合成与陈旧数据模型；
- control/node 进程与服务监督生命周期；
- 单一 owner、Passkey、HTTPS、初始化与恢复；
- OpenAPI/JSON Schema 生成、operation、SSE cursor、错误和审计；
- TrafficPolicy、SecurityPolicy、组成员与能力授权；
- Exit/Relay/Coordinator/File 的领域操作；
- GPUI 与 WebUI 的共享设计令牌和视觉回归；
- WebUI 前端源码与单二进制嵌入构建链。

### 8.3 删除

- `UNIFIED_MASTER_HTML` 旧产品表面；
- node manifest 旧 11 页一级导航；
- 旧 controller 业务状态拼接和兼容 API；
- 返回 `Unavailable` 的 Relay 可点击伪开关；
- 关闭 GPUI 自动关闭 node 的路径；
- 无行动价值的大型实时图表；
- 旧双 WebUI 并行的测试与回退维护；
- 过时 CLI 产品路径和旧配置/邀请票据兼容层（按 W1 决策执行）。

### 8.4 不可删除或不可误提交

- 三个受保护文件本身不能删除，也不能在本轮提交；
- `release/fuckipv6-coordinator-linux-x86_64` 是构建产物，不提交、不删除；
- `crates/client/src/p2p/inbound_queue.rs` 与已暂存 P2P 改动属于其他在途工作，不能擅自清理；
- secure-session/UDP 适配器按依赖顺序提交，不插入 W0 文档提交。

## 9. 文件级责任与变更边界

| 范围 | 代表性文件 | W0 决定 | 后续归属 |
|---|---|---|---|
| 产品目标/审计 | `docs/PRODUCT_UI_REDESIGN*.md` | 保留并作为基线 | W0 |
| GPUI 页面与模型 | `crates/client/src/gpui/*` | 不继续扩旧 11 页，等待 W5/W6 | W5/W6/W7 |
| 共享 node 契约 | `node/{command,state,event,ui_manifest,http_adapter}.rs` | 保留分层，演进为 W1/W4 | W1/W4 |
| 运行时控制 | `node/{runtime_handler,runtime_control}.rs` | 保留能力约束，不伪造成功 | W2/W4/W11 |
| 旧统一 WebUI | `webui/mod.rs` 内嵌 HTML/JS | 冻结功能，不再新增业务 | W4/W6 |
| node WebUI | `webui/node_ui.{html,js}` | 只作迁移证据 | W4/W6 |
| controller | `webui/controller.rs` | 只读，禁止本轮提交 | W2/W4 |
| CLI 入口 | `cli/{mod,commands}.rs` | 只读，禁止本轮提交 | W2/W6 |
| P2P/secure-session/UDP | `p2p/*`、benchmark | 独立按依赖顺序提交 | 独立数据面工作流 |
| 发布产物 | `release/*` | 不提交、不删除 | 发布工作流 |

## 10. 未知项与限制

- 未重跑 Rust 测试，因此没有新的 E3 测试通过证据；
- 未验证当前 GPUI 与 WebUI 的实际截图、交互和像素差异；
- 未确认所有 CLI 子命令在新运行时监督器下的行为；
- 未确认 Coordinator 自动接管、多数证明、fencing 和紧急权限冻结的实现状态；
- 未确认 Relay 独立启停后端何时具备；
- 未确认 Exit 在 Linux/macOS/Windows 的真实内核/NAT/清理差异；
- 未确认 Android 仓库、构建和 VpnService 状态；
- 未确认 WebUI HTTPS、Passkey、恢复码和公网首个访问者初始化；
- 未确认 EOF/event cursor 的持久化恢复语义；
- 未确认 File ACL、配额、传输和续传；
- 未确认抗干扰、Google/YouTube、DNS 泄漏和网速。

这些未知项不得用“页面存在”或“代码可编译”替代。

## 11. W0 冻结结论

已冻结：

- 单二进制是当前代码事实，但 control/node 生命周期尚未拆分；
- GPUI 和两套 WebUI 都是功能垂直切片，不是最终产品表面；
- 11 页旧信息架构全部有目标归处，不再作为一级导航；
- 新五模块为“概览/节点/网络/活动/设置”；
- Relay 热切换当前 `Unavailable`，Exit 受主机能力限制；
- 当前命令总线是可保留的基础，但 operation、幂等、审计、SSE cursor 尚未完成；
- 关闭 GPUI 会关闭节点，属于必须修复的生命周期缺陷；
- 翻墙、抗干扰和网速不能宣称已经达标；
- W0 后续进入 W1 前只允许共享契约和审计，不继续扩展旧 UI。

仍需所有者确认：

- 以本审计作为本轮重构的冻结基线；
- 确认先进入 W1 领域契约，而不是直接改页面；
- 确认旧双 WebUI 只保留迁移价值，最终由新的 React/Vite 单表面替代；
- 确认 W2 首要验收是“关闭 UI 不断网、control 异常时 node 继续运行、失败可自动清理”。

## 12. 文档提交验证

本轮只修改：

```text
docs/PRODUCT_UI_REDESIGN_AUDIT.zh-CN.md
docs/PRODUCT_UI_REDESIGN_TODO.zh-CN.md
```

验证要求：

- `git diff --check` 无空白错误；
- Markdown fenced code block 成对；
- 不包含私钥、Token、密码、初始化码或远程凭据；
- 暂存区不包含三个受保护文件；
- 不把已有 P2P 暂存内容混入文档提交；
- 不需要运行 Rust fmt/check/clippy/test，因为没有 Rust 代码变更。

## 13. 下一步

W0 完成后进入 W1：领域契约。

W1 的第一批产物应按依赖顺序为：

1. `Node`、`NodeOwner`、`Network`、`NetworkMembership` 的稳定 ID 与序列化契约；
2. 角色、组、能力注册表和拒绝优先权限模型；
3. `TrafficPolicy`、`SecurityPolicy`、`CoordinatorTerm`；
4. `Operation`、durable `Event`、`AuditEntry` 和 SSE cursor；
5. schema 校验、非法状态转移、权限矩阵、重放和回滚测试。

在 W1 契约冻结前，不开始 W6-W15 页面实现，也不继续给旧 UI 增加按钮。
