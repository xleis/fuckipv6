# fuckipv6 产品化 UI 与节点控制面重构 TODO

> 来源：`docs/PRODUCT_UI_REDESIGN.zh-CN.md`
> 建立日期：2026-10-31
> 规划基线：`b1bb837`
> 状态：规划清单；除明确标记为进行中的事项外，均尚未完成
> 原则：每个功能必须形成“领域模型 → 权限/状态机 → 后端命令 → 操作/事件/审计 → GPUI → WebUI → Android（适用） → 自动化/实测”的垂直闭环

## 0. 使用规则

### 0.1 状态标记

- `[ ]` 未开始
- `[~]` 进行中
- `[x]` 已完成，且有当前提交上的测试或实测证据
- `[!]` 阻塞

不得仅因页面能打开、命令能编译、单次请求返回成功或截图看起来正确而勾选完成。

### 0.2 任务完成定义

每项功能任务只有同时满足以下条件才能标记完成：

- [ ] 领域模型和状态机已定义；
- [ ] 权限检查在服务端执行；
- [ ] 后端命令具备幂等、错误和审计语义；
- [ ] UI 有加载、空、错误、无权限、不支持、降级和陈旧数据状态；
- [ ] GPUI/WebUI 行为一致（Android 范围内另加 Compose）；
- [ ] 自动化测试覆盖成功、失败和恢复；
- [ ] 用户可理解的活动记录；
- [ ] 文档与能力声明和实际证据一致。

### 0.3 禁止事项

- [x] `crates/client/src/cli/commands.rs` 可按任务修改，提交前逐文件审阅；
- [x] `crates/client/src/cli/mod.rs` 可按任务修改，提交前逐文件审阅；
- [x] `crates/client/src/webui/controller.rs` 可按任务修改，提交前逐文件审阅；
- [ ] 不用 WebView 替换 GPUI；
- [ ] 不保留旧双 WebUI 作为新产品表面；
- [ ] 不显示只返回 `Unavailable` 的可点击伪按钮；
- [ ] 不在日常主机直接做改默认路由/TUN 的实网测试；
- [ ] 不把密码、私钥、Passkey、初始化码、Token 或远程凭据写入仓库和日志；
- [ ] 不大范围 `reset`、force push 或覆盖并发工作区修改。

## 1. 总进度看板

| 工作包 | 状态 | 优先级 | 依赖 | 完成证据 |
|---|---|---|---|---|
| W0 现状冻结 | `[x]` | P0 | 无 | 2026-10-31 所有者确认冻结；映射表与代码清单见 `docs/PRODUCT_UI_REDESIGN_AUDIT.zh-CN.md` |
| W1 领域契约 | `[x]` | P0 | W0 | 15 个领域模型、状态机与测试；版本化 schema 与消费端已冻结 |
| W2 运行时拆分 | `[~]` | P0 | W1 | 本地 IPC、control/node 进程拆分、GPUI/WebUI 远程接线、node 故障恢复入口、脱敏诊断、stop/start 语义和资源清理重试已落地；系统服务管理器和完整资源看门狗未完成 |
| W3 所有者安全 | `[x]` | P0 | W2 | W3-01 本机一次性初始化、W3-02 公网首次认领、W3-03 HTTPS/安全响应头、W3-04 Passkey 全生命周期、W3-05 本地所有者恢复、W3-06 会话/Web 安全、W3-07 认证管理 UI 已完成；自动化边界与未做的真实公网/硬件认证器实测见各分项证据 |
| W4 控制 API | `[x]` | P0 | W1,W2 | W4-01 至 W4-07 已完成；版本化 API、operation/SSE、稳定错误、服务端授权、生成客户端、限额与自动化攻击面门禁均有测试证据。未进行真实公网环境实机压测 |
| W5 设计系统 | `[~]` | P0 | W1,W4 | 令牌、组件、i18n、a11y、视觉基线 |
| W6 外壳与概览 | `[~]` | P1 | W4,W5 | 五导航与单一主状态已落地；概览、设置和状态矩阵待完成 |
| W7 节点页 | `[~]` | P1 | W2,W4,W5 | W7-01 启停、W7-03 权威路由摘要、W7-04 身份和密钥、W7-05 诊断恢复与 W7-06 状态权限矩阵已完成；W7-02 接口/TUN 的实机平台验收进行中 |
| W8 网络创建与发现 | `[~]` | P1 | W3,W4,W5 | 创建、发现、开放/审批/私有邀请和强制 Exit 知情已完成；索引故障隔离与发现全链路测试待完成 |
| W9 成员、组与拓扑 | `[~]` | P1 | W8 | 成员列表、邀请审批、组目录和拓扑聚合已完成；多网络视觉、交互、性能与 E2E 进行中 |
| W10 安全与隐私 | `[x]` | P1 | W9 | 权限模型与矩阵、ACL/拒绝优先/本地否决、字段级隐私、策略编辑器与测试器、差异预览/原子应用/回滚、权威活动时间线、可验证长期审计、安全页面状态和可重复治理证据闭环均已完成 |
| W11 流量路由 | `[~]` | P1 | W7,W10 | W11-01 四类决策与首条命中、W11-02 匹配条件、W11-03 应用身份与子进程继承、W11-04 未知应用与失败策略、W11-05 规则测试器领域契约、W11-06 原子应用/差异/回滚/崩溃恢复、W11-07 有界自动恢复协调器契约和 W11-09 隔离路由 E2E/失败关闭/网络报告已完成；W11-08 WebUI 已接入同一权威页面的结构化编辑、测试、应用、历史和回滚入口，GPUI 已接入同一页面的权威投影、规则/目标/影响展示和精确跳转，真实命中观测、GPUI/WebUI/Android 三端一致、生产后端接线及真实平台监听/适配待完成 |
| W12 Exit | `[~]` | P1 | W10,W11 | W12-01 至 W12-05 已完成：本机启停、贡献治理、管理员选择、分层健康与故障切换/撤权均有权威状态和自动化证据；W12-06 已完成真实 Chrome DNS、IPv4/IPv6 受控/外部探测与失败关闭复核，IPv6 外部目标仍为 `not_tested` 且保持 `fail_closed_required=true`；W12-07 已验证 Google/YouTube 页面、实际视频播放、DNS/TLS/H2/H3 和 TUN 源路径，但 yt-dlp 媒体下载仍 403；W12-08 已完成香港 Exit 对直连基线的 DNS/RTT/TCP/TLS/H3、单流吞吐和并发实测，并定位瓶颈到香港 co-located Relay/Exit 数据路径；独立 netns 已通过正式邀请入网后的双向 P2P ICMP 与直接 UDP 路径，真实香港节点随后完成 IPv4/IPv6 双向 ICMP、TCP 262144 字节、UDP 65536 字节/64 包和精确路由验收；两侧 Exit 可达、Peer 级统计映射和 W12-09 的 24 小时稳定性/自动清理仍待完成 |
| W13 Relay | `[ ]` | P1 | W10 | 授权、配额、健康、回退 |
| W14 Coordinator 接管 | `[ ]` | P1 | W10,W12,W13 | 任期、多数接管、紧急权限冻结 |
| W15 File | `[~]` | P2 | W10 | 本地 domain/ACL/路径/临时文件/续传适配器、GPUI owner 设置与 IPC、并发/速率/磁盘/扫描/审计上界已落地；WebUI/Android File 表面、真实网络传输、跨平台竞态与性能/OS 资源基线仍未完成 |
| W16 Android | `[ ]` | P2 | W2,W4,W5,W7-W15 | Compose、VpnService、全能力 |
| W17 跨平台硬化 | `[ ]` | P2 | W6-W16 | Linux/macOS/Windows 实机证据 |
| W18 发布门禁 | `[ ]` | P0 | 全部 | 自动化、实测、安全和发布证据 |

## 2. 关键路径

```text
W0 现状冻结
  → W1 领域契约
    → W2 运行时拆分
      → W3 所有者安全
      → W4 控制 API
        → W5 设计系统
          → W6 外壳与概览
          → W7 节点页
            → W10 安全与隐私（同时依赖 W8/W9）
              → W11 流量路由
                → W12 Exit
                → W13 Relay
                  → W14 Coordinator 自动接管
              → W15 File
          → W8 网络创建与发现
            → W9 成员、组与拓扑
              → W10
W2,W4,W5,W7-W15
  → W16 Android
W6-W16
  → W17 跨平台硬化
  → W18 发布门禁
```

硬依赖：

- W1 未冻结前，不继续扩展旧 UI 页面；
- W2 未完成前，不把任何新 UI 当作长期运行控制台；
- W3/W4 未完成前，不开放公网写操作；
- W10 未完成前，不开放 Exit、Relay、File 的成员级授权；
- W11 未完成前，不宣称翻墙、分流或失败关闭可用；
- W12 未提供 Google/DNS/性能报告前，不宣称抗干扰或网速合格；
- 所有工作包未完成前，不发布第一版。

## 3. W0：现状冻结

目标：先明确保留、重写、删除的边界，避免在旧信息架构和双 WebUI 上继续堆功能。

### W0-01 当前代码审计归档 `[x]`

- [x] 将 GPUI 11 页 manifest、WebUI 两套表面、CLI UI 选择和生命周期事实写入审计文档；
- [x] 为每条审计结论补代码路径和关键符号；
- [x] 标注“已验证/已实现待验证/垂直切片/目标/未支持/未知”；
- [x] 所有者于 2026-10-31 确认，以当前审计冻结为本轮重构基线。

**验收**：README、产品文档和代码事实一致；不存在把垂直切片描述为完成品的条目。

### W0-02 旧页面到新信息架构映射 `[x]`

- [x] 映射旧 `overview/networks/peers/topology/traffic/routing/exit_node/relay/logs/identity/settings`；
- [x] 映射到新“概览/节点/网络/活动/设置”；
- [x] 标记每个旧页面的保留组件、迁移内容、删除原因；
- [x] 标记旧页面中不存在真实后端闭环的动作；
- [x] 为多网络能力设计统一对象详情，不复制页面。

**验收**：每个旧页面都有归处；没有“暂不处置”的旧一级入口。

### W0-03 双 WebUI 表面处置计划 `[x]`

- [x] 列出 `UNIFIED_MASTER_HTML` 的页面、API 和状态来源；
- [x] 列出 node manifest 表面的页面、命令和状态来源；
- [x] 比较两套 UI 的重复、冲突和缺失能力；
- [x] 决定最终只保留一套新的 React/Vite 产品表面；
- [x] 明确旧表面删除顺序、回退窗口和测试替换；
- [x] 禁止新功能继续进入旧内嵌 HTML/JS。

**验收**：旧双表面不会继续并行演进；所有能力只在新契约下实现。

### W0-04 旧命令与新命令映射 `[x]`

- [x] 建立现有 `NodeCommand`、WebUI controller API、manifest action 和未来领域命令的映射；
- [x] 标记已闭环、部分闭环和仅返回 `Unavailable` 的动作；
- [x] 为每个动作记录权限、前置状态、幂等性和审计要求；
- [x] 找出只在 CLI 存在但 UI 无权访问的能力；
- [x] 找出只在旧 WebUI 存在但 NodeCommandBus 没有的能力。

**验收**：没有可见动作缺少后端处理；没有后端能力只能靠旧 controller 操作。

### W0-05 状态语义映射 `[x]`

- [x] 映射当前 control、node、network、peer、Exit、Relay 状态；
- [x] 定义唯一主状态优先级；
- [x] 区分 control 异常与 node 故障；
- [x] 定义陈旧快照和事件缺口；
- [x] 删除或降级无用户价值的实时图表；
- [x] 为每个状态建立可行动说明。

**验收**：UI 不再并列展示互相矛盾的双状态；所有状态有权威来源。

### W0-06 代码保留/重写/删除清单 `[x]`

- [x] 保留：身份、TUN、P2P、QUIC、Relay、Exit、Coordinator、secure-session 基础和测试；
- [x] 保留：NodeSnapshot、NodeCommandBus、manifest 校验原则；
- [x] 重写：UI 信息架构、生命周期归属、认证、API、策略和活动；
- [x] 删除：旧双 WebUI 业务逻辑、伪动作、过时 CLI 产品路径；
- [x] 标记不可提交或需其他工作流合入的文件；
- [x] 建立文件级责任与变更边界。

**验收**：重构不是盲目推倒协议栈，也不是继续修补旧产品层。

### W0-07 工作区保护 `[x]`

- [x] 记录当前脏工作区和未跟踪文件；
- [x] 提交前始终检查 `git status --short --branch`；
- [x] 三个指定文件不再受保护，但只提交与当前任务直接相关的 diff；
- [x] 不删除 release 产物或他人在途改动；
- [x] 为 secure-session/UDP 适配器保持依赖提交顺序。

**验收**：文档、代码和测试提交不会误带无关修改或未审阅的并发内容。

## 4. W1：领域契约

目标：冻结新 UI 和所有后端共享的唯一事实来源。

### W1-01 Node 与 NodeOwner `[x]`

- [x] 定义全局稳定 Node ID、公钥、平台、版本和健康状态；
- [x] 定义单一 NodeOwner 和多个 Passkey 凭据引用；
- [x] 区分节点所有者与网络成员角色；
- [x] 定义密钥、恢复材料和会话引用，不保存明文秘密；
- [x] 定义节点本地隐私字段；
- [x] 添加 schema 校验和序列化测试。

### W1-02 Network `[x]`

- [x] 定义与 Coordinator 地址解耦的稳定签名网络 ID；
- [x] 定义名称、用途、可见性、加入模式、公开资料；
- [x] 定义 owner、管理员和成员引用；
- [x] 定义网络版本和策略 revision；
- [x] 定义发现记录的签名、过期和单调版本；
- [x] 添加冒名、回滚和过期测试。

### W1-03 NetworkMembership `[x]`

- [x] 定义 `discovered/applying/pending_approval/accepted/key_ready/connecting/active/degraded/leaving/left/rejected/suspended` 状态机；
- [x] 定义加入时接受的强制 Exit 贡献和隐私条款；
- [x] 定义每个网络的角色、能力和本地策略引用；
- [x] 禁止 accepted 前访问网络资源；
- [x] 添加非法状态转移和重启恢复测试。

### W1-04 角色、组与权限 `[x]`

- [x] 定义网络 owner、管理员、普通成员和临时 Coordinator；
- [x] 定义自定义组和动态组；
- [x] 定义主体、资源、动作和条件；
- [x] 实现拒绝优先的交集合成；
- [x] 保证节点本地拒绝不能被网络管理员覆盖；
- [x] 保证网络拒绝不能被本地允许覆盖；
- [x] 添加角色×操作×对象权限矩阵测试。

### W1-05 能力注册表 `[x]`

- [x] 定义 Coordinator、Relay、Exit、File 四种能力；
- [x] 能力注册表可扩展，新增能力不要求重画所有页面；
- [x] 定义能力的启停、授权、资源限制、健康状态和失败关闭；
- [x] 定义普通成员中性状态；
- [x] 定义能力环固定图标和颜色语义；
- [x] 添加未知能力默认拒绝/隐藏测试。

### W1-06 节点 TrafficPolicy `[x]`

- [x] 定义直连、进入网络、指定 Exit、拒绝；
- [x] 定义有序规则、首条命中；
- [x] 定义规则内条件 AND、多值条件 OR；
- [x] 定义域名、IP/CIDR、端口、协议、应用/进程匹配器；
- [x] 定义未知应用不可猜测；
- [x] 定义失败关闭和规则回滚；
- [x] 添加匹配优先级和组合测试。

### W1-07 网络 SecurityPolicy `[x]`

- [x] 定义成员/组、资源、动作、允许/拒绝；
- [x] 定义 File、Exit、Relay、拓扑和审计的字段级授权；
- [x] 定义隐私默认值和管理员可见范围；
- [x] 定义测试器、差异应用和原子回滚；
- [x] 定义策略 revision 与过期时间；
- [x] 添加拒绝优先、越权和撤权测试。

### W1-08 CoordinatorTerm `[x]`

- [x] 定义网络 ID、任期号、Leader、候选组多数证明、权限类别；
- [x] 定义 full/emergency 权限；
- [x] 定义 fencing token、签发和过期时间；
- [x] 定义旧 Leader 恢复拒绝旧任期；
- [x] 定义临时 Leader 禁写范围；
- [x] 添加脑裂和任期回退测试。

### W1-09 Operation/Event/Audit `[x]`

- [x] 定义 operation 状态机；
- [x] 定义幂等键和重试语义；
- [x] 定义结构化事件、快照 revision 和 cursor；
- [x] 定义稳定错误码、参数和恢复建议；
- [x] 定义审计与用户活动的投影关系；
- [x] 定义敏感字段脱敏；
- [x] 添加事件缺口、重连和重复事件测试。

### W1-10 FileShare `[x]`

- [x] 定义共享根、配额、只读/读写、ACL 和审计；
- [x] 定义上传、下载、分块、校验和断点状态；
- [x] 定义路径、符号链接、竞态和删除语义；
- [x] 明确第一版不做同步、实时镜像和系统挂载；
- [x] 添加路径安全和续传状态测试。

**实现与测试证据（2026-10-31）**

- 领域模型位于 `crates/common/src/domain/`：`node.rs`、`network.rs`、`membership.rs`、
  `permissions.rs`、`capability.rs`、`traffic_policy.rs`、`security_policy.rs`、
  `coordinator.rs`、`operation.rs`、`event.rs`、`audit.rs`、`file_share.rs` 和 `ids.rs`；
- 持久 Operation/Event/Audit 契约提交：`bc21b7c`；FileShare 传输契约提交：`9bd3ae2`；
- `cargo test -p fuckipv6-common domain::`：65 passed，覆盖非法状态转移、拒绝优先、
  撤权、任期回退/脑裂、事件缺口与重放、敏感字段脱敏、路径穿越/符号链接/续传绑定；
- W1 契约已通过生成与漂移门禁，并由 `3e35749` 冻结消费端 DTO。

### W1-11 Schema 与生成类型 `[x]`

- [x] 从 Rust 模型生成 JSON Schema draft 2020-12；
- [x] 生成 TypeScript 类型和客户端；
- [x] 生成并校验 Android 命令枚举、请求信封和完整 DTO/联合类型绑定；
- [x] 将生成漂移检查加入 CI；
- [x] 冻结 schema version 和兼容策略；
- [x] 禁止手写第二套命令枚举。

证据：`contracts/schema/`、`contracts/typescript/`、`contracts/android/`、
`crates/client/src/node/contract.rs`、`scripts/check-contracts.sh`，以及 CI 的
`contracts` 作业。Android 生成器当前产出 42 个顶层 DTO/联合类型；Java 8 测试验证
DTO 清单、`NodeSnapshot` 关键字段、命令/结果/事件/网络选择/WebSocket 构造，以及
未知命令和枚举失败关闭。Rust、TypeScript、Java 均包含未知值或版本不匹配测试。

### W1-12 契约门禁 `[x]`

- [x] 每个可见 UI 动作在后端有处理；manifest 动作集合与 canonical `ACTIONS`
  完全一致，生产 handler 对 `NodeCommand` 做无通配符穷尽分派；
- [x] 每个后端动作有权限、状态和审计声明；
- [x] 每个能力在模型中有未知/不支持语义，并投影到 Coordinator/Relay/Exit/File
  四能力统一注册表；
- [x] 每个状态有唯一主状态映射；
- [x] `cargo fmt --check`；
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings`；
- [x] `cargo test --workspace`。

证据：`NodeUiAudit` 与 `audit_for_command()` 统一声明动作审计；`NodeSnapshot` 的
`capabilities` 由 `NodeStateStore::update()` 从权威运行时字段重算；`primary_status()`
是 GPUI/WebUI 共用的唯一用户主状态映射。Rust、生成 Schema/TypeScript/Java 绑定和
WebUI 内嵌 manifest 一致性测试均通过。

## 5. W2：运行时拆分与生命周期

目标：让 UI 成为控制台，而不是 node 的所有者。

证据（2026-10-31，`abda0d5`）：真实二进制进程测试覆盖 UI 断开后 node
继续运行、control `SIGKILL` 后 replacement control 接管同一 node、node `SIGKILL`
后自动恢复，以及 node 被 `SIGSTOP` 时 control 有界强杀并清理陈旧 socket。

```text
GPUI / WebUI (目标形态；WebUI 接线未完成)
             │ owner-only authenticated local IPC
             ▼
control ─────┼── spawn / adopt / health-check ──► node
   │         │                                      │
   │         └── never owns TUN/routing ────────────┤
   └── instance lock + bounded shutdown watchdog    └── TUN/P2P/relay/exit/File
```

### W2-01 control/node 进程模型 `[x]`

- [x] 同一二进制支持 control 与 node 内部角色；
- [x] 明确启动参数、环境、IPC 地址和退出码；
- [x] control 不要求 TUN/路由特权；
- [x] node 不直接暴露公网 WebUI；
- [x] 禁止 UI 每次启动创建第二份 runtime：GPUI 与 WebUI 均通过 `RemoteNodeSession` 连接同一 control/node 控制面；
- [x] 输出进程模型图。

### W2-02 IPC 契约与认证 `[x]`

- [x] Linux Unix Socket、Windows Named Pipe、macOS 等价通道：基于 owner-only 运行目录和 `interprocess` 本地套接字实现；Linux 已实机验证，Windows GNU target 与 macOS 核心路径（`--no-default-features`）的交叉 check/clippy 已通过；macOS 默认 GPUI 构建被第三方 `gpui_media` 的 host-cfg 构建脚本阻断，三平台实机可用性统一由 W17 验收；
- [x] 本机同用户认证和权限检查：Unix 校验 peer euid、目录 uid、0700 目录和 0600 socket，并有真实第二 UID 内核凭据测试；Windows named pipe 使用仅授予 `OWNER_RIGHTS` 的 protected DACL，并在每条连接上校验 peer token SID；
- [x] 与 HTTP API 共用命令/事件/错误模型：IPC 请求复用 `NodeCommandRequest`，快照与事件复用 `NodeSnapshot`/`NodeEvent`，错误使用稳定 `IpcErrorCode`；
- [x] 禁止远程无认证 IPC：不监听 TCP，Unix 使用本地 socket，Windows 使用 named pipe，并拒绝非当前用户 peer；
- [x] IPC 断线重连和快照同步：已实现断线后重新握手、以权威快照和原子事件边界重新订阅；事件序号断档或广播滞后返回可重试的 `event_gap`，由客户端重连取快照；
- [x] 长期运行服务循环：已实现认证连接接受、并发握手、snapshot/execute/subscribe 分发、请求 ID 校验、命令超时、事件推送、关闭宽限和 socket 清理；
- [x] 添加冒用、权限和重连测试：覆盖同用户/异用户授权、真实第二 UID、目录/socket 权限、版本错误、断线快照恢复、请求 ID、订阅顺序、事件缺口，以及 control/node 生产进程接线 E2E；Windows token 与 macOS 实机证据由 W17 负责；
- [x] 将服务循环接入实际 control/node 进程并完成启动、崩溃、停止和服务监督联调。

### W2-03 UI 与 node 生命周期解耦 `[x]`

- [x] GPUI 窗口关闭不调用 node shutdown；
- [x] WebUI 标签关闭不影响 control/node；
- [x] node 运行时间不随 UI 启动重置；
- [x] UI 重开后从权威快照恢复；
- [x] 删除 UI 进程持有 supervisor 的设计：GPUI 与 WebUI 均通过 `RemoteNodeSession` 连接 control/node；
- [x] 添加 UI 退出不断网 E2E：`closing_headless_webui_does_not_stop_the_node` 在 `ab18b8e` 验证关闭 WebUI 进程后 node PID 不变且 IPC 仍可查询。

### W2-04 start/stop/restart 语义 `[x]`

- [x] stop node 保留 control/WebUI；
- [x] start node 从持久化配置恢复成员关系；
- [x] restart 有硬超时和失败诊断；
- [x] 操作期间禁止重复提交；
- [x] 清理 TUN、路由、DNS、监听和子进程；
- [x] 添加停止后残留资源检查。
- [x] stop/stop-node 仅在清理报告完全干净时返回成功；任何残留都作为可重试错误展示，并将 node 生命周期保持为失败。

**验收**：运行时返回逐项清理结果；只有清理成功才注销监听和 peer 注册。停止超时、清理失败或子任务未退出时保留残留事实并阻止同一网络重启。回归覆盖停止超时后延迟完成、确定性清理失败、重启前清理门禁、12 个网络并行停止、固定端口释放，以及 fake runner 下的 TUN/路由/DNS/监听资源恢复。

### W2-05 control 崩溃恢复 `[x]`

- [x] control 崩溃时 node 按最后有效策略继续；
- [x] UI 启动路径可补拉 control；systemd/launchd/Windows Service 归入 W2-08；
- [x] control 重新读取 node 真实状态；
- [x] 事件缺口触发快照重拉；
- [x] 不因 control 恢复重启数据面；
- [x] 添加 SIGKILL/硬重启测试。

**验收**：`new_headless_ui_restarts_crashed_control_and_adopts_live_node` 覆盖旧 control 被 SIGKILL、旧 UI 关闭、新 UI 启动后自动补拉 control；新 control 接管原 node，node PID 不变，新 WebUI 可查询权威快照。UI 对 detached control 设置独立收尸线程，避免 control 异常退出后在 UI 生命周期内遗留僵尸进程。

### W2-06 node 故障处理 `[x]`

- [x] node 异常退出时依赖动作失败关闭；
- [x] control 保持可用；
- [x] UI 显示节点故障而非控制服务异常；
- [x] 提供重启、诊断和残留清理入口；
- [x] 不静默直连；
- [x] 添加 node crash 故障注入。

**验收**：共享 manifest 暴露 `restart_node`、`get_node_diagnostics` 与
`cleanup_node_residuals`；GPUI 和 WebUI 均通过同一类型化命令层执行。运行中会话不能被
cleanup 绕过，超时或 owner 丢失会保持残留事实并允许重试；部分成功、owner 丢失和
超时后成功重试均有运行时测试覆盖。

### W2-07 启动 UI 策略 `[x]`

- [x] 图形会话默认 GPUI；
- [x] 无头按配置启用 WebUI；
- [x] 不按公网 IP 推断；
- [x] 所有入口连接同一 control；
- [x] 命令子入口和顶层入口一致；
- [x] 添加 X11/Wayland/无 DISPLAY 场景测试。
- [x] WebUI 监听器必须持有运行目录独占表面租约，租约存活到监听任务结束。

**验收**：X11、Wayland、Windows/macOS 桌面在 GPUI 可用时只选择 GPUI；桌面显式 `--webui`/`--headless` 被拒绝，GPUI 不可用时失败关闭且不回退 WebUI。无 DISPLAY 自动选择 Headless WebUI，显式 GPUI 因无显示环境被拒绝；`FUCKIPV6_PUBLIC_IP` 不参与显示环境判断，`FUCKIPV6_HEADLESS`/`FUCKIPV6_GUI` 不能掩盖实际桌面检测。顶层入口与 `host/join/connect/up/start` 子入口使用同一模式解析，GPUI 只在 OS 主线程启动。`headless_command_entrypoint_reexports_the_same_control` 验证 `up` 子入口关闭 UI 后仍连接同一 control/node。

**证据（2026-11-02）**：`start_app_webui_with_node` 只允许 crate 内生产调用，并强制接收不可克隆的 `SurfaceLease`；租约移入 WebUI 监听任务，直到服务停止才释放，因此普通 WebUI、系统服务和 GPUI 无法同时暴露界面。桌面自动模式回归断言解析为 GPUI 且 `webui_port == 0`；WebUI 最终监听边界和 `__service` 启动边界都会重新检测桌面环境并失败关闭。`cargo test -p fuckipv6-client --all-features` 通过（库测试 671 passed、3 ignored；`register_check` 1 passed；`w2_runtime` 11 passed，包含 `desktop_hosts_refuse_webui_before_starting_the_node`；特权 lifecycle 1 ignored）。

### W2-08 服务监督 `[~]`

- [x] Linux systemd 用户/系统服务方案；
- [x] macOS launchd 方案；
- [x] Windows Service 方案；
- [ ] Android 前台服务方案（W16）；
- [x] 安装、升级、重启和卸载行为；
- [x] 不依赖打开图形界面来维持网络。

**已有证据**：统一 `fuckipv6 service` 已生成 systemd unit、launchd plist 和
Windows SCM 定义，安装脚本只调用该入口；`__service` 负责监督 control/node/WebUI，
安装、升级、重启、停止和卸载均有自动化生命周期测试。当前主机已通过
`systemd-analyze verify`，launchd plist 可被 `plistlib` 解析，Windows GNU target
可交叉编译。macOS/Windows 尚无实机服务验证，Android 前台服务尚未实现，因此整体保持
`[~]`，相关实机验收仍由 W16/W17 跟踪。

### W2-09 关闭与清理看门狗 `[x]`

- [x] 每个启动/停止步骤有硬超时：node ready、IPC 绑定/启动、状态刷新、UI 会话/WebUI 启动、优雅停止、强制终止、任务取消和 IPC 服务均有硬超时；
- [x] 失败后杀进程并可观察；
- [x] 恢复 DNS/策略路由/路由表/TUN；
- [x] 用户不需要手工 kill；
- [x] 生成脱敏诊断；
- [x] 清理失败是发布阻断。

**已有证据**：服务监督路径为 IPC 绑定、节点状态刷新、UI IPC 会话、WebUI 启动、
优雅停止、强制终止和任务取消设置独立硬期限；超时不会被静默忽略，control 清理失败会
向上返回并使服务失败关闭，无响应节点在强制终止超时后保留子进程所有权供后续重试。
`get_node_diagnostics` 返回 owner-only 运行时生命周期、保留 owner、
cleanup 候选、会话/监听器/Peer/内嵌服务计数，以及逐项 cleanup step；不包含凭据或
敏感配置值。GPUI/WebUI 使用共享诊断结果展示可恢复问题，WebUI 对未知 cleanup 状态和
失败原因失败关闭。发布门禁在全部平台强制要求 `lifecycle-cleanup` 精确回归通过；
Linux 报告还强制要求策略路由/代理路由表、DoH DNS、TUN 和 Exit 规则清理回归通过。
任一项缺失或失败都会使平台报告及聚合报告不可发布。Linux 清理语义同时由 fake-runner
单测和 `scripts/test-lifecycle-cleanup-netns.sh` 的真实 private network/mount namespace
验收覆盖：后者实际创建 TUN、安装策略规则/私有路由表、接管 DoH DNS，并分别注入一次
规则删除失败和 TUN 删除失败，验证 owner 保留、DNS/路由表恢复、第二次清理及幂等性。
该特权测试只修改命名空间内网络和临时挂载，不触碰宿主日常路由或 `/etc/resolv.conf`；
它仍不替代 macOS/Windows 实机恢复验证。

## 6. W3：所有者认证与公网安全

目标：公网无头 WebUI 只能由单一所有者安全访问。

### W3-01 本地首次初始化 `[x]`

- [x] 生成一次性初始化上下文；
- [x] 注册 Passkey；
- [x] 恢复码仅显示一次并确认保存；
- [x] 原子创建 owner；
- [x] 中断后从安全状态继续；
- [x] 禁止产生两个 owner。

完成证据（实现提交 `c4764f8fa92c9a148ed114184d9e574cf92c5659`）：

- `cargo test -p fuckipv6-client --no-default-features owner_security::`：10 passed；
- `cargo test -p fuckipv6-client --no-default-features webui::tests::owner_`：5 passed；
- 初始化码仅保存 SHA-256 验证器，明文只通过本机终端显示；恢复码只在完成 Passkey 注册后返回一次，确认保存前不提交 owner；
- `full_local_bootstrap_is_atomic_and_recovery_is_single_use` 覆盖完整流程、恢复码单次性及初始化码/恢复码不落盘；
- `interrupted_or_expired_ceremonies_do_not_create_an_owner` 和 `completed_owner_survives_restart_and_permanently_closes_initialization` 覆盖中断、过期及重启后继续状态；
- `concurrent_recovery_confirmation_commits_exactly_once`、`commit_is_atomic_private_and_exactly_once` 和 `incomplete_owner_directory_fails_closed` 覆盖并发单 owner、原子提交和损坏状态失败关闭；
- `owner_bootstrap_completion_closes_initialization_and_unlocks_product_ui` 覆盖 WebUI 初始化接口完成后永久关闭入口。

真实浏览器硬件 Passkey、平台兼容性和公网 HTTPS 认领不属于 W3-01 的本机自动化验收证据，分别由 W3-04 与 W3-02/W3-03 验证。

### W3-02 公网首次认领 `[x]`

- [x] 有效 HTTPS 才开放公网认领；
- [x] 首个访问者输入一次性初始化码；
- [x] 并发认领只有一个事务成功；
- [x] 成功后永久关闭认领入口；
- [x] 失败限流和审计；
- [x] 初始化码不进入 URL/日志。

完成证据（实现提交 `df23a79a8162117070548269cce8d41874b5a0bd`）：

- 公网认领入口仅在显式配置的可信 HTTPS 反向代理后开放，并校验协议、来源和单一转发客户端地址；直接 HTTP 或不受信代理不会解锁公网写路径；
- `cargo test -p fuckipv6-client --no-default-features owner_security::`：15 passed；
- `cargo test -p fuckipv6-client --no-default-features webui::tests::owner_`：6 passed；
- `cargo test -p fuckipv6-client --no-default-features webui::tests::public_owner`：2 passed；
- 公网/local 并发认领测试验证只有一个事务成功；成功后入口返回 `410 permanently_closed`，不能再次认领；
- 连续 5 次错误初始化码后第 6 次返回 `429`，审计分别记录 `invalid_code` 和 `rate_limited`；
- 审计文件使用 `0600` 权限和 append-only 语义，每行同步落盘；审计只记录固定请求元数据、最终状态和结果，不记录 body、headers、初始化码、恢复码或 Passkey；
- 初始化页面只在请求 body 中提交初始化码，不把初始化码放入 URL；审计与日志脱敏测试验证秘密不落盘；
- 审计写入失败时控制面返回 `500` 并失败关闭。

真实公网 TLS、浏览器硬件 Passkey 与平台兼容性仍需 W3-03/W3-04 的实机证据，本项完成声明不代替上述验收。

### W3-03 HTTPS 与反向代理 `[x]`

- [x] 直接 HTTPS 方案；
- [x] 可信反向代理方案；
- [x] 证书错误失败关闭；
- [x] 禁止 HTTP 降级；
- [x] HSTS、CSP、来源校验；
- [x] 文档记录证书/域名/代理限制。

完成证据（实现提交 `bc4e0da`、`8f1aac0`）：

- 内置直接 HTTPS 支持 `FUCKIPV6_WEBUI_TLS_CERT` 与 `FUCKIPV6_WEBUI_TLS_KEY`，启动前验证 PEM、当前有效期、DNS SAN、EKU `serverAuth` 和证书/私钥匹配；任一失败均拒绝启动，不回退到 HTTP；
- 直接 TLS 与可信反向代理 CIDR 互斥；直接 TLS 拒绝全部代理转发头，反向代理模式只接受受信 CIDR、单一 `X-Forwarded-For`、精确 `Host`/`X-Forwarded-Host`/`Origin` 和 `X-Forwarded-Proto: https`，并拒绝 `Forwarded`、`X-Real-IP`、重复头与 HTTP 降级；
- 公网反向代理模式强制 WebUI 监听回环；非回环直接 TLS 仍需显式 `FUCKIPV6_WEBUI_ALLOW_REMOTE=1`；
- 已验证 HTTPS 边界返回一年期 HSTS；本机 HTTP、明文代理请求和未通过来源校验的请求不会误发 HSTS；
- 生产 node surface 每次响应生成随机 CSP nonce，并让 HTML 中的 `<style nonce>`/`<script nonce>` 与 CSP 头一致；同时返回 `X-Frame-Options: DENY`、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`Permissions-Policy` 和同源隔离响应头；
- 文档已记录证书、域名、文件权限、直接 TLS/反向代理互斥、代理头覆盖、HTTP 降级和 HSTS/CSP 限制；
- `cargo test -p fuckipv6-client --no-default-features webui::`：64 passed；
- `cargo check -p fuckipv6-client --no-default-features --all-targets`、`cargo clippy -p fuckipv6-client --no-default-features --all-targets -- -D warnings` 与 `git diff --check` 通过；
- 真实公网证书、真实反向代理和浏览器 CSP 加载尚未在外部网络实测；本项完成的是实现与自动化验收，不据此宣称真实公网部署已验收。

### W3-04 Passkey/WebAuthn `[x]`

- [x] 注册、登录、撤销多个 Passkey；
- [x] 绑定 RP ID 和 origin；
- [x] 防重放和凭据替换；
- [x] 会话过期；
- [x] 高风险操作近期重认证；
- [x] 添加浏览器与平台测试。

完成证据（本轮提交）：

- 新增真实浏览器验收脚本 `scripts/test_webui_webauthn_browser.mjs`，由忽略型 Rust 集成入口在随机 `127.0.0.1` 端口启动隔离 WebUI fixture，再通过 Chrome DevTools Protocol 驱动真实 `navigator.credentials.create/get`；测试不启动 TUN、不修改默认路由；
- Chrome 使用 `internal`、`usb`、`ble` 三个虚拟认证器覆盖首个 Passkey 初始化、恢复码确认、第二/第三个 Passkey 注册、退出后使用新 Passkey 登录、撤销以及禁止撤销最后一个 Passkey；
- `excludeCredentials` 验证已有驻留凭据不能被替换；重认证 finish 的请求体重放被拒绝，重认证后 Cookie 轮换且旧 Cookie 返回 `401`；
- RP ID/origin 绑定、会话过期、错误 origin/节点绑定失败关闭和最近五分钟重认证由 `owner_security::` 服务测试覆盖；浏览器 Cookie 验证为 HttpOnly/SameSite 会话机制，Token 不进入 URL；
- `cargo test -p fuckipv6-client --no-default-features webui::tests::passkey_browser_lifecycle_uses_real_webauthn_api -- --ignored --nocapture`：1 passed；
- `cargo test -p fuckipv6-client --no-default-features owner_security::`：26 passed；`webui::`：66 passed、1 ignored（即上述需显式启用的浏览器测试）；
- `cargo test -p fuckipv6-client --no-default-features`：525 passed、3 ignored；另有集成测试 9 passed、1 ignored；
- `cargo check -p fuckipv6-client --no-default-features --all-targets`、`cargo clippy -p fuckipv6-client --no-default-features --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 和 Node 脚本检查均通过；
- 本项是 Chrome 虚拟认证器自动化验收，不等同于已覆盖所有物理安全密钥、Touch ID/Windows Hello、Android 和跨平台浏览器组合。

### W3-05 本地恢复 `[x]`

- [x] 全部 Passkey 丢失后只能本地系统权限恢复；
- [x] 撤销旧会话；
- [x] 注册新 Passkey；
- [x] 记录高风险审计；
- [x] 不提供公网找回；
- [x] 添加旧设备失效测试。

完成证据（实现提交 `d5b81bc`）：

- `fuckipv6 recover-owner` 从终端隐藏输入恢复码，不提供命令行参数，避免恢复码进入 shell history 或进程参数；恢复服务只绑定随机端口的 `127.0.0.1`，不开放公网入口；
- 恢复流程使用独立进程锁、10 分钟 TTL 和仅存在于 POST body/页面 DOM 的一次性随机 ticket；严格校验 `Host`、`Origin`、`Sec-Fetch-Site` 并拒绝 query string，ticket 不进入 URL、`localStorage` 或 `sessionStorage`；
- 新 Passkey 注册完成后只暂存 owner；用户确认后才原子撤销全部旧 Passkey 和旧会话、提交新 owner 并轮换恢复码，旧浏览器会话随即失效；
- 错误恢复码、取消、过期、并发恢复和审计失败路径都不会替换现有 owner；恢复已提交但事后审计失败时返回独立错误，不误报为“旧凭据仍有效”；
- 高风险审计只记录固定事件元数据和真实回环来源，不记录恢复码、Passkey ID 或 ticket；
- `cargo test -p fuckipv6-client --no-default-features owner_security::`：24 passed；
- 全量单元测试 527 个，525 passed、2 ignored；集成测试 9 passed、1 ignored（特权 lifecycle 测试按设计忽略）；
- `cargo check -p fuckipv6-client --no-default-features --all-targets`、`cargo clippy -p fuckipv6-client --no-default-features --all-targets -- -D warnings`、`node --check crates/client/src/owner_security/owner_recovery.js`、`node scripts/test_webui_node_ui.mjs`、`cargo fmt --all -- --check` 与 `git diff --check` 均通过；
- 本项完成自动化实现与事务边界验收；真实浏览器硬件 Passkey、平台兼容性及完整认证生命周期仍由 W3-04 的实机验收覆盖。

### W3-06 会话与 Web 安全 `[x]`

- [x] CSRF 防护；
- [x] 安全 Cookie 或等价会话机制；
- [x] CSP、X-Frame-Options/框架策略；
- [x] 登录和命令限流；
- [x] 不使用 query string 传 Bearer Token；
- [x] 公网安全测试。

完成证据（本轮提交）：

- 所有受保护 API 的非只读请求先完成认证，再执行同源校验；浏览器所有者会话要求唯一的 `Origin` 与配置的公网 origin 或当前 local `Host` 的 scheme/host/port 完全一致，`Sec-Fetch-Site` 存在时必须为 `same-origin`，缺失或重复 `Origin` 失败关闭；
- WebUI 认证中间件把真实所有者会话的五分钟近期认证状态传入命令上下文；命令 HTTP 适配器在登记 operation 和调用 handler 前再次预授权。关停、配置应用/回滚、节点重启、TUN 重建、节点改名和残留清理等敏感命令在会话过期时返回 `401 authentication_required`、`authorization_code=permission.recent_authentication_required`，且不创建 operation、不触达 handler；普通读取仍可继续；
- 公网路径继续由 `PublicClaimConfig::authorize` 校验受信反向代理/直接 TLS、单一转发客户端地址、固定 `Host`/`Origin` 和跨站 Fetch Metadata 后才接受所有者 Cookie；显式 Bearer 认证按非浏览器调用处理，不依赖 Cookie 同源边界；
- 所有者会话 Cookie 使用 `HttpOnly`、`SameSite=Strict`、`Path=/` 和 12 小时过期，公网通道额外设置 `Secure`；CSP 使用逐响应 nonce 和 `frame-ancestors 'none'`，HTTPS 响应同时设置 HSTS、`X-Frame-Options: DENY`、`nosniff`、`no-referrer`、Permissions Policy 与同源隔离头；
- 登录失败使用独立的重试退避；已认证状态变更使用按来源 IP 的独立 token bucket，默认突发 20 次、每 200ms 恢复 1 次、条目 15 分钟过期且最多 4096 个来源。限流表满时不淘汰活跃来源，拒绝结果带 `Cache-Control: no-store` 和 `Retry-After`；
- 认证中间件拒绝 query string 中的 Token，只接受 `Authorization: Bearer` 或同源所有者 Cookie；新增测试覆盖跨站/缺失 Origin、`Sec-Fetch-Site`、Cookie/Bearer 边界、按来源命令限流与登录/命令限流状态隔离；
- `cargo test -p fuckipv6-client --no-default-features webui::`：71 passed、1 ignored（需显式启用的真实浏览器 Passkey 生命周期）；
- `cargo test -p fuckipv6-client --no-default-features`：单元测试 530 passed、3 ignored；集成测试 9 passed、1 ignored（特权 lifecycle 测试按设计忽略）；
- `cargo check -p fuckipv6-client --no-default-features --all-targets`、`cargo clippy -p fuckipv6-client --no-default-features --all-targets -- -D warnings`、`node scripts/test_webui_node_ui.mjs`、`cargo fmt --all -- --check` 与 `git diff --check` 均通过；
- 公网项完成的是受信反向代理和直接 TLS 安全边界的自动化测试，不等同于已在真实外部公网、真实证书链、真实代理和全部浏览器组合上完成部署实测。

### W3-07 认证 UI `[x]`

- [x] 初始化页；
- [x] 登录页；
- [x] 重认证对话框；
- [x] 恢复码确认；
- [x] 证书/域名错误状态；
- [x] 认证界面仅属于 WebUI；GPUI 不显示登录、Passkey 或恢复入口。
- [x] GPUI 本地所有者命令不依赖 WebUI 会话或 Passkey 重认证；WebUI 即使回环访问也必须认证。

完成证据（本轮提交）：

- WebUI 所有者安全对话框提供会话状态、Passkey 列表、注册和撤销；最后一个可用 Passkey 的撤销由后端 `last_passkey` 稳定错误码失败关闭；
- WebUI 写命令使用 `idem_` 加 43 位无填充 base64url 的契约幂等键；遇到上述近期认证拒绝时保留原命令 envelope 与幂等键，完成 Passkey 重认证后自动重试一次，取消则明确提示操作未执行且不登记乐观 operation；
- Rust 测试覆盖 fresh/stale 会话分流、预授权阻断 operation 登记、普通读取兼容；WebUI 测试覆盖近期认证错误识别、合法幂等键、重认证后自动重试与取消路径；GPUI 继续直接使用桌面 OS 会话作为本地所有者信任边界，不出现 WebUI 登录、Passkey 恢复或重认证界面；
- 初始化页、登录页、本地恢复页和安全对话框统一使用共享 `i18n.json` 目录，覆盖登录边界、恢复码单次展示、`recover-owner` 本地恢复、origin/安全上下文与证书/域名错误提示；断言页面不使用 `localStorage`、`sessionStorage`、Bearer 或 query token；
- `/api/v1/auth/session` 返回通道对应的 `expected_origin`：local 为 `http://localhost:<port>`，public 为配置的 HTTPS 公网 origin；安全 UI 用它诊断 WebAuthn origin 不匹配，不能跳过受信任上下文或 TLS 校验；
- owner API 错误增加稳定 `code`：`recent_authentication_required`、`last_passkey`、`invalid_session` 等，供共享前端处理且不依赖本地化错误文本；
- 本地 mutation CSRF origin 与 owner service 的 local WebAuthn origin 对齐；`http://127.0.0.1:<port>` 不再被误认为合法的 `http://localhost:<port>` 所有者来源；
- GPUI 身份与设置页不显示应用内登录、Passkey、恢复、WebUI 会话或监听器配置，由 `desktop_identity_surface_trusts_the_os_session_without_web_authentication` 与设置投影测试防止回归；Passkey/HttpOnly 同源会话只属于 WebUI；
- GPUI 使用操作系统当前用户会话作为信任边界，命令上下文直接由本地所有者 IPC 构造；自动化测试验证其可执行 `RebuildTun` 和启动网络等敏感命令，不要求 WebUI Passkey、登录或恢复流程。WebUI 控制面即使经回环访问也要求所有者 Cookie，公网还要求可信 HTTPS/代理、同源校验和限流；
- `cargo test -p fuckipv6-client --no-default-features`：库测试 596 passed、3 ignored；集成测试 1 passed（`register_check`）与 11 passed（`w2_runtime`）；特权 lifecycle 集成测试 1 ignored；
- `cargo test -p fuckipv6-client --all-features`：库测试 671 passed、3 ignored；集成测试 12 passed，特权 lifecycle 1 ignored；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；
- `cargo test -p fuckipv6-client gpui::view_model`：17 passed；
- `cargo check -p fuckipv6-client --no-default-features --all-targets`、`cargo clippy -p fuckipv6-client --no-default-features --all-targets -- -D warnings`、`cargo check -p fuckipv6-client --all-targets`、`cargo clippy -p fuckipv6-client --all-targets -- -D warnings` 均通过；
- `node --check crates/client/src/webui/owner_bootstrap.js`、`node --check crates/client/src/owner_security/owner_recovery.js`、`node scripts/test_webui_node_ui.mjs`、`cargo fmt --all -- --check` 与 `git diff --check` 均通过；
- 真实浏览器硬件 Passkey 生命周期测试仍需显式启用 Chrome/Chromium 虚拟认证器以外的物理认证器矩阵；公网 TLS 域名/证书目前只有自动化配置与安全边界验证，未在本轮做真实外部 TLS 部署实测。

## 7. W4：控制 API 与状态同步

目标：所有控制台只依据权威快照、操作和事件行动。

### W4-01 API 版本与路径 `[x]`

- [x] 建立 `/api/v1/snapshot`；
- [x] 建立 `/api/v1/commands`；
- [x] 建立 `/api/v1/operations/<id>`；
- [x] 建立 `/api/v1/events`；
- [x] 建立对象按需读取；
- [x] 定义 schema version 和 request ID。

**实现与测试证据（2026-11-01）**

- v1 路由保留 `/api/node/*` 迁移兼容；`/api/v1/snapshot`、`/api/v1/manifest`、`/api/v1/commands`、`/api/v1/operations/<id>`、`/api/v1/objects/<kind>/<id>` 和 `/api/v1/events` 均由同一 `NodeHttpAdapter` 投影共享命令总线、状态存储和事件总线，没有复制业务状态机；
- 所有 v1 成功和错误响应包含 `schema_version`、`request_id`、RFC3339 `server_time`；命令响应回显客户端命令 `request_id`，读请求使用适配器单调分配的内部请求 ID；
- 对象读取仅从权威 `NodeSnapshot` 投影 `node`、`network`、`peer`、`embedded_service`、`network_configuration`，不存在或未知对象统一返回稳定 `not_found`；
- `/api/v1/events` 为 SSE，首个事件是权威快照；W4-03 已在此基础上补齐历史回放、断线恢复、快照 revision 对齐和缺口修复，cursor 不可安全续接时返回带稳定原因的 `resync_required`；
- `/api/v1/operations/<id>` 已建立并受契约约束；W4-01 时对任何 ID 返回稳定 404，W4-02 建立真实注册表后，已知 ID 返回权威 operation，未知 ID 仍稳定返回 404；
- 新 DTO 已加入 `NodeHttpContract`，重新生成 JSON Schema、TypeScript 与 Android 绑定；`scripts/generate-contracts.sh` 的生成、校验和 7 个 Rust 契约测试通过，`node scripts/test_contract_bindings.mjs` 通过；
- `cargo test -p fuckipv6-client --no-default-features`：库测试 540 passed、3 ignored；集成测试 1 passed（`register_check`）与 8 passed（`w2_runtime`）；特权 lifecycle 集成测试 1 ignored；
- `cargo test -p fuckipv6-client gpui::view_model`：17 passed；两套目标的 `cargo clippy --all-targets -- -D warnings` 均通过。

### W4-02 Operation 生命周期 `[x]`

- [x] 写命令返回 `operation_id`；
- [x] 幂等键复用同一操作；
- [x] 阶段进度；
- [x] succeeded/failed/cancelled/timed_out；
- [x] 失败返回错误码、对象和恢复建议；
- [x] UI 不乐观宣告成功。

**实现与测试证据（2026-11-01）**

- `/api/v1/commands` 使用 `NodeCommandKind::is_write()` 区分读写：写命令必须提交 `idempotency_key`，成功注册后返回 `202 Accepted` 和 queued operation；读命令保持同步结果；重复键与相同命令返回原 operation，不同命令返回稳定 `operation_conflict`，并发重试不会重复派发副作用；
- operation 注册表只保存适配器生命周期投影，实际执行仍通过共享 `NodeCommandBus`。阶段事件按 `queued → running/dispatching → running/executing → succeeded` 发布；取消和超时分别为 `cancelled`、`timed_out`，业务拒绝为 `failed`；
- 失败 operation 保留稳定错误码、`message_key`、目标对象、`retryable`、恢复建议和 `partial_changes`；`TimedOut` HTTP 映射为 504，未知 operation 为 404；
- 当前旧 WebUI 仍调用同步兼容 `/api/node/commands`，只在拿到真实 `NodeCommandResult` 后提示成功，因此不存在乐观宣告；v1 operation 的 UI 消费迁移留给 W6 新界面，届时必须轮询/订阅到终态后才宣告成功；
- `cargo test -p fuckipv6-client --no-default-features`：库测试 548 passed、3 ignored；集成测试 1 passed（`register_check`）与 8 passed（`w2_runtime`）；特权 lifecycle 集成测试 1 ignored；
- `cargo test -p fuckipv6-client gpui::view_model`：17 passed；两套目标的 `cargo clippy --all-targets -- -D warnings` 均通过；
- operation 生命周期、幂等冲突、缺失幂等键、失败恢复字段、取消/超时终态、阶段事件和未知 ID 均由 `node::http_adapter::tests` 覆盖；重新生成 JSON Schema、TypeScript 与 Android 绑定，`scripts/check-contracts.sh` 的 7 个 Rust 契约测试、TypeScript 构建、WebUI 契约测试与 Android `javac` 全部通过；生成器现会把 `NodeId` 等 primitive ID alias 投影为底层 `String`，避免 Android 引用不存在的生成类。

### W4-03 SSE 与重连 `[x]`

- [x] 事件 cursor；
- [x] 断线重连；
- [x] 事件缺口返回 `resync_required`；
- [x] 快照 revision 对齐；
- [x] 重复事件去重；
- [x] 不通过 URL 携带敏感 Token。

**实现与测试证据（2026-11-01）**

- `NodeEvent` 增加 `snapshot_revision`，事件总线维护有界历史并暴露 `NodeEventCursor { sequence, snapshot_revision }`；状态写入在 `StateChanged` fan-out 前推进 revision，避免状态与事件 epoch 错位；
- `/api/v1/events` 接受 `after=<sequence>:<snapshot_revision>` 和标准 `Last-Event-ID`，兼容裸 `after=<sequence>`；SSE `id` 使用 `sequence:revision`，快照事件携带续传 cursor；
- 重连时先原子确定订阅边界，再回放边界内历史、读取权威快照并继续消费边界后事件；历史窗口不足、cursor 超前、revision 回退、广播滞后和 revision 跳号分别返回稳定 `resync_required` 原因，不静默跳事件；
- 已确认 sequence 在流状态内循环丢弃，不产生只有 comment 的空 SSE chunk；`node::http_adapter::tests` 的三个 SSE 重连/过期 cursor/缺口测试与 `node::event::tests` 的三个事件总线测试通过；
- 旧 WebUI 的 Bearer WebSocket 认证改由 `Sec-WebSocket-Protocol` 携带，URL 不再包含 Token；测试同时验证 query token 被拒绝、子协议认证通过认证中间件；
- `cargo test -p fuckipv6-client --no-default-features`：库测试 553 passed、3 ignored；集成测试 1 passed（`register_check`）与 8 passed（`w2_runtime`）；特权 lifecycle 集成测试 1 ignored；
- `cargo test -p fuckipv6-client gpui::view_model`：17 passed；默认特性与 `--no-default-features` 两套 `cargo clippy --all-targets -- -D warnings` 均通过；
- 重新生成 JSON Schema、TypeScript 与 Android 绑定；`scripts/check-contracts.sh` 的 7 个 Rust 契约测试、TypeScript 构建、WebUI 契约测试与 Android `NodeContractV1Test` 全部通过，`node scripts/test_contract_bindings.mjs` 通过。

### W4-04 错误模型 `[x]`

- [x] 区分认证、权限、校验、状态、能力、依赖和内部错误；
- [x] 稳定 message key；
- [x] 结构化参数；
- [x] retryable 和 recovery；
- [x] 不泄漏无权限对象存在性；
- [x] 错误码生成 TypeScript 类型。

**实现与测试证据（2026-11-01）**

- `NodeErrorCategory` 固定为 `authentication`、`authorization`、`validation`、`state`、`capability`、`dependency`、`internal` 七类；命令结果使用 `NodeProblem`，HTTP/WebSocket/operation 使用同类的结构化 `NodeErrorDetails`/`OperationError`，UI 不解析人类可读文案来判断错误类型；
- 每个错误同时携带稳定 `code`、`message_key`、可选结构化 `params`、`retryable` 和安全 `recovery`；文案只作为回退，参数值使用字符串映射并通过契约生成到 JSON Schema、TypeScript 和 Android；
- 认证/授权错误在同步命令结果、异步 operation 终态和 HTTP/WebSocket 错误投影中统一脱敏：不返回受保护对象引用、网络名、对象参数或原始诊断文案，只保留稳定错误码、通用文案、`message_key` 和恢复建议；
- `authorization_errors_do_not_reveal_protected_objects_or_parameters` 同时覆盖旧命令响应与 v1 operation，验证参数、网络名、对象 ID 和原始消息均不会进入序列化响应；`node_problem_round_trips_with_stable_codes_and_flags` 覆盖全部错误码及稳定标志序列化；
- TypeScript 契约生成 `NodeErrorCategory` 与 `NodeHttpErrorCode` 联合类型，`scripts/check-contracts.sh` 的 7 个 Rust 契约测试、TypeScript 构建、WebUI 契约测试和 Android `NodeContractV1Test` 全部通过；
- `cargo test --workspace`：958 passed、0 failed、4 ignored；`cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；
- 对应提交：`871060e`（错误模型主体）与 `24e97d7`（授权错误脱敏及回归测试）。

### W4-05 服务端权限校验 `[x]`

- [x] 所有命令重新校验角色、ACL、状态和能力；
- [x] 不信任 UI 的权限预判；
- [x] 本地所有权边界；
- [x] 临时 Coordinator 禁写；
- [x] 审计 actor 和原因；
- [x] 添加越权测试。

**实现与测试证据（2026-11-01）**

- `NodeCommandBus::execute` 在任何 handler 分派前调用同一授权入口；请求 JSON 和 UI capability/availability 声明不参与可信上下文构造，未认证传输返回 `permission.authentication_required` 且不会触达 handler；
- `NodeCommandContext` 的字段保持私有，只能由本地 owner、已验证 owner session、本地 owner snapshot 或认证适配器构造；网络会话由适配器提供逐网络 `NetworkCommandScope`，命令载荷中的网络名只能缩小到该可信作用域，越界、`All` 和空目标全部失败关闭；
- 每条网络命令同时检查能力、成员状态、角色上限、本地策略和网络策略。成员只有 `Active`/`Degraded` 可访问网络资源；本地层与网络层都必须存在匹配 allow，任一层 deny 优先；显式 allow 不能突破角色上限；
- 节点级快照、UI manifest、诊断、创建/加入网络、节点启停、残留清理和关停保持 local-owner-only；临时 Coordinator 可以读取其网络范围，但任何写命令返回 `permission.temporary_coordinator_read_only`；
- 审计记录 actor、认证来源、命令 kind、request ID、逐网络角色、network ID、结果和授权原因，不记录邀请票据或其它命令载荷；拒绝结果对 UI 只暴露稳定的 `authorization_code`，详细原因保留在服务端审计中；
- `node::authorization::tests` 的 10 个测试覆盖未认证、非活动成员、缺能力、owner-only、scope 越界/All、双层 deny 优先、缺少任一 allow、组选择、重认证时效、角色上限、跨网络条件和临时 Coordinator 只读；`node::command::tests` 的 12 个测试覆盖分派前拒绝、审计关联及载荷秘密不进入审计；
- `cargo check --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 全部通过；
- 对应提交：`605a9c0`（可信传输上下文与命令总线入口）与 `0dc1214`（逐网络双策略授权、临时 Coordinator 禁写和越权回归）。

### W4-06 生成客户端与漂移检查 `[x]`

- [x] OpenAPI/JSON Schema；
- [x] TypeScript 客户端；
- [x] 生成文件漂移检查；
- [x] 命令/事件/错误枚举一致性；
- [x] Android 契约验证；
- [x] CI 阻断漂移。

**实现与测试证据（2026-11-01）**

- `contracts/schema/` 包含 7 份由 Rust `JsonSchema` 生成的 draft 2020-12 契约：command、request、result、event、HTTP、snapshot 和 UI manifest；该条要求的是 OpenAPI 或 JSON Schema 二选一，因此不额外维护第二套 OpenAPI；
- TypeScript 绑定包含完整 DTO、命令/事件/错误枚举、schema 版本检查和 HTTP 客户端；Android 绑定包含 Java 8 可用的 DTO、枚举、联合类型和命令信封校验；
- 命令常量来自 `NodeCommandKind` 的单一权威定义，事件、错误分类和 HTTP 错误码由同一批 schema `$defs` 投影；生成器会拒绝不同 schema 之间的同名定义冲突，`--check` 会阻断生成文件漂移；
- `./scripts/check-contracts.sh` 通过：Rust 契约测试 7 passed、TypeScript strict check/build、`test_contract_bindings`、`test_webui_node_ui` 和 Android `NodeContractV1Test` 全部 passed；
- `./scripts/generate-contracts.sh` 后 `git diff --exit-code -- contracts` 通过，证明重新生成无漂移；
- CI 的 `contracts` 作业执行 schema/绑定门禁、完整重新生成，并以 `git diff --exit-code -- contracts` 阻断漂移。

### W4-07 API 安全与限额 `[x]`

- [x] 请求大小限制；
- [x] 并发限制；
- [x] 登录/命令限流；
- [x] 可信代理；
- [x] 审计关联；
- [x] 公网攻击面测试。

**已实现与测试证据（2026-11-01）**

- 登录失败按来源 IP 与用户名分别累计，连续失败后指数退避；认证成功会清除失败状态，活跃退避项不会被来源洪泛驱逐，条目有 TTL 和容量上限。认证后的状态变更请求另有按来源 IP 的 token bucket，登录限流与命令限流互不消耗；
- 可信代理只接受显式 CIDR allowlist 内的连接，并严格校验单一 `X-Forwarded-For`、`X-Forwarded-Host`、`X-Forwarded-Proto: https`、`Host` 和 `Origin`；拒绝 `Forwarded`、`X-Real-IP`、重复或不明确的转发头。直连 TLS 模式使用对端地址且拒绝所有代理转发头；
- `webui::security` 的退避、成功解锁、TTL/容量边界、token bucket 和来源隔离测试通过；`webui::public_claim` 的伪造代理、重复头、Host/Origin 注入、直连 TLS 与不受信代理测试通过；`webui` 路由测试覆盖登录端点限流和认证命令按来源限流。
- 旧版 `/api/node/commands` 与 v1 `/api/v1/commands` 在 JSON 反序列化前统一限制请求体为 256 KiB，超限返回 HTTP 413、稳定 `payload_too_large` / `error.payload_too_large` 和 `limit_bytes` 参数；回归测试同时覆盖两个入口，确保超限请求不会进入命令解析与分派。
- 两个命令入口共享每个适配器 32 个并发执行槽，耗尽时立即返回 HTTP 429、`Retry-After: 1` 和稳定 `rate_limited` / `error.rate_limited` 契约，不排队也不进入命令总线；成功和拒绝路径都会释放 permit，超限响应包含实际的 `concurrency_limit` 参数。针对性测试覆盖双入口快速失败、总线隔离、可重试语义和 permit 回收。
- HTTP 适配器创建写操作后，将服务端生成的 operation ID 附加到可信 `NodeCommandContext`，命令审计再写入 `AuditEntry.operation_id`；该关联不读取请求 JSON 中的不可信字段。测试覆盖命令总线关联以及 v1 HTTP 操作完成后的 request ID、operation ID、actor/source、role 和 outcome 一致性，并验证邀请票据秘密不会进入审计详情。
- `scripts/test-public-api-attack-surface.sh` 汇总 18 个精确命名的 HTTP/WebSocket 边界回归，覆盖未认证/无效 Bearer、URL 查询参数泄密、登录与命令限流、匿名和所有者 Cookie 跨站写、伪造代理头/Host/Origin、直连 TLS 代理头、请求体超限、并发槽耗尽与回收、legacy/v1 schema 降级、未知命令失败关闭；每一例都要求实际执行且恰好 1 passed，避免测试被改名或过滤成 0 项后静默通过。
- CI 新增独立 `api-security` 作业执行该门禁并上传逐项日志；`release-gate.py aggregate` 现强制要求 `contracts` 与 `api-security` 上游作业均成功，避免汇总时遗漏契约或攻击面门禁。这只证明自动化边界验收完成，不代表已在真实公网、真实反向代理或独立攻击机上完成实机压测。

## 8. W5：共享设计系统

目标：先统一信息和交互语言，再实现页面。

### W5-01 设计令牌 `[x]`

- [x] 浅色/深色背景、文本、边框和主色；
- [x] 成功/警告/危险/信息状态色；
- [x] Coordinator/Relay/Exit/File 能力色；
- [x] 字体、字号、行高和等宽字体；
- [x] 间距、圆角、阴影、尺寸；
- [x] 动效和减少动态效果。

实现说明：`crates/common/src/design_tokens.rs` 是唯一语义来源；WebUI 的深浅主题与排版/间距/尺寸/动效 CSS 自定义属性在运行时由该模块生成，GPUI 颜色直接引用同一组 Rust 常量。基础样式表中的过渡和动画已消费动效令牌，`prefers-reduced-motion` 使用统一的减少动态效果时长。测试校验双主题、状态色、四种能力色唯一性、正文对比度、CSS 令牌完整性和基础样式表不重复声明主题令牌。

### W5-02 基础组件 `[x]`

- [x] Button、Input、Select、Checkbox、Radio；
- [x] Tabs、Breadcrumb、Toolbar；
- [x] Table/VirtualList；
- [x] Badge、StatusBadge、CapabilityRing；
- [x] Dialog、Menu、ContextMenu、Tooltip、Toast；
- [x] Empty/Error/PermissionDenied/Unsupported 状态组件。

实现说明：`crates/common/src/ui_components.rs` 定义 22 个稳定 `data-component` 名称、根 CSS 类、无障碍契约、状态语义和四种能力语义；`assets/ui-components.js` 提供无框架浏览器构造器，用户文本只通过 `textContent`/文本节点写入，Dialog、Tabs、菜单、Tooltip、Toast、表和虚拟列表包含键盘/焦点行为；`assets/ui-components.css` 只消费 W5-01 令牌。客户端与 Coordinator WebUI 均加载同一 CSS/JS。GPUI 的 `gpui/components.rs` 提供同一清单的适配器映射，并让现有导航、输入、选择、工具栏、状态徽标、能力图例、Toast 与空/错/权限/不支持状态实际消费共享令牌和状态语义。

边界：本项只冻结可复用组件契约和两端适配器，不代表 W5-03 页面布局、W5-05 完整无障碍、W5-06 GPUI IME/原生菜单或 W5-07 React 工程已经完成；GPUI 当前渲染的仍是 manifest 驱动的基础页面。

### W5-03 布局原语 `[x]`

- [x] 桌面侧栏 + 顶栏 + 内容 + 详情面板；
- [x] 窄屏顶部菜单/抽屉；
- [x] Android 底部导航；
- [x] 页面标题和局部 Tab；
- [x] 加载骨架和稳定刷新；
- [x] 960×640 最小桌面布局。

已实现：`crates/common/src/ui_layout.rs` 冻结 600/900/1280 三档断点、侧栏/顶栏/内容/详情区域、五项主导航、Data/Actions 局部 Tab 和稳定刷新语义；设计令牌新增 232px 侧栏、56px 顶栏、320–480px 详情宽度、64px 底部导航、960×640 最小窗口。`ui-components.css/js` 提供响应式侧栏、导航/详情抽屉、底部导航、骨架、busy 指示、焦点恢复和减少动态效果。节点 WebUI 已改为真实双区域布局，manifest 的完整 11 页保留在侧栏，底部导航只映射五项一级目的地，刷新时保留旧快照内容并显示 busy 状态。GPUI 创建窗口时强制 960×640 最小尺寸，按实际窗口宽度读取共享断点；页面默认进入 Data 局部 Tab，Actions 独立展示共享动作，manifest 首次加载显示骨架，宽度达到 1280px 时显示固定宽度上下文详情面板。

边界：GPUI 已消费桌面/宽屏断点、局部 Tab、骨架与详情契约；由于原生窗口最小宽度为 960px，窄屏抽屉路径由 WebUI 验收。Android 本项只完成共享 NavigationBar 契约，原生 Compose 实现仍由 W16-01 验收。React/Vite 单一应用壳由 W5-07 验收，跨端视觉门禁由 W5-08 验收。

### W5-04 国际化 `[x]`

- [x] 简中与英文翻译键；
- [x] 跟随系统和手动切换；
- [x] 数字、时间、时区、单位和地址格式；
- [x] 错误/状态/空状态/危险操作全部可翻译；
- [x] 伪本地化和截断测试；
- [x] Rust 不返回自然语言作为稳定业务标识。

实现说明：`crates/common/assets/i18n.json` 是简中/英文双目录的唯一可见文案来源，Rust、节点 WebUI 与 GPUI 共享同一套稳定键。系统语言检测、显式语言偏好和浏览器/桌面手动切换均已接通，偏好持久化到节点配置；数字、字节、时长、显式时区时间和 IPv4/IPv6 地址使用确定性格式化。浏览器运行时与 Rust 均支持占位符安全的伪本地化，并按 Unicode grapheme cluster 截断，组合字符和 ZWJ emoji 不会被拆开。`NodeProblem` 的业务分支使用稳定的 snake_case `code` 与 `error.*` `message_key`，测试门禁要求 code/key 仅含稳定 ASCII 字符且两个语言目录都存在对应键；自然语言 `message` 只用于兼容展示，不作为业务判定依据。

边界：本项完成第一版简中/英文及现有产品表面的国际化，不包含后续新增语言；WebUI 可访问性、React 单一应用壳和跨端视觉门禁分别由 W5-05、W5-07、W5-08 验收。

### W5-05 可访问性 `[x]`

- [x] 全核心操作键盘完成；
- [x] 焦点顺序和焦点环；
- [x] 菜单键和右键等价；
- [x] 拓扑列表等价视图；
- [x] 颜色不作为唯一信息；
- [x] WCAG 2.2 AA 检查。

实现说明：共享 WebUI 组件补齐 Dialog 初始焦点、Tab/Shift+Tab 焦点陷阱、Escape/遮罩关闭、关闭后焦点恢复、嵌套对话框栈和背景 `inert`；Menu/ContextMenu 统一点击、右键、`ContextMenu`/Apps、Shift+F10、方向键、Home/End、Escape 与焦点恢复行为，并明确 `aria-haspopup`、`aria-controls` 和 `aria-expanded`。Tabs 生成稳定 tab/panel 关联并支持方向键、Home/End 切换；CapabilityRing 同时输出能力名称与启用状态文本，不再只靠颜色表达状态。节点 WebUI 的核心控件增加 `:focus-visible` 焦点环，登录、所有者安全和重认证统一接入可访问 Dialog；拓扑页新增逐网络、逐 Peer 的文本等价视图，并为每个网络/Peer 行提供可执行“在节点列表中查看”的键盘与右键菜单，菜单完成后把焦点恢复到新页面内容。`scripts/test_webui_accessibility.mjs` 使用无网络 Chromium/CDP fixture 验证焦点陷阱、背景隔离、焦点恢复、菜单键/右键等价、方向键、Tabs ARIA 和 CapabilityRing 文本状态；`scripts/test_webui_node_ui.mjs` 验证中英文拓扑投影与能力快照契约。

边界：本项覆盖节点 WebUI、共享浏览器组件及当前自动化/人工键盘路径审查，不等于第三方完整 WCAG 认证或真实辅助技术全组合验收。GPUI 原生无障碍属于 W5-06；旧 Coordinator/legacy `UNIFIED_MASTER_HTML` 表面由 W5-07 移除，本项不宣称其完整通过 WCAG 2.2 AA。

### W5-06 GPUI 基础适配 `[x]`

- [x] 原生窗口与布局；
- [x] IME；
- [x] 原生文件选择；
- [x] 快捷键和菜单；
- [x] 高 DPI；
- [x] 无障碍语义。

实现说明：GPUI 继续使用原生窗口，并按共享断点消费逻辑像素宽度，避免高 DPI 下把物理像素误判为布局宽度；自动测试覆盖 1.5× 缩放后 900px 逻辑宽度不变及 1280px 详情面板断点。原生单行输入实体接入 GPUI `EntityInputHandler`，覆盖 UTF-16 与 Rust UTF-8 偏移转换、Unicode grapheme 光标、鼠标/键盘选区、复制/粘贴/剪切、全选和中英文组合输入。原生路径适配器统一封装文件多选、目录单选、取消与平台错误语义；当前 manifest 尚无路径字段，因此适配器保持待接入状态。应用注册 `Ctrl/Cmd+R` 刷新与 `Ctrl/Cmd+Q` 退出，原生菜单文案随简中/英文切换。主窗口结构声明 Application、Navigation、Main、Heading、TabList 和 TabPanel 语义角色，页面局部切换使用真正的 Tab/TabPanel 控件，不再用单选按钮模拟。

边界：自动测试验证快捷键注册、双语言菜单、角色声明、a11y 启用和逻辑像素断点，不等同于第三方屏幕阅读器全组合验收；当前 GPUI 测试平台也未暴露辅助技术激活回调，因此不以合成无障碍树代替真实辅助技术测试。原生路径选择在本项只完成适配器，不代表已有业务页面使用路径字段。

### W5-07 WebUI 基础工程 `[x]`

- [x] React 19 + TypeScript + Vite；
- [x] 单一应用壳，不保留旧双表面；旧内嵌 `node_ui.html` / `node_ui.js` 已删除；
- [x] Radix 行为原语（Dialog、Dropdown、ContextMenu、Tooltip、Tabs）；
- [x] TanStack Query/Table/Virtual；
- [x] Vite 生产资源（`app.js`、`app.css`、`index.html`）通过 `include_bytes!` 嵌入单一二进制；
- [x] `npm run build`、`npm run lint`、`npm run typecheck`、`npm test -- --run` 通过；
- [x] 契约固件接受/拒绝、快照校验、命令封包/请求 ID/幂等键、Passkey 未认证视图、五模块与十一页面 manifest 驱动外壳均有自动化测试。

**证据（2026-11-01）**：React 单元测试 8/8 通过；WebUI 生产构建成功；Rust `webui::node_surface` 嵌入资源测试通过。CSP 已收紧为 `script-src 'self'; script-src-attr 'none'; style-src 'self'; style-src-attr 'unsafe-inline'`：内联脚本继续完全禁止，仅允许 Radix 弹层和虚拟列表所需的 `style` 属性。跨端视觉门禁见 W5-08。

### W5-08 视觉与语义回归 `[x]`

- [x] 同平台、同窗口、同字体、同 DPR 截图；
- [x] GPUI/WebUI 页面层级和控件顺序对比；
- [x] 可接受差异清单只包含字体栅格化、原生滚动条、IME 等；
- [x] 功能/权限/状态/流程分叉为零；
- [x] 视觉回归纳入 CI。

**证据（2026-11-02）**：`npm run visual:regression --prefix webui` 在 Linux、1440×900 逻辑窗口、2× DPR、Inter 字体和英文 Overview fixture 上同时驱动 GPUI headless 截图与 Chrome/WebUI 截图。门禁严格比较标题、语言、字体、视口、五模块、页面 Tab、主状态、页面标题、动作面板、动作顺序、快捷入口顺序和网络状态顺序，当前语义差异为 0；比较 22 个层级/布局节点的 88 个位置与尺寸属性，固定几何容差为 1.5px，仅 `hero_copy.width` 因按钮文本测量形成弹性剩余宽度而允许 2.5px，实测最大偏差为 1.9844px。2880×1800 截图的 5,184,000 个像素指标为 MAE 8.272661、RMSE 19.986747、通道差大于 16 的像素占比 21.6003%、大于 32 的占比 2.1752%，均低于门禁阈值 10、22、23% 和 3%。脚本生成 `target/visual-regression/{gpui,webui}/overview.png`、`diff.png` 和 `visual-regression.json`；CI 的 `visual-regression` job 在 Xvfb/Vulkan 环境中运行同一命令并上传全部产物。

边界：允许差异仅为平台字体栅格化/亚像素文本测量、可滚动时原生与浏览器滚动条绘制、以及空闲截图中不出现的 IME 组合界面；不接受布局节点缺失、控件顺序变化、状态/权限/动作分叉或几何超差。像素阈值用于约束抗锯齿和字体 hinting，不宣称两种渲染器逐字节相同。

## 9. W6：应用外壳、概览与设置

### W6-01 五模块应用外壳 `[x]`

- [x] 概览；
- [x] 节点；
- [x] 网络；
- [x] 活动；
- [x] 设置；
- [x] 全局搜索和对象跳转；
- [x] 不允许旧 11 页直接作为一级导航。

**证据（2026-11-02）**：GPUI 与 WebUI 均以五个一级模块承载原有十一页；全局搜索在本地索引页面、身份、网络和 Peer，支持 `Ctrl/Cmd+K` 聚焦、`Escape` 清除、空结果提示，并在打开网络/Peer 结果时同步网络上下文、跳转到目标页和恢复内容焦点。WebUI 搜索逻辑与 GPUI 使用相同的完全匹配、前缀、单词前缀、包含排序规则并限制最多 12 条结果。`cargo test -p fuckipv6-client --all-features` 通过（lib 628 passed、3 ignored；集成测试 9 passed、1 ignored）；`cargo clippy -p fuckipv6-client --all-features --all-targets -- -D warnings`、`npm run typecheck --prefix webui`、`npm run lint --prefix webui`、`npm test --prefix webui -- --run`（26/26）和 `npm run build --prefix webui` 均通过。运行态回归同时修正了 WebUI 监听端口早于信号处理器注册造成的 SIGTERM 非优雅退出窗口。

### W6-02 唯一主状态 `[x]`

- [x] 实现未初始化/已停止/过渡/正常/降级/节点故障/控制服务异常；
- [x] 优先级：意图/过渡 → 节点故障 → 降级 → 控制服务异常 → 正常；
- [x] 无问题时只显示“运行正常”；
- [x] 有且仅有一条可行动说明；
- [x] 不并列展示 control/node 双状态；
- [x] 添加状态组合测试。

### W6-03 概览 `[x]`

- [x] 节点名称和主操作；
- [x] 需要处理事项最多三条；
- [x] 紧凑网络摘要；
- [x] 最近五条重要活动；
- [x] 创建/发现/加入快捷入口；
- [x] 无营销卡片和无价值实时图表。

**证据（2026-11-02）**：GPUI 与 WebUI 使用相同的概览信息层级：节点名称、唯一主状态和唯一主操作位于首屏顶部；无执行权限时隐藏不可执行主按钮。需要处理事项按影响排序、去重并最多显示三条；网络摘要统一展示在线状态、Peer 数、TUN、路由模式、出口和角色；最近活动按时间倒序取五条，只展示脱敏后的动作、结果、资源和命令。创建/加入入口打开真实网络动作；发现索引尚未接入，入口保持可见并给出稳定的不可用原因。概览不渲染通用指标卡、营销卡片、Canvas 或无行动价值的实时图表。`cargo test -p fuckipv6-client --all-features` 通过（lib 629 passed、3 ignored；集成测试 9 passed、1 ignored），`cargo clippy -p fuckipv6-client --all-features --all-targets -- -D warnings`、WebUI `typecheck`、`lint`、28/28 单元测试和嵌入式生产构建均通过。

### W6-04 设置基础页 `[x]`

- [x] 所有者与安全；
- [x] 远程访问；
- [x] 服务与更新；
- [x] 发现索引；
- [x] 数据与恢复；
- [x] 平台不支持项的只读说明。

**证据（2026-11-02）**：GPUI 与 WebUI 统一按“所有者与安全、远程访问、服务与更新、发现索引、数据与恢复”五组展示权威快照，不再使用通用指标卡或旧设置摘要。所有者身份、Node ID、公钥指纹、访问模型、权限、WebUI 会话、所有者恢复、UI/显示环境、WebUI 监听来源、四项能力状态、内嵌服务、更新与备份未接入原因、发现角色、revision 和只读写入状态均来自现有快照；平台/构建不支持的能力单独展示状态与原因，所有尚未接入的功能明确标注为不可用，不渲染伪开关或伪按钮。`cargo test -p fuckipv6-client --all-features` 通过（lib 630 passed、3 ignored；集成测试 9 passed、1 ignored），`cargo clippy -p fuckipv6-client --all-features --all-targets -- -D warnings`、WebUI `typecheck`、`lint`、29/29 单元测试和嵌入式生产构建均通过，`dist/assets/app.js` 已同步刷新。

### W6-05 高级设置 `[x]`

- [x] 可搜索；
- [x] 监听地址、端口、接口白名单；
- [x] DNS/DoH/DoT/泄漏保护；
- [x] NAT、Keepalive、并发和资源限制；
- [x] WebUI 会话、代理、限流、HSTS；
- [x] 日志、指标、更新、维护窗口。

**证据（2026-11-02）**：GPUI 与 WebUI 同时提供同一套 5 组、21 个高级设置项：监听与接口、DNS 与隐私、NAT/连接与资源、WebUI 与访问安全、日志/指标与维护。每项均标注 `启动配置`、`只读`、`部分接入` 或 `不支持`，并显示真实快照值或稳定的未接入原因；P2P 实时监听地址优先于配置端口，Keepalive、连接超时和日志级别来自逐网络安全快照。全局搜索可从中英文关键词定位高级设置并导航到设置页。当前切片不渲染伪开关或伪按钮，事务性编辑、完整配置预览、差异应用和回滚仍由 W6-06 负责。`cargo test -p fuckipv6-client --all-features` 通过（lib 630 passed、3 ignored；集成测试 9 passed、1 ignored），`cargo clippy -p fuckipv6-client --all-features --all-targets -- -D warnings`、WebUI `typecheck`、`lint`、31/31 单元测试和嵌入式生产构建均通过，`dist/assets/app.js` 已同步刷新。

### W6-06 配置预览与回滚 `[x]`

- [x] 完整配置预览；
- [x] 差异导入；
- [x] 校验错误定位；
- [x] 原子应用；
- [x] 回滚；
- [x] 配置变更审计。

**证据（2026-11-02）**：GPUI 与 WebUI 均接入完整事务配置工作台，提供当前 revision、可编辑 networks 文档、JSON 文件导入、候选配置、逐路径 before/after 差异、影响级别、校验问题和稳定的禁用原因。编辑后必须对当前草稿重新预览，且预览 revision 与当前文档一致才允许应用；支持冲突、校验失败、无变化和命令不可用四类阻断。校验问题可定位到 JSON Pointer 对应编辑位置；应用和回滚均有二次确认，成功后刷新文档与审计历史。审计详情展示 transaction、revision、操作者、来源、时间、目标 revision 和变更路径。GPUI 工作流状态机有 6 个专项测试，WebUI 工作流有 5 个专项测试；`cargo test -p fuckipv6-client --all-features` 通过（lib 646 passed、3 ignored；集成测试 9 passed、1 ignored），`cargo test -p fuckipv6-common i18n` 9/9、WebUI 37/37、`typecheck`、`lint`、生产构建及 `cargo clippy --workspace --all-targets --all-features -- -D warnings` 均通过。

### W6-07 状态矩阵落地 `[x]`

- [x] 加载、刷新、空、错误、无权限、不支持；
- [x] 降级、维护、陈旧数据、离线；
- [x] 会话过期保留编辑上下文；
- [x] control/node 不可达分离；
- [x] 所有禁用控件有可见原因；
- [x] 页面状态测试完整。

**证据（2026-11-02）**：WebUI 将页面状态收敛为一个带固定优先级的权威主状态，不再并列渲染多套状态：`error → initial_loading → offline → maintenance → permission_denied → unsupported → node_failed → degraded → control_unavailable → stale → refreshing → empty → ready`。浏览器离线、WebSocket 断开/重同步、控制服务 503、节点 `failed`、运行降级、无查看权限、平台能力不支持、空数据和刷新均有独立信号与可见说明；控制服务异常与节点故障使用不同文案，陈旧状态显示最近权威快照时间。维护状态由控制面临时拒绝请求触发并保留最后快照；陈旧、离线和错误状态会阻断写操作。动作表单、二次确认、配置加载/导入/预览/应用/回滚/历史及全局临时操作均提供稳定的禁用原因。会话过期后保留当前页、网络、搜索、待打开动作，以及配置文档、编辑器内容、预览、应用结果、导入名和校验定位。WebUI 8 个测试文件 60/60 通过（含状态矩阵、维护映射、control/node 分离、陈旧、离线、不支持、会话恢复、配置阻断和确认弹窗阻断），`typecheck`、`lint`、生产构建通过（仅有现存 Vite >500 kB chunk 警告）；`cargo test -p fuckipv6-common i18n` 9/9、`cargo clippy -p fuckipv6-client --all-features --all-targets -- -D warnings` 和 `cargo fmt --all -- --check` 通过，`dist/assets/app.js` 与 `app.css` 已同步刷新。

## 10. W7：节点页

目标：节点页是本地所有者管理“本机运行时”的入口，不是全局产品中心。启停、接口、身份和诊断必须操作同一个权威 runtime，不得由 UI 生命周期隐式接管。

### W7-01 节点运行状态与启停 `[x]`

- [x] 从 control 读取权威 NodeSnapshot，不从 UI 本地状态推断；
- [x] 展示未初始化、已停止、启动中、运行中、停止中、降级、故障；
- [x] 实现启动、停止、重启，并展示 operation 进度、阶段和可取消性；
- [x] 启动失败返回结构化原因、建议动作和可复制诊断 ID；
- [x] 停止 node 后保持 control 与 WebUI 存活；
- [x] 关闭 GPUI/WebUI 不触发 node shutdown；
- [x] 停止操作在硬超时后执行受控清理，不要求用户手工 kill；
- [x] 启动/停止幂等，重复请求返回同一 operation 或明确无操作结果；
- [x] 覆盖控制服务重启、node 崩溃、系统重启后的状态恢复测试。

**证据（2026-11-02）**：GPUI 的写操作统一改走权威 operation IPC，不再由 UI 命令总线直接执行；运行卡片展示 operation 状态、阶段、尝试次数、可取消性及阻止原因，失败时展示稳定错误码、可重试标记、恢复建议和可复制诊断 ID。GPUI 每 400ms 查询一次进度并设置 120 秒查询截止时间；查询超时只停止 UI 轮询并明确提示后台可能仍在运行，不终止 operation。幂等键仅在 operation 成功后释放，失败、取消、超时和查询超时均保留以便安全重试。WebUI 同步展示结构化 operation 卡片，按动作、网络和排序后的表单值复用幂等键；节点页在 running 时提供停止主操作与重启次操作，在 degraded/failed 时提供诊断主操作与重启次操作，control_unavailable 不再显示误导性主操作。新增端到端测试通过 `NodeCommand::StopNode` 停止全部数据面，断言快照进入 `Stopped` 且活动网络归零，同时 control、node IPC 进程、节点 IPC 快照和 WebUI TCP 监听均继续存活；新增冷启动测试先 SIGSTOP 再 SIGKILL control 与 node，确认 stale socket/lock 被保留后，以同一 runtime 目录直接启动新 control/node，验证 PID 替换、旧 IPC 可安全重建且无需手工删除文件，覆盖系统重启后的关键残留状态。无头 WebUI 路径在隔离 mount/user namespace 中隐藏桌面对话 socket 后执行，`w2_runtime_process` 11/11 通过；测试夹具现在按临时 runtime 环境精确回收被 UI 主动脱离的 control/node，失败或 panic 时也不再残留进程。`cargo test -p fuckipv6-client --all-features` 通过（lib 660 passed、3 ignored；`register_check_process` 1 passed；`w2_runtime_process` 11 passed；`lifecycle_cleanup_linux` 1 ignored）；`cargo test -p fuckipv6-common i18n` 9/9 通过；WebUI 8 个测试文件 64/64 通过，`typecheck`、`lint` 和生产构建通过（仅有现存 Vite 非 module i18n 提示与 >500 kB chunk 警告）；`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 和 `git diff --check` 通过，`dist/assets/app.js` 与 `app.css` 已同步刷新。

**验收**：用户只通过产品操作即可可靠启停节点；任何界面关闭、control/node 独立崩溃都不会产生第二份 runtime 或残留进程。

### W7-02 网络接口与 TUN `[~]`

- [x] 展示回环、物理、虚拟、TUN 接口及其地址、MTU、状态；
- [x] 区分系统接口、本产品创建接口和未知接口；
- [x] 展示 IPv4/IPv6 地址和默认路由归属；
- [x] 允许无敏感信息的基础接口诊断；
- [x] 允许有权限时刷新/重建 TUN，并走 operation 与审计；
- [x] TUN 创建失败、地址冲突、路由失败、DNS 失败分别可解释；
- [x] 平台不支持或权限不足时禁用动作并显示原因；
- [x] UI 不直接调用系统命令修改路由或 DNS。

**证据（2026-11-02）**：GPUI 与 WebUI 节点运行页均直接投影服务端 `NodeSnapshot.interfaces` 和 `interface_problems`，展示接口名称、类型、系统/产品/未知归属、up/down/unknown 状态、MTU、IPv4/IPv6 地址及 IPv4/IPv6 默认路由归属；中英文共享 i18n，诊断消息优先使用稳定 `message_key` 和参数本地化。新增 `RebuildTun` 领域命令和 manifest 动作，按网络校验本地所有者权限，经统一 operation 服务执行，operation kind 为 `TunRebuild`、目标为对应 Network，并记录 `node.tun.rebuild` 审计动作；UI 不直接运行系统网络命令。本轮新增 `NetworkSetupStage` 错误链标记，将 TUN 创建、地址冲突、路由、DNS、权限、平台、设备、工具及未知失败映射为稳定 `message_key`，公开命令响应不复制底层 stderr；Linux TUN/路由/DNS 配置失败改为 fail-closed，不再 warning 后伪装成功。`TunCapabilities` 由节点进程预检并进入权威 `NodeSnapshot`，缺失/旧数据默认不可重建；GPUI/WebUI 同时展示平台、支持状态、可重建状态和全部阻断原因，平台或权限不足时禁用入口。`RebuildTun` 在销毁现有会话前再次检查权威能力，不可重建时保持原会话运行，可重建时才走同一网络的停止、清理、重启流程。契约生成已增加 `TunCapabilities`、`TunPlatform` 和 `TunCapabilityReason`，Android DTO 清单与 fail-closed 枚举测试同步更新。验证通过：`cargo test -p fuckipv6-client --all-features`（lib 685 passed、3 ignored；`register_check_process` 1 passed；`w2_runtime_process` 11 passed；`lifecycle_cleanup_linux` 1 ignored）、WebUI 8 个测试文件 67/67、`cargo test -p fuckipv6-common i18n` 9/9、`./scripts/check-contracts.sh` 与 `cargo fmt --all -- --check`。本轮没有执行真实 TUN、路由、DNS 或断网环境测试，因此 W7-02 保持进行中，不宣称完整平台可用。

**验收**：接口状态来自服务端快照；所有修改可恢复、可审计；不存在“看起来已开启”但实际 TUN 未建立的伪状态。

### W7-03 流量路由摘要与入口 `[x]`

- [x] 展示当前默认策略：直连、进入网络、指定 Exit、拒绝；
- [x] 展示当前生效的固定 Exit 或网络路由目标；
- [x] 展示规则数量、最近一次应用版本和应用时间；
- [x] 提供进入 W11 流量路由页面的对象化入口；
- [x] 策略无效、陈旧或应用失败时显示阻断原因；
- [x] 不在此页复制完整规则编辑器。

**证据（2026-11-02）**：`NodeSnapshot.routing_summaries` 是节点运行页唯一的路由摘要来源，GPUI 与 WebUI 不从可编辑配置推导“已生效”状态。运行时 actor 在网络启动、路由热更新和角色更新成功后发布 `Current`，保留递增的应用版本与应用时间；策略校验失败发布 `Invalid`，热更新、发布或回滚失败发布 `ApplyFailed`，网络停止后将最后一次成功摘要标为 `Stale`。阻断状态保留最后一次成功版本/时间并附带稳定 `message_key`，无效策略不会触碰运行态，非运行网络更新也不会伪造摘要。两个界面均展示直连、进入网络、指定 Exit、拒绝、固定 Exit/网络目标、规则数、版本、时间及阻断原因；每行通过携带网络名的对象化入口进入 `routing` 页，不复制规则编辑器。运行时测试覆盖首次启动版本为 1、成功后递增、无效策略零副作用、停止转 `Stale`、发布失败转 `ApplyFailed` 及回滚；GPUI 定向投影/导航测试和 WebUI 路由摘要/对象导航测试通过。验证通过：`cargo fmt --all -- --check`、`cargo test -p fuckipv6-client --lib`（681 passed、3 ignored）、`cargo clippy -p fuckipv6-client --all-targets -- -D warnings`、`cargo check --workspace`、WebUI 8 个测试文件 68/68、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/check-contracts.sh`（Rust 7/7、TypeScript bindings、Java DTO 测试通过）。本轮没有执行真实 TUN、默认路由、DNS 或断网环境测试，不宣称数据面已在这些主机环境实网验证。

**验收**：节点页只呈现权威摘要，详情编辑只有一个入口，避免两处规则状态分叉。

### W7-04 身份与密钥 `[x]`

- [x] 展示节点名称、稳定 Node ID、公钥指纹和创建时间；
- [x] 支持安全重命名并立即反映到所有网络视图；
- [x] 支持密钥轮换的前置检查、确认、operation 和结果；
- [x] 重认证后再允许轮换或其他高危身份操作；
- [x] 明确轮换对成员关系、邀请和已建立会话的影响；
- [x] 不在 UI、日志或活动正文中显示私钥、Token 或完整敏感材料；
- [x] 提供本地恢复和 Passkey 管理入口，但不在此页复制完整安全设置。

**证据（2026-11-03）**：GPUI 与 WebUI 身份页均投影权威快照中的显示名称、稳定 Node ID、公钥指纹和创建时间；重命名继续走 `SetNodeDisplayName` 持久化命令并刷新同一快照，不建立第二份身份状态。新增 `RotateNodeIdentity` 领域命令、manifest 动作、确认文案和 `IdentityRotated` 公共结果；轮换只接受当前 43 字符无填充 Base64URL 且解码为 32 字节的指纹，服务端同时检查本地所有者、`ManageNetworks` 权限、WebUI 近期 Passkey 重认证和活动网络数为零，并由统一 operation 服务提供幂等重试与审计。轮换先写入仅含公开标识的私有 journal，再以原子方式替换密钥文件，最后迁移所有者记录、刷新身份创建时间并清理 journal；崩溃恢复会在任何身份消费者加载前完成迁移。Passkey 与本地恢复验证材料保留，全部 Web 所有者会话失效；UI 明确说明成员关系、邀请、既有会话和旧公钥信任可能需重建，并提供设置页的 Passkey/本地恢复入口和 `fuckipv6 recover-owner` 无公网接口说明。命令、事件、日志和活动仅包含 Node ID、指纹、计数和状态，不暴露私钥、Token 或完整敏感材料。WebUI 对轮换只发送一次带幂等键的 POST：成功时 HTTP 200 同时返回终态结果和 operation，超过等待窗口时 HTTP 202 返回真实非终态 operation；成功后在本地快照应用新 Node ID、指纹和创建时间，不轮询 operation 或触发会因会话失效而失败的刷新。验证通过：`cargo test -p fuckipv6-client --lib`（696 passed、3 ignored）、`cargo test -p fuckipv6-common i18n`（9/9）、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、WebUI 8 个测试文件 78/78、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/check-contracts.sh`（Rust 7/7、TypeScript bindings、Java DTO 测试通过）、`cargo fmt --all -- --check` 和 `git diff --check`。构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。

**验收**：身份信息可核验，轮换有明确影响说明和恢复路径，秘密不越过服务端安全边界。

### W7-05 诊断与恢复 `[x]`

- [x] 提供网络、DNS、TUN、Coordinator、Relay、Exit 的分层诊断；
- [x] 诊断每项展示开始时间、结果、耗时、错误码和修复建议；
- [x] 支持复制脱敏报告和诊断 ID；
- [x] 支持取消诊断并清理子进程与临时文件；
- [x] 区分“未测试”“通过”“失败”“不支持”“无权限”；
- [x] 不在页面常驻无价值的实时曲线或高频轮询；
- [x] 诊断失败不得静默修改系统配置；
- [x] 对恢复动作展示影响范围和确认要求。

**证据（2026-11-03）**：GPUI 与 WebUI 节点运行页按固定的 Network、DNS、TUN、Coordinator、Relay、Exit 六层顺序投影诊断结果；服务端缺失的层显式显示为“未测试”，每层展示开始时间、结果、耗时、稳定错误码、消息键和修复建议。诊断由统一 operation 服务执行，支持 HTTP 与本地 IPC 协作取消、硬超时和 120 秒 UI 查询截止时间；取消请求不会提前伪造终态，只有 handler 释放子进程、临时状态并返回权威结果后才进入 `cancelled`。结果包含诊断 ID、脱敏报告、恢复动作的影响范围及确认要求，以及会话、监听、peer、内嵌服务和残留网络摘要；复制入口只使用服务端生成的脱敏报告，诊断失败不会静默修改系统配置。WebUI 仅在活动 operation 期间轮询并支持取消，GPUI 仅对活动 operation 查询，不常驻实时曲线。契约新增固定诊断层、五态、检查项、恢复动作和扩展报告字段，并同步 TypeScript、JSON Schema 与 Android DTO/fail-closed 枚举测试。验证通过：`cargo test -p fuckipv6-client --all-features`（lib 722 passed、3 ignored；`register_check_process` 1 passed；`w2_runtime_process` 11 passed；`lifecycle_cleanup_linux` 1 ignored）、`cargo test -p fuckipv6-common i18n` 9/9、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、WebUI 8 个测试文件 83/83、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/check-contracts.sh`、`cargo fmt --all -- --check` 和 `git diff --check`。构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。

**验收**：用户能定位是节点、网络还是出口故障；诊断有硬超时、可取消、可脱敏导出且无残留。

### W7-06 节点页状态与权限矩阵 `[x]`

- [x] 加载、刷新、空、错误、陈旧、无权限、不支持、维护状态全部实现；
- [x] 禁用控件均显示本地或网络权限原因；
- [x] GPUI、WebUI 和适用时的 Android 使用同一状态/错误契约；
- [x] 窄屏采用纵向摘要和折叠详情；
- [x] 危险操作具备二次确认、影响说明和近期重认证；
- [x] 完成键盘、屏幕阅读器、焦点和对比度测试；
- [x] 页面不出现静态成功文案、假指标或不可执行按钮。

**证据（2026-11-03）**：节点 UI manifest 现在显式发布 13 个固定顺序页面状态和 12 个固定顺序动作禁用原因；Rust 校验、WebUI 运行时校验、TypeScript 生成类型和 Android fail-closed 枚举共同拒绝集合或顺序漂移。GPUI 的本地动作阻断原因统一投影到 manifest 的 `NodeUiActionBlockReason` 后再取文案，WebUI 直接使用生成的 `NodeUiPageState`/`NodeUiActionBlockReason` 闭集，并将 Exit 角色不支持统一映射为 `exit_node_unsupported`。危险命令的近期重认证策略收敛到 `NodeCommandKind::requires_recent_reauthentication()`，服务端授权与 manifest 确认声明共用该策略；WebUI 和 GPUI 在二次确认中展示操作影响及 WebUI Passkey 重认证要求，并为先前缺失的改名、残留清理加上可翻译确认文案。既有唯一主状态、动作/配置/操作禁用原因、窄屏纵向布局与折叠详情继续由 W5-03、W5-05、W6-07 的共享契约和测试覆盖；本轮重新执行无网络 Chromium 无障碍夹具，焦点陷阱、焦点恢复、菜单键/右键等价、Tabs ARIA 和能力文本状态均通过。验证通过：`cargo test -p fuckipv6-client --all-features`（lib 726 passed、3 ignored；`register_check_process` 1 passed；`w2_runtime_process` 11 passed；`lifecycle_cleanup_linux` 1 ignored）、`cargo test -p fuckipv6-common i18n` 9/9、WebUI 8 个测试文件 86/86、`npm run typecheck`、`npm run lint`、`npm run build`、`node scripts/test_webui_accessibility.mjs`、`./scripts/check-contracts.sh`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 和 `git diff --check`。构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。Android 当前仅完成生成契约和 fail-closed 解码验证，原生 Compose 页面行为仍由 W16 验收。

**验收**：节点页在成功、失败、降级和权限受限场景均有完整且一致的产品行为。

## 11. W8：网络创建与发现

目标：建立可验证身份、稳定网络 ID、签名公开资料和加入治理。发现索引不可用不得影响已加入网络的数据面。

### W8-01 创建网络与稳定 ID `[x]`

- [x] 定义 Network 领域模型、创建命令和 schema；
- [x] 网络具有稳定、不可伪造且可长期引用的 Network ID；
- [x] 名称、描述、图标、语言和公开资料字段完成校验；
- [x] 创建者成为首个 Owner/Administrator，并记录治理事件；
- [x] 创建过程幂等，网络创建成功后刷新本地网络列表；
- [x] 空名称、重复 ID、存储失败、权限失败有结构化错误；
- [x] 提供创建后继续邀请、配置策略和贡献能力的下一步。

**验收**：重复提交不会创建多个网络；网络 ID 在重启、改名和同步后保持稳定。

**证据（2026-11-03）**

- `NetworkRecord` 与 `NetworkCreateProfile` 已覆盖 owner、administrator、member、可见性、
  加入模式和公开资料；网络使用独立的 `network_identity.pem`，Network ID 从网络公钥派生，
  与节点身份分离。`rename_and_json_reload_preserve_network_id` 验证改名和 JSON 重载后 ID
  不变，`creates_owner_network_with_independent_identity_and_governance` 验证关闭并重新打开
  `ConfigManager` 后 ID 不变。
- 创建者写入为首个 Owner 和 Member，并生成带 `network.create` 动作的治理审计事件；
  网络记录、签名身份、治理日志和网络列表均持久化。重复名称、重复 ID、存储失败和回滚均有
  自动化测试，其中 Unix 身份文件权限为 `0600`。
- `first_submission_is_persisted_and_idempotent_reuse_dispatches_once` 验证同一幂等键只分派一次；
  `duplicate_network_name_is_a_structured_conflict_and_creates_nothing` 验证不同请求重复创建
  同名网络返回 `CreationConflict { reason: "duplicate_name" }` 且不产生第二条配置或服务。
- 创建成功后运行快照与 configuration snapshot 立即出现新网络；`auto_connect=false` 时状态为
  `Offline` 且 peer 数为 0。命令返回 `network_id`、`public_key`、`owner_role` 和
  `next_steps`（邀请成员、配置策略、贡献能力），WebUI 成功摘要会翻译并展示这些下一步。
- 空名称和非法的 purpose/icon/language/visibility/join_mode 返回字段级
  `validation_issues`；网络管理员调用创建命令在 handler 前被拒绝并记录拒绝审计。
  生成的 Rust/TypeScript/Android/schema 契约已同步，旧版创建载荷使用安全公开资料默认值。
- 验证：`cargo test -p fuckipv6-common`（174 passed）；`cargo test -p fuckipv6-client
  --all-features`（库 736 passed、3 ignored；register 进程 1 passed；运行时进程 11 passed）；
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`；
  `./scripts/check-contracts.sh`；WebUI 87 passed，typecheck、lint、build 均通过。

### W8-02 公开资料与隐私预览 `[x]`

- [x] 区分公开发现卡片、成员可见资料、管理员可见资料和本地秘密；
- [x] 创建/编辑时提供实际公开预览；
- [x] 公开字段不包含成员地址、私钥、内部拓扑和活动记录；
- [x] 支持语言、地区、标签、简介和能力声明；
- [x] 资料显式使用 schema version 和签名；
- [x] 变更采用差异预览、原子应用和审计；
- [x] 对误导性能力声明和过期资料显示警告。

**证据（2026-11-03）**：`NetworkPublicProfileDocument` 使用显式公开字段 allowlist、固定 `schema_version`，覆盖名称、简介、图标、语言、地区、标签、能力声明、可见性和加入模式；`NetworkPublicProfileRecord` 由网络独立身份签名，并在读取、预览和应用前校验网络 ID/公钥绑定、schema、revision、签名及有效期。预览统一生成公开发现卡片、成员视图、管理员视图和本地秘密四层投影：公开卡只携带可发现字段和 withheld 类别名，成员/管理员视图仅增加人数、revision 与策略版本，本地秘密仅报告身份是否存在、公钥指纹、存储类别以及 `secret_material_included=false`，不返回成员地址、私钥、内部拓扑、活动记录或邀请凭据。WebUI 与 GPUI 的创建/编辑资料动作均消费预览、差异、版本冲突和 `can_apply`；更新命令在排他文件锁内重新读取并复核 revision，重新签名后原子写入网络记录和治理审计，审计写入失败会恢复旧记录。运行时将能力声明与当前 CapabilityRegistry 对比，对未观测声明、未声明但已观测能力和旧版未签名资料给出警告，并对过期资料提示重新签发。字段级隐私测试枚举并拒绝成员、地址、端点、拓扑、活动、审计、私钥、签名密钥、身份和邀请字段；另有签名篡改/schema/revision 拒绝、四层预览、过期与冲突、原子保存和审计回滚测试。验证通过：`cargo test -p fuckipv6-common --all-features`（178 passed）、`cargo test -p fuckipv6-client --all-features`（库 743 passed、3 ignored；进程集成测试 12 passed；特权生命周期测试 1 ignored）、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、WebUI 8 个测试文件 92/92、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/generate-contracts.sh`、`./scripts/check-contracts.sh`（含 Rust、TypeScript、Android DTO 测试）、`cargo fmt --all -- --check` 和 `git diff --check`。构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。

**验收**：所有者能准确知道公网将看到什么；字段级隐私测试证明无越界泄漏。

### W8-03 签名发现索引 `[x]`

- [x] 定义可签名的 DiscoveryRecord，包含 Network ID、版本、发布者、过期时间和能力摘要；
- [x] 客户端在展示和加入前验证签名、版本、过期时间与吊销状态；
- [x] 索引记录单调更新，拒绝回放旧版本覆盖新版本；
- [x] 支持索引发布、更新、下线、重试和失败状态；
- [x] 索引服务器不可用时不阻塞本地网络启动和数据面；
- [x] 记录索引请求结果，但不记录不必要的用户搜索隐私；
- [x] 提供索引故障的降级状态和手动刷新；
- [x] 加入流程重新验证资料，而不是信任搜索结果缓存。

**证据（2026-11-03）**：`NetworkDiscoveryDocument` 使用独立 schema version，绑定 Network ID、网络公钥、所有者/发布者、单调 revision、签发/过期时间、规范化能力摘要和可选吊销时间；`NetworkDiscoveryRecord` 由网络身份签名，读取、刷新、展示和加入前均验证签名、版本、有效期、发布者绑定与吊销状态。索引状态覆盖 `idle/pending/ready/degraded/failed/unsupported`，持久化只包含公开签名记录、摘要、revision watermark 和稳定错误分类，不接收或记录搜索文本、邀请凭据和 ticket。相同 revision 仅接受字节摘要一致的幂等重试，旧 revision 和冲突 revision 被拒绝；已接受吊销 tombstone 后，任何非同一 tombstone 的后续记录均被拒绝。发布、更新、下线、刷新和重试均通过统一命令、本地所有者授权、操作审计和快照投影暴露；索引请求超时、离线、服务端错误、非法响应和存储错误不会停止或重建已加入网络，`data_plane_affected=false`。加入流程在 `runtime.join` 前重新验证调用方提供的签名 DiscoveryRecord，搜索缓存不能绕过验证。验证通过：`cargo test -p fuckipv6-common --all-features`（182 passed）、`cargo test -p fuckipv6-client --all-features`（库 749 passed、3 ignored；register 进程 1 passed；运行时进程 11 passed）、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、WebUI 8 个测试文件 92/92、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/check-contracts.sh`（含 Rust、TypeScript、Android DTO 测试）、`cargo fmt --all -- --check` 和 `git diff --check`。构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。

**验收**：伪造、过期、重放记录无法作为有效网络展示或加入；索引故障与已加入网络故障隔离。

### W8-04 搜索、筛选与网络详情 `[x]`

- [x] 支持按名称、标签、能力和加入模式搜索；
- [x] 支持公开/私有、开放/审批、语言、地区筛选；
- [x] 明确“发现结果”和“已加入网络”的区别；
- [x] 网络详情展示能力声明、加入模式、隐私摘要和最近验证时间；
- [x] 空结果、无更多结果、索引超时、部分结果和陈旧结果可区分；
- [x] 搜索输入防抖且不被写入永久活动日志；
- [x] 支持从发现结果进入申请或邀请流程；
- [x] 完成键盘和移动端等价操作。

**证据（2026-11-04）**：WebUI 与 GPUI 的 Networks 页面均将“已加入网络”和“发现结果”分成独立区域；即使没有已加入网络，发现结果仍可独立展示。搜索统一覆盖名称、简介、标签、能力摘要、加入模式和语言/地区文本，并支持公开/私有、开放/审批/邀请、语言和地区筛选。验证记录按 schema、时效、可发现性、吊销和摘要签名过滤，结果按名称精确度、名称和网络 ID 稳定排序；详情展示网络 ID、能力、标签、加入模式、隐私摘要、最近验证时间和陈旧原因。状态区分 `idle/pending/empty/no_matches/degraded/failed/unsupported`，部分索引不会误报为完整空结果，只有完整响应才显示“无更多结果”。WebUI 搜索本地防抖 250ms，GPUI 同样使用 250ms 本地防抖，二者均不发送命令、不上传、不持久化搜索文本或写入活动日志；WebUI 快捷入口与 GPUI 对齐为进入 Networks 并打开真实的 `refresh_discovery_index` 动作。发现卡 CTA 进入真实的开放加入流程；W8-05 已将其升级为先调用 `preview_join_network` 获取签名预览，再由用户确认条款后提交，而不是信任搜索结果缓存直接加入。WebUI 使用原生搜索框、筛选控件、可键盘展开的详情和响应式布局；GPUI 使用原生输入/按钮及同一断点布局。新增 GPUI 测试覆盖双区与验证详情、无已加入网络时发现结果仍显示、本地防抖/筛选且零命令、空/无匹配/失败/部分/陈旧状态区分，以及 CTA 打开签名预览且不直接提交。验证通过：`cargo test -p fuckipv6-common --all-features`（182 passed）、`cargo test -p fuckipv6-client --all-features`（库 761 passed、3 ignored；register 进程 1 passed；运行时进程 11 passed；特权生命周期测试 1 ignored）、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、WebUI 9 个测试文件 101/101、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/check-contracts.sh`（含 Rust、TypeScript、Android DTO 测试）、`cargo fmt --all -- --check` 和 `git diff --check`。Android Compose 等价界面属于 W16，不在本项重复实现。构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。

**验收**：搜索只展示可验证对象；无结果不会被误报为索引失败，陈旧结果有明确标识。

### W8-05 开放加入流程 `[x]`

- [x] 加入前展示网络身份、管理员、策略、Exit 贡献要求和隐私变化；
- [x] 用户可以查看本机将贡献的能力及可撤回方式；
- [x] 要求确认后提交 JoinRequest/直接加入 operation；
- [x] 服务端验证签名、策略版本、成员上限和重复加入；
- [x] 加入成功产生成员状态和活动，刷新所有相关视图；
- [x] 加入失败保留用户选择并提供可重试错误；
- [x] 退出网络不误删其他网络成员关系；
- [x] 覆盖并发加入、网络关闭加入、策略变化和版本竞争测试。

**证据（2026-11-04）**：WebUI 与 GPUI 均把发现结果接入公开加入流程；GPUI 的发现结果 CTA 先调用 `preview_join_network`，按 Network ID 取得只读签名预览，再打开真实的 `join_network` 动作。预览展示 Network ID、网络公钥、Owner 与管理员、发现/网络/隐私策略版本、成员数与上限、加入模式、可见性、Exit 贡献要求、隐私变化、网络声明能力、本机将贡献的能力、能力仍由本机控制且可随时撤回、公开端点、签发/过期时间和预览摘要；`already_member`、不可加入原因及条款确认状态有明确文案。用户必须勾选条款后才能确认，确认时把预览摘要、策略版本、隐私版本、条款哈希和确认时间填入内部命令字段；存在签名预览时隐藏通用 ticket 表单，避免绕过确认。无预览的直接 ticket/URL 加入仍保留兼容路径，但会先剥离内部确认字段并校验目标。运行时在 `join_gate` 下串行完成重新验证、成员上限检查、成员关系持久化和启动，避免并发 UI 提交竞争；`confirm_join_record` 重新校验签名发现记录、预览摘要、网络/隐私策略版本、条款哈希、加入模式、端点、成员上限、重复加入和 revision，网络关闭、满员、策略变化及预览后 revision 竞争均返回可恢复的具体错误。成功后写入 accepted membership 并刷新成员投影和活动；失败时保留预览、条款勾选和确认字段，可直接重试，且 GPUI 回归测试验证具体 `join.error.*` 文案和重试提示仍可见。退出使用稳定 Network ID 删除单条成员关系，`accepted_memberships_are_idempotent_and_removed_by_stable_id` 验证不会误删其他网络，`concurrent_membership_updates_do_not_overwrite_each_other` 验证并发写入不丢更新；WebUI `leaves the selected network without touching other memberships` 覆盖 UI 隔离。后端测试还包括 `join_preview_is_complete_read_only_and_digest_is_stable`、`join_confirmation_rejects_tampered_digest_and_terms`、`join_preview_blocks_closed_and_full_networks` 和 `join_confirmation_rejects_revision_race_after_preview`。验证通过：`cargo test -p fuckipv6-common --all-features`（182 passed）、`cargo test -p fuckipv6-client --all-features`（库 771 passed、3 ignored；register 进程 1 passed；运行时进程 11 passed；特权生命周期测试 1 ignored）、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、WebUI 9 个测试文件 107/107、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/check-contracts.sh`（含 Rust、TypeScript、Android DTO 测试）、`node scripts/test_webui_accessibility.mjs`、`cargo fmt --all -- --check` 和 `git diff --check`。构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。Android Compose 等价界面属于 W16，不在本项重复实现。

**验收**：用户清楚知道加入后发生什么；加入操作幂等，跨网络状态互不污染。

### W8-06 审批加入 `[x]`

- [x] 提供申请理由字段、可见范围说明和长度限制；
- [x] 管理员可查看待审批列表、申请人公开资料、理由和风险提示；
- [x] 支持批准、拒绝、要求补充信息和批量审批；
- [x] 审批前显示将授予的成员权限和 Exit/File 影响；
- [x] 并发审批只能产生一个最终成员状态；
- [x] 申请过期、撤回、管理员撤权时有一致状态；
- [x] 申请理由只对获授权角色可见，并纳入保留策略；
- [x] 所有审批动作有审计、活动投影和通知。

**证据（2026-11-04，审批命令与 UI 契约切片）**：加入申请状态机、服务端权限、幂等、
revision 冲突、审计和活动/通知后端语义已由前序提交落地；本轮把
`submit_join_request`、`list_join_requests`、`review_join_request`、
`batch_review_join_requests`、`withdraw_join_request` 和 `resubmit_join_request`
全部加入 `NodeCommandKind::ACTIONS`、UI manifest、生成的 Rust/TypeScript/Android/schema
契约与测试夹具。manifest 为每个动作声明了字段、权限、availability、确认和审计策略；
GPUI 的 44 项动作绑定与类型化命令构造覆盖六个动作，WebUI 的契约校验和命令构造也已覆盖
申请理由、审批动作、revision、说明、批量 JSON 与撤回/重申，并提供批准/拒绝/要求补充信息
的下拉选择及中英文标签。WebUI 校验与后端一致：申请 ID 为 `join_` 加 43 位 Base64URL，
理由最多 1000 字符，说明最多 500 字符，批量上限 100 条且禁止重复申请，批准不得附说明，
要求补充信息必须附说明。验证通过：
`cargo test -p fuckipv6-client --all-features --lib node::ui_manifest`（17 passed）、
`cargo test -p fuckipv6-client --all-features --lib gpui::view_model`（34 passed）、
`cargo test -p fuckipv6-common i18n`（9 passed）、WebUI 9 个测试文件 110/110、
`npm run typecheck`、`npm run lint`、`./scripts/check-contracts.sh`、
`cargo fmt --all -- --check` 和 `git diff --check`。

本轮同时完成待审批列表面：WebUI 与 GPUI 都保存 `list_join_requests` 的完整结果，并在切换
网络或重新打开动作时清除旧列表。列表展示申请人 display name、Node ID、公钥指纹、request ID、
revision、状态与过期时间，并明确说明理由仅对网络所有者和管理员可见；申请理由、补充信息要求、
授予角色、capabilities、Exit/File 影响与数据面访问风险均有独立提示，空列表也有明确状态。
WebUI 新增 `shows applicant details, reason visibility and approval risks` 与
`renders an explicit empty review list` 测试，GPUI 新增两条 visual-selector 回归测试；
同时修复 GPUI 六个加入申请动作的中文标签映射。

本轮完成审批操作表面：WebUI 与 GPUI 为 `pending` / `needs_information` 申请提供批准、
拒绝和要求补充信息按钮；WebUI 支持逐行复选、全选及批量批准/拒绝/要求补充信息，GPUI 同步
提供复选、全选和批量操作。确认对话框或引导卡片固定展示本次 request ID、revision 与动作，
成功响应原地更新列表中的状态和 revision 并清除对应选择；revision 冲突等拒绝结果显示在
操作表面内，可刷新后重试。批准不发送说明，拒绝可附可选说明，要求补充信息必须填写说明；
批量说明只复制到拒绝/要求补充信息的条目，不会污染批准记录。WebUI 在集成审批列表可用时
隐藏原始 `review_join_request` / `batch_review_join_requests` 表单，GPUI 引导卡片隐藏原始
request ID、revision、动作和 reviews 字段，避免用户绕过固定目标与 revision。回归测试包括
WebUI `reviews one request with the pinned revision and inline approval controls`、
`requires a note before requesting more information`、`batch approves selected requests and
updates their pinned revisions`，GPUI
`join_request_review_actions_pin_revisions_and_apply_review_results`，以及批量说明仅作用于
非批准动作的 `batch_join_request_note_applies_to_non_approval_actions_only`。验证通过：
`cargo test -p fuckipv6-client --all-features --lib gpui::view`（71 passed）、
`cargo test -p fuckipv6-common i18n`（9 passed）、WebUI 9 个测试文件 113/113、
`npm run typecheck`、`npm run lint`、`npm run build`、
`node scripts/test_webui_accessibility.mjs`、`./scripts/check-contracts.sh`、
`cargo fmt --all -- --check` 和 `git diff --check`；生成的 `app.js` / `app.css` 已同步刷新。
构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。

本轮补齐申请理由输入表面：WebUI 的提交/重申理由字段使用 1000 字符 HTML 长度上限，
展示“仅网络所有者和管理员可见”的隐私说明与实时 Unicode 字符计数，并通过
`aria-describedby` 关联帮助文本；GPUI 使用同一共享上限显示可见范围说明和实时计数。
WebUI 新增 `explains reason visibility and enforces its character limit`，GPUI 新增
`join_request_reason_editor_explains_visibility_and_limit`。验证通过：
`cargo test -p fuckipv6-client --all-features --lib gpui::view`（72 passed）、
`cargo test -p fuckipv6-common i18n`（9 passed）、WebUI 9 个测试文件 114/114、
`npm run typecheck`、`npm run lint`、`npm run build`、
`node scripts/test_webui_accessibility.mjs`、`./scripts/check-contracts.sh`、
`cargo fmt --all -- --check` 和 `git diff --check`。

本轮补齐审批前影响预览：WebUI 的单条与批量审批对话框会逐个展示将授予的 role、
capabilities、Exit 要求、File access 与数据面访问范围，并为影响区提供可访问名称；
GPUI 引导审批按勾选的 request ID 查找当前申请，在确认前展示同一组权限与影响信息。
WebUI 回归测试覆盖单条和批量审批的影响预览，GPUI 新增
`visual-join-request-review-grant-*` 选择器断言。验证通过：
`cargo test -p fuckipv6-client --all-features --lib gpui::view`（72 passed）、
`cargo test -p fuckipv6-common i18n`（9 passed）、WebUI 9 个测试文件 114/114、
`npm run typecheck`、`npm run lint`、`npm run build`、
`node scripts/test_webui_accessibility.mjs`、`./scripts/check-contracts.sh`、
`cargo fmt --all -- --check` 和 `git diff --check`；生成的 `app.js` / `app.css` 已同步刷新。
构建仅保留既有 Vite 非 module i18n 提示与大于 500 kB chunk 警告。

并发审批由请求文件锁、Coordinator 进程锁和网络 profile 锁共同串行化：审批先在候选请求账本
中校验 revision、更新状态并收集准入成员，再验证请求/名册投影，最后原子持久化；任何一步失败
都不会留下部分成员状态。回归测试
`concurrent_approval_produces_one_final_member_state` 使用两个线程同时批准同一申请，断言只有
一次审批成功且网络名册中该申请人恰好出现一次。验证通过：
`cargo test -p fuckipv6-coordinator --lib signaling::join_requests::tests::concurrent_approval_produces_one_final_member_state -- --exact`
（1 passed，83 filtered）。

本轮补齐生命周期一致性回归测试：申请到达过期时间后由列表事务持久化为 Expired，后续批准
被拒绝且不会写入成员；申请人撤回后状态为 Withdrawn，同样不产生成员关系；管理员被撤权后
立即从审批列表、审批授权和变更通知收件人中消失，未决申请保持 Pending，Owner 仍可正常审批。
该测试同时校验过期、撤回、撤权三个阶段及最终批准后的网络名册，确保请求账本和成员投影一致。
验证通过：`cargo test -p fuckipv6-coordinator --lib signaling::join_requests::tests`
（9 passed，76 filtered）、`cargo fmt --all -- --check` 和 `git diff --check`。

本轮补齐申请理由的授权可见性与保留期验证：Coordinator 存储按当前 Owner/管理员名册过滤完整
请求，申请人只能读取自己的申请，无关节点既不能通过列表也不能通过 request ID 读取理由；
`JoinRequest` 的 Debug 输出继续脱敏请求理由和管理员补充说明，变更通知仅携带 request ID、
状态与 revision。拒绝、撤回、过期和批准四种终态共用 30 天保留期限，存储读取事务到期后原子
擦除申请理由及补充说明，测试重新打开持久化文件后确认原文不可恢复。新增
`reason_visibility_and_retention_are_enforced_in_the_durable_store`，并扩展
`terminal_reason_is_retained_then_scrubbed_at_retention_deadline` 覆盖全部终态。验证通过：
`cargo test -p fuckipv6-coordinator --lib signaling::join_requests::tests`（10 passed）、
`cargo test -p fuckipv6-common --lib domain::membership::tests`（14 passed）、
`cargo fmt --all -- --check` 和 `git diff --check`。

本轮补齐全部加入申请动作的审计、活动投影和通知契约：Submit、Approve、Reject、
RequestInformation、BatchReview、Withdraw、Resubmit 七次写入均产生对应 canonical audit action，
命令入口的授权、成功和失败路径统一记录；写入活动按最新优先保留五条，公开审阅命令的拒绝、
补充信息和批量说明、重申理由均不进入审计或活动详情。变更通知仅携带 request ID、状态和
revision，并由持久化通知构造函数的序列化回归测试锁定；申请人和 Owner 始终收到通知，当前
管理员在名册内时收到、撤权后立即移除，无关节点不在收件人集合。新增
`join_request_audit_and_activity_cover_every_action_without_prose`、
`join_request_changed_notification_contains_only_identity_and_state`，并扩展
`expiry_withdrawal_and_admin_revocation_keep_state_and_roster_consistent`。验证通过：
`cargo test -p fuckipv6-client --all-features --lib node::command::tests::join_request_audit_and_activity_cover_every_action_without_prose`
（1 passed）、
`cargo test -p fuckipv6-coordinator --lib signaling::join_requests::tests`（10 passed）、
`cargo test -p fuckipv6-coordinator --lib signaling::server::tests::join_request_changed_notification_contains_only_identity_and_state`
（1 passed）、`cargo fmt --all -- --check` 和 `git diff --check`。

**验收**：审批状态机无重复成员和悬空申请；隐私范围与拒绝路径经过测试；所有审批动作的
审计、活动投影和通知收件人/载荷均有自动化证据。

### W8-07 私有网络邀请 `[x]`

- [x] 生成一次性、可过期、可撤销的邀请；
- [x] 邀请绑定网络、角色、能力和使用次数；
- [x] 邀请凭据不出现在 URL 查询、日志、活动正文或应用生成的截图内容中；
- [x] 支持复制安全邀请、撤销和状态查看；
- [x] 接受邀请前重新认证并展示权限变化；
- [x] 接受幂等，重放、过期和已撤销邀请被拒绝；
- [x] 邀请被管理员撤权时立即停止后续接受；
- [x] 不把邀请等同于永久成员身份。

GPUI 与 WebUI 已接通创建、列表、状态、撤销、预览和兑换六个邀请动作。创建成功后邀请码
只在一次性签发卡中本地展示供复制，notice、列表、状态刷新、活动、URL 和 Debug 输出均不含
明文码；创建卡本身可由用户主动截屏，这是复制凭据所必需的显式边界，应用不生成或持久化
邀请截图。列表和状态操作绑定当前 invitation revision，预览会固定 invitation revision、
preview digest 与 terms hash；兑换前展示角色、能力、File/数据面权限、Exit 义务、条款和近期
重新认证要求，并明确这是普通成员关系而非永久身份。expired、revoked、exhausted 和永久成员
状态均禁止确认。新增并通过 GPUI 测试
`invitation_creation_shows_the_code_once_and_never_puts_it_in_the_notice`、
`invitation_status_and_revoke_actions_pin_current_revisions`、
`invitation_preview_prefills_pinned_redemption_fields_and_requires_terms`、
`non_accepting_invitation_states_never_submit_redemption`。WebUI 邀请流程 7 个测试通过；
`npm test -- --run`（121 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`node ../scripts/test_webui_accessibility.mjs`、`./scripts/check-contracts.sh`、
`cargo test -p fuckipv6-client --all-features`（lib 811 passed，3 ignored）、
`cargo test -p fuckipv6-coordinator --lib signaling::invitations`（7 passed）和
`cargo clippy --workspace --all-targets --all-features -- -D warnings` 均通过。

**验收**：邀请可安全传递、审计和撤销；泄漏单个邀请不能越权加入其他网络。

### W8-08 强制 Exit 贡献知情 `[x]`

- [x] 网络可配置“加入即贡献 Exit”及生效范围；
- [x] 加入前明确展示出口用途、带宽影响、日志边界和关闭条件；
- [x] 节点本地 owner 始终保留最终启停权；
- [x] 管理员不能远程强制启用节点本地 Exit；
- [x] 用户拒绝本地贡献时显示可加入/不可加入的准确结果；
- [x] 策略变更时通知现有成员并获得必要确认；
- [x] 能力声明与实际启用状态分离；
- [x] 审计记录策略版本、同意/拒绝和生效状态。

**证据（2026-11-04）**：领域模型新增 `ExitContributionObligation`
（`required` / `not_required`）与 `ExitContributionScope`（`joining_members` /
`all_members` / `not_applicable`），并贯通创建网络、资料更新、公开资料、加入预览、
邀请预览、成员条款和网络记录；旧数据有兼容迁移和签名回归测试。加入与邀请预览固定展示
出口用途、带宽影响、日志边界和本地关闭条件，邀请兑换和加入提交均要求独立
`exit_contribution_acknowledged`，普通条款同意或能力声明不会隐式产生该同意。Required 网络在
本机不支持 Exit 时明确显示不可加入，NotRequired 网络仍可加入；网络策略不能远程启用本机
Exit，本地 owner 继续通过本机能力状态和启停动作保留最终控制权。AllMembers 策略变化会设置
成员待确认状态，新增 `acknowledge_membership_policy` 命令持久化接受或拒绝及精确策略版本；
GPUI 与 WebUI 均要求单独勾选 Exit 同意并在加入、邀请兑换和策略重确认时提交。快照和能力
注册表把“支持 Exit”与“Exit 当前启用”分开展示，Required/NotRequired 与实际本地降级状态
不会互相冒充。审计使用 typed allowlist 记录策略版本、接受/拒绝、是否生效和确认时间，不记录
邀请码、自由文本或敏感参数。新增并通过
`join_audit_separates_consent_policy_and_actual_local_exit_state`、
`declined_redemption_audit_keeps_typed_policy_facts_and_drops_sensitive_params`、
`policy_reconfirmation_audit_records_the_exact_acknowledged_version`、
`exit_contribution_requires_capability_and_explicit_consent`、
`policy_reconfirmation_requires_exit_consent_and_clears_pending_state`、
`required_invitation_exit_obligation_requires_separate_consent`、
`required_exit_without_local_support_is_explicitly_not_joinable`、
`required_exit_with_local_support_keeps_capability_and_enabled_state_separate`、
`optional_exit_join_remains_available_without_local_exit_support`、
`policy_changes_require_confirmation_only_for_all_members_and_pin_the_hash`、
`policy_observations_are_atomic_and_acknowledgements_persist` 和 WebUI
`requires a distinct Exit consent and blocks Required invitations when local Exit is unsupported`。
验证通过：`cargo test -p fuckipv6-common --all-features`（198 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（821 passed，3 ignored）、
`cargo test -p fuckipv6-coordinator --lib signaling::`（59 passed）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`./scripts/generate-contracts.sh`、`./scripts/check-contracts.sh`（Rust、TypeScript 和 Android DTO）、
WebUI `npm test -- --run`（124 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`node ../scripts/test_webui_accessibility.mjs`、`cargo fmt --all -- --check` 和
`git diff --check`。构建仅保留既有 Vite i18n 非 module 提示及大于 500 kB chunk 警告。

**验收**：不存在隐式贡献；网络声明与节点实际能力不一致时 UI 必须明确降级。

### W8-09 索引故障隔离 `[x]`

- [ ] 索引超时、TLS 错误、签名错误、部分损坏和版本不兼容分别处理；
- [ ] 缓存结果标注时间与来源，可手动刷新；
- [ ] 已有网络成员关系和 Coordinator 通信不依赖索引；
- [ ] 网络发布失败不删除本地网络；
- [ ] 重试有退避、上限和用户可停止；
- [ ] 索引异常不导致无限重连或电量/CPU 持续占用；
- [ ] 监控指标只包含脱敏聚合信息。

**验收**：索引完全离线时已加入网络仍能启动和运行；恢复后只做有限增量同步。

**证据（2026-11-04）**：索引错误已拆分为超时、TLS、签名、部分损坏、
版本不兼容、取消及离线/服务端等可操作类别；协议逐条解析记录，坏记录不再拖垮同一响应中的
有效记录，持久化 schema 不兼容也不会覆盖已有缓存。快照新增 `source`、`last_attempt_unix_ms`
和脱敏聚合 `metrics`（请求、重试、取消、有效/损坏记录与耗时），GPUI 与 WebUI 均展示来源、
时间和聚合指标，并保留手动刷新入口；取消操作会中止请求或退避并恢复上一份投影。刷新与发布
统一采用最多 3 次、250ms 起指数退避且上限 2s 的有限重试，协作端把刷新/重试登记为可取消
operation，状态更新仅替换 discovery 投影，成员关系、已加入网络和 Coordinator 运行路径保持独立；
发布失败不清除本地 `NetworkRecord`。新增并通过
`refresh_retries_timeout_with_bounded_attempts_and_aggregate_metrics`、
`tls_failures_are_categorized_and_retried_without_leaking_details`、
`signature_failures_are_terminal_and_never_replace_the_cache`、
`partial_corruption_keeps_valid_records_and_marks_degraded_cache`、
`persisted_incompatible_schema_is_reported_without_overwriting_cache`、
`retry_backoff_is_exponential_and_capped`、
`cancelled_retry_restores_projection_and_stops_further_attempts`、
`offline_index_preserves_joined_network_and_membership_projections` 和
`publication_failure_preserves_local_network_record`。验证通过：
`cargo test -p fuckipv6-common --all-features`（198 passed）、
`cargo test -p fuckipv6-client --all-features --lib discovery_index`（20 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（821 passed，3 ignored）、
`cargo test -p fuckipv6-coordinator --lib signaling::`（59 passed）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`./scripts/generate-contracts.sh`、`./scripts/check-contracts.sh`、
WebUI `npm run typecheck`、`npm test -- --run`（124 passed）、`npm run lint`、`npm run build`、
`node ../scripts/test_webui_accessibility.mjs`、`cargo fmt --all -- --check` 和
`git diff --check`。构建仅保留既有 Vite i18n 非 module 提示及大于 500 kB chunk 警告。

### W8-10 网络发现测试与活动 `[x]`

- [x] 建立签名、重放、伪造、过期、吊销和索引离线测试；
- [x] 建立开放、审批、私有邀请三条 E2E；
- [x] 建立加入竞争、成员上限和策略变化测试；
- [x] 验证公开卡片不含敏感字段；
- [x] 验证所有创建、发布、加入、审批、邀请动作可追溯；
- [x] GPUI/WebUI 完成同一流程和行为对比；
- [x] 移动端完成长按、键盘替代和返回路径；
- [x] 所有失败都有可执行恢复动作。

**验收**：网络创建与发现形成完整垂直切片，不依赖静态列表、假审批结果或未签名的公开数据。

**证据（2026-11-04）**：`join_acceptance_rejects_forgery_expiry_revocation_replay_and_offline_index`
覆盖签名、重放、伪造、过期、吊销和索引离线；`open_join_end_to_end_uses_signed_discovery_preview_and_confirmation`、
`approval_join_end_to_end_submits_reviews_and_admits_the_applicant` 和
`private_invitation_end_to_end_previews_redeems_and_admits_the_invitee` 覆盖三条加入 E2E；
`invitation_join_rechecks_member_limit_and_capability_policy` 与既有并发审批测试覆盖成员上限和策略变化；
`public_discovery_card_serializes_only_the_signed_allowlist` 校验公开卡片仅包含签名公开字段。
命令总线新增 `network_discovery_actions_have_traceable_actor_action_and_operation_ids`，在同一声明来源和
owner 角色下验证创建、发布、加入、审批、邀请分别产生稳定的 `network.create`、
`network.discovery.publish`、`network.join`、`network.join_request.review`、
`network.invitation.create` 动作，并核对 actor、request、受信任 operation ID、结果与脱敏活动；
邀请票据和审批说明不会进入审计或活动投影。
`contracts/typescript/fixtures/discovery-ui-parity.json` 作为 GPUI 与 WebUI 共用的发现流程夹具，
依次覆盖创建、发布、加入预览、确认加入、审批、创建邀请、邀请预览、兑换邀请和索引重试；
`discovery_flow_parity_fixture_matches_gpui_command_building` 与
`matches the shared discovery flow fixture` 分别校验 manifest 动作/命令、全部表单字段、确认预览派生字段
和最终序列化命令完全一致。夹具只使用邀请票据占位符，测试运行时生成临时值；WebUI 校验同时支持
后端实际的 `inv_` 邀请 ID 与 `hash_` 内容摘要格式。
移动端交互已补齐：已加入网络卡片提供可见、可聚焦的动作菜单按钮，Enter/Space 与右键共用
“导航 → 分隔线 → 危险操作”菜单；触摸长按 450 ms 打开底部 Action Sheet，滚动、指针移出或
超过 10 px 位移会取消，浏览器返回/Escape 关闭后焦点回到菜单按钮。GPUI 的已加入网络卡片提供
同构可聚焦动作入口，并覆盖网络上下文导航与离开网络动作。
失败恢复已补齐：GPUI 与 WebUI 的 operation 终态可重试失败显示重试动作，WebUI 以原命令和参数重发并
复用同一提交幂等键；加入与邀请预览提供“重新预览”，不可接受邀请可返回编辑且保留源输入；发现索引
失败按钮直接提交 `retry_discovery_index`，无可用动作的页面仍渲染操作状态卡以保留恢复入口。
`operation_card_offers_retry_only_for_terminal_retryable_failures`、
`keeps an unconfirmed preview available when the final join is rejected`、
`returning_to_invitation_edit_preserves_source_inputs`、`shows structured failure details and copies the diagnostic id`
和 `distinguishes discovery failure from an empty result and exposes retry` 覆盖上述恢复路径。
命令：`cargo test -p fuckipv6-client --all-features --lib gpui::view_model::tests`（36 passed）、
`cargo test -p fuckipv6-client --all-features --lib gpui::view::tests`（44 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（839 passed，3 ignored）、
WebUI `npm test -- --run`（129 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`node ../scripts/test_webui_accessibility.mjs`、`cargo fmt --all -- --check` 和 `git diff --check`。

## 12. W9：成员、组与拓扑

目标：网络治理对象与拓扑共享同一权威数据、筛选和选择；拓扑是操作入口，不是只读装饰图。

### W9-01 成员列表与权威状态 `[x]`

- [x] 展示成员名称、身份指纹、角色、组、能力、在线状态和加入时间；
- [x] 成员状态覆盖申请中、已批准、活跃、降级、暂停、退出、移除、过期；
- [x] 在线状态注明新鲜度和来源，不把未知显示为离线；
- [x] 支持按角色、组、能力和健康筛选；
- [x] 列表虚拟化并保持选择稳定；
- [x] 点击对象打开统一详情面板，不复制页面；
- [x] 所有可见字段经过隐私策略过滤；
- [x] 支持导出当前视图的脱敏清单。

**验收**：成员列表与事件/快照最终一致；权限外字段不会被前端隐藏后仍下发。

**证据（2026-11-04）**：新增服务端权威 `NetworkMemberDirectory` 投影及脱敏成员 DTO，统一生成名称、
身份指纹、角色、组、能力、状态、presence、观测来源/时间、新鲜度和加入时间；加入时间按
`approved_at_unix_ms`、兑换记录、Owner 创建时间依次取权威值，未知时不伪造。DTO 与 GPUI/WebUI
覆盖申请中、已批准、活跃、降级、暂停、退出、移除、过期八种状态，但投影只输出当前数据源能够证明的
状态，不会从缺失记录推断降级、暂停或移除；能力用 `null` 表示未知、`[]` 表示明确无能力。
presence 仅由当前网络的 session 观测生成，未知状态显示“未知”并附带来源、观测时间和新鲜度，绝不
降级显示为离线。`members.rs` 在权限判定前完成过滤，普通成员只能看到自己的待审申请，审批者才获得
完整目录；邀请码、摘要、审批理由、端点、传输信息与公钥原文均不进入响应。GPUI 与 WebUI 共用相同的
角色、组、能力、健康筛选语义，使用虚拟列表、单一共享详情面板和稳定的 NodeId 选择，并支持将当前筛选
视图导出为仅含 DTO allowlist 字段的 TSV；WebUI 在网络切换、刷新、折叠和重新请求时清理失效目录与选择，
GPUI 仅在成员关系返回真实 `network_id` 时暴露入口。新增并通过
`projection_omits_sensitive_material_and_preserves_unknown_presence_and_times`、
`non_member_and_missing_roster_fail_closed`、
`member_directory_requests_hide_other_applicants_from_ordinary_members`、
`member_directory_redemptions_are_permission_filtered_and_never_expose_ticket_material`、
`member_directory_filters_are_stable_and_distinguish_unknown_capabilities`、
`member_directory_health_filters_cover_statuses_and_unknown_presence`、
`member_directory_selection_survives_refresh_only_while_member_exists`、
`member_directory_virtualizes_rows_and_opens_one_shared_detail_panel`、
`member_directory_exports_only_the_sanitized_filtered_view`、WebUI `members.test.ts`（7 passed）和
`member-directory.test.tsx`（3 passed）。验证通过：
`cargo test -p fuckipv6-common --all-features`（199 passed）、
`cargo test -p fuckipv6-coordinator --all-features`（105 passed，另 2 个 signaling flow）、
`cargo test -p fuckipv6-client --all-features --lib`（846 passed，3 ignored）、
WebUI `npm test -- --run`（145 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`node ../scripts/test_webui_accessibility.mjs`、
`./scripts/generate-contracts.sh`、`./scripts/check-contracts.sh`、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`cargo fmt --all -- --check` 和 `git diff --check`。

### W9-02 邀请、审批与移除 `[x]`

- [x] 统一成员生命周期操作及允许前置状态；
- [x] 邀请、批准、拒绝、暂停、恢复、退出、移除均使用 operation；
- [x] 移除前展示网络访问、File 权限和数据影响；
- [x] 移除后立即撤销会话、授权和未完成传输；
- [x] 支持撤销误操作时的安全恢复路径，不复活泄漏凭据；
- [x] 管理员不能移除 Owner 或绕过保护阈值；
- [x] 普通成员不能操作其他成员；
- [x] 所有操作幂等并产生活动与审计。

**验收**：任何治理操作都有清晰状态转移、权限校验、撤销边界和并发测试。

**证据（2026-11-05）**：`lifecycle_transition_table_covers_every_governance_action` 与
`illegal_lifecycle_predecessors_fail_closed` 覆盖邀请、批准、拒绝、暂停、恢复、退出、移除和恢复到待审批的全部合法/非法前置状态；
`signaling::member_lifecycle::tests`（8 passed）在 Coordinator 侧逐项验证目标功能：

- `every_lifecycle_action_uses_the_same_operation_and_audit_ledger`：邀请、批准、拒绝、暂停、恢复、退出、移除和撤销移除均产生
  同一 operation/audit 台账记录，且复用 idempotency key 时不会重复写入；
- `removal_preview_discloses_revocations_without_deleting_files_or_reviving_credentials`：移除预览披露网络访问、File 权限、
  未完成传输和凭据影响，明确本地文件保留、旧凭据不恢复；
- `last_administrator_and_owner_cannot_be_removed`：Owner 与最后一名管理员受保护，无法被移除或降级绕过阈值；
- `ordinary_members_cannot_operate_on_other_members_and_owner_is_protected`：普通成员不能对他人执行治理操作；
- `concurrent_mutations_use_cas_and_only_one_wins`：生命周期存储以 CAS revision 串行化并发治理；
- `idempotency_key_reuse_with_different_payload_is_rejected`：同一幂等键复用不同请求内容会被拒绝；
- `lifecycle_mutations_are_authorized_and_idempotent`：重复提交同一请求返回幂等结果并保持审计一致；
- `remove_undo_requires_reapproval_and_never_restores_old_epoch`：撤销误移除只会回到待审批并递增访问世代，旧凭据不复活。

**移除后撤销边界**：Coordinator 在拒绝、暂停、退出和移除时断开目标节点会话
（`signaling/session.rs::disconnect_node_sessions` 由 `signaling/server.rs` 调用），并把成员访问世代递增；
`crates/common/src/domain/file_share.rs` 的 `resume_is_bound_to_identity_share_revision_and_file_version` 验证当前世代或请求世代不匹配时
`resume()` 返回 `PermissionRevoked` 并把传输置为 `Cancelled`。当前尚未建立独立传输注册表或主动取消枚举，
未完成传输由 access epoch 失效 fail closed，不能继续续传。

**WebUI 证据**：`member-directory.test.tsx`（8 passed）覆盖治理区仅在 `can_manage_members` 时展示、可执行动作完全取自服务端
`available_actions`、移除前先调用 `preview_member_removal` 并展示完整影响、移除提交携带 `target_node_id`/`role`/`expected_revision`、
全部非移除动作直接走 operation、拒绝预览时禁用确认以及服务端拒绝不关闭对话框；`action-panel.test.tsx`（30 passed）覆盖
`preview_member_removal`/`change_member_lifecycle` 动作卡、生命周期结果不关闭动作卡以及成功后按 `network_id` 刷新目录；
`invariants.test.ts`（25 passed）覆盖新命令 payload、预览/变更 outcome 的 fail-closed 校验与非法动作拒绝。

**验证通过**：`cargo test -p fuckipv6-common --all-features`（206 passed）、
`cargo test -p fuckipv6-coordinator --all-features`（117 passed，另 2 个 signaling flow）、
`cargo test -p fuckipv6-client --all-features --lib`（846 passed，3 ignored）、
WebUI `npm test -- --run`（155 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`node ../scripts/test_webui_accessibility.mjs`、`./scripts/check-contracts.sh`、`./scripts/generate-contracts.sh`、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check` 和 `git diff --check`。

### W9-03 自定义组与动态组 `[x]`

- [x] 建立静态组、动态查询组和保留系统角色；
- [x] 组表达式使用结构化字段，不执行任意脚本；
- [x] 支持组嵌套、循环检测和深度限制；
- [x] 支持预览成员集合、差异和权限影响；
- [x] 组变更原子应用并可回滚；
- [x] 动态组成员变化只影响授权，不隐式修改成员身份；
- [x] 删除组前处理引用、空组和并发编辑；
- [x] 拒绝优先于任何组允许。

**验收**：组求值确定、可解释、可测试；不存在循环、隐式提权或陈旧成员授权。

**证据（2026-11-05）**：`crates/common/src/domain/groups.rs` 新增版本化 `GroupCatalog`
（`network_id` + 单调 `revision` + 组集合）与确定性求值器，`crates/common/src/domain/permissions.rs`
把 `GroupKind` 扩展为 `Manual`/`Dynamic`/`Nested`，`DynamicGroupSelector` 增加能力选择字段并加
`deny_unknown_fields`，`GroupDefinition::validate` 下沉为组定义自校验。`domain::groups::tests`
（11 passed）逐项覆盖：

- `manual_dynamic_and_nested_groups_resolve_deterministically`：静态组按 `NodeId` 过滤花名册、动态组按
  角色/节点/能力求值、嵌套组返回去重且有序的并集，并对每个节点记录贡献来源组（`via`），同一目录重复求值结果完全一致；
- `reserved_system_role_names_cannot_be_shadowed_by_custom_groups`：`owner`/`administrator`/`member`
  为保留系统角色名，大小写与首尾空白归一化后仍被拒绝，组名大小写不敏感唯一；
- `group_expressions_are_structured_fields_and_never_scripts`：任意 `script`/`expression` 载荷在反序列化边界即失败，
  空的动态选择器不会静默放行或匹配全体，只能使用结构化字段；
- `nested_group_cycles_depth_and_unknown_references_are_rejected`：自引用与双组循环报 `domain.group_cycle`，
  悬空引用报 `domain.group_unknown_reference`，8 层内允许、9 层报 `domain.group_depth_exceeded`，
  单网络超过 256 组被拒绝；
- `preview_reports_member_sets_diffs_and_permission_impact`：预览给出基线与候选 revision、增删改组集合、
  每个组的成员增删集合以及逐成员权限影响（`AccessDecisionView`），预览不改变线上 revision 与求值结果；
- `group_changes_apply_atomically_and_roll_back`：一次变更要么整体替换、要么完全不生效，回滚 token 在校验
  `network_id`/revision/内容摘要后恢复上一版，任意后续版本落地后回滚报 `domain.group_stale_revision`；
- `dynamic_membership_changes_only_affect_authorization`：动态组选择器改变只移动授权集合，`identity_changes == 0`
  且花名册（身份、角色、启用状态）完全不变；
- `group_deletion_requires_reference_cleanup_and_handles_empty_groups`：被嵌套组或权限规则引用的组删除时
  返回 `blocking_references` 并以 `domain.group_change_blocked` 拒绝，替换组只重写嵌套引用不重写策略规则，
  空组在解除引用后可删除，陈旧 `expected_revision` 报 `domain.group_stale_revision`；
- `deny_rules_outrank_any_group_allow_and_the_role_ceiling`：组允许无法覆盖任一层拒绝（`permission.denied`），
  也无法突破角色上限（`permission.role_ceiling_exceeded`）；
- `roster_and_selector_inputs_reject_duplicates`、`catalog_rejects_groups_from_another_network`：
  花名册、选择器、嵌套引用拒绝重复项，跨网络组与零 revision 目录被拒绝。

**验证通过**：`cargo test -p fuckipv6-common --all-features`（217 passed）、
`cargo test -p fuckipv6-coordinator --all-features`（117 passed，另 2 个 signaling flow）、
`cargo test -p fuckipv6-client --all-features --lib`（846 passed，3 ignored）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`cargo fmt --all -- --check` 和 `git diff --check`。

**范围说明**：本项交付领域层的组目录、求值与变更契约；Coordinator 存储、节点契约动作与 WebUI 组管理界面
尚未接线，后续拓扑/批量操作项复用同一 `GroupCatalog`，不会引入第二套状态机。

### W9-04 拓扑数据与聚合 `[x]`

- [x] 建立节点、网络边界、连接和能力的单一拓扑模型；
- [x] 服务端按隐私策略返回最小必要节点集合；
- [x] 同一节点多网络聚合为单个对象；
- [x] 连接去重并保留所属网络关系；
- [x] 支持稳定布局、增量更新和对象身份保持；
- [x] 对超大网络采用聚合、分层或按需展开；
- [x] 拓扑和列表共享搜索、筛选、选择；
- [x] 不把陈旧连接绘制为当前活跃链路。

**验收**：同一节点只绘制一次；多网络关系通过边界和详情表达；增量刷新不跳变或丢失选择。

**证据（2026-11-05）**：`crates/common/src/domain/topology.rs` 新增唯一拓扑模型 `TopologySnapshot`
（`revision`、`generated_at_unix_ms`、`viewer`、`truncated`、`boundaries`、`nodes`、`connections`、
`clusters`），由 `TopologyBuilder` 从 `TopologyNetworkInput`/`TopologyMemberInput`/`TopologyLinkObservation`
投影生成；节点能力用 `CapabilityKind`、健康用 `TopologyNodeHealth`（`Healthy`/`Unknown`/`Degraded`/
`Unreachable`，取最严重者）、连接状态用 `TopologyConnectionState`（`Active`/`Idle`/`Stale`）。
`domain::topology::tests`（10 passed）逐项覆盖：

- `nodes_are_aggregated_across_networks_exactly_once`：同一 `NodeId` 出现在多个网络时只产生一个
  `TopologyNode`，`networks` 按 `network_id` 排序并逐网络保留角色、状态、能力与健康，聚合后的能力为各网络并集，
  健康取最严重等级，`node_ids()` 长度等于去重节点数；
- `privacy_projection_omits_invisible_networks_and_their_nodes`：不在 `viewer.visible_networks` 内的网络
  不产生边界、节点或连接，也不贡献任何计数，`boundary()` 对不可见网络返回 `None`；
- `connections_are_deduplicated_and_keep_network_relations`：连接按无序节点对归一（`from`/`to` 排序），
  `connection_id` 为 `ContentHash("from|to")`，同一条连接在多个网络被观测到时只出现一次并保留全部网络 id；
- `stale_observations_are_never_reported_as_active`：观测时间距 `generated_at_unix_ms` 超过
  `CONNECTION_STALE_AFTER_MS`（60s）的链路为 `Stale`，`active=false` 的新鲜链路为 `Idle`，
  `active_connections()` 只返回非陈旧且仍在活跃的链路；陈旧链路不会因为曾经活跃而继续显示为活跃；
- `layout_is_deterministic_and_preserves_object_identity`：`layout()` 对同一快照与视口给出完全相同的锚点/槽位/坐标，
  `layout_preserving()` 复用旧布局中仍存在对象的锚点、槽位与坐标，只为新对象分配空槽，因此刷新不跳变；
  锚点按 `ANCHOR_SLOTS`（12 个/环）排布、节点按 `NODE_SLOTS_PER_RING`（16 个/环）围绕锚点排布并输出整数坐标，
  多网络节点只归属其排序后第一个网络的锚点，因此仍只绘制一次；
- `large_networks_are_aggregated_and_expandable`：单网络可见成员超过 `MAX_VISIBLE_MEMBERS_PER_NETWORK`（64）
  时按 `NodeId` 排序截断并生成 `TopologyCluster`（`hidden_member_count`、3 个 `sample`、`expandable`），
  `truncated=true`，`expand_network()` 后该网络全量展开且集群消失；`TopologyBoundary` 同时给出权威
  `member_count` 与 `visible_member_count`；
- `topology_and_list_share_search_filter_and_selection`：`query()` 复用与成员列表相同的筛选维度
  （名称包含、`NodeId` 前缀、角色、能力、健康、`network_id`），`retain_selection()` 仅在节点仍存在时保留选择、
  节点消失后清空，因此筛选变化与增量刷新后选择行为确定；
- `diff_reports_incremental_changes_without_duplicating_nodes`：`diff()` 按 `NodeId`/`connection_id`/
  `network_id` 输出新增、更新、移除的节点、连接与边界，并给出 `unchanged_node_count`，未变化对象不重复出现在
  增量结果中；
- `non_members_are_not_projected_and_duplicates_are_rejected`：非成员（邀请中/已移除等）身份不投影为节点，
  同一网络内重复 `NodeId` 直接报领域不变量错误而不是静默去重；
- `snapshot_round_trips_through_serde`：快照可经 JSON 序列化/反序列化往返保持等价，契约可直接下发到界面层。

**验证通过**：`cargo test -p fuckipv6-common --all-features`（227 passed）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`cargo fmt --all -- --check` 和 `git diff --check`。

**范围说明**：本项交付领域层的拓扑投影、聚合、布局、筛选与增量差异契约；Coordinator 实时链路观测接入和
WebUI 画布替换在 W9-05/W9-06 复用同一 `TopologySnapshot`，不另建拓扑状态。

### W9-05 多网络边界与视觉编码 `[x]`

- [x] 每个网络使用可区分颜色的虚线边界；
- [x] 网络颜色不覆盖节点能力和健康色；
- [x] 节点能力采用多色分段圆环表达 Coordinator/Relay/Exit/File；
- [x] 健康、异常和未知使用独立角标，不仅依赖颜色；
- [x] 网络重叠、嵌套和公共节点有清晰表达；
- [x] 提供可切换的传统列表等价视图；
- [x] 图例可折叠且与实际编码一致；
- [x] 高对比度和色弱模式完成验证。

**验收**：用户无需靠猜测区分网络、能力和健康；所有视觉编码有文本/形状等价信息。

**证据（2026-11-06）**：WebUI 拓扑页已从占位的 `snapshot.networks`/`snapshot.peers` 圆形示意替换为
`node.topology` 的单一 `TopologySnapshot` 渲染，不生成假节点或静态连接。`webui/src/topology.ts` 提供
确定性的网络颜色与虚线组合、固定顺序的四段能力环、健康形状/字符编码、稳定布局、等价列表行和共用图例
数据；`webui/src/components/topology-view.tsx` 将网络编码限制在边界与节点外圈，把能力保留在分段圆环，
把健康保留在带形状和字符的角标，因此三类语义不争用同一颜色通道。公共节点只绘制一次，徽标显示 `×网络数`，
ARIA 名称和列表列出全部网络归属；超大网络的未展开成员以 `+N` 集群角标解释。图形视图可切换到包含网络、
角色、能力、健康和 active/idle/stale 连接计数的传统表格；图例默认折叠，直接从画布同源函数生成。
“色弱友好配色”切换专用 Okabe-Ito 色板，“高对比度”提高边界、节点、文字和连接对比与线宽；
`topology-view.test.tsx`（9 tests）覆盖网络虚线、三类视觉通道隔离、四种能力分段、四种健康形状/字符、
公共节点去重、列表等价、图例同步、色弱/高对比切换和快照失败/空状态，PageContent 集成测试确认只消费
`snapshot.topology`。

**验证通过**：WebUI `npm test -- --run`（164 passed）、`npm run typecheck`、`npm run lint`、
`npm run build`、`node ../scripts/test_webui_accessibility.mjs`、
`cargo test -p fuckipv6-common --all-features i18n`（9 passed）和 `git diff --check`。

### W9-06 悬停、右键与批量操作 `[~]`

- [x] 悬停展示名称、角色、能力和简要健康；
- [x] 右键/菜单键打开对象操作菜单；
- [ ] 选择多个节点后提供权限允许的批量邀请、分组或策略操作；
- [x] 批量操作前显示对象数量、失败策略和权限差异；
- [x] 操作结果逐项展示成功、跳过和失败原因；
- [x] 取消选择、筛选变化和刷新后选择行为明确；
- [x] 危险批量操作要求确认和近期重认证；
- [ ] 所有结果写入活动与审计。

**验收**：拓扑操作与列表一致；批量失败不会被总成功提示掩盖。

**证据（2026-11-06）**：拓扑页接入服务端权威 `list_network_members` 目录，批量计划只从
`can_manage_members`、`viewer_role` 和每个成员自己的 `available_actions` 生成；`viewer_role=member`
或目录未授权时不渲染生命周期动作，也不提交伪命令。当前可安全批量执行的是已有成员生命周期
`approve/reject/suspend/resume/remove/undo_remove`；当前命令清单没有批量邀请、组成员变更或安全策略命令，
因此不以可点击伪按钮声称覆盖这些能力，第 3 项保持未完成。移除前逐成员调用
`preview_member_removal`，被服务端拒绝的成员记为该批量中的 `skipped` 且绝不提交生命周期命令；
其余动作逐成员使用目录中的 revision，避免全批共用一个过期版本。执行前对话框显示选中数、可执行数、
跳过数、失败继续策略、危险确认和近期认证要求；服务端要求重新认证时，`App.tsx` 通过 Passkey 重认证后
以原命令和参数重发。结果逐项区分成功、跳过和失败并披露服务端返回的真实 `audit_id`，混合失败不会被
总成功摘要掩盖；跳过和未提交项不会伪造审计编号，批量级活动/审计汇总仍待后端命令支持，因此第 8 项
保持未完成。

悬停卡展示名称、当前网络角色、能力、健康和 presence；右键、`Shift+F10` 与菜单键打开同一对象菜单。
取消选择、筛选变化和网络切换会清空选择并给出原因提示；实时刷新只保留仍存在的节点身份，消失后再次
出现的节点不会复活旧选择。`topology-page.test.tsx`（7 tests）覆盖悬停、右键/`Shift+F10`、筛选清空、
刷新保留/永久移除、无管理权限、服务端移除预检拒绝、Bob 成功/Carol 拒绝的混合结果及逐成员 revision；
`topology-actions.test.ts`（4 tests）覆盖共享筛选、选择保留、权限差异和目录整体禁止管理。

**验证通过**：WebUI `npm test -- --run`（15 files，175 passed）、`npm run typecheck`、`npm run lint`、
`npm run build`、`node ../scripts/test_webui_accessibility.mjs`、
`cargo test -p fuckipv6-common --all-features i18n`（9 passed）和 `git diff --check`。

### W9-07 键盘、触控与无障碍 `[x]`

- [x] 拓扑支持方向键移动、Enter 打开、菜单键操作；
- [x] 提供可搜索的对象列表作为等价导航；
- [x] 窄屏默认列表，长按等价右键菜单；
- [x] 冻结/缩放/定位当前节点可仅用键盘完成；
- [x] 焦点移动不因实时刷新丢失；
- [x] 屏幕阅读器获得对象名称、能力和关系摘要；
- [x] 减少动效设置生效；
- [x] 完成无鼠标 E2E。

**验收**：拓扑不是必须使用鼠标的孤岛，所有治理操作有键盘/触控等价路径。

**证据（2026-11-06）**：图节点和列表行使用 roving tabindex，支持方向键循环、Home/End、
Enter 打开详情、Space 选择、Shift+F10/ContextMenu 打开菜单；菜单支持方向键、Home/End、Escape
恢复焦点和 Enter 执行。窄屏首次进入默认列表，显式切换后尊重用户选择；触控长按 450ms 打开同一
菜单，移动超过 8px、滚动或取消会中止，长按后的合成 click 被抑制。缩放、重置、冻结和定位当前
节点均可仅用键盘完成，冻结固定完整拓扑快照；实时刷新保留仍存在对象的焦点，对象消失时转移到
自身或回退对象。搜索框通过 `aria-controls` 关联列表，筛选结果为 live region；节点无障碍名称包含
名称、健康、能力、网络角色和连接摘要，`prefers-reduced-motion: reduce` 禁用拓扑动效。

`topology-view.test.tsx`（15 tests）覆盖键盘移动/打开/选择、菜单、实时刷新焦点、窄屏默认列表、
触控长按取消和冻结布局；新增 `topology-keyboard-e2e.mjs` 在真实 Chrome 中完成 10 项无鼠标检查，
覆盖纯 Tab/Enter 导航、ARIA 摘要、减少动效、全部键盘控件、图/列表菜单、窄屏、搜索、列表键盘
导航和真实触摸长按，报告写入 `target/topology-keyboard-e2e/report.json`。

**验证通过**：WebUI `npm test -- --run`（15 files，181 passed）、`npm run typecheck`、
`npm run lint`、`npm run build`、`node webui/scripts/topology-keyboard-e2e.mjs --skip-build`
（10 checks passed）、`node scripts/test_webui_accessibility.mjs`、
`cargo test -p fuckipv6-common --all-features i18n`（9 passed）和 `git diff --check`。

### W9-08 1,000 节点/5,000 连接性能 `[~]`

- [x] 建立可重复的大图基准数据；
- [x] 布局、筛选、选择和增量更新不阻塞主线程；
- [x] 首次绘制、刷新和交互达到冻结的性能预算；
- [x] 使用索引、空间分区或聚合避免 O(n²) 路径；
- [x] 内存和 DOM/原生对象数量有上限；
- [x] 持续更新时不产生无界事件队列；
- [ ] 在最小支持桌面尺寸和 Android 真机测试；
- [x] 性能报告绑定 commit、数据集和平台。

**验收**：目标规模下可交互且无卡死；报告可复核，不以小规模演示代替容量测试。
桌面 960×640 容量验收已通过；Android 真机尚无设备可测，本项保持 `[~]`。

**证据（2026-11-06）**：`webui/scripts/topology-capacity-e2e.mjs` 生成固定 1,000 节点、
5,000 连接的基准拓扑，数据集 SHA-256 为
`2d36eef16c8451f17c15c2a76469d0ecc66f34158d4a1179272d059d4dc81e4c`。报告记录
Chrome `157.0.8103.38`、Linux x64、960×640、DPR 1、减少动效；提交后 clean-tree 报告绑定
commit `839569f4f9f4610bc67847a55880fcc2074e0eb8`、分支 `main`、`working_tree_clean: true`，
报告 SHA-256 为 `408686ffc4ff3c7078dc8ce68a602b29476379f0c69bba57408823ebd74bf8f5`。
冻结预算为首次可交互 2,000ms、筛选/选择/增量刷新/列表键盘导航各 500ms、Long Task 50ms、
图工作集 80 节点/200 连接、列表 DOM 40 行、500 个事件最多 2 次快照请求、JS heap 256MB；
测试没有放宽这些预算。

最终桌面实测：首次可交互 77ms、筛选 11ms、选择 14ms、500 事件刷新 194ms、列表键盘导航
24ms；500 帧事件突发只产生 1 次 snapshot 请求。图工作集渲染 80 节点/50 连接，列表 DOM
13/14 行但通过 `aria-rowcount=1001` 暴露完整 1,000 节点；无超过 50ms 的 Long Task，
JS heap 为 9.21MB。快照 JSON 解析与 patch diff、拓扑布局均移到 Worker；拓扑索引按快照使用
`WeakMap` 缓存，500 帧事件只保留最新帧并合并一次 trailing snapshot 失效，避免无界队列。
`TopologyPage` 不再因每次 topology revision 变化自动拉取成员目录，空选择刷新也不会扫描全部
节点生成批量计划。

容量脚本此前连续 4 次、提交后再次在 clean worktree 通过 10 项检查；键盘 E2E 同时通过
10 项检查。Android 方面 `adb devices -l` 可用，但没有连接设备；报告明确写入 `android_real_device.tested: false`，因此不宣称 Android
真机通过，第 7 项保持未完成。报告路径为 `target/topology-capacity-e2e/report.json`。

**验证通过**：WebUI `npm test -- --run`（17 files，187 passed）、`npm run typecheck`、
`npm run lint`、`npm run build`、`node webui/scripts/topology-capacity-e2e.mjs --skip-build`、
`node webui/scripts/topology-keyboard-e2e.mjs --skip-build`、
`node scripts/test_webui_accessibility.mjs`、`cargo test -p fuckipv6-common --all-features i18n`
（9 passed）和 `git diff --check`。

### W9-09 拓扑状态与视觉回归 `[x]`

- [x] 加载、部分数据、陈旧、空、错误、无权限状态完整；
- [x] 单网络、多网络、公共节点、离线、降级、能力混合场景有基线；
- [x] GPUI 与 WebUI 使用同一对象详情、菜单和状态；
- [x] 同尺寸/DPR/字体下视觉回归；
- [x] 选择、悬停和菜单状态可对比；
- [x] 布局重置和持久化行为一致；
- [x] 禁止拓扑展示假节点或静态连接；
- [x] 证据包含筛选、选择和动作结果。

**验收**：拓扑完成从数据、权限、交互到视觉回归的闭环。

**证据（2026-11-06，状态契约）**：`webui/src/topology-presentation.ts` 建立拓扑专用状态模型，
显式区分 `initial_loading`、`partial`、`stale`、`offline`、`refreshing`、`empty`、`error`、
`permission_denied`、`unavailable` 和 `ready`。同一快照可同时保留陈旧、刷新与裁剪状态；除无权限和
首次加载外，陈旧/错误状态继续展示最后权威拓扑，不用空态覆盖已有对象。`TopologySnapshot.truncated`
现在有独立可见状态。`pageHasContent("topology")` 改为只依据 `snapshot.topology.boundaries`，不再因
`snapshot.networks` 非空而误判拓扑有内容。

**验证通过**：WebUI `npm test -- --run`（18 files，194 passed）、`npm run typecheck`、
`npm run lint`、`node scripts/test_i18n_runtime.mjs`、
`cargo test -p fuckipv6-common --all-features i18n`（9 passed）和 `git diff --check`。

**证据（2026-11-06，GPUI 共享投影）**：GPUI 新增与 WebUI 一致的拓扑状态优先级
`error → permission_denied → initial_loading → offline → stale → refreshing → partial → unavailable → empty → ready`，
陈旧、离线、错误和刷新时只要仍有权威边界就继续展示最后快照；节点详情按“成员网络关系 → 权威成员目录 → 未知”
取值，角色、状态、presence 与能力均走同一 DTO/本地化键。对象菜单只消费服务端目录的
`available_actions`，同时要求目录网络匹配当前边界、`can_manage_members=true` 且 viewer 不是普通成员；
生命周期动作使用稳定 wire value，不依赖本地化文案。真实点击测试
`topology_detail_and_menu_are_independent_actions` 验证点击节点只打开详情、独立菜单按钮才打开菜单；
`topology_presentation_matches_webui_priority_including_stale`、`topology_detail_uses_membership_then_directory_then_unknown`、
`topology_member_actions_require_a_matching_manageable_directory` 和
`topology_lifecycle_wire_values_do_not_depend_on_localized_copy` 验证状态、详情和菜单权限语义。
验证通过：`cargo test -p fuckipv6-client --all-features --lib`（857 passed，3 ignored）、
`cargo fmt --all -- --check` 和 `git diff --check`。

**证据（2026-11-06，场景基线）**：`contracts/typescript/fixtures/topology-presentation-scenarios.json`
提供单网络、多网络、跨网络公共节点、离线、降级和能力混合六个独立权威拓扑快照，fixture SHA-256 为
`756f7ea4fdc9a4b6e7a996a30462fff3ef118db699cd6ef726014b79957beb47`。这些是可重复测试夹具，
不冒充生产运行数据。`webui/scripts/topology-visual-baseline.mjs` 启动真实 Chrome/CDP 和产品 WebUI，
由 fixture 服务端在 `/api/v1/snapshot` 返回对应 `snapshot.topology`，不从浏览器注入假节点；逐场景校验
边界、节点顺序、健康、能力环、共享节点标记、连接状态、ready 状态和节点详情文本。

固定环境为 1440×900 viewport、DPR 2、Inter、`en-US`、UTC、减少动效；截图均为 2880×1800，
运行环境为 Chrome `157.0.8103.38`、Node `v26.13.0`、Linux x86_64。使用
`cd webui && npm run visual:topology-baseline` 完成构建并通过 6 个场景；不跳过构建的最终运行和
提交前连续 5 次运行结果一致。截图 SHA-256 分别为：single-network
`4dba5bec6825e0a778acc6c68958e51c08931e49b6971ffc722ef1c285b7cc7f`、multi-network
`6dbf043970c912db24a78d2d25a978f0bcd1e545e2dacfc20ff5050e6d9c7587`、public-node
`68dff66426ea2ff97976a93ecc1dfb969ec3fad3ca0c69082a7451a986f2e6e1`、offline
`543a3672f2382e24ad5f127a819f0a717dbf9e291d8b46e28114a4f84da54a5f`、degraded
`04ee4853faa3859649fe70c9cf4889e99bcdb087a08c76a0dba941f97c0c99e8`、capability-mixed
`4bb71067f298f54f76aadbb2edad64ccd6d710ec641cb69d122d132b40715701`。人工审查拼图确认公共节点
共享计数、不可达节点、降级警告、能力环和连接虚线均实际可见。

**证据（2026-11-06，交互状态基线）**：同一视觉基线脚本通过 CDP `Input.dispatchMouseEvent`
发送真实指针事件，分别移动到 `Studio Peer` 生成悬停卡、Shift+单击生成批量选择栏、右键打开对象菜单；
截图前不使用 DOM 注入或直接修改 React 状态。脚本逐项校验悬停卡的成员角色/能力/健康/presence、选择
节点 ID 与选择栏文案、对象菜单标题/NodeId/菜单项及服务端无动作时的禁用说明。三种状态均为
2880×1800，连续 5 次运行哈希稳定：悬停
`f18326db7d5a044503a2e810f2812c3144477c292adc2d7b5bb9996015347a9b`、选择
`afc9fe572d0a7d1c89435212f9f6c3e31f19b427c96d2b8d30a842fd6cab9b8a`、菜单
`1046faafcdda8b6749b10fb54e0950642aee23d74dbe9641eca3ac1f57808ba4`。人工审查截图确认
悬停卡未遮挡关键节点、选择栏完整可见、右键菜单无截断且禁用动作有明确说明。

**证据（2026-11-06，布局重置与持久化）**：WebUI 图形拓扑支持鼠标拖动节点、右键
“固定当前布局/更新固定布局”，以及工具栏“重置固定布局”。布局存储键为
`fuckipv6.topology.layout.v1`，文档版本为 `1`，最多保存 2,000 个位置，结构仅为
`{version, positions: {<stable-node-id>: {x, y}}}`；名称、网络、角色、地址、健康与连接
均不写入存储。加载时安全忽略损坏 JSON、未知版本、非法/非有限坐标和超限集合，并按当前权威
布局尺寸钳制坐标；存储不可用或写入失败时保留本次会话位置并显示警告。重置只清除固定坐标，
不改变缩放、冻结、筛选或选择状态；固定坐标只覆盖位置，节点网络归属、槽位与连接仍来自当前
`TopologySnapshot`。

真实 Chrome/CDP 验证使用鼠标事件拖动 `node-multi-studio`：自动坐标为
`(482.4, 24.233873484032657)`，拖动后固定为
`(547.0810913085937, 67.85599338759711)`；刷新后坐标与固定状态恢复，点击真实重置按钮后
回到自动坐标，再次刷新仍保持自动布局且存储为空。浏览器存储实测仅包含上述版本、稳定 NodeId
和 x/y。固定态截图 SHA-256 为
`9fd9dba5973c2f9b7c11d6f56219f255fe21c70dac1e8aea62fec4f8fd701c73`，重置后截图
`d5f6ea5968807021a2bfc2afb0c43d5715558acecd60c7d93e54acea86e066d5`，均为
2880×1800；最终脚本连续两次 `--skip-build` 运行哈希一致。新增控件后当前场景/交互基线哈希为：
single-network `20868c8baa22f663a98acaf464ecf8591a2877e6a5863a8e96d7f0eea234b7e6`、
multi-network `6ddb3c5d0bbe0e6c112103b0c0528d76835272b73a5843834647a0c81cbd8ab0`、
public-node `a7bc80e6b719500555b970d2e7f4c8fc5251fc6df169ca0c2459ad382b5f62f2`、
offline `76352da42226c7764856be75ddd9a80a537c3272976fb466a174a868c31afddc`、
degraded `00560e084c67df9cda6b62fd115fa0c17194e2894549e3e1da7728d2a2bdd6a5`、
capability-mixed `32ac8d8f9b8d6ba0fa14784f5c4abba6898457c3429c01dadd3a99e54a44ed5d`；
悬停 `0b08d93116650553b88ea0003095f0d344b6518848e0bc89623582d3b8ed723d`、选择
`6b38897db9c5e5e0cf11df1079da821eac5a01ce71e4e74080f2f47e1f09ffef`、菜单
`89402af3ca89e885246f679eea24b24edbb7a378ba2b46089c15c4e7fd964bc2`。

能力边界：GPUI 当前拓扑使用权威卡片/连接列表，没有可拖动的坐标系或用户可编辑节点位置，
因此不存在可等价的坐标持久化/重置操作，本项未在 GPUI 添加伪控件；两端的节点归属、连接和
动作数据仍共用同一权威快照与状态契约。人工审查固定态与重置态截图，确认拖动后的标签和连接
随节点移动、固定状态提示可见，重置后节点、边界和连接恢复自动布局，无残留固定状态。

**证据（2026-11-06，权威对象渲染）**：WebUI 图形拓扑先按当前权威快照的实际布局结果生成
`renderedNodes` 与 `renderedConnections`，只渲染拥有坐标的节点以及两端均可布局的连接；画布
`data-rendered-nodes`、`data-rendered-connections` 和 SVG ARIA 描述也使用真实渲染数量。连接 DOM
暴露稳定连接 ID、起止节点、网络和观测时间，供自动化逐项绑定。测试
`renders only authoritative nodes and observed connections across refreshes` 验证初始节点顺序/ID、
连接 ID/端点/网络/状态与快照完全一致；刷新移除节点或连接后旧 DOM 不残留；服务端连接缺少可布局
端点时不画臆造连接；节点为空时显示空态而非占位画布。CDP 视觉基线同步逐个深比较连接身份、端点和
网络，不再只比较数量与状态。GPUI 既有
`topology_shared_scenarios_render_only_authoritative_objects` 逐场景验证权威节点/连接齐全且不存在
额外节点、静态连接或多余连接，本项单独复跑通过。生产 UI 未从本地常量、浏览器注入或占位数据生成
拓扑对象。

**验证通过**：WebUI `npm test -- --run`（19 files，208 passed）、`npm run typecheck`、
`npm run lint`、`npm run visual:topology-baseline`（含构建，6 scenarios、3 interaction states、
1 layout persistence check）、最终脚本 `--skip-build` 连续 2 次、`node scripts/test_i18n_runtime.mjs`、
`node scripts/test_webui_accessibility.mjs`、
`cargo test -p fuckipv6-client --all-features --lib topology_shared_scenarios_render_only_authoritative_objects`
（1 passed）、`cargo test -p fuckipv6-common --all-features i18n`（9 passed）和 `git diff --check`。

**证据（2026-11-06，筛选、选择与动作结果）**：同一真实 Chrome/CDP 基线新增 `filtered` 与
`action-result` 状态。筛选态通过真实键盘输入 `Studio`，只保留 `node-multi-studio`，并校验
“Showing 1 of 3 nodes”、筛选框值与画布 `data-rendered-nodes=1`；选择态继续校验批量选择栏和唯一
NodeId。动作结果态从 `Home Peer` 的右键菜单发起真实 `Suspend`，在确认对话框点击执行，服务端严格
收到一次 `list_network_members`、一次 `change_member_lifecycle`（`action=suspend`、
`expected_revision=9002`）和动作后一次目录刷新；结果面板逐项显示 `Succeeded 1 · Skipped 0 · Failed 0`、
变更后 revision `9003` 与服务端 audit ID，不使用本地伪成功或伪审计。动作目标改用符合正式 NodeId
契约的稳定值，避免视觉夹具绕过生产端预检。另修复拓扑页在提交回调及等价 manifest 对象刷新时反复
加载成员目录的循环；`topology-page.test.tsx` 的
`loads the member directory once across equivalent callback and manifest refreshes` 回归覆盖该场景。

筛选、选择与动作结果截图均为 2880×1800；最终脚本连续两次 `--skip-build` 哈希完全一致：
筛选 `566e807a75a02afbab187e1aed9d93fb1378246329b3472c6ac8b7b67a7944f6`、选择
`6b38897db9c5e5e0cf11df1079da821eac5a01ce71e4e74080f2f47e1f09ffef`、动作结果
`43e4d389066275601212accdc6f1d638ea168376d72e8f95d8dc440d42845030`。当前场景 fixture
SHA-256 为 `e0b7288b5e609a6767117622aed323c2fa3c0bda3de8c40da2c34fbed93ab306`，基线报告
`target/topology-visual-baseline/manifest.json` 的 SHA-256 为
`6b100e279b4996fb8f2357dad3cd8a8ee3947939461273b0ee069159f5d73baa`。人工审查确认筛选计数、
选择反馈和动作结果逐项状态在固定视口内完整可读，失败/跳过区未被总摘要遮盖。

**验证通过**：WebUI `npm test -- --run`（19 files，209 passed）、`npm run typecheck`、
`npm run lint`、`npm run build`、`node scripts/topology-visual-baseline.mjs --skip-build`
（最终连续 2 次，6 scenarios、5 interaction states、1 layout persistence check）、
`node ../scripts/test_webui_accessibility.mjs`、`node ../scripts/test_i18n_runtime.mjs` 和
`git diff --check`。

### W9-10 拓扑与成员 E2E `[x]`

- [x] 覆盖邀请→批准→在线→降级→移除完整旅程；
- [x] 覆盖多网络公共节点和跨网络筛选；
- [x] 覆盖组策略变化后拓扑能力变化；
- [x] 覆盖权限撤销后对象和操作即时消失；
- [x] 覆盖右键、菜单键、长按和键盘操作；
- [x] 覆盖大图增量更新和断线重连；
- [x] 验证所有批量操作有逐项结果；
- [x] 测试证据与活动记录可相互追溯。

**验收**：拓扑与治理不是两张皮；用户从图或列表均可完成同一真实操作。

**证据（2026-11-06）**：`webui/scripts/topology-member-e2e.mjs` 在真实无头 Chrome/CDP 中完成 17 项断言，
使用服务端权威拓扑和成员目录走通邀请创建、Bob 的 pending/online 多网络身份、批准、降级和移除；批准与
移除响应中的 audit ID 均与后续 Overview 活动和权威快照一致，邀请码只做格式校验且不写入报告。组策略
场景先完成批准结果面板并重新以 `Space` 选中真实节点，再把 Bob 从 `file-readers`/File 收紧为
`relay-operators`/Relay，同时撤销 `can_manage_members`；拓扑角色、能力和右键/键盘菜单动作立即消失，
随后恢复策略并等待两次权威目录刷新，File/Exit 能力和 Suspend/Remove 动作随授权恢复。移除后 Bob 的
Home membership 与操作消失，但 Studio 公共节点及其动作保留。旅程还覆盖真实右键、`Shift+F10`、
方向键/`End`/`Enter`、窄屏触控长按和跨网络筛选；122 节点大图仅渲染 80 个 DOM 节点，断线重连不刷新
文档、重新获取权威快照、移除陈旧节点并应用重命名。批准在 E2E 中展示逐项结果与真实 audit ID；单元测试
另行覆盖 approve/reject/suspend/resume/remove/undo_remove 六种批量动作、混合失败逐项披露和
operation-only 伪成功拒绝。组策略目录响应与快照 revision 竞争时不会把旧授权写回 UI，新增回归覆盖
in-flight 响应后立即刷新权威目录。

**验证通过**：`node webui/scripts/topology-member-e2e.mjs --skip-build`（17 checks）、WebUI
`npm test -- --run`（19 files，221 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`node scripts/test_webui_accessibility.mjs`、`node scripts/test_i18n_runtime.mjs`、
`cargo test -p fuckipv6-client --all-features --lib topology_shared_scenarios_render_only_authoritative_objects`
（1 passed）、`cargo test -p fuckipv6-common --all-features i18n`（9 passed）和 `git diff --check`。
报告位于 `target/topology-member-e2e/report.json`，脚本 SHA-256 为
`728e60b31f6f761b1269a0a328f37c1aa95219336a8c9967dca7b4f1c4f2e5a3`。

## 13. W10：安全、隐私与活动

目标：权限、字段隐私、策略应用和审计使用同一权威契约。默认拒绝，节点本地否决不可被远程管理员覆盖。

### W10-01 权限模型与矩阵冻结 `[x]`

- [x] 定义 Node Owner、Network Owner、Administrator、Member、Guest 和临时 Coordinator 身份；
- [x] 定义资源、动作、作用域和前置状态；
- [x] 实现权限合成算法：显式拒绝优先、本地否决其次、允许需全部满足；
- [x] 区分网络授权与节点本地授权；
- [x] 临时 Coordinator 的写权限冻结规则进入领域层；
- [x] 建立权限矩阵契约测试；
- [x] 前端只根据服务端 capability 结果启用控件；
- [x] 所有拒绝返回稳定错误码和可解释原因。

身份层使用 `NodeOwner`、`Network(role)`、`DiscoveryGuest` 和 `UnauthenticatedPublic` 明确分离节点所有权、网络角色与公开发现身份；
网络持久角色为 Owner/Administrator/Member，临时 Coordinator 仅作为运行时角色。权限契约冻结 13 个动作、12 类资源、只读/变更意图
和 14 个稳定拒绝码。本地与网络策略必须同时存在匹配允许，任一层显式拒绝优先；临时 Coordinator 的所有变更意图在领域层冻结。
命令总线从认证后的持久 membership 构造网络作用域，Node Owner 不再隐式获得网络治理权限，畸形网络 ID 失败关闭；拒绝结果保留
策略层、规则 ID 和可解释原因。GPUI/WebUI 删除角色标签授权推断，只依据服务端 `can_manage_members` 与逐成员
`available_actions`。

**验证通过（2026-11-07）**：`cargo test -p fuckipv6-common --all-features`（231 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（863 passed，3 ignored）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、WebUI `npm test -- --run`
（19 files，223 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、`./scripts/check-contracts.sh`
（契约 Rust 7 passed，生成物 verified 2，TypeScript/Java 绑定测试通过）、
`node webui/scripts/topology-visual-baseline.mjs --skip-build`（6 scenarios、5 interaction states、1 layout persistence check）、
`node webui/scripts/topology-member-e2e.mjs --skip-build`（17 checks）和 `git diff --check`。角色矩阵测试覆盖
4 角色 × 13 动作 × 12 资源共 624 个组合；报告位于 `target/topology-member-e2e/report.json` 与
`target/topology-visual-baseline/manifest.json`。

**验收**：任何授权判断不依赖 UI 隐藏；角色、组、本地限制和临时任期组合均能确定结果。

### W10-02 ACL、拒绝优先与本地否决 `[x]`

- [x] 实现网络 ACL 和节点本地限制；
- [x] 显式拒绝覆盖角色、组和直接允许；
- [x] 节点 owner 可拒绝贡献 Relay/Exit/File 或限制网络范围；
- [x] 管理员无法覆盖节点本地否决；
- [x] 支持授权到期、撤销和重新认证要求；
- [x] 对冲突策略展示最终效果，而非仅展示原始规则；
- [x] 覆盖并发编辑、策略版本和撤销竞争；
- [x] 所有拒绝/授权变更审计。

网络 ACL 与节点本地策略使用独立 revision/content digest 条件写；并发编辑、过期、撤销和重新认证要求均失败关闭。
求值器先应用任意一层显式 deny，再检查节点 owner 的网络范围/贡献否决，最后要求两层均匹配 allow；网络管理员授权不能放宽本地限制。
可信 `NodeAccessControlSnapshot` 自动进入授权上下文，命令载荷不能注入 ACL 或本地策略。拒绝结果包含稳定错误码、策略层、规则 ID
和最终效果；拒绝及 ACL/本地策略变更均进入原子审计，且审计不记录命令载荷中的 token、邀请码、私钥或秘密值。

**验证通过（2026-11-07）**：`cargo test -p fuckipv6-common --all-features`（239 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（870 passed，3 ignored）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`./scripts/check-contracts.sh`（契约 Rust 7 passed，生成物 verified 2，TypeScript/Java 绑定测试通过）、
WebUI `npm test -- --run`（19 files，223 passed）、`npm run typecheck`、`npm run lint`、
`npm run build` 和 `git diff --check`。测试覆盖 ACL/本地 deny 覆盖管理员 allow、owner 网络范围不可被网络管理员放宽、
Relay/Exit/File 本地贡献否决、到期/撤销/重新认证、畸形与重复 ACL 失败关闭、ACL 拒绝发生在 handler 前，
以及审计保留稳定拒绝码和规则 ID 但不泄露载荷敏感值。

**验收**：越权测试全部失败关闭；节点 owner 的最终本地控制不可被网络治理绕过。

### W10-03 字段级隐私 `[x]`

- [x] 为成员、节点、拓扑、活动和 File 元数据定义字段可见性；
- [x] 支持 Owner-only、Administrator-only、Network、Public 级别；
- [x] 默认选择最小可见范围；
- [x] 服务端在序列化前裁剪字段；
- [x] 搜索、排序、计数和导出不得侧漏隐藏字段；
- [x] 记录字段可见性变更及生效版本；
- [x] 提供管理员公开预览；
- [x] 建立角色 × 字段 × 对象状态测试矩阵。

字段隐私策略按网络版本化并记录生效时间；成员、节点、拓扑、活动和 File 元数据均有显式字段规则，未配置时采用字段的最小可见默认值。
节点本地进程、路径和详细日志永久锁定为 Owner-only，网络策略不能放宽。HTTP legacy/v1、SSE、WebSocket、命令结果、活动事件和
File/成员目录均在服务端序列化为 `Projected*` DTO，隐藏值在进入响应、缓存或搜索/排序/计数/导出投影前被移除；重复、畸形、
过期或尚未生效的策略失败关闭。操作查询与取消对非提交者返回 `404`，认证/授权拒绝不回显受保护对象或参数。管理员可通过
`PublicFieldPreview` 查看公开字段集合和策略 revision，而不会接触隐藏值。

**验证通过（2026-11-07）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-client --all-features --lib node::http_adapter`（44 passed）、
`cargo test -p fuckipv6-client --all-features --lib node::privacy_projection`（15 passed）、
`cargo test -p fuckipv6-common --all-features`（253 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（890 passed，3 ignored）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`./scripts/check-contracts.sh`（契约 Rust 7 passed，生成物 verified 2，TypeScript/Java 绑定测试通过）、
`git diff --check`。测试覆盖四层可见性、默认最小范围、角色 × 字段 × 对象状态矩阵、成员/拓扑/活动/File 投影、
HTTP/SSE/WebSocket/命令结果裁剪、操作存在性隐藏，以及重复或畸形策略失败关闭。

**验收**：隐藏字段不进入响应、缓存、搜索索引或活动正文；不存在仅靠前端遮罩的隐私。

### W10-04 策略编辑器与测试器 `[x]`

- [x] 策略使用结构化表单，不要求手写 JSON；
- [x] 高级模式允许查看/导入完整配置，但必须校验；
- [x] 显示每条规则的来源、优先级和影响对象；
- [x] 测试器可输入成员/节点/资源上下文并显示最终结果；
- [x] 显示拒绝优先、本地否决和临时任期的影响；
- [x] 无效规则定位到字段和冲突来源；
- [x] 测试结果不执行真实授权副作用；
- [x] 不提供不可解释的“智能推荐”自动改权。

策略工作流默认使用结构化表单，覆盖成员/节点/资源上下文、请求、本地策略、ACL、rules、grants 和 vetoes，不要求管理员手写 JSON。
高级模式可查看、编辑和导入完整配置；前后端均严格校验未知字段、重复键、语法错误和尾随输入，并把错误定位到具体字段路径。
结果区显示最终决策、规则来源、优先级、命中/未命中状态、影响对象、资源范围和逐条解释；拒绝优先、本地否决、临时 Coordinator
任期及重认证条件直接进入同一求值链。测试通过无副作用的共享求值器完成，不应用策略、不写审计、不创建操作，也不提供自动改权的
“智能推荐”。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、`jq empty crates/common/assets/i18n.json`、
`cargo test -p fuckipv6-common --all-features`（267 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（900 passed，3 ignored）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`./scripts/check-contracts.sh`（契约 Rust 7 passed，生成物 verified 2，TypeScript/Java 绑定测试通过）、
`npm test -- --run`（233 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、`git diff --check`。
策略求值与 WebUI 组件测试覆盖合法/拒绝结果、严格 JSON 路径、validation issue 定位、规则来源/优先级/资源/解释、
临时任期和本地否决，以及“测试不应用策略、不提供推荐”的界面约束。

**验收**：管理员在应用前可预测策略效果，所有结果由与运行时相同的求值器产生。

### W10-05 差异预览、原子应用与回滚 `[x]`

- [x] 生成策略差异摘要和受影响对象列表；
- [x] 高风险权限扩大要求近期重认证和显式确认；
- [x] 使用版本号和条件写防止并发覆盖；
- [x] 应用失败保持旧版本有效；
- [x] 成功后广播权威事件并刷新受影响视图；
- [x] 提供安全回滚到指定策略版本；
- [x] 回滚本身也是一次新版本和审计事件；
- [x] 覆盖断线、进程崩溃和版本竞争测试。

策略事务以完整、归一化的权威快照为单位，按稳定网络 ID 排序并拒绝重复 ACL，生成带稳定 JSON Pointer、变更前后值、
受影响策略对象和风险原因的完整差异。预览同时返回期望/当前 revision、冲突状态和是否为 no-op；清除 ACL、删除拒绝规则、
扩大主语/资源/动作/条件范围、增加授权或延长有效期等权限扩大操作会提升风险等级，并要求近期所有者重认证与显式确认。
应用和回滚都携带 `expected_revision`，在文件锁内复核当前 revision 后条件提交；不符时失败关闭，不会覆盖并发写入者。
提交使用 staging 文件、目录同步和最后原子切换权威状态指针，失败或进程崩溃发生在指针切换前时旧版本继续有效，
启动恢复会清理未发布 generation、history 和 staging 残留。成功事务发布权威策略快照事件，WebUI 据此刷新策略、
历史和受影响视图；no-op 不产生新 revision、事件或审计。

回滚先预览目标版本作为“新版本”的差异、风险、作用范围与重认证要求，再通过相同的条件写链路提交；它不会移动或删除历史，
而是生成递增 revision，在新审计中同时记录 `rollback` 动作、目标 revision、actor、来源、差异、影响对象和风险摘要。
后端事务测试覆盖两写者同 revision 竞争、过期预览、并发覆盖拒绝、高风险确认失败后旧快照保持、回滚失败、崩溃 staging 清理、
提交后重开恢复以及 local policy/ACL revision 同步；运行时与 WebUI 测试覆盖权威事件刷新、revision 冲突、no-op 和回滚审计展示。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、`jq empty crates/common/assets/i18n.json`、
`cargo test -p fuckipv6-common --all-features`（267 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（923 passed，3 ignored）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`./scripts/check-contracts.sh`（契约 Rust 7 passed，生成物 verified 2，TypeScript/Java 绑定测试通过）、
`npm test -- --run`（20 files，244 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`git diff --check`。

**验收**：不存在部分应用或静默覆盖；任何策略变化均可追责、回滚和解释。

### W10-06 活动时间线 `[x]`

- [x] 活动使用稳定事件 ID、时间、actor、对象、动作、结果和原因；
- [x] 从权威事件投影，不把 UI toast 当活动；
- [x] 支持按网络、对象、角色、严重度、时间和结果筛选；
- [x] 区分用户活动、系统事件、安全事件和操作失败；
- [x] 实时更新不打断用户阅读或破坏滚动位置；
- [x] 断线重连后补偿缺口并标记陈旧；
- [x] 活动正文经过字段级隐私裁剪；
- [x] 活动中的对象链接同时检查当前权限。

活动页以权威 `snapshot.recent_activity` 为唯一来源，不依赖 toast、本地通知或时间线缓存重建事实。服务端为每条活动保留稳定
`audit_id`、发生时间、actor、类型化对象、规范动作、结果、稳定原因码、类别和严重度；重复快照或重放事件按 `audit_id`
去重，后出现副本覆盖旧副本。活动按时间降序、同一时间按 `audit_id` 降序稳定排序，服务端短期投影保留最新 256 条，
概览与活动页均消费同一有序投影，不各自复制或重新推断活动。

GPUI 与 WebUI 支持按网络、对象、角色、严重度、时间闭区间和结果组合筛选，并明确区分用户活动、系统事件、安全事件和操作失败。
revision 更新只替换权威行，不改变用户当前阅读位置；断线重连后通过快照补偿缺口，与当前行无冲突时保留阅读位置，并显示陈旧、
刷新中或权限拒绝提示。活动正文在服务端序列化前按字段级隐私规则裁剪；对象链接仅在当前视图仍具备权限、对象仍存在且网络归属匹配时
提供，权限撤销、对象删除或跨网络 ID 都不会留下可导航历史入口。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、`jq empty crates/common/assets/i18n.json`、
`cargo test -p fuckipv6-common --all-features`（267 passed）、
`cargo test -p fuckipv6-client --all-features --lib`（926 passed，3 ignored）、
`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
`./scripts/check-contracts.sh`（契约 Rust 7 passed，生成物 verified 2，TypeScript/Java 绑定测试通过）、
`npm test -- --run`（21 files，251 passed）、`npm run typecheck`、`npm run lint`、`npm run build`、
`git diff --check`。后端、GPUI 与 WebUI 测试覆盖稳定去重和排序、256 条保留、全部筛选字段与闭区间时间、页面级过滤、
隐私正文不进入活动投影、滚动位置保持、陈旧/刷新提示、权限撤销和对象删除后的链接移除，以及活动对象网络归属校验。

**验收**：活动可重建关键状态变化；重连不丢事件、不重复显示，隐私不因历史记录泄漏。

### W10-07 审计保留、导出与完整性 `[x]`

- [x] 区分短期活动投影与长期不可静默删除的安全审计；
- [x] 配置保留期、容量上限和轮转策略；
- [x] 脱敏导出支持时间范围和对象范围；
- [x] 导出前检查权限和二次确认；
- [x] 审计序号/哈希链或等价机制可检测篡改；
- [x] 不把秘密、完整邀请或私钥写入审计；
- [x] 磁盘满、写入失败和时钟漂移有告警；
- [x] 恢复和迁移后保持可验证顺序。

短期活动仍只保留最新 256 条，长期安全审计则写入独立的追加式哈希链账本，不因普通读命令、UI 刷新或活动裁剪而删除。
账本按保留天数、单段记录数和最大段数配置分段轮转；容量耗尽时拒绝覆盖历史并失败关闭，只有显式保留策略允许时才会轮转。
每次写入同时更新 atomically replaced checkpoint，记录全局 sequence、前序哈希和 entry hash；重开、轮转、legacy 迁移和尾部删除检测
都会先验证同一顺序链。HTTP 所有者端点记录与节点安全命令复用同一个 `Arc<OwnerSecurityAudit>`，共享持久序号，避免两条独立链。

导出按闭区间时间、actor、动作、网络、对象类型和对象 ID 过滤，并在服务端重新验证完整链；调用者必须具有近期认证的所有者会话且
显式二次确认，普通读取和预览不进入长期账本。命令审计只保留稳定 reason code 和类型化 allowlist 字段，拒绝自由文本 `reason_detail`；
测试覆盖邀请码、凭据、请求正文和私钥等秘密不进入持久记录。磁盘满、写/同步失败、容量耗尽、时钟回退、checkpoint 恢复和完整性
校验失败都会进入健康告警；安全命令的审计写入失败会返回 `AuditUnavailable`，不会把业务成功结果交给调用方。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-client --all-features --lib`（939 passed，3 ignored）、
`cargo clippy -p fuckipv6-client --all-targets --all-features -- -D warnings`、`git diff --check`。
审计定向回归覆盖请求/命令共享链、重开后导出验证、legacy 迁移、轮转顺序、checkpoint 篡改和尾部删除、容量耗尽、
磁盘满/写失败/时钟回退告警、导出权限/近期认证/二次确认/范围和脱敏，以及审计失败时不返回命令成功结果。

**验收**：审计记录可验证、可脱敏导出且不会因普通 UI 操作被静默清除。

### W10-08 安全与隐私页面状态 `[x]`

- [x] 安全页面只保留真实可配置项和当前生效结果；
- [x] 无权限策略修改时展示只读原因和请求路径；
- [x] 策略冲突、陈旧、应用中、失败和回滚状态齐全；
- [x] 危险操作展示作用范围、持续时间与撤销方法；
- [x] GPUI/WebUI 字段、顺序、错误和确认一致；
- [x] 移动端使用全屏确认而不是无法阅读的弹窗；
- [x] 所有控件有键盘与屏幕阅读器路径；
- [x] 禁止展示“安全已开启”等无证据宣传文案。

WebUI 安全页统一投影 13 条可验证事实：所有者身份、Node ID、公钥指纹、访问模型、已授予权限、本地策略 revision/有效期/规则/否决、
网络 ACL、字段隐私策略、成员关系策略 revision，以及策略写入权限。缺失快照字段显示“当前快照未暴露”，空集合显示“无”，
不再用 WebUI/会话安全宣传代替服务端证据；WebUI 配置和会话安全也明确显示当前快照未暴露。

策略写入权限直接复用 `apply_policy` 的动作阻塞原因。无 `manage_networks` 时，安全页和策略工作流都会展示只读状态、类型化原因、
所需权限、请求路径和命令；可应用、回滚、预览、测试和历史刷新控件均通过稳定的 `aria-describedby` 关联可见原因。
策略事务增加统一状态轨道，覆盖冲突、陈旧、应用中、回滚中、失败、已应用、已回滚和无状态；回滚完成后使用独立完成标题，
不再依赖通用回滚提示。

应用与回滚确认固定展示六项事实：作用范围、revision、持续时间、撤销方法、失败关闭语义和审计结果。桌面使用可滚动确认框，
窄屏改为带安全区留白、纵向操作和至少 44px 主操作按钮的全屏确认；所有危险操作保留键盘焦点、关闭按钮和屏幕阅读器描述路径。
GPUI 在同一所有者安全区按相同顺序展示 13 条事实，并复用 manifest/action 的类型化阻塞原因，不额外暴露 WebUI-only 认证界面。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-client --all-features --lib -- --test-threads=1`（940 passed，3 ignored）、
`cargo clippy -p fuckipv6-client --all-targets --all-features -- -D warnings`、WebUI `npm test -- --run`（254 passed）、
`npm run lint`、`npm run typecheck`、`npm run build`、i18n key/placeholder 一致性检查以及 `git diff --check`。
定向回归覆盖 WebUI/GPUI 安全事实顺序、只读策略原因与请求路径、13 条 GPUI 事实、策略状态与危险确认六项事实、
ARIA 关联和旧宣传文案移除。

**验收**：安全能力用可验证状态表达，不用营销标签代替实际配置。

### W10-09 权限、隐私与审计测试 `[x]`

- [x] 覆盖角色/组/本地否决/拒绝优先组合；
- [x] 覆盖临时 Coordinator 冻结和恢复；
- [x] 覆盖字段级读取、搜索、导出和活动侧漏；
- [x] 覆盖并发策略应用、回滚和断线；
- [x] 覆盖审计篡改、磁盘满、轮转和导出；
- [x] 覆盖对象删除后链接和权限检查；
- [x] 完成后端、GPUI、WebUI 和 Android 契约对齐；
- [x] 测试证据绑定策略版本和 commit。

新增 `scripts/governance-evidence.py` 作为 W10-09 的可重复证据采集器：每次运行记录 Git commit、工作区洁净状态、平台信息、
策略事务/审计链版本，以及 Rust、TypeScript、WebUI、Android 与 7 个 JSON Schema 的契约版本；逐项执行 12 组检查并生成
`target/governance-evidence/report.json` 与独立日志。报告只有在该 commit 前后不变、工作区在 `--require-clean` 下保持洁净、
契约版本对齐且 8 项 coverage 全部通过时才标记为 `passed`。

定向测试补充角色 allow、直接 grant 与显式 deny 的组合记录，以及 role/group/node grant 不得绕过本地否决；Coordinator 从
Emergency 经 Handoff 回到 Stable 后仍保持治理冻结，只有更高 term/token 的 Full + Stable successor 才恢复治理写；策略
应用与回滚在事件订阅断开后可从旧 cursor 续接，事件序号不跳号、状态 revision 不回退，最终 store snapshot 与运行时一致。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、`cargo check --workspace`、
`cargo test -p fuckipv6-common --lib`（275 passed）、
`cargo test -p fuckipv6-client --all-features --lib -- --test-threads=1`（941 passed，3 ignored）、
`cargo clippy -p fuckipv6-client --all-targets --all-features -- -D warnings`、
`python3 -m unittest scripts/test_governance_evidence.py -v`（6 passed）、
`python3 scripts/governance-evidence.py --require-clean`（12/12 checks，8/8 coverage）、
WebUI `npm test -- --run`（254 passed）、`npm run lint`、`npm run typecheck`、`npm run build`、
`./scripts/check-contracts.sh`、`python3 scripts/check-docs.py` 和 `git diff --check`。
CI 新增独立 `governance` job，执行采集器单测和洁净工作区证据采集并上传报告与日志。

**验收**：安全、隐私与活动形成完整治理闭环，并有可重复证据支撑发布声明。

## 14. W11：流量路由

目标：所有流量决策显式、可测试、可回滚；未知流量不得被“智能猜测”，失败时不得静默直连。

### W11-01 四类决策与首条命中 `[x]`

- [x] 定义直连、进入指定网络、指定 Exit、拒绝四类决策；
- [x] 规则按用户可见顺序求值，首条命中；
- [x] 无规则命中时使用显式默认策略；
- [x] 默认策略必须由用户/管理员确认，不使用隐藏推荐；
- [x] 规则优先级调整使用稳定 ID 并允许并发保护；
- [x] 决策结果包含命中规则、目标网络/Exit 和原因；
- [x] 不允许同一规则返回模糊的多出口结果；
- [x] 完成纯函数和属性测试。

领域模型现在把路由动作限定为 `Direct`、带具体网络 ID 的 `Network`、带具体节点 ID 的 `Exit` 和 `Reject`；网络与
Exit 目标直接包含在动作变体中，不可能由单条规则返回多个出口。未命中时只使用带确认主体和确认时间的
`TrafficDefaultPolicy`，缺失确认会以 `domain.traffic_default_unconfirmed` 失败关闭。规则向量顺序就是 UI、API 与运行时
共同使用的求值顺序，`priority` 必须严格等于可见序号，避免第二套隐藏排序；`with_reordered_rules` 以稳定规则 ID 重排，
要求调用方提交期望 revision、拒绝遗漏/重复/未知 ID，并返回递增 revision 的新策略而不原地修改已生效版本。

决策结果固定包含动作、命中规则 ID 和 `first_match`/`default` 原因。序列化测试确认四类动作都只有一个明确目标，API
往返后仍产生相同决策；属性测试覆盖任意启用/禁用规则序列，验证首个启用命中规则或显式默认策略始终产生稳定、唯一结果。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-common --lib domain::traffic_policy`（9 passed）、
`cargo test -p fuckipv6-common --lib`（280 passed）、`cargo check --workspace`、
`cargo clippy -p fuckipv6-common --all-targets -- -D warnings`、`cargo test --workspace`、
`python3 scripts/check-docs.py` 和 `git diff --check`。
本轮完成领域契约与纯函数求值器；真实 TUN/路由数据面接线、测试器页面和原子应用仍由 W11-05 至 W11-08 完成。

**验收**：相同输入始终产生可解释的同一决策；规则顺序在 UI、API 和运行时完全一致。

### W11-02 匹配条件 `[x]`

- [x] 支持域名、精确 IP、CIDR、端口、协议和应用条件；
- [x] 同一规则内不同条件使用 AND；
- [x] 同一条件多值使用 OR；
- [x] 支持 IPv4/IPv6、通配符边界和域名规范化；
- [x] 域名与 IP 的匹配语义分别定义，不混用；
- [x] 处理 IDN、大小写、尾点和无效 CIDR；
- [x] 空条件、永真条件和过宽规则显示风险；
- [x] 建立边界与冲突测试。

域名条件拆分为规范化后的精确域名与完整标签后缀；精确域名不匹配子域，后缀 `example.com` 可匹配自身和
`www.example.com`，但不会匹配 `notexample.com` 或 `example.com.evil`。IDN 在建模时转为 ASCII A-label，大小写和单个
根尾点被归一化，`*.example.com` 仅是完整标签后缀规则的显式写法；IP/CIDR 字段单独解析并只比较目标 IP，精确 IP
规范化为 `/32` 或 `/128`。同一字段内多值 OR，不同字段之间 AND，域名与 IP 不会互相冒充或混用。

`validate` 在应用前拒绝非法域名、通配符位置、非法 IPv4/IPv6 与 CIDR、零端口和重复值。`risk_report` 对每条规则返回
稳定 ID、顺序、启用状态和结构化风险：全空规则为 `always_matches`，未限制目的地/端口/协议/应用分别产生对应的
通配警告；IPv4 前缀不超过 `/8`、IPv6 前缀不超过 `/32` 产生 `broad_cidr`，`1..65535` 产生 `full_port_range`。
边界测试覆盖精确域名与子域、后缀标签边界、IDN/大小写/尾点、IPv4/IPv6 精确地址与 CIDR、非法输入、AND/OR 组合、
重叠规则的顺序冲突、风险报告和规范化幂等属性。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-common --lib domain::traffic_policy`（16 passed）、
`cargo test -p fuckipv6-common --lib`（287 passed）、`cargo check --workspace`、
`cargo clippy -p fuckipv6-common --all-targets -- -D warnings` 和 `git diff --check`。
当前风险结果由共享领域 API 提供，实际规则页面的风险展示和控制面接线由 W11-05、W11-06、W11-08 完成。

**验收**：条件语义无歧义；无效或危险规则在应用前被拒绝或明确警告。

### W11-03 应用身份与子进程继承 `[x]`

- [x] 定义各平台支持的应用身份格式和能力；
- [x] 实现进程/应用识别，并记录识别来源；
- [x] 子进程按明确规则继承父应用身份；
- [x] 短命进程、容器、解释器和服务进程有确定语义；
- [x] 无法识别时返回 Unknown，不猜测为可信应用；
- [x] 应用条件在平台不支持时禁用并解释；
- [x] 覆盖进程复用、PID 重用和更新后身份变化；
- [x] 隐私上不记录不必要的进程命令行。

应用身份已拆分为独立领域模块。平台契约明确 Linux 允许可执行路径，macOS/Windows 允许包/Bundle ID 与可执行路径，Android 允许包 ID；
实际运行时能力通过 `ApplicationIdentityCapability` 单独报告，不支持或不可用时清空可选格式，并给出可见解释。规则编辑器可据此禁用
应用条件，服务端 `validate_for_application_capability` 会再次拒绝不支持的条件，避免陈旧 UI 绕过。`Unknown` 不再是合法正向匹配值，
两个未知身份也永不相等，因此未知流量不会被具体应用规则意外捕获。

进程观察以 `(pid, start_time_token)` 作为实例键，开始时间令牌必须来自操作系统进程创建元数据，不能由首次看到 PID 的墙上时间推断；
PID 重用会形成不同的实例键。`ApplicationIdentityResolver` 只消费注入式观察，并返回规范身份、证据来源（可执行路径/平台包/服务注册/
容器映射）和完整继承链。普通应用使用自身身份；显式子进程总是继承已知父应用，即使其自身可执行文件不同；解释器优先继承已知
父应用，独立解释器才回退到解释器可执行文件；容器只接受显式容器映射，服务只接受服务注册，二者都不会继承 supervisor；短命进程
观察被移除后返回 `Unknown`。缺失父身份、继承环、超过 64 层和缺失元数据均失败关闭。

观察契约没有命令行、参数或环境字段，并拒绝包含这些额外字段的输入。测试覆盖平台格式矩阵、禁用解释、直接来源、子进程/解释器继承、
容器/服务隔离、短命进程、PID 重用、同实例应用元数据刷新、继承环、缺失父级和命令行隐私边界。真实 Linux/macOS/Windows/Android
进程采集适配器、平台权限探测和 TUN 数据面接线尚未实现，本项只完成可注入、可测试的共享领域契约；不得据此宣称应用分流已上线。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-common --lib domain::application_identity`（11 passed）、
`cargo test -p fuckipv6-common --lib domain::traffic_policy`（18 passed）、
`cargo test -p fuckipv6-common --lib`（300 passed）、`cargo check --workspace`、
`cargo clippy -p fuckipv6-common --all-targets -- -D warnings`、
`python3 scripts/check-docs.py` 和 `git diff --check`。

**验收**：应用分流可解释、可测试；未知身份不会因规则漏洞被错误放行。

### W11-04 未知应用与失败策略 `[x]`

- [x] 为未知应用、DNS 失败、Exit 不可用、网络选择失败定义决策；
- [x] 用户可选择失败关闭或显式直连，但默认失败关闭；
- [x] 明确哪些失败可以 fallback，哪些绝不能直连；
- [x] UI 在应用策略前显示 fallback 后果；
- [x] 运行时可动态降级但必须产生可见状态和活动；
- [x] 防止 fallback 绕过固定 Exit 或拒绝规则；
- [x] 覆盖 DNS 污染、连接超时和部分连接失败；
- [x] 所有 fallback 事件进入审计。

`TrafficPolicy` 现在将 `TrafficFailurePolicy` 纳入同一 revision/内容哈希；字段缺失时默认失败关闭，不存在隐藏直连。失败类型包括未知应用、
DNS 解析超时、DNS 污染、Exit 不可用、网络选择不可用、连接超时和部分连接失败。只有未知应用、Exit 不可用、网络选择失败和连接超时
可由 owner/管理员显式确认后直连；DNS 超时/污染和部分连接失败永远失败关闭，因为他们无法在保持原安全属性的前提下安全切换路径。

显式直连要求列出被允许的失败类，并记录确认主体与时间；空集合或未确认配置会被拒绝，把绝不能直连的失败类写入策略也会以
`domain.traffic_direct_fallback_unsafe` 失败。求值顺序先检查拒绝规则、固定 Exit、固定网络和本身已直连的动作，任何硬约束都不会被
fallback 绕过。`TrafficFailurePolicy::preview` 对全部失败类返回稳定 `consequence_key`，UI 必须在应用前展示；运行时使用同一
`decide` 方法生成 `TrafficFailureOutcome`、可见的 `TrafficRuntimeDegradation` 和重试恢复动作。

每个失败决策都要求审计；`audit_entry` 生成真实 `AuditEntry`，直连事件固定为 `traffic.failure.direct_fallback`，失败关闭事件为
`traffic.failure.fail_closed`，并投影为 `SystemEvent`/`Warning` 活动。测试覆盖默认失败关闭、显式确认、硬约束不可绕过、UI 预览、
DNS 污染、连接超时、部分连接失败、降级状态及审计/活动投影。真实 TUN/DNS 探测适配器、运行时自动降级接线和页面展示仍由 W11-07、
W11-08 完成，本项只完成可测试的共享领域契约，不得据此宣称失败策略已在线生效。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-common --lib domain::traffic_failure`（8 passed）、
`cargo test -p fuckipv6-common --lib domain::traffic_`（27 passed）、
`cargo test -p fuckipv6-common --lib`、`cargo check --workspace`、`cargo test --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings`、`python3 scripts/check-docs.py` 和 `git diff --check`。

**验收**：不存在静默泄漏真实出口；降级状态必须对用户可见且可恢复。

### W11-05 规则测试器 `[x]`

- [x] 输入域名/IP、端口、协议、应用和网络上下文；
- [x] 输出四类决策、命中规则、网络/Exit、reason 和失败策略；
- [x] 测试器复用生产求值器；
- [x] 支持从活动/诊断对象一键带入上下文；
- [x] 显示规则变更前后的差异结果；
- [x] 测试不创建真实网络连接；
- [x] 无权限时可测试但不可查看隐藏策略细节；
- [x] 建立固定样例和回归快照。

`TrafficPolicyTestInput` 接受完整 `TrafficFlow`、网络 ID、可选固定 Exit/固定网络和测试上下文来源。活动与诊断来源携带稳定对象 ID，
UI 可据此一键预填而没有复制本地化正文；固定 Exit 与固定网络不能同时设置，候选策略必须通过与已应用策略相同的 revision/successor 校验。
求值直接调用 `TrafficPolicy::evaluate` 和 `preview_failure_policy`，输入校验、首条命中、默认策略和失败策略不会另写一套测试器逻辑。

`compare` 对当前策略与候选策略使用完全相同的流量上下文，返回 `route_changed`、`matched_rule_changed`、`failure_outcomes_changed`
和汇总 `changed`。`Full` 投影包含规则的稳定 ID、reason 和全部七类失败后果；`RouteSummary` 只序列化最终动作，规则 ID、reason、
匹配解释和失败策略字段在共享域层物理省略，调用方可根据 owner/管理员授权选择投影，不能依赖前端遮罩。测试器不持有连接、存储、
审计或活动句柄，因此不能安装路由、访问目标或产生状态副作用。

测试覆盖生产求值器一致性、四类动作与失败策略、固定 Exit 不可绕过、活动/诊断一键上下文、非法上下文、非递增候选版本、完整/摘要
两种权限投影、前后差异和固定样例 JSON 回归快照。页面按钮、活动/诊断入口和运行时命令接线仍由 W11-08 完成；本项只证明共享
测试器契约可被 UI 和运行时共同消费，不宣称测试器页面已经上线。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-common --lib domain::traffic_policy_test`（7 passed）、
`cargo test -p fuckipv6-common --lib domain::traffic_`（34 passed）、
`cargo test -p fuckipv6-common --lib`、`cargo check --workspace`、`cargo test --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings`、`python3 scripts/check-docs.py` 和 `git diff --check`。

**验收**：管理员和 owner 能在应用前确认流量会被送到哪里，测试结果与运行时一致。

### W11-06 原子应用、差异与回滚 `[x]`

- [x] 编辑草稿与已应用版本分离；
- [x] 应用前生成规则、默认策略和影响对象差异；
- [x] 服务端校验 schema、目标存在、权限和平台能力；
- [x] 使用版本/条件写避免覆盖并发修改；
- [x] 路由、DNS、TUN 配置使用事务或补偿事务；
- [x] 失败恢复上一可用版本；
- [x] 活动记录应用者、版本、结果和回滚原因；
- [x] 覆盖进程崩溃、断线、非法配置和版本竞争。

草稿不携带 revision，preview 只在服务端权威快照上计算 successor；应用时在文件锁内重新检查 expected revision，条件写失败不会产生运行时或审计副作用。
候选先经过 `TrafficPolicy::validate` 和 successor 校验，再由可信上下文确认节点身份、Network/Exit 目标、应用身份能力与 TUN 可用性；权限拒绝、非法配置、陈旧 revision
均在 pending journal 和运行时修改前终止。差异按稳定规则 ID 比较新增、删除、更新和重排，同时报告默认策略、失败策略、规则、Network、Exit 和相关风险；
无实际变化时不会推进 revision、写历史或调用运行时。

路由、DNS 和 TUN 副作用统一位于 `TrafficPolicyRuntimeAdapter` 之后。事务在修改运行时前持久化 pending journal，依次执行 apply 与 verify，成功后先写 generation/history，
最后原子切换权威 state pointer；apply、verify 或持久化失败会恢复上一可用策略并记录失败阶段、稳定原因码和消息。若补偿本身失败，错误同时包含原始与回滚诊断，
pending journal 保留，`current` 返回 `RecoveryRequired` 而不是暴露半配置状态。启动恢复会区分提交点前后：未提交事务恢复上一策略，已提交事务只清理残留 journal。
回滚不是倒填旧版本，而是把目标内容作为新的递增 revision 再经过完整校验、运行时应用与验证，并在历史中记录目标版本、应用者、成功/失败结果和失败原因。

测试覆盖预览无副作用、成功 apply/verify 顺序、默认策略与目标影响差异、版本竞争、无变化、权限/非法候选/缺失目标/TUN/应用能力拒绝、apply 与 verify 断线补偿、
补偿失败保留 journal、提交前后崩溃恢复、成功回滚新 revision、失败回滚审计，以及事务目录与安全策略存储隔离。真实 TUN、系统路由和 DNS 适配器尚未接入本事务，
本项只证明可注入的原子/补偿边界与持久化恢复契约，不宣称这些副作用已在真实主机闭环，自动恢复由 W11-07 继续。

**验证通过（2026-11-08）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-client --lib traffic_policy_transaction --no-default-features`（16 passed）、
`cargo check --workspace`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
`python3 scripts/check-docs.py` 和 `git diff --check`。

**验收**：事务只暴露一个权威已应用 revision；失败会恢复上一可用策略，无法补偿时保留恢复日志并拒绝读取半配置状态。

### W11-07 路由与 DNS 自动恢复 `[x]`

- [x] 恢复协调器在 `SleepWake` 触发器下重新验证 TUN、路由和 DNS；
- [x] 恢复协调器在 `NetworkChanged` 触发器下重新评估策略并重配 DNS；
- [x] `ExitDisconnected`/`RelayDisconnected` 撤销失效路径，并按版本化失败策略处理；
- [x] 可注入清理接口覆盖路由残留、重复路由和失效接口路由；
- [x] DNS/DoH/DoT 配置失败不静默使用不安全 fallback；
- [x] 整轮恢复共享硬超时，默认 10 秒且可配置；
- [x] 记录脱敏恢复前后快照、稳定诊断 ID 和分阶段探测结果；
- [x] 恢复后验证 Google/YouTube 或部署受控目标可达性。

恢复协调器先采集前后资源快照，再按触发类型撤销失效路径、协调 TUN/路由/DNS 并重新校验资源。DNS 不安全 fallback、失效路径撤销失败和恢复后资源不健康会强制 fail-closed；
DNS 不安全 fallback 时不执行可达性探测，避免把“探测偶然成功”当作安全证明。Exit 断开按 `ExitUnavailable` 决策，Relay 断开按
`NetworkSelectionUnavailable` 决策；撤销失败会覆盖任何 direct fallback 许可。报告包含诊断 ID、前后快照、修复计数、撤销路径、失败决策、降级状态、
探测目标/阶段/耗时和稳定错误码，不包含接口名、地址、resolver URL、命令输出或目标主机名。

- [x] 接入真实 OS 休眠/唤醒和网络变化监听源；
- [x] 接入生产 TUN、系统路由和 DNS/DoH/DoT 快照/协调适配器；
- [x] 接入生产 Exit/Relay 路径撤销、审计持久化和受控可达性探针；
- [x] 在隔离主机/network namespace 中加入真实休眠、切网、Exit/Relay 断开和 DNS 超时故障注入。

主循环通过有界恢复事件队列消费 Linux D-Bus 事件：systemd-logind 仅在 `PrepareForSleep(false)` 恢复时触发 `SleepWake`，
NetworkManager 仅在稳定的本地、站点、全局或 VPN 已连接状态触发 `NetworkChanged`；事件源独立重连，不可用时显式报告而不冒充监听成功。
Exit 或 Relay 断开会在真实传输状态变化后入队，陈旧 generation 不会产生误触发。生产 `LinuxRecoveryAdapter` 读取实际 TUN、策略路由和
DNS/DoH/DoT 资源，按 TUN → 策略路由/DNS 顺序补偿，并在恢复后重新采集快照验证。

固定 Exit 使用已认证 Peer 解析出的长期 `NodeId`，不会因 Temporary 下线丢失身份；已配置但无法解析的 Exit 使用 `Reject`，代理模式缺少
Network 时同样拒绝，MeshOnly 才允许 Direct。所有运行时恢复请求使用 fail-closed 失败策略，Exit/Relay 触发只撤销对应路径，恢复结束后
重新同步真实路径状态。失败决策写入网络配置目录下的 `recovery-audit.jsonl`，目录权限为 `0700`、文件为 `0600` 并使用
`O_NOFOLLOW`；日志和审计仅保留诊断 ID、触发类型、状态、错误码、修复计数和探测阶段，不输出接口名、地址、resolver URL、
命令输出或目标主机名。默认可达性探针使用 Google/YouTube，部署受控目标只能由显式配置提供。

**验证通过（2026-11-09）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-client --lib routing::recovery --no-default-features`（11 passed）、
`cargo test -p fuckipv6-client --lib cli::commands::tests --no-default-features`（32 passed）、
`cargo test -p fuckipv6-client --test recovery_linux --no-default-features`（5 ignored，需特权拓扑）、
`cargo check --workspace`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
`python3 scripts/check-docs.py` 和 `git diff --check`。普通用户执行 `./scripts/test-recovery-netns.sh` 返回 77 并要求 root；
使用 root 后在私有 network/mount namespace 中实际执行 5 项故障注入，全部通过：休眠唤醒重建 TUN/路由/DNS、切网恢复策略路由与 DNS、
Exit 断开失败关闭、Relay 断开失败关闭，以及真实 UDP DNS 超时被识别为 `probe.dns_timeout` 并进入 fail-closed 降级。

**验收**：Linux 生产运行时已由真实 OS 事件触发，在硬超时内完成 TUN、策略路由和 DNS 快照、补偿、失败关闭、安全审计与分阶段可达性验证；
隔离拓扑证明常见故障不会静默绕过失效路径。

### W11-08 流量路由页面 `[~]`

- [x] 默认策略、固定 Exit 和规则列表位于同一权威页面；
- [x] 支持拖拽/键盘调整顺序并有等价按钮；
- [x] 草稿、应用中、已应用、失败、陈旧状态清晰；
- [x] 规则条件使用结构化编辑，不要求手写底层配置；
- [x] 目标对象可搜索和跳转；
- [x] 删除/停用规则显示活动影响和命中样例；
- [ ] GPUI/WebUI/Android 布局和语义一致；
- [x] 不在节点页复制第二套编辑器。

WebUI 的 routing 页面只消费 `snapshot.traffic_routing_page`；页面缺失时明确显示权威模型不可用，不构造本地假策略。
默认策略、失败策略、固定 Exit/网络约束、规则、目标、revision 和工作流状态位于同一页面。规则支持结构化编辑
精确域名、域名后缀、目的 CIDR、端口范围、协议，以及仅限 `package_id`/`executable_path` 的应用身份；拖拽、
`Alt+ArrowUp/Down` 和上移/下移按钮共用同一稳定 ID 重排逻辑。测试、预览、应用、历史、回滚预览与确认均使用
manifest action 的 `backend_status` 门禁，并在请求中携带权威 revision；目标搜索只匹配服务端提供的 `search_terms`，
跳转只使用服务端提供的 `page_id`/`network_name`。

自动化用 fake backend 跑通测试 → 预览 → 应用 → 历史 → 回滚预览 → 回滚确认的 WebUI 闭环，并验证结构化测试输入、
current/candidate revision、应用与回滚 payload。页面契约现已区分观测源可用与不可用：可用窗口同时携带观测流量总数和
权威命中样例，零命中明确表示为“已观察且未命中”，不可用时才显示“无权威观测数据”，UI 不从本地配置推断命中。
GPUI 的 routing 页面同样只消费 `snapshot.traffic_routing_page`，页面缺失时显示权威模型不可用，不从旧
`configuration.networks` 回退构造策略。页面展示 authority revision、工作流、默认与失败策略、硬约束、只读状态、规则、
风险、命中影响、观测窗口/样例以及 activity/diagnostic/flow 跳转；规则和目标的顺序、活动影响与命中样例均使用同一
权威页面模型。GPUI 目标搜索只调用 `TrafficRoutingPageModel::search_targets` 匹配服务端提供的名称与别名，并优先按
服务端 `object_id` 打开精确拓扑节点，缺少对象身份时才回退到网络边界。新增测试覆盖页面缺失 fail-closed、权威字段/
风险/目标投影、observed-zero-hit 与 unavailable 区分、命中样例链接、权威搜索和精确导航；GPUI `gpui::` 测试共
124 passed。Android UI 尚不存在，三端一致项由 W16 阻塞，不能据此勾选。

当前生产 manifest 仍只有 `test_traffic_policy` 为 `implemented`，其余流量策略写操作保持 `unavailable`；生产运行时
仍未设置 `traffic_routing_page`，也未提供真实逐流 observed hits，因此 WebUI/GPUI 目前只完成权威契约与页面投影闭环，
生产后端和真实平台数据面仍未闭环，完整验收仍未满足。

**验证通过（2026-11-09）**：WebUI `npm test -- --run`（270 passed，其中流量路由工作流 12 passed）、
`npm run typecheck`、`npm run lint`、`npm run build`、`cargo check -p fuckipv6-client --lib`、
`cargo test -p fuckipv6-client --lib 'gpui::'`（124 passed）、`cargo test --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings`、`node scripts/test_i18n_runtime.mjs`、
`./scripts/check-contracts.sh`、`python3 scripts/check-docs.py`、`cargo fmt --all -- --check` 和
`git diff --check`。

**验收**：用户能完成从规则设计、测试、应用、观察到回滚的完整闭环。

### W11-09 路由测试与发布证据 `[x]`

- [x] 单元测试覆盖顺序、AND/OR、边界和未知应用；
- [x] 集成测试覆盖 TUN/路由/DNS 应用与补偿；
- [x] 失败注入覆盖休眠、切网、Exit/Relay 断开和 DNS 超时；
- [x] E2E 覆盖域名和应用分流及回滚；
- [x] 验证拒绝/失败关闭时无真实源 IP 泄漏；
- [x] 验证固定 Exit 不可被 fallback 绕过；
- [x] 网络报告包含规则、目标、结果和清理情况；
- [x] 未通过前不宣称翻墙、分流或抗干扰可用。

**验证通过（2026-11-09）**：`cargo test -p fuckipv6-common --lib domain::traffic_policy`（26 passed）和
`cargo test -p fuckipv6-common --lib domain::traffic_policy_test`（7 passed）。测试直接调用生产领域求值器，覆盖首条命中顺序、
字段内 AND/值列表 OR、IPv4/IPv6 CIDR 与域名标签边界、未知应用不猜测，以及顺序 API 往返后仍产生相同运行时决策。

`cargo test -p fuckipv6-common --lib domain::traffic_failure`（8 passed）还验证了显式 direct fallback 配置下，固定 Exit、
固定网络和 Reject 规则仍分别以 `FixedExit`、`FixedNetwork`、`RejectRule` 失败关闭，且不会产生 fallback 记录。

`unshare -Urnm --propagation private -- ./scripts/test-lifecycle-cleanup-netns.sh`（1 passed）在私有 network/mount namespace
中创建真实 TUN，安装 policy rule、路由表和隔离 `/etc/resolv.conf`，注入首次路由规则删除与 TUN 删除失败，验证 DNS 恢复、
保留资源的补偿重试、成功清理及幂等清理。该测试同时发现并修复了 TUN 在 link up 前添加设备路由会失败的真实初始化顺序问题。

`unshare -Urnm --propagation private -- ./scripts/test-recovery-netns.sh`（5 passed）在私有 network/mount namespace 中使用真实
TUN、policy rule、路由表和隔离 `/etc/resolv.conf`，分别注入休眠唤醒后的 TUN/路由丢失、切网后的路由与 DNS 丢失、Exit
监听断开、Relay 监听断开，以及真实 UDP DNS 查询超时。测试直接调用生产 `RecoveryCoordinator` 与 Linux TUN/路由适配器，
验证恢复/失败关闭状态、分阶段探测、脱敏失败决策审计和清理；不访问或修改宿主网络。

`./scripts/test-traffic-routing-netns.sh`（1 passed）在私有 user/network/mount namespace 中建立四条真实 veth 路径和合成 DNS：
`domain.test`、`app.test`、`fail.test`、`blocked.test` 分别解析到稳定测试地址。测试直接调用生产 `TrafficPolicy::evaluate`
与 `decide_failure`，再由目标策略路由和 UID 策略路由执行域名/应用分流；nftables 拒绝未批准输出。上游 peer 记录实际到达的
TCP 源地址，得到域名路径 `10.10.0.2`、应用 UID 路径 `10.20.0.2`，策略回滚后同一域名切换为 direct 路径 `10.30.0.2`。
Reject 与 blackhole 两类失败关闭连接均未到达上游，报告记录 `attempted_connections=2`、`blocked_connections=2`、
`leaked_connections=0`；不存在真实源地址泄漏记录。

同一 E2E 生成脱敏 `report.json`：每个 flow 含 kind、可选命中的 `rule_id`、域名、目标 IP/端口、具体 action、原因、结果与
观测源地址；默认 Reject 以 `rule_id: null` 明确表示没有猜测规则。报告还包含回滚前后源地址、失败关闭统计和清理状态。
测试只在清理完成后通过，当前记录 `nft_table_removed=true`、`policy_rule_removed=true`、`namespaces_removed=true`、
`residual_resources=0`。`cargo test -p fuckipv6-client --no-default-features --lib routing::traffic_report`（1 passed）
验证报告序列化/反序列化及字段约束。

以上证据仅证明 Linux 隔离测试拓扑中的策略语义、分包路径、失败关闭和清理，不代表生产 Exit/Relay 后端已经接线，也不代表
GPUI、Android、常见受限网络或一般抗干扰能力已验收；在相应平台与生产适配证据完成前不宣称这些能力可用。

**验收**：路由声明由可复核测试支撑；任何静态规则示例都不能代替运行时证据。

## 15. W12：Exit

目标：Exit 是节点自主提供、网络治理授权、用户显式选择的互联网出口。必须验证真实可访问性、DNS/IPv6 泄漏、性能和稳定性。

### W12-01 本机 Exit 能力启停 `[x]`

- [x] 节点 owner 可独立启用/关闭本机 Exit 能力；
- [x] 启停是 operation，展示监听、健康检查和清理阶段；
- [x] 关闭后停止接受新会话并优雅处理已有会话；
- [x] 节点本地拒绝优先于网络管理员要求；
- [x] 启停不误停 Coordinator、Relay 或 File；
- [x] 资源不足、平台不支持、权限不足有结构化原因；
- [x] 不要求管理员远程开启节点本机能力；
- [x] 覆盖崩溃、重启和残留监听清理。

本地 owner 通过 `CapabilityEnable`/`CapabilityDisable` operation 启停本机 Exit，operation 与权威快照返回同一 `LocalExitCapability`；
GPUI 和 WebUI 均展示 `waiting_for_network_authorization`、`listening`、`health_checking`、`enabled`、`draining`、`cleaning`、`failed`
等权威阶段以及结构化 problem。网络 Owner、管理员等远程身份不能修改节点本机能力，本地 veto 始终优先于网络 Exit 角色。
Linux forwarder 进入 drain 后先 fence 新客户端并保留已有 flow 资源，残留清理覆盖 IPv4/IPv6 且可幂等重试；重启只恢复 owner intent，
仍须重新获得网络授权。能力隔离测试确认启停前后 Coordinator、Relay、File、embedded services、网络角色、peer 数和网络启动状态不变；
权限不足、平台不支持、资源不足和一般不可用均保留稳定原因、可重试性和诊断信息。

**验证通过（2026-11-09）**：`cargo fmt --all -- --check`、
`cargo test -p fuckipv6-client --no-default-features --lib local_exit`（25 passed）、
`cargo test -p fuckipv6-client --no-default-features --lib routing::linux_exit::tests::`（21 passed，2 ignored，需隔离特权 netns 拓扑）、
`cargo test -p fuckipv6-client --all-features --lib gpui::view_model::tests::exit_node_page_projects_authoritative_local_exit_lifecycle -- --exact`
（1 passed）、`cargo check --workspace`、`node scripts/test_i18n_runtime.mjs`、WebUI `typecheck`、`lint`、
`npm test -- --run`（271 passed）和 `npm run build`（仅保留现有 i18n 非 module 与 >500 kB chunk 警告）。
相关提交：`29b75bc`、`1230aad`、`01bd928`、`939266f`、`6f499df`、`ce15a9a`、`09c7b4e`、`484b259`。
未执行需要隔离特权的 `linux_netns_*` 用例，也未做真实平台崩溃/防火墙清理实测；这些边界留给后续稳定性与跨平台实机验收。

**验收**：Exit 能力的所有权边界清晰，可由本机可靠启停且不影响其他能力。

### W12-02 贡献策略与成员告知 `[x]`

- [x] 网络支持自主贡献、加入即贡献、禁止贡献和申请授权模式；
- [x] 策略展示带宽、流量、隐私和运营影响；
- [x] 加入前和策略变更后要求知情确认；
- [x] 节点 owner 可限制可用网络、成员范围、时段和配额；
- [x] 管理员可要求贡献但不能覆盖本地否决；
- [x] 实际状态与声明能力不一致时标记降级；
- [x] 所有同意、拒绝、撤回和策略版本审计；
- [x] 成员列表展示真实 Exit 可用性。

网络策略使用 `autonomous`、`required_on_join`、`disabled`、`approval_required` 四种稳定模式，`NetworkCreateProfile`、
`MembershipTerms` 和运行时上下文统一携带模式与 content hash。邀请、加入预览和策略重确认均展示带宽、流量、隐私、运营四项影响以及
确认/授权要求；普通条款必须确认，`required_on_join` 额外要求独立的 Exit 同意，`autonomous` 与 `approval_required` 只披露事实、不强制
加入时承诺。缺少精确版本确认时 `ExitContributionGate` 返回 `PolicyConfirmationRequired` 并拒绝贡献，而不是仅在 UI 提示。
节点 owner 的网络、成员范围、时段、带宽和配额限制与本地 veto 独立生效，管理员要求不能覆盖本地否决。成员目录展示声明状态与观测状态、
来源、时间戳、新鲜度及原因；声明可用但实际不可用、实际可用但声明不可用、缺失或陈旧证据分别显示降级/未知，不把未知当作可用。
Exit 申请的提交、审核、撤回、过期和策略 supersede 均写入持久审计；加入同意、拒绝与策略确认记录精确 terms hash、模式、时间和
同意事实，命令审计同时记录网络创建、策略更新、成员快照及 rejected problem 中的 `exit_contribution_mode`。

**验证通过（2026-11-09）**：`cargo fmt --all -- --check`、`cargo check --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`（1683 passed、10 ignored）、
`./scripts/generate-contracts.sh`、`./scripts/check-contracts.sh`、`node scripts/test_i18n_runtime.mjs`、WebUI `typecheck`、`lint`、
`npm test -- --run`（276 passed）和 `npm run build`（仅保留现有 i18n 非 module 与 >500 kB chunk 警告）。
相关提交：`59312ee`。本项不宣称 W12-03 至 W12-07 的治理、健康、故障切换、DNS/IPv4/IPv6 泄漏、Google/YouTube 或性能稳定性验收完成。

**验收**：不存在隐式 Exit 贡献；治理策略与节点实际状态可核对。

### W12-03 管理员出口治理 `[x]`

- [x] 管理员可定义默认 Exit 和可用 Exit 集合；
- [x] 管理员可指定固定 Exit 或允许成员自行选择；
- [x] 管理员可控制普通成员可选择哪些 Exit；
- [x] 策略支持网络级、组级和成员级范围，但拒绝优先；
- [x] 被撤权 Exit 立即退出新会话候选；
- [x] 已建立会话按策略迁移、终止或失败关闭；
- [x] UI 显示最终生效集合及来源；
- [x] 覆盖管理员撤权、节点离线、策略竞争测试。

网络创建、预览和更新统一携带版本化的 `exit_selection_policy`，支持 `member_choice`/`fixed`、默认 Exit、网络可用/拒绝集合，
以及组级和成员级 allow/deny 收窄。所有作用域按 deny-first 求值，固定 Exit 同时投影为路由硬约束；旧客户端创建时默认空策略，
旧版更新未提供策略时保留现有治理，不把兼容载荷解释为撤权。网络策略 revision 随治理变更递增，持久快照、成员目录和审计记录
引用同一 revision；普通成员收到的成员目录不包含组/成员规则，只允许查看自己的最终 resolution，管理员和本地 owner 才能查看完整策略。

运行时 `ExitSelectionGate` 只接受网络 ID 匹配、revision 非零、策略有效且目录生成/接收时间在 90 秒内的成员目录；低 revision 或同
revision 的旧 generation 不覆盖新权威。候选同时要求成员仍在 roster、生命周期允许访问、在线且可用性证据新鲜；未知、陈旧、
离线、降级、撤权或不在 roster 的 Exit 均不可选。请求固定 Exit 之外的本地选择会返回 `domain.exit_selection_fixed_exit`，不能在
fallback 或 recovery 中静默绕过；既有会话按策略生成 `migrate`、`terminate` 或 `fail_closed` 转换，迁移没有合法目标时失败关闭。
节点快照复用同一运行时解析函数，目录缺失或过期时保留最后 revision 供审计但清空 resolution；WebUI 和 GPUI Exit 页面展示模式、
默认/固定 Exit、最终可选集合、候选状态、稳定 reason/source、可用性证据以及目录时间，不在客户端重新计算治理结果。

**验证通过（2026-11-10）**：`cargo fmt --all -- --check`、`cargo check --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test -p fuckipv6-common exit_selection --lib`（11 passed）、
`cargo test -p fuckipv6-client exit_selection --lib`（13 passed）、
`cargo test -p fuckipv6-client exit_node_page_projects_authoritative_selection_set_and_evidence --lib`（1 passed）、
`./scripts/generate-contracts.sh`、`./scripts/check-contracts.sh`（含 TypeScript/Java 绑定与 Android 契约测试）、
`node scripts/test_i18n_runtime.mjs`、WebUI `npm run typecheck`、`npm run lint`、`npm test -- --run`（278 passed）、
`npm run build`（仅保留现有 i18n 非 module 与 >500 kB chunk 警告）和 `npm run visual:regression`（semantics、84 项 geometry、
pixels 全部通过，geometry max delta 2.1875px；未放宽阈值）。测试覆盖 deny-first、固定 Exit 绕过拒绝、撤权成员即时退出、
节点离线/未知/陈旧证据、revision 与 generation 竞争、三种已建立会话转换、普通成员隐私投影，以及 WebUI/GPUI 的最终集合、
候选来源和过期目录展示。
相关提交：`2c10552`、`3653622`、`78da35a`、`1388a7d`、`349abdb`。本项证据来自领域/运行时/协议与 UI 自动化测试，不代表已完成
真实多节点 Coordinator 治理竞争演练、真实数据面 Exit 切换、DNS/IPv4/IPv6 泄漏、Google/YouTube 或性能稳定性验收。

**验收**：用户可解释为何某 Exit 可选/不可选；固定 Exit 不可被本地或 fallback 静默绕过。

### W12-04 Exit 健康与可达性 `[x]`

- [x] 健康检查区分进程、监听、网络、DNS 和互联网可达；
- [x] 使用短超时、并发上限和退避，避免探测造成负担；
- [x] 健康结果带时间戳、新鲜度和来源；
- [x] 不可用 Exit 不进入默认候选；
- [x] 部分可用状态明确哪些目标/协议受影响；
- [x] 健康检查不向第三方泄漏成员身份；
- [x] 支持手动复测并取消；
- [x] 建立误报、抖动和恢复测试。

新增的 `ExitHealthSnapshot` 按进程、监听、网络、DNS、互联网可达五层保存证据。每条证据携带 passed/degraded/failed/unsupported/not_tested/unknown
状态、来源、观测时间、耗时、可选目标/协议和稳定错误码；默认 90 秒新鲜度并拒绝超出 5 秒的未来时钟偏移。只有五层都存在新鲜且
passed 的证据时，`default_candidate` 才为真；缺失、陈旧、降级、失败和未测试均不可进入默认候选，并通过稳定 reason code 与受影响
层/目标/协议解释原因。探测策略将单次超时限制在 5 秒、并发限制在 8、尝试次数限制在 5，并使用有界指数退避和确定性抖动。

手动诊断运行会生成新的分层快照，复测状态机明确区分 requested/running/cancelled/completed/failed；取消是终态，不能被后续完成覆盖。
运行时不会从健康进程推断 DNS 或互联网可达，二者保持 `not_tested`，直到受控探测提供新证据。诊断 DTO 由后端计算
`ExitHealthAssessment`，WebUI 和 GPUI 只展示该权威结果及逐层证据，不在客户端重新判定健康。目标 ID 只允许不透明的稳定 token，错误码
禁止包含命令输出、地址或成员身份；可复制诊断报告只包含稳定摘要，不导出原始探测细节。

**验证通过（2026-11-10）**：`cargo test -p fuckipv6-common exit_health --lib`（8 passed）、
`cargo test -p fuckipv6-client diagnostic --lib`（14 passed）、`cargo test -p fuckipv6-client exit_health --lib`（1 passed）、
`cargo clippy -p fuckipv6-common -p fuckipv6-client --lib -- -D warnings`、`./scripts/generate-contracts.sh`、
`./scripts/check-contracts.sh`（含 TypeScript/Java 绑定与 Android 契约测试）、`node scripts/test_i18n_runtime.mjs`，
以及 WebUI `npm run typecheck`、`npm run lint`、`npm test -- --run`（280 passed）、`npm run build`（仅保留现有 i18n 非 module 与
>500 kB chunk 警告）。最终回归 `cargo test --workspace --lib` 为 1752 passed、3 ignored。测试覆盖五层完整性、新鲜度/时钟偏移、未知与陈旧不可选、部分失败目标/协议、探测边界和退避、误报/漏报恢复、
取消终态、进程健康但 DNS/互联网未测试，以及 WebUI/GPUI 权威状态和逐层证据展示。

本项证据来自领域、运行时、协议和 UI 自动化测试，不代表已执行真实受控 Exit 探测、真实泄漏服务、浏览器/应用 DNS 行为审查、
真实数据面探测或真实多节点 Exit 故障演练；这些未测试路径没有被标记为 passed。

**验收**：候选选择基于新鲜健康证据；未知/陈旧不会被当成健康。

### W12-05 Exit 故障、切换与撤权 `[x]`

- [x] 连接超时、握手中断、运行中断开分别处理；
- [x] 按策略选择替代 Exit 或失败关闭；
- [x] 切换过程展示过渡状态，不伪报已连接；
- [x] 撤销授权后禁止新连接并终止/迁移既有连接；
- [x] 失败关闭时停止受影响流量且不泄漏源地址；
- [x] 恢复后重新验证 DNS、TUN 和路由；
- [x] 活动记录故障、切换和最终状态；
- [x] 覆盖断网、后台、休眠和重启场景。

Exit 连接 watchdog 将“目标 Exit 已选择但 Secure Session 尚未完成密钥确认”保持为过渡态；只有已建立的安全会话才能把恢复路径标为 `connected`，
否则在期限到达时只发出一次 `ExitConnectionTimeout`，撤销陈旧 Exit 路径并继续失败关闭。连接超时、握手中断、Exit/Relay/Runtime 断开分别映射到
稳定 trigger、failure kind 和 reason code；替代 Exit 必须先通过管理员授权、健康与新鲜度门禁，固定 Exit 或缺失授权时不会静默直连。授权目录或
成员条款变化会撤销待提交选择并进入迁移/重连，既有流量在 TUN、路由、DNS 和可达性重新验证完成前不可承载，只有完整新鲜证据才能提交
`Protected`。活动快照记录迁移开始、故障、失败关闭/显式直连和最终受保护状态，WebUI/GPUI 消费同一权威状态。Linux suspend/resume、稳定网络
变化和运行时断开均接入有界恢复事件通道；重启不会沿用旧的受保护声明。

**验证通过（2026-11-10）**：`cargo test --workspace --lib`（1752 passed、3 ignored）、
`cargo test -p fuckipv6-common traffic_session --lib`（8 passed）、
`cargo test -p fuckipv6-client routing::recovery --lib`（15 passed）、
`cargo test -p fuckipv6-client cli::commands::tests --lib`（42 passed）、
`cargo test -p fuckipv6-client node::runtime_control --lib`（76 passed）、
`cargo test -p fuckipv6-client --all-features --lib gpui::view_model::tests:: -- --test-threads=1`（61 passed）、
`cargo clippy -p fuckipv6-common -p fuckipv6-client --lib -- -D warnings`、`cargo check --workspace`、
`cargo fmt --all -- --check`、`./scripts/generate-contracts.sh`、`./scripts/check-contracts.sh`、`node scripts/test_i18n_runtime.mjs`，
以及 WebUI `npm run typecheck`、`npm run lint`、`npm test -- --run`（280 passed）、`npm run build`（仅保留现有 i18n 非 module 与
>500 kB chunk 警告）。`npm run visual:regression --prefix webui` 单独执行：语义检查通过，但概述页仍存在既有几何/像素基线差异
（内容纵向偏移约 68px），未计入本项功能通过证据。测试覆盖超时只触发一次、安全会话门禁、撤权迁移、固定 Exit 失败关闭、
SleepWake/NetworkChanged 重验 TUN/路由/DNS、故障活动证据和 GPUI 权威投影。

相关提交：`29dd41b`、`1711082`、`3cde450`、`94393ce`、`22fdc3b`、`08b9c7d`、`5a177a7`、`86cce8e`。
本项证据来自领域、Linux 恢复、命令层、运行时和 UI 自动化测试；未执行真实多节点 Exit 故障、真实休眠/唤醒、真实后台进程或完整重启演练，
这些实机路径留给 W12-09/W17，不把自动化模拟扩大解释为生产网络已稳定可用。

**验收**：Exit 故障不会静默直连；用户能知道当前是否仍有受保护连接。

### W12-06 DNS、IPv4 与 IPv6 泄漏 `[x]`

- [x] 明确 Exit 支持的 DNS 模式：系统、DoH、DoT 或网络指定；
- [x] 验证系统 resolver 和浏览器/应用 DNS 行为；
- [x] 检查 IPv4 源地址与默认路由；
- [x] 检查 IPv6 是否有可用泄漏路径；
- [x] 禁用/隔离不可由网络承载的协议与接口；
- [x] 失败关闭覆盖 DNS 查询失败和 IPv6 连接；
- [x] 使用受控泄漏检测服务和外部可达性目标复核；
- [x] 报告保存查询目标、结果和环境，避免只写“无泄漏”。

已建立 `LeakDetectionSnapshot`/`LeakDetectionAssessment`：环境记录平台、DNS 模式（`system_resolver`、`doh`、`dot`、
`network_specified`、`unmanaged`）、受保护路径状态和 IPv6 carrier 可用性；证据按 DNS、IPv4、IPv6 与稳定目标记录
passed/failed/unsupported/not_tested、来源、时间、耗时和稳定错误码。Linux collector 使用 2 秒上限的 `ip -4/-6` 检查受保护接口上的
global 地址、源地址、默认路由和 catch-all 策略表；缺失、陈旧、未来时钟、重复、未测试或不受控 DNS 均失败关闭，未承载的 IPv6 只有带
`blocked_unavailable_ipv6` 隔离动作时才可标为 isolated。评估采用最坏优先，泄漏失败或 required check 失败不可被其他 passed 证据覆盖；
错误码和公共报告不包含地址、resolver URL、接口名、命令输出或成员身份。受控服务和浏览器/应用行为 probe 已有明确注入边界，
但生产默认 runner 仍是 noop，因此真实外部检查会保留 `not_tested` 并触发 `fail_closed_required`。

**验证通过（2026-11-10，自动化边界）**：`cargo test --workspace --lib`（1752 passed、3 ignored）、
`cargo test -p fuckipv6-common leak_detection --lib`（6 passed）、
`cargo test -p fuckipv6-client routing::leak_detection --lib`（6 passed）、
`cargo test -p fuckipv6-client node::runtime_control --lib`（76 passed，含泄漏证据聚合/投影）、
`cargo test -p fuckipv6-client diagnostic --lib`（14 passed）、
`cargo clippy -p fuckipv6-common -p fuckipv6-client --lib -- -D warnings`、`cargo check --workspace`、
`cargo fmt --all -- --check`、`./scripts/generate-contracts.sh`（生成资产无变化）、`./scripts/check-contracts.sh`、
`node scripts/test_i18n_runtime.mjs`，以及 WebUI `npm run typecheck`、`npm run lint`、`npm test -- --run`（280 passed）、
`npm run build`（仅保留现有 i18n 非 module 与 >500 kB chunk 警告）。

**实机复核（2026-11-10，`root@xlei.cc`，Linux，`w12-hk`）**：部署 release 二进制的 SHA-256 为
`49efc97f42c40cc4e63fa34ac724cc1e88af983f4fb378756fb82bc0bff73971`，以无头 WebUI
`127.0.0.1:18081`运行。替换前旧进程运行时，CLI 可稳定复现审计账本跨进程锁错误
`Resource temporarily unavailable (os error 11)`；替换后在同一 WebUI 运行期间连续 5 次
`status --json` 均为 rc=0、12–15ms、stderr 为空，再并发 10 次读取全部 rc=0（墙钟 128ms，
单次 67–105ms、stderr 全空），未再出现审计锁错误。`exit disable`（约 5.1s）与
`exit enable`（54ms）均 rc=0，raw snapshot 和 `exit status --json` 的 owner intent、stage、
healthy、network_allowed 一致，覆盖了共享 ledger 的真实写路径。运行日志只有已知的无 TTY
owner 初始化锁定 WARN；未出现身份绑定误报、postcard/decode、错误撤权或虚假 Protected。

同一实机执行 `down w12-hk`（79ms）后 TUN、16666/16667 监听立即消失，节点 IPC 与 raw snapshot
仍可用；raw `stop_networks`（111ms）后 lifecycle 为 `active_networks=0`、接口离线、本机 Exit
进入 waiting/disabled。随后 raw `start_networks`（1666ms）恢复 TUN、`10.8.0.1/24`、
`fd00::1001/128`、IPv4/IPv6 路由和两项监听，lifecycle 恢复 `active_networks=1`，Exit 恢复
enabled。`up w12-hk` 也恢复了数据面与地址，但命令在 1627ms 后因既有 WebUI 的独占表面租约返回
rc=1；这属于“先完成网络变更、再启动第二个 UI 失败”的部分成功风险，本轮未将其计为 CLI 启停通过，
也不修改既有“子入口启动 UI”的策略。

`diagnostics --json`（28ms）确认当前没有受保护流量且没有 Exit candidate 时，报告不是伪装成功：
leak assessment 为 `leak_detected`、`fail_closed_required=true`，DNS 为 insecure fallback，
IPv4 默认路由位于受保护路径之外；browser/application DNS、IPv4/IPv6 受控泄漏服务和外部可达性
目标全部保留 `not_tested`/`probe_not_configured`，IPv6 默认出口为 `unsupported`。脱敏报告未包含
真实 IP、TUN 名、resolver URL 或节点身份。WebUI 因远端无 TTY 未能完成 owner 初始化，API 返回
401，因此没有真实浏览器/应用 DNS 证据；当时 `peer_count=0` 且 `selectable_exits=[]`，也无法执行
经 Exit 的外部复核。测试结束后已优雅终止顶层 control、detached control/node 与残留测试监听，确认
16666/16667/18081/18082 均无监听且 `fk-w12-hk` 已删除；仅清理 `runtime/w12-*` 和 `.err` 临时输出，
配置、审计账本和正式日志保留。

**补充实机复核（2026-11-11，香港 Exit 数据面）**：拓扑为
`w12-client` 网络命名空间（`10.8.0.2`、`fd00::1002`）→ Secure Session/信令中继 →
`root@xlei.cc` 上的 Exit Node（`10.8.0.1`、`fd00::1001`，公网 `101.36.116.21`）→ 外部目标。
远端主机确认存在 `fk-w12-hk` TUN、16666/16667 协调/中继监听；本机 `status --json` 为
`traffic_session.state=protected`，TUN/路由/DNS/reachability 四项校验均为 `true`，`peer_count=1`。
本轮使用 release 二进制的 SHA-256 为
`75bcfd7fea611024497b08e8966fd685ce1552cd3b7de8aaf1792d92ee065c92`，配置和运行时目录分别为
`/tmp/w12-client-config`、`/tmp/w12-client-runtime`。

`diagnostics --json` 生成 `diag-1794337060397-00000001`：`dns_resolver`、`browser_application_dns`、
IPv4 controlled/external、IPv6 controlled 均为 `passed`；`www.youtube.com` 和 `googlevideo.com` 的真实
Chrome 查询经 `127.0.0.1 -> 127.0.0.2:53` 被 DNS pcap 捕获。IPv4 受控服务验证源地址为 `10.8.0.2`，
外部 `ifconfig.me` 返回 Exit 公网出口 `101.36.116.21`；IPv6 受控 echo 返回 `fd00::1002`。IPv6 外部目标
`ipv6.google.com` 的 probe 以 `leak_detection.probe_timeout` 结束并保留为 `not_tested`，独立 curl 也在 25 秒内不可达。
因此评估结果明确为 `insufficient_evidence`、`affected_protocols=["ipv6"]`、
`affected_targets=["ipv6_external_target"]`、`fail_closed_required=true`；本项不据此宣称“无泄漏”或 IPv6 外部路径可用。

相关提交：`68f630a`、`7b0a84c`、`96e527e`、`a8e0393`、`827086d`、`be791f0`、`5ca9a3e`、`0ff7439`、`dbd226e`。

**验收**：DNS、IPv4 与 IPv6 泄漏测试有可复核证据；任何未测试路径不得标记通过。

### W12-07 Google、YouTube 与 ALPN/H2/H3 `[~]`

- [x] 通过真实 Exit 访问 Google 和 YouTube；
- [x] 验证页面加载、视频分片播放和持续会话；
- [x] 覆盖 DNS、TLS、HTTP/2 和可用的 HTTP/3；
- [x] 区分所有流量经过 Exit 与仅终端 curl 成功；
- [~] 验证至少一个非浏览器客户端；
- [x] 记录失败域名、错误和恢复动作；
- [x] 测试前后检查本机网络未被破坏；
- [x] 单次成功不得标记“稳定可用”。

**实机浏览器证据（2026-11-11，香港 Exit）**：香港 `root@xlei.cc` 承担 Exit，本机 `w12-client`
命名空间承担发起端；Exit 侧 TUN 为 `10.8.0.1/fd00::1001`，发起侧 TUN 为 `10.8.0.2/fd00::1002`。
真实 Chrome 首轮访问 `https://www.google.com/` 得到 `readyState=complete`、34 个响应和 `h2/h3`；
访问 `https://www.youtube.com/watch?v=jNQXAC9IVRw` 得到 `Me at the zoo - YouTube`，185 个响应，视频从
`14.160s` 播放到 `19.021s` 结束，`readyState=4`、`buffered=0–19.021s`、`video.error=null`。
长视频 `dQw4w9WgXcQ` 的 25 秒窗口内，`currentTime` 从 `14.278s` 前进到 `39.339s`，`paused=false`、
`readyState=4`、`buffered=0–96.32s`、无媒体错误；成功媒体响应包含 HTTP/1.1 UMP 1,102,951 bytes，以及 H3 UMP
379,104、775,189、793,283、396,320 bytes。页面协议集合包含 `h2`、`h3` 和 `http/1.1`，不是仅终端 curl 成功。

**源路径与协议证据**：浏览器 DNS pcap（`/tmp/w12-browserdns.pcap`）捕获 Chrome 对 Google、
`www.youtube.com` 和 `googlevideo.com` 的查询经 `127.0.0.1 -> 127.0.0.2:53`；长会话 TUN pcap
（`/tmp/w12-browser-long443.pcap`）捕获 48,022 个 443 包，其中从 `10.8.0.2` 发出的 TCP 2,386 包、UDP
4,560 包，涉及 27 个远端 443 地址。非浏览器 curl 复核也经同一 TUN：Google H2 返回 204、TLS 1.3、ALPN
`h2`，Google H3 返回 204，YouTube 页面 H3 返回 200，YouTube range 请求返回 HTTP 206
`content-range: bytes 0-791/792`，`ifconfig.me` 返回 `101.36.116.21`，受控 IPv6 echo 返回 `fd00::1002`。
这些结果证明浏览器流量和至少一个基础 HTTP 客户端实际经过 Exit，但不足以推出所有非浏览器应用或全部 DNS/QUIC 路径均可用。

**失败、噪声与限制**：Google 的 `csp.withgoogle.com/...` 被 ORB 阻止，YouTube 广告归因请求因
`MissingAllowOriginHeader` 失败，根页面 fetch 被页面主动取消；这些均未阻断页面和视频播放。部分 H3 媒体请求
返回 403，但同一视频仍有成功 H3 分片并完整播放。`yt-dlp` 能提取 `Me at the zoo|jNQXAC9IVRw|19` 元数据，
但 format 18 下载返回 HTTP 403，追加 `--js-runtimes node` 后仍为 403，故不能宣称任意非浏览器下载客户端已验证。
IPv6 外部目标和 `ipv6.google.com` 仍为 `not_tested`，不得由浏览器 IPv4/H3 成功推断 IPv6 无泄漏。

**清理与结论**：测试结束后已终止本轮 control/node，`w12-client` 内 `fk-w12-hk` 和 16666/16667/18082 监听均不存在，
香港受控 echo 的 18093/18094 与临时文件均已清理，宿主默认路由和 DNS 解析保持正常；但 `w12-client` 挂载的
`/etc/netns/w12-client/resolv.conf` 在停止后仍指向 `127.0.0.2`，已手工恢复为 `127.0.0.53` 并重新验证解析。
该残留说明自动清理仍需在 W12-09 复核，故本项保持 `[~]`；单次 25 秒播放和一次诊断不能标记“稳定可用”。

相关提交：`0ff7439`、`dbd226e`。

**验收**：报告证明真实应用流量通过 Exit 可达目标，且测试后有明确清理结果。

### W12-08 吞吐、延迟、丢包与并发 `[x]`

- [x] 定义测试拓扑、本机、Relay、Exit、目标、ISP 和时间；
- [x] 测量上传/下载吞吐、RTT、抖动、丢包和连接建立时间；
- [x] 测量单流与多流，并记录并发上限；
- [x] 对比直连基线和 Exit 路径，不使用无基线单值；
- [x] 记录 CPU、内存、带宽和连接数；
- [x] 覆盖 TCP 与 QUIC/UDP 可用场景；
- [x] 给出 p50/p95/p99 和失败率；
- [x] 报告明确瓶颈位于本机、Relay、Exit、ISP 还是目标。

**拓扑、时间与环境（2026-11-11 03:37:02–03:40:32，Asia/Shanghai）**：发起端为独立
`w12-client` 网络命名空间（TUN `10.8.0.2`/`fd00::1002`），Exit 路径为
`w12-client -> fk-w12-hk TUN -> Secure Session/信令 Relay -> root@xlei.cc Exit -> Internet`；
香港 Exit TUN 为 `10.8.0.1/fd00::1001`，公网 `101.36.116.21`（AS135377 UCloud，2 vCPU/3.65 GiB，
eth0 MTU 1452）。本机为 16 vCPU/28 GiB，家庭 WAN 为 PPPoE/CGNAT `100.64.64.39`。
Exit 路径没有临时 `ip rule` 绕过 TUN，netns 策略表 `23469` 前后均为
`default dev fk-w12-hk proto openr scope link src 10.8.0.2`；控制连接是
`10.0.0.189 -> 101.36.116.21:16666`，路由器 conntrack 的回包目的地址为公网侧 `100.64.64.39`，
不是 OpenClash 的 `10.0.0.1:7892`。

直连基线使用另一条明确的旁路：仅在 netns 中为目标 `/32` 临时添加 `pref 900 lookup main`，
绕过 TUN；并使用 DSCP 63 在 `inet ai_bridge` 与 `inet fw4` 的窄范围 `return` 规则上绕过
OpenClash 的 DNS/TPROXY/redirect 处理。该规则只匹配发起地址 `10.0.0.189` 和 DSCP 63，
不是全局关闭 OpenClash。测试期间 OpenClash PID `32545`/watchdog PID `32549` 始终运行，
未停止、重启或重载。两种模式的目标固定为 `speed.cloudflare.com`（`162.159.140.220`）及
`www.youtube.com`（`142.251.152.4`）。

**可复核产物**：原始结果 `/tmp/w12-bench-clean-20261111T1938Z.json`，schema 1，SHA-256
`686e7138f34a138f9b31c6716fee5c1cc12cf3aaca5894498c674e1e63f0ee9f`；生成脚本
`/tmp/w12_bench_clean.py`。发起端二进制 SHA-256 为
`75bcfd7fea611024497b08e8966fd685ce1552cd3b7de8aaf1792d92ee065c92`，香港 Exit 二进制 SHA-256 为
`e674b034b8511c374045386a09d3084119d532b721f65980d4ac226cb3cbf004`。

**DNS 与 RTT/丢包**：DNS 查询目标为 `1.1.1.1:53`，每路径 30 次；RTT 使用 50 个 ICMP echo。

| 路径 | DNS 成功/失败 | DNS mean / p50 / p95 / p99 | ICMP 收包/丢包 | RTT mean / p50 / p95 / p99 | 相邻采样抖动 |
|---|---:|---:|---:|---:|---:|
| 直连基线 | 30/0 | 185.6 / 185.5 / 195.8 / 198.7 ms | 50/50，0% | 186.98 / 187.0 / 187.0 / 188.02 ms | 0.204 ms |
| 香港 Exit | 30/0 | 44.9 / 44.5 / 46.6 / 52.0 ms | 50/50，0% | 43.96 / 43.65 / 46.665 / 49.187 ms | 1.331 ms |

DNS 两侧均返回真实 Cloudflare A 记录 `162.159.140.220`、`172.66.0.218`，`fake_ip_answer=false`；
直连侧的 DSCP 63 DNS 放行规则确有命中，未把 `198.18.x.x` 的 fake-IP 当作成功结果。

**TCP/TLS/TTFB**：对 `https://speed.cloudflare.com/__down?bytes=4096` 每路径请求 40 次，均为 HTTP 200。
请求参数包含 `--http2`，但目标实际协商结果为 HTTP/1.1（40/40），因此本表不能冒充 HTTP/2 时延证据。

| 路径 | 成功/失败 | connect p50/p95/p99 | TLS p50/p95/p99 | TTFB p50/p95/p99 | total p50/p95/p99 |
|---|---:|---:|---:|---:|---:|
| 直连基线 | 40/0 | 211.6 / 244.5 / 247.7 ms | 428.9 / 493.1 / 500.2 ms | 664.8 / 759.8 / 778.2 ms | 664.8 / 759.9 / 778.2 ms |
| 香港 Exit | 40/0 | 44.28 / 46.89 / 47.19 ms | 171.97 / 179.47 / 183.99 ms | 260.36 / 273.65 / 364.15 ms | 297.19 / 327.00 / 405.58 ms |

**HTTP/3/QUIC**：对 `www.youtube.com` 每路径执行 12 次 `--http3-only`。香港 Exit 为 12/12 HTTP 200、
全部 HTTP/3；connect p50/p95/p99 为 49.1/81.2/88.2 ms，TLS 为 93.6/127.0/131.8 ms，
TTFB 为 184.9/223.9/227.0 ms，失败率 0%。直连基线为 0/12，全部在 UDP/443 连接阶段失败
（`curl (7) Could not connect to server`），因此本次没有可用的直连 QUIC 对照值，不能声称 H3 相比直连无损。

**单流吞吐**：每路径各 3 轮，下载 5 MiB、上传 2 MiB；表中为十进制 MB/s。

| 路径/方向 | mean | p50 | p95 | p99 | min–max | 失败率 |
|---|---:|---:|---:|---:|---:|---:|
| 直连下载 | 2.282 | 2.261 | 2.400 | 2.412 | 2.170–2.415 | 0% |
| Exit 下载 | 0.528 | 0.475 | 0.814 | 0.844 | 0.259–0.852 | 0% |
| 直连上传 | 0.827 | 0.798 | 0.896 | 0.905 | 0.776–0.907 | 0% |
| Exit 上传 | 0.646 | 0.622 | 0.692 | 0.699 | 0.616–0.700 | 0% |

**并发扫描**：每流固定下载 512 KiB，统计所有流完成时的聚合吞吐；1/4/8/16 并发全部请求成功，失败率均为 0%。

| 并发 | 直连 MB/s | Exit MB/s |
|---:|---:|---:|
| 1 | 0.369 | 0.413 |
| 4 | 1.303 | 1.085 |
| 8 | 2.546 | 0.923 |
| 16 | 4.603 | 0.504 |

**资源与稳定性**：采样窗口内客户端进程 CPU 为单核的 64.5%–95.0%，香港 co-located 进程为
74.1%–115.5%（吞吐串行和并发用例均计入）；Exit 后采样 RSS 在 534,848–540,464 KiB，FD 为 23–27，
客户端 FD 为 22–24。整体测试前后 Exit 连接数均为 2，Exit FD 25→26、RSS 542,064→533,572 KiB，
客户端 FD 23→22；路由与策略规则前后完全一致。传输时香港主机只有 2 vCPU，而同一目标在直连基线
16 并发达到 4.603 MB/s、Exit 在 4 并发达到约 1.085 MB/s 后随并发上升降到 0.504 MB/s，说明目标端
并非本组对比的主要瓶颈，瓶颈落在香港 co-located Relay/Exit 数据路径，并与该进程的高 CPU 占用同时出现。
Relay 与 Exit 当前同机、同进程路径，本组数据不能进一步把两者拆开，也不能仅凭 CPU 百分比断言唯一根因是
CPU 饱和；结论限定为“香港 co-located Relay/Exit 数据路径（加密/转发/调度组合）”。

**清理与限制**：测试客户端 control/node 已停止，`w12-client` netns、运行时目录、netns resolv.conf
和全部 `fuckipv6-w12-*` 临时 nft 规则已清理；香港 Exit 和 OpenClash 均未停止，OpenClash 仍可服务其他应用。
直接向顶层测试进程发送 TERM 后其 control/node 仍存活，需再次终止 control 才完全退出，说明自动清理仍需
在 W12-09 复核。本次约 210 秒的基准只能证明该时段的性能与资源行为，不构成 24 小时稳定性结论；
W12-09 继续保持 `[ ]`。

**组网边界（2026-11-11 侧线复核）**：香港 egress 为 `101.36.116.21` 已确认，但这只证明 Exit 能把流量
转发到外网，不能证明虚拟局域网内节点之间的 P2P 连通。侧线观察时宿主 `fk-w12-hk` 为 `10.8.0.3`，
`RX=0/TX=482`（主线程复核时 TX 已增至 486），香港 Exit 的 `10.8.0.1` 仍活跃，而 `w12-client` netns
已清理；此时宿主 ping `10.8.0.1` 不通不能单独判定组网故障，因为当前没有同网发起端，且宿主 TUN 侧从未
收到有效入站包，不属于有效的 P2P 验收。该侧线观察不能替代独立 netns 验收，也不能用 W12-08 的外网性能
结果替代组网验收。

**最小 P2P 组网验收（2026-11-11，独立 netns）**：脚本
`scripts/test-w12-p2p-invite-netns.sh` 在 `w12pinvh`/`w12pinv1`/`w12pinv2` 三个独立 netns 中启动
Coordinator、node1 和 node2，通过普通 `fk://` location ticket 加入 InviteOnly 网络被正确拒绝；随后通过
max_uses=2 的正式邀请完成预览与兑换，Coordinator 权威 roster 包含三个节点。网络配置均为
`routing_mode=mesh_only`、`exit_node=null`、`exit_node_enabled=false`，未使用 Exit、香港机器、路由器
OpenClash 或宿主默认路由。node1/node2 虚拟地址分别为 `fd00::1003`/`fd00::1004`，双方在私有
`10.78.0.0/16` veth underlay 上发现直接端点；node1 -> node2 与 node2 -> node1 的 5 个 ICMP 请求均为
5/5 成功。稳定后双方目标 peer 均为 `online=true`、`p2p_active=true`、`p2p_healthy=true`，traffic 均为
`p2p_rx_packets=10`、`p2p_tx_packets=10`、`error_count=0`；两侧 tcpdump 均捕获
`10.78.1.2 <-> 10.78.2.2` 的直接 UDP。tcpdump 的 `bad udp cksum` 来自虚拟网卡校验和卸载呈现，不能单独
判为路径丢包，实际双向 UDP 与 ICMP 均到达。

证据目录为 `/tmp/w12-p2p-invite.20261110T210219Z.31844`，二进制 SHA-256 为
`05c13e82db06a6d99404423af432f844f49d081b485922a41a456431d7147740`。测试前后宿主 IPv4/IPv6 policy
rules 与默认路由逐字节一致；结束后无 `w12pinv*` netns、veth 或测试端口监听残留，OpenClash 未停止或重载。
该结果定位了此前的失败边界：InviteOnly 授权曾错误接受 `fk://` location ticket，产品修复后正式邀请入册与
直接 P2P 数据路径均正常；本次只证明双向 ICMP 和 P2P 底层直接 UDP，不证明节点间任意 TCP/业务 UDP 负载、
两侧 Exit 可达或 24 小时稳定性。相关提交：`c02679f`、`14c8c4c`、`a720ae4`。

**真实香港节点 P2P 与业务负载验收（2026-11-11）**：本地宿主与香港节点均直接在虚拟 IP 上执行普通
`ping`，没有使用 Exit、OpenClash 或 netns。本地 -> 香港的 `10.8.0.1`/`fd00::1001` 与香港 -> 本地的
`10.8.0.2`/`fd00::1002` 均为 5/5、0% 丢包；路由分别使用 TUN 上的 IPv4 `/32` 和 IPv6 `/128`
`proto openr` 精确项。真实 P2P socket 为本地 `10.0.0.189:52668 -> 101.36.116.21:38983`
（node pid 312099、fd 13）和香港 `10.7.53.243:38983 <- 58.82.230.109:48786`
（node pid 833653、fd 22），香港端收到的是本地公网物理端点直连。

同网业务负载分别按 IPv4/IPv6 双向执行，全部成功：TCP 每方向 262144 字节，接收端 SHA-256 均为
`2312394bd99545d9de131c24efb781e765ac1aec243f2ed9347597a793a415e9`；UDP 每方向 64 包、
65536 字节，接收端 SHA-256 均为
`fafee8a375c9a47fa435ed22592dad07cb24189bfb2ae7d101947681ec8ea812`。传输后两侧快照的目标 Peer 均为
`online=true`、`p2p_active=true`、`p2p_healthy=true`、`loss_rate=0`，网络级 `error_count=0`；本地和香港
的精确路由在传输后仍存在。

边界：Peer 级 `p2p_tx_packets` 仍与网络级双向计数不一致（本次快照本地为 `rx=1940/tx=500`，香港为
`rx=2832/tx=525`），这是统计归属/展示问题，不能据此否定已由真实 socket、双向 ICMP 和双向业务负载证明的
数据面；该映射问题继续保持待修，不能宣称已经修复。两侧 Exit 可达和 24 小时稳定性仍未验收。
证据目录为 `/tmp/fuckipv6-w12-real-verify/transport-20261111T144646Z`（其中 `postcheck/` 保存四向 ping、
socket、路由、进程和两侧快照）。本地二进制 SHA-256 为
`31630ce13454da3c363f65e9b15a410675066129ccb956c23a2538226988627b`，香港二进制 SHA-256 为
`1bddce44ebc84e832b397a69189a72e7a8248899b0c505a7b42840b3a94c303d`。

**验收**：网速结论有基线和分布；UI 不展示无来源的“高速”宣传值。

### W12-09 抗干扰与 24 小时稳定性 `[ ]`

- [ ] 覆盖高延迟、抖动、丢包、断连、DNS 污染和 MTU 问题；
- [ ] 覆盖端口阻断、QUIC 降级和 TCP fallback（若支持）；
- [ ] 覆盖网络切换、休眠、唤醒和长时间空闲；
- [x] 两个独立 netns/节点同网后完成双向 ICMP，并捕获直接 P2P UDP；
- [x] 真实香港节点同网后完成 IPv4/IPv6 双向 TCP 262144 字节与 UDP 65536 字节/64 包负载；
- [ ] 分别验证两侧 Exit 可达；
- [ ] 运行 24 小时持续会话并记录失败/恢复；
- [ ] 监控内存、连接、文件描述符和临时文件增长；
- [ ] 测试结束自动清理 TUN、路由、DNS 和子进程；
- [ ] 失败能恢复，不能留下需手工 kill 的状态；
- [ ] 抗干扰报告只声明特定环境和时间段的结果。

**验收**：没有 24 小时稳定性与多场景证据时，不得给出“抗干扰强”或“长期可用”结论。

### W12-10 Exit 页面与活动 `[ ]`

- [ ] 本机贡献、网络策略、候选 Exit 和当前连接摘要分层展示；
- [ ] 默认/固定 Exit 及选择来源清楚；
- [ ] 健康、吞吐、延迟、泄漏和最近测试结果有新鲜度；
- [ ] Exit 故障、切换、撤权和恢复活动可追踪；
- [ ] 普通成员不能看到未授权 Exit 或敏感出口信息；
- [ ] 所有危险变更展示影响和确认；
- [ ] GPUI/WebUI/Android 完成视觉和语义一致；
- [ ] 禁止静态质量图、伪测速和“已连接”占位。

**验收**：Exit 的功能、质量证据、权限和 UI 状态使用同一权威来源。

## 16. W13：Relay

目标：Relay 是独立能力，不由 Exit 权限或 UI 自动推断。授权、配额、健康、故障回退和连接元数据边界必须可验证。

### W13-01 Relay 能力独立启停 `[ ]`

- [ ] 节点 owner 可独立启用/关闭 Relay；
- [ ] 展示监听、可达性、配额和当前负载；
- [ ] 启停 operation 不改变 Exit、Coordinator 或 File；
- [ ] 关闭时停止新中继并优雅结束已有会话；
- [ ] 平台/网络/端口不支持有可解释状态；
- [ ] 节点本地权限优先；
- [ ] 崩溃或重启后清理旧监听/会话；
- [ ] 活动记录真实启停和清理结果。

**验收**：Relay 生命周期独立且可观察，不与其他能力共享隐式开关。

### W13-02 使用授权与提供授权 `[ ]`

- [ ] 网络管理员可配置是否允许使用 Relay、哪些成员/组可使用；
- [ ] 节点 owner 可限制为哪些网络/成员提供 Relay；
- [ ] 授权取网络与节点本地条件交集；
- [ ] 拒绝优先，临时 Coordinator 不能扩大授权；
- [ ] 授权变化实时影响新会话；
- [ ] 既有会话按策略继续、迁移或终止；
- [ ] UI 显示授权来源和最终结果；
- [ ] 所有授权变化有审计。

**验收**：不能被“可见即可用”；使用方和提供方双侧授权都需明确满足。

### W13-03 资源、带宽与连接配额 `[ ]`

- [ ] 定义最大连接、并发流、带宽、流量、CPU/内存和会话时长限制；
- [ ] 网络策略和节点本地限制取更严格值；
- [ ] 达到配额时拒绝新会话并保留已有会话策略；
- [ ] 展示配额来源、当前使用和重置时间；
- [ ] 防止恶意连接耗尽文件描述符或内存；
- [ ] 限制每成员和全局速率；
- [ ] 配额变化不导致无界队列或静默丢弃关键事件；
- [ ] 完成压力与资源回收测试。

**验收**：Relay 在满载和攻击流量下保持有界资源并可恢复，不拖垮节点其他能力。

### W13-04 健康、选择与故障回退 `[ ]`

- [ ] 健康状态覆盖进程、监听、网络、配额和实际转发；
- [ ] 候选 Relay 排除离线、过期和未授权对象；
- [ ] 连接失败时按显式策略尝试替代或失败关闭；
- [ ] 切换不泄漏真实源地址或绕过 Exit 要求；
- [ ] 回退过程有状态、时长和原因；
- [ ] 恢复后验证路径和会话；
- [ ] 避免无限重试和路由环路；
- [ ] 覆盖 Relay、Coordinator、Exit 同时故障。

**验收**：Relay 故障可预测降级；不会因重试风暴造成断网或手工清理。

### W13-05 连接元数据边界 `[ ]`

- [ ] 定义 Relay 可见与不可见的元数据；
- [ ] 明确 Relay 不获得应用层明文和私钥；
- [ ] 对源/目标地址、时间和流量体积的可见性写入威胁模型；
- [ ] 活动与日志只保留最小必要字段；
- [ ] 支持脱敏导出；
- [ ] 禁止通过 UI 向未授权成员展示中继拓扑细节；
- [ ] 完成 Relay 运营者视角隐私测试；
- [ ] 文档不夸大匿名性。

**验收**：Relay 隐私声明与实际可见数据一致，不以“加密”掩盖流量分析风险。

### W13-06 Relay 审计与治理 `[ ]`

- [ ] 记录授权、配额变更、启停和故障转移；
- [ ] 记录拒绝原因和配额命中，不记录敏感载荷；
- [ ] 管理员撤权与节点本地关闭都可追溯；
- [ ] 支持按网络/节点筛选活动；
- [ ] 临时 Coordinator 变更进入冻结检查；
- [ ] 审计保留和导出遵循 W10；
- [ ] 页面不展示无权限的连接详情；
- [ ] 所有治理操作完成权限测试。

**验收**：Relay 使用和提供均可追责，同时不产生超出威胁模型的元数据。

### W13-07 Relay 集成与性能测试 `[ ]`

- [ ] 覆盖双边授权交集和拒绝优先；
- [ ] 覆盖配额、连接风暴、慢连接和资源耗尽；
- [ ] 覆盖链路断开、切换、重连和环路；
- [ ] 覆盖与 Exit/Coordinator 同时故障；
- [ ] 测量吞吐、延迟、并发、CPU 和内存；
- [ ] 验证满载时节点其他能力仍可用；
- [ ] GPUI/WebUI/Android 状态与操作一致；
- [ ] 报告绑定 commit、平台和测试拓扑。

**验收**：Relay 独立能力通过功能、隐私、资源和故障测试后才可声明可用。

## 17. W14：Coordinator 自动接管

目标：在候选组内实现可验证的任期与接管；无多数时不选举，紧急 Leader 只获得临时运行权限，数据面不无条件中断。

### W14-01 候选组与选举配置 `[ ]`

- [ ] 网络管理员配置候选成员组和优先级；
- [ ] 候选节点声明公网可达性、资源和意愿；
- [ ] 普通成员不会被自动选为 Coordinator；
- [ ] 候选配置有版本、签名和生效时间；
- [ ] UI 展示候选、健康、当前 Leader 和最近任期；
- [ ] 节点 owner 可拒绝本机候选角色；
- [ ] 候选全失时进入明确的紧急状态；
- [ ] 覆盖候选变更并发和离线更新。

**验收**：节点身份与候选资格可验证；不存在通过伪造健康或自荐直接成为 Leader 的路径。

### W14-02 任期、租约与 fencing token `[ ]`

- [ ] 定义 CoordinatorTerm、leader ID、任期号、租约和 fencing token；
- [ ] 所有治理写入携带任期和 token；
- [ ] 旧任期的写入被拒绝且可审计；
- [ ] 任期推进单调，不因重启回退；
- [ ] 租约续期失败进入只读/过渡状态；
- [ ] 时间源漂移有界限和告警；
- [ ] 定义持久化与恢复语义；
- [ ] 完成任期回退和重放测试。

**验收**：任何旧 Leader 无法覆盖新任期；所有治理写入可识别权威任期。

### W14-03 正常多数接管 `[ ]`

- [ ] Leader 失联后按候选组和健康状态发起选举；
- [ ] 仅在达到配置多数/法定人数时产生新 Leader；
- [ ] 选择协议采用经过评估的成熟实现或经过审计的自研设计；
- [ ] 选举过程有超时、退避和幂等重试；
- [ ] 客户端重连后刷新 Leader 与任期中继信息；
- [ ] 数据面连接不因控制面选举无条件中断；
- [ ] 完成后台节点故障接管 E2E；
- [ ] 记录选举阶段、参与者集合和结果。

**验收**：正常多数接管可重复、可观察，且不会产生双 Leader 写入。

### W14-04 分区与无多数保护 `[ ]`

- [ ] 少数分区不选举新 Leader；
- [ ] 无多数时治理写入失败关闭；
- [ ] 网络分区期间数据面按既有连接策略继续或安全降级；
- [ ] 分区恢复后执行状态与任期对齐；
- [ ] 防止重复提案、过期事件和租户状态回滚；
- [ ] UI 明确“控制面受限”而非误报节点故障；
- [ ] 完成网络分区、延迟、丢包和时钟漂移测试；
- [ ] 所有分区事件可审计。

**验收**：系统宁可暂停治理也不制造双写；分区恢复不会回滚已确认状态。

### W14-05 旧 Leader 恢复 `[ ]`

- [ ] 旧 Leader 恢复后先验证当前任期，不直接恢复写入；
- [ ] 低任期写入、事件和状态同步被拒绝；
- [ ] 旧 Leader 以普通候选或成员身份重新加入；
- [ ] 完成状态快照/日志追赶和只读阶段；
- [ ] 客户端拒绝旧 Leader 的治理事件；
- [ ] 旧 Leader 健康检查和数据面能力可继续；
- [ ] 记录恢复过程和被拒绝写入；
- [ ] 完成旧 Leader 并发恢复测试。

**验收**：旧 Leader 恢复不会形成脑裂或覆盖新状态，数据面与控制面可安全重连。

### W14-06 紧急临时 Leader `[ ]`

- [ ] 候选组全部失联时允许在线成员按明确规则进入临时模式；
- [ ] 临时 Leader 获得运行协调权限，而非完整治理权限；
- [ ] 禁止修改成员、角色、安全策略、密钥、邀请和长期配置；
- [ ] 临时权限有短租约、自动过期和显著 UI 标识；
- [ ] 临时 Leader 不能提升自己为正式 Leader；
- [ ] 数据面仅在必要范围维持，避免不可追踪的长期状态；
- [ ] 所有临时动作进入高优先级审计；
- [ ] 完成断电、断网和候选全失演练。

**验收**：紧急模式保障基本运行，但不成为治理后门；权限范围和持续时间均有硬限制。

### W14-07 临时权限冻结与恢复 `[ ]`

- [ ] 建立允许/禁止操作清单并落实服务端校验；
- [ ] 临时 Leader 的所有治理命令返回明确受限错误；
- [ ] UI 禁用入口并显示恢复条件；
- [ ] 正式候选恢复后验证多数和任期；
- [ ] 执行安全交回、状态校验和临时权限撤销；
- [ ] 交回失败保持受限状态，不降级为永久权限；
- [ ] 管理员阈值恢复流程需要多成员签名/批准；
- [ ] 覆盖恢复期间并发展开 old/new Leader 测试。

**验收**：临时状态不可被静默延长或转正；交回失败时仍保持最小权限。

### W14-08 管理员阈值恢复 `[ ]`

- [ ] 定义管理员阈值、签名集合和成员快照版本；
- [ ] 全部 Passkey 丢失等灾难场景提供本地/离线恢复设计；
- [ ] 恢复请求展示影响、参与者、有效期和撤销路径；
- [ ] 防止少数管理员伪造多数签名；
- [ ] 恢复后使旧会话、邀请和未确认写入失效；
- [ ] 恢复动作形成不可静默删除的审计；
- [ ] 完成后端、GPUI、WebUI 和 Android 等价安全流程；
- [ ] 完成签名重放、成员变更和阈值不足测试。

**验收**：灾难恢复不依赖单个隐藏后门；恢复权限与正常治理权限有清晰边界。

### W14-09 数据面连续性 `[ ]`

- [ ] 区分控制面故障与数据面故障；
- [ ] Leader 选举期间不需要重建全部 P2P/Relay/Exit 会话；
- [ ] 必需的状态同步失败时按安全策略降级；
- [ ] 控制面恢复后增量同步，不重复创建隧道；
- [ ] UI 只显示一个主状态并指出控制服务异常或节点降级；
- [ ] 不因 Coordinator 瞬断自动 kill node；
- [ ] 记录数据面连续性指标和恢复时间；
- [ ] 完成 Leader 切换期间视频/长连接场景测试。

**验收**：控制面接管不以无差别断网为代价；无法保持的会话明确降级而非伪报正常。

### W14-10 脑裂、接管与恢复测试 `[ ]`

- [ ] 建立可重复的多节点故障注入拓扑；
- [ ] 覆盖正常多数、无多数、旧 Leader 恢复和候选全失；
- [ ] 覆盖网络分区、进程暂停、时钟漂移和消息重放；
- [ ] 验证只有一个权威任期可写；
- [ ] 验证临时 Leader 治理冻结；
- [ ] 验证恢复阈值和旧会话失效；
- [ ] 验证数据面连续性和自动恢复；
- [ ] 报告包含状态机轨迹、任期和审计证据。

**验收**：Coordinator 自动接管只有在脑裂、分区和恢复测试全部通过后才算完成。

## 18. W15：File

目标：File 是受 ACL、配额和路径边界保护的共享能力，不扩张为同步盘、镜像或系统挂载。所有者选择共享目录，网络管理员只控制网络是否允许及授权范围。

### W15-01 共享目录与能力启停 `[~]`

- [ ] 节点 owner 通过原生目录选择器选择共享根目录；
- [ ] 不把整个磁盘或敏感系统目录设为默认共享；
- [ ] 启用前展示目录、容量、访问范围和最小权限；
- [ ] File 能力独立于 Relay/Exit/Coordinator；
- [ ] 关闭能力停止新传输并处理已有传输；
- [ ] 目录不可读、权限变化、磁盘满有明确状态；
- [ ] 不自动创建桌面/下载目录共享；
- [ ] 所有配置变化有审计。

**验收**：共享范围由本地 owner 明确授权，关闭后不继续暴露文件元数据或传输。

### W15-02 ACL、配额与可见性 `[~]`

- [ ] 定义文件/目录级 ACL、组授权和拒绝优先；
- [ ] 未授权成员看不到文件名称、大小、时间和目录结构；
- [ ] 配置每成员/网络/全局配额；
- [ ] 限制最大文件、最大目录、并发传输和速率；
- [ ] 管理员可限制网络是否启用 File，但不能浏览 owner 文件；
- [ ] 授权变化即时影响元数据和传输接口；
- [ ] 配额和 ACL 错误结构化展示；
- [ ] 完成字段级隐私与资源上限测试。

**验收**：File 权限是网络授权与本地目录控制的交集；管理员身份不自动等于文件内容权限。

### W15-03 上传、下载与断点续传 `[~]`

- [x] 使用受认证、可取消的 operation 开始传输；
- [ ] 上传/下载支持校验、进度、暂停/恢复和最终确认；
- [ ] 断点续传绑定文件版本、分块和身份，防止拼接错误内容；
- [ ] 文件更新后旧续传状态失效；
- [x] 支持取消并清理临时文件；
- [ ] 区分网络中断、权限撤销、配额超限和磁盘错误；
- [x] 完成后校验内容哈希并原子落位；
- [ ] 覆盖并发同名、重复完成和重放测试。

**验收**：传输中断可安全恢复，损坏/过期分块不会被合并成成功文件。

**证据（2026-11-12，requester 状态机与本地 IPC）**：新增 `FileNetworkOrigin` requester 状态机，上传按 manifest 发送有界分片，
下载写入同目录私有 staging 文件，逐块校验长度/offset/checksum，完成响应后校验整文件哈希并以 `persist_noclobber` 原子落位。
所有响应、分片和完成确认严格绑定已认证 peer、request ID、transfer ID、operation ID；错 peer、错 request ID、乱序、
重放、损坏分片和失效绑定只会失败关闭对应 operation，并清理其 staging，不影响其他传输。`StartFileTransfer` /
`CancelFileTransfer` 仅通过本地 IPC 进入运行时的受认证 operation；取消、发送失败和过期都会唤醒事件循环并排空状态机队列。
`outbound_notify_wakes_all_transfer_transitions`、`fail_pending_request_only_fails_the_correlated_operation_and_wakes_pump`、
`misattributed_download_fragment_cleans_only_target_staging`、`wrong_download_request_id_cleans_only_target_staging` 和
`download_reassembles_verifies_and_atomically_persists` 覆盖上述状态转换与失败关闭边界；IPC 成功、运行时错误和断链回复由
`file_transfer_start_maps_runtime_replies` 与 `file_transfer_cancel_maps_runtime_replies` 覆盖。该证据仍是进程内状态机/IPC
自动化测试，不代表真实网络传输、断点跨重启或大文件性能验收已经完成。

### W15-04 canonicalize 与路径边界 `[~]`

- [ ] 所有路径在服务端规范化并验证位于共享根目录；
- [ ] 拒绝绝对路径逃逸、`..`、空字节和平台特殊路径；
- [ ] 处理大小写、Unicode、尾随分隔符和 Windows 路径差异；
- [ ] 在打开文件前后复核真实路径；
- [ ] 对允许链接的策略明确默认禁止；
- [ ] 建立跨平台路径 fuzz/property 测试；
- [ ] 失败返回稳定安全错误，不泄漏根目录外信息；
- [ ] 代码审查覆盖 TOCTOU 风险。

**验收**：任何输入无法读写共享根目录之外的文件，路径测试覆盖目标平台。

### W15-05 符号链接、竞态与临时文件 `[~]`

- [ ] 拒绝指向共享根外的符号链接/重解析点；
- [ ] 对目录替换、链接交换和并发重命名进行防护；
- [ ] 临时文件使用受控目录、随机名和权限；
- [ ] 上传完成前不可出现在普通文件列表；
- [x] 失败/崩溃后清理过期临时文件（启用时在锚定的私有 staging 目录中仅删除超过 30 分钟的 `up-*` 普通文件；不跟随符号链接，清理失败则拒绝启用）；
- [ ] 文件句柄、大小和磁盘使用有上限；
- [ ] 覆盖 symlink race、硬链接和权限变化测试；
- [ ] 不依赖仅字符串前缀判断路径安全。

**验收**：路径穿越与竞态攻击不能越界；临时文件不泄漏、不无限增长。

### W15-06 并发、速率与资源限制 `[~]`

- [x] 限制每成员、每目录和全局并发；
- [x] 实现读写速率和带宽配额；
- [x] 进行磁盘空间预留与低空间保护；
- [x] 限制目录扫描深度和条目数；
- [x] 限制事件队列和日志增长；
- [ ] 满载时不影响 node/control 基本功能；
- [ ] 压力测试后文件句柄、线程和内存回到基线；
- [x] 报告录明阈值与恢复结果。

**验收**：恶意或误用传输无法耗尽节点资源，File 故障不拖垮数据面。

**证据（2026-11-12，自动化边界）**：File runtime 在授权后、staging 前按已认证 actor 与目标父目录执行准入；上传和下载共享同一字节 token bucket，
限速时返回 `RateLimited { retry_after_ms }` 而不会阻塞 async runtime。默认阈值如下：

| 限制项 | 默认/上限 | 执行点 |
|---|---:|---|
| 全局活动传输 | 4 | `FileQuota::max_concurrent_transfers` + `FileRuntime::reserve` |
| 每成员活动传输 | 2 | `FileQuota::max_concurrent_transfers_per_member` |
| 每目录活动传输 | 2 | `FileQuota::max_concurrent_transfers_per_directory` |
| 磁盘预留 | 256 MiB | `FileQuota::reserve_bytes` + `fs2::available_space`，上传准入前失败关闭 |
| 共享总配额 | 10 GiB | `FileQuota::total_bytes` |
| 每成员配额 | 2 GiB | `FileQuota::per_member_bytes` |
| 单文件配额 | 1 GiB | `FileQuota::max_file_bytes` |
| 目录条目审计 | 100,000 | `FileQuota::max_entries` |
| 配额扫描深度/条目 | 64 / 100,000 | `QuotaScanLimits::default`，不跟随符号链接 |
| 单块上限 | 16 MiB | `MAX_TRANSFER_CHUNK_BYTES` |
| 带宽 | 0 = 不限额 | `max_bandwidth_kbps`；启用时按聚合 File 流量计量，桶容量不超过单块上限 |
| 节点活动/命令审计 | 256 / 256 | `ACTIVITY_RETENTION_LIMIT` / `COMMAND_AUDIT_RETENTION_LIMIT` |
| File capability 审计 | 256 | `CAPABILITY_AUDIT_RETENTION_LIMIT`；持久化 owner 安全台账不受内存淘汰影响 |

**自动化恢复结果**：`repeated_transfer_lifecycles_return_admission_and_staging_to_baseline` 连续执行 512 次上传生命周期，
每次 drop 后活动准入回到 0，最终 staging 目录为空；成员、目录与全局并发额度的独立/同时生效由
`member_and_directory_limits_are_independent_within_global_limit` 与 `directory_limit_blocks_only_the_same_parent_directory` 覆盖；
低空间在 staging 创建前返回 `file_disk_space_reserved`；上传/下载限速后保持可重试并最终完成；
`command_audit_keeps_only_the_latest_bounded_entries` 验证命令审计只保留最新 256 条，
`capability_audit_keeps_only_the_latest_bounded_events` 验证 File capability 审计只保留最新 256 条。

**未测边界**：尚未启动独立 node/control 进程做 File 满载隔离测试；尚未在 Linux/macOS/Windows 实机上采样压力前后的
文件句柄、线程数和 RSS，因此不能声称 OS 资源已回到基线。大文件、多文件并发和真实网络中断恢复性能也未测试。

### W15-07 撤权与传输中断 `[~]`

- [ ] 撤销 ACL 后立即停止新元数据访问和传输请求；
- [x] 已有传输在安全边界内终止或完成策略明确；
- [ ] 成员退出/移除后会话和临时凭据失效；
- [ ] 节点 owner 关闭共享后所有客户端看到不可用；
- [x] 不删除 owner 本地文件；
- [ ] 活动记录撤权、中断和清理结果；
- [ ] 覆盖并发上传中撤权、节点重启和网络断开；
- [ ] 验证无未授权断点续传恢复。

**验收**：撤权快速生效，不留下可继续下载的会话、URL 或临时凭据。

**证据（2026-11-12，安全边界失败关闭）**：requester 在每次发送前重新核对稳定 NodeId、virtual IP、P2P established session、
网络 File capability 和权威成员目录；收到响应或下载分片后也会复核成员授权。响应复核失败时按已认证 request ID 只失败对应
operation，不信任帧内 transfer/operation ID；分片复核失败时取消对应传输。取消和撤权只删除 requester staging，
owner 本地共享文件不在失败路径中；owner 下载/上传侧撤权取消及 staging 清理已有
`disabling_capability_cancels_an_active_download`、`disabling_capability_cancels_an_active_upload_and_cleans_staging` 和
`operation_cancellation_before_upload_finish_never_publishes` 覆盖。尚未启动真实双节点链路演练网络断开、成员并发撤权和
节点重启，因此 W15-07 继续保持 `[~]`。

### W15-08 File 非目标与发布证据 `[ ]`

- [x] 明确不做自动同步、双向镜像、系统挂载和远程目录替换；
- [x] UI 不展示这些非目标为可用功能；
- [ ] 完成 ACL、路径、符号链接、竞态、配额、续传和撤权测试；
- [ ] GPUI/WebUI/Android 使用同一文件和权限语义；
- [x] 移动端 SAF 与桌面目录权限边界清楚；
- [x] 活动与审计可追溯；
- [ ] 性能报告包含大文件、多文件和中断恢复；
- [x] 未通过路径安全测试不得发布。

**验收**：File 第一版范围明确、边界安全、传输闭环，不以同步盘宣传超出实现。

**边界与发布结论（2026-11-12）**：

- W15 第一版不做自动同步、双向镜像、系统挂载或远程目录替换；GPUI 设置页明确提示这些能力不存在。
- Android SAF 属于 W16，不是 W15 已交付能力；Android 端不得据此声称 File 目录选择或传输已经完成。
- 当前已有 GPUI owner File 设置流程、本地 owner IPC、持久化、domain ACL、路径边界、临时文件、可恢复传输适配器和资源限制。
- 当前 WebUI 与 Android 尚无 File 内容浏览/传输表面；不得宣称 GPUI/WebUI/Android 三端 File 语义已经一致。
- 自动化测试覆盖 Linux 临时目录及 portable 路径规则，但尚未完成 Windows GNU/macOS 实机重解析点、硬链接、目录交换、
  并发重命名和权限变化测试，也没有真实网络传输链路、撤权中断、节点重启或大文件/多文件性能报告。
- 因此 W15 保持 `[~]`，第二版能力队列中的“W15 File 完成”继续保持 `[ ]`；真实路径安全、跨平台和三端门禁通过前不得发布为完成。

## 19. W16：Android

目标：Android 与桌面共享领域契约，但按平台实现 Compose、前台 VpnService、Credential Manager 和 SAF。关闭 Activity 不断网，能力由节点 owner 决定。

### W16-01 Compose 五模块与共享契约 `[ ]`

- [ ] 实现概览、节点、网络、活动、设置五模块；
- [ ] 使用生成/共享的 Node/Network/Operation/Error 契约；
- [ ] 术语、状态优先级和危险操作语义与桌面一致；
- [ ] 窄屏使用底部导航、全屏确认和可滚动详情；
- [ ] 页面具备加载、空、错误、无权限、不支持、降级和陈旧状态；
- [ ] 支持中英文和系统深色模式；
- [ ] 无障碍标签、焦点和触控目标达到规范；
- [ ] 不展示桌面专用但 Android 不支持的操作。

**验收**：Android 是同一产品的平台实现，不是独立概念或静态展示应用。

### W16-02 前台 VpnService 生命周期 `[ ]`

- [ ] 使用前台服务承载 VpnService 和数据面；
- [ ] 关闭 Activity 后隧道继续；
- [ ] 通知展示运行状态、主要告警和停止入口；
- [ ] stop node 不停止控制服务或必要 UI 后端；
- [ ] 用户停止后释放 TUN、路由、DNS 和前台通知；
- [ ] 系统重启/进程被杀后的恢复符合配置；
- [ ] 申请 VpnService 权限前解释用途和影响；
- [ ] 完成 Activity 关闭、进程被杀、系统重启测试。

**验收**：所谓“运行中”不依赖 Activity 存活；停止与恢复不会残留 VPN 或通知。

### W16-03 Passkey 与 Credential Manager `[ ]`

- [ ] 使用 Credential Manager 完成 Passkey 注册/登录；
- [ ] 覆盖无生物识别、无锁屏、密钥不可用和设备迁移；
- [ ] 高危操作触发近期重认证；
- [ ] 自定义 Tab/WebView 场景校验来源和回调；
- [ ] 所有 Passkey 丢失后的本地恢复遵循 W3/W14；
- [ ] 不在日志、Intent 或 URL 中暴露初始化码；
- [ ] 会话撤销后清除本地敏感缓存；
- [ ] 完成真机 Passkey E2E。

**验收**：Android 认证不引入密码/弱口令分支，并与所有者单账户模型一致。

### W16-04 SAF 与 File 边界 `[ ]`

- [ ] 使用 Storage Access Framework 选择共享目录；
- [ ] 持久化 URI 权限并处理权限撤销；
- [ ] 路径与 ACL 仍由服务端/共享核心校验；
- [ ] 不假设 SAF URI 是普通文件路径；
- [ ] 处理云盘、虚拟文档和不可 seek 流；
- [ ] 限制配额、并发和临时缓存；
- [ ] 上传/下载/续传在权限撤销后停止；
- [ ] 完成 SAF 真机与重连测试。

**验收**：Android File 不绕过桌面相同的 ACL/路径安全模型，SAF 生命周期可恢复。

### W16-05 Android 全能力支持 `[ ]`

- [ ] 节点 owner 可独立启停 Coordinator 候选、Relay、Exit、File；
- [ ] 展示电池、移动流量、CPU 和带宽影响；
- [ ] 支持网络创建、发现、邀请、审批和成员治理；
- [ ] 支持多网络拓扑的列表等价视图；
- [ ] 支持流量路由的查看、测试和必要编辑；
- [ ] 支持 Exit 选择和泄漏/质量结果；
- [ ] 支持 Coordinator 状态与临时权限限制说明；
- [ ] 不支持的平台子能力明确禁用。

**验收**：第一版能力不是桌面专属占位；Android 可选能力由 owner 决定并可真实启停。

### W16-06 后台限制、电量与移动流量 `[ ]`

- [ ] 检测并解释 Doze、厂商后台限制和电池优化影响；
- [ ] 提供系统设置引导，不伪报已解除限制；
- [ ] 展示前台服务持续时间和资源使用；
- [ ] 提供移动网络/漫游下主数据面默认限制；
- [ ] Exit/Relay/File 显示上传下载流量并支持配额；
- [ ] 低电量或高热时按策略降级并提示；
- [ ] 不在无用户知情时后台消耗大量流量；
- [ ] 完成长时间移动网络稳定性测试。

**验收**：资源消耗可观察、可控；后台限制和流量成本不会隐藏在“运行正常”下。

### W16-07 网络切换、休眠与恢复 `[ ]`

- [ ] 覆盖 Wi-Fi/移动网络切换和 IPv4/IPv6 变化；
- [ ] 覆盖锁屏、Doze、休眠、唤醒和飞行模式；
- [ ] 恢复后重新计算 TUN、路由、DNS 和 Exit；
- [ ] 失败时进入可见降级或停止状态；
- [ ] 不无限重连或保持失效通知；
- [ ] 恢复过程有硬超时与自动清理；
- [ ] 覆盖 Android 进程被系统杀死后重启；
- [ ] 报告真机型号、系统版本和厂商策略。

**验收**：常见移动网络生命周期可恢复，不需要用户手工强停应用或重启设备。

### W16-08 Android 实机 E2E `[ ]`

- [ ] 真机完成首次启动、Passkey、节点启停；
- [ ] 完成创建/发现/加入/审批；
- [ ] 完成 Exit Google/YouTube、DNS/IPv6 泄漏检查；
- [ ] 完成 Relay、Coordinator、File 能力测试；
- [ ] 完成 Activity 关闭、后台、休眠、切网和进程被杀；
- [ ] 完成无鼠标、无键盘触控/无障碍测试；
- [ ] 记录耗电、移动流量、内存和崩溃；
- [ ] 不使用模拟器或单次启动代替真机证据。

**验收**：Android 第一版在目标真机和系统版本上通过完整测试矩阵。

### W16-09 桌面与 Android 一致性 `[ ]`

- [ ] 使用同一状态优先级、权限结果和错误码；
- [ ] 五模块命名、对象详情和危险操作语义一致；
- [ ] 平台差异以能力说明表达，不创造新业务概念；
- [ ] 网络/Exit/Relay/File 状态一致；
- [ ] 活动术语和脱敏规则一致；
- [ ] 视觉差异只包含平台导航、控件和窗口习惯；
- [ ] 建立跨平台契约快照；
- [ ] 完成桌面—Android 互动 E2E。

**验收**：同一用户跨平台看到同一事实，仅交互形态随平台变化。

## 20. W17：跨平台硬化

目标：Linux、macOS、Windows 的 TUN、路由、DNS、服务、安装和 UI 必须经过实机验证。交叉编译只能证明可编译，不能证明可用。

### W17-01 Linux 实机基线 `[ ]`

- [ ] 在支持的发行版和内核上完成 TUN/路由/DNS；
- [ ] 验证 systemd 或目标服务管理的启动、停止、崩溃恢复；
- [ ] 验证普通用户/提权边界和最小 capability；
- [ ] 验证 IPv4/IPv6、网络切换和休眠；
- [ ] 验证 Exit/Relay/Coordinator/File；
- [ ] 验证安装、升级、回滚和卸载；
- [ ] 验证 WebUI 公网访问与所有者认证；
- [ ] 记录桌面/无头启动差异。

**验收**：Linux 支持矩阵内实机通过，残留配置和进程清理可验证。

### W17-02 macOS 实机基线 `[ ]`

- [ ] 验证 utun、路由、DNS 和系统权限；
- [ ] 验证 LaunchAgent/LaunchDaemon 或选定服务模型；
- [ ] 验证 Intel/Apple Silicon 目标；
- [ ] 验证 GPUI 原生窗口、菜单和文件选择；
- [ ] 验证 WebUI 公网 TLS、所有者认领、Passkey 和回环会话保护；
- [ ] 验证 Exit/Relay/Coordinator/File；
- [ ] 验证安装、签名、升级、回滚和卸载；
- [ ] 验证睡眠/唤醒和网络切换；
- [ ] 记录 Gatekeeper/公证要求。

**验收**：macOS 不以“Unix 类似”推断；所有核心路径有目标实机证据。

### W17-03 Windows 实机基线 `[ ]`

- [ ] 验证 Wintun/路由/DNS 和防火墙；
- [ ] 验证 Windows 服务工作方式与权限提升；
- [ ] 验证路径、ACL、符号链接/重解析点和文件锁；
- [ ] 验证 GPUI 原生窗口、IME、高 DPI 和文件选择；
- [ ] 验证 Exit/Relay/Coordinator/File；
- [ ] 验证安装、升级、回滚和卸载；
- [ ] 验证睡眠/唤醒和网络切换；
- [ ] 记录杀毒/企业策略兼容风险。

**验收**：Windows 文件路径、服务和网络行为经过实机验证，不以交叉编译替代。

### W17-04 TUN、路由与 DNS 平台矩阵 `[ ]`

- [ ] 为每个平台记录 TUN 创建/清理机制；
- [ ] 记录 IPv4/IPv6 路由语义和冲突处理；
- [ ] 记录 DNS 系统集成、DoH/DoT 与恢复；
- [ ] 记录权限失败、接口消失和残留清理；
- [ ] 记录应用/进程识别的平台能力；
- [ ] 记录固定 Exit、失败关闭和 fallback；
- [ ] 覆盖休眠、网络切换和强制崩溃；
- [ ] 在发布矩阵中标记已测/未测/不支持。

**验收**：平台差异由能力矩阵明确表达，不把 Linux 行为当作所有平台行为。

### W17-05 服务托管与进程监督 `[ ]`

- [ ] 各平台服务管理器能启动 control 和 node 的正确关系；
- [ ] control 崩溃不终止 node，node 崩溃可恢复；
- [ ] 关闭 UI 不停止服务；
- [ ] stop node 不停止 control/WebUI；
- [ ] 重启后读取真实状态并做一致性检查；
- [ ] 日志轮转、工作目录和权限最小化；
- [ ] 系统关机时受控停止并清理；
- [ ] 提供不需要手工 kill 的故障恢复路径。

**验收**：进程生命周期遵循产品模型，不依赖终端会话或桌面进程。

### W17-06 安装、升级、回滚与卸载 `[ ]`

- [ ] 提供签名安装包和可验证构建来源；
- [ ] 安装前检查平台、架构、端口和磁盘；
- [ ] 升级执行 schema/配置迁移和备份；
- [ ] 升级失败自动回滚到可用版本；
- [ ] 回滚不恢复已撤销凭据或旧策略；
- [ ] 卸载明确保留/删除的配置、审计和共享文件；
- [ ] 提供干净环境安装 E2E；
- [ ] 产物与 commit、哈希和平台报告绑定。

**验收**：安装和生命周期操作可逆、可审计，不把用户数据静默破坏。

### W17-07 跨平台 UI 一致性 `[ ]`

- [ ] GPUI 与 WebUI 使用同一信息架构、状态、权限和错误；
- [ ] 最小窗口、DPR、系统字体、缩放和主题验证；
- [ ] IME、剪贴板、文件选择和快捷键验证；
- [ ] 右键/菜单键/触控等价；
- [ ] 中文/英文伪本地化、截断和换行；
- [ ] 无障碍与高对比度；
- [ ] 视觉差异清单仅保留平台必要项；
- [ ] 迁移后删除旧双 WebUI 表面。

**验收**：平台差异可解释；不存在同一功能在不同 UI 上分叉或缺失。

### W17-08 平台证据与发布矩阵 `[ ]`

- [ ] 每个目标平台有真实机器、系统版本和架构；
- [ ] 网络测试包含拓扑、时间和清理证据；
- [ ] UI 证据包含尺寸、DPR、字体、语言和主题；
- [ ] 服务证据包含重启和故障恢复；
- [ ] File 证据覆盖平台路径语义；
- [ ] 安装/升级/回滚/卸载证据齐全；
- [ ] 未验证平台在 UI 和能力表中标记不支持/实验性；
- [ ] 不以 CI 编译通过替代实机运行。

**验收**：发布平台矩阵与证据逐项对应，没有空白格被当作支持。

## 21. W18：发布门禁

目标：只有在真实功能、自动化、安全、网络质量和平台证据全部通过后发布第一版；任何阻断项存在时不得宣布完成。

### W18-01 功能闭环审计 `[ ]`

- [ ] 遍历五模块全部可见控件和动作；
- [ ] 每个动作绑定真实后端命令、权限、operation、活动和恢复；
- [ ] 删除所有只返回 `Unavailable`、静态成功或假数据的入口；
- [ ] 核对 Coordinator/Relay/Exit/File 全能力；
- [ ] 核对多网络、发现、审批、治理和隐私；
- [ ] 核对 Android 与桌面能力矩阵；
- [ ] 核对旧双 WebUI 已移除或被禁用；
- [ ] 形成功能—证据映射表。

**验收**：不存在“看起来可用但实际未闭环”的发布阻断项。

### W18-02 Rust 质量门禁 `[ ]`

- [ ] `cargo fmt --check` 通过；
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；
- [ ] `cargo test --workspace --all-features` 通过；
- [ ] feature 组合无法一次构建时提供明确矩阵结果；
- [ ] 契约测试、集成测试和故障注入通过；
- [ ] 无禁用测试、吞错或无条件 pass；
- [ ] 编译警告和未使用代码处理完成；
- [ ] 结果绑定 commit 和工具链版本。

**验收**：Rust 门禁可复现通过，不以局部 crate 结果代替 workspace。

### W18-03 前端质量门禁 `[ ]`

- [ ] TypeScript 严格类型检查通过；
- [ ] ESLint/等价 Lint 通过；
- [ ] 单元测试和组件测试通过；
- [ ] Playwright E2E 通过；
- [ ] i18n 键完整性、伪本地化和截断通过；
- [ ] 可访问性自动与人工测试通过；
- [ ] 构建产物嵌入单一二进制并可离线加载；
- [ ] 构建哈希绑定 commit。

**验收**：新 React/Vite 表面是唯一产品 WebUI，所有门禁无跳过。

### W18-04 Schema 与兼容闸门 `[ ]`

- [ ] Rust/TypeScript/Android 类型从单一权威 schema 生成；
- [ ] CI 检查生成漂移；
- [ ] 命令、事件、错误和权限枚举版本化；
- [ ] schema 不兼容变化有迁移/拒绝策略；
- [ ] 前端不手写重复领域枚举；
- [ ] API 样本和契约测试覆盖所有命令；
- [ ] OpenAPI/JSON Schema 与实现匹配；
- [ ] 发布记录 schema 版本。

**验收**：不存在手工同步导致的类型漂移或未知命令静默成功。

### W18-05 视觉与交互回归 `[ ]`

- [ ] GPUI/WebUI 同平台、同尺寸、同 DPR、同字体、同语言截图；
- [ ] 五模块、对象详情、菜单、表单和状态矩阵基线；
- [ ] 拓扑多网络、能力和交互基线；
- [ ] 键盘、屏幕阅读器、右键和长按 E2E；
- [ ] 加载、错误、无权限、不支持、降级和陈旧状态；
- [ ] 视觉差异只允许平台必要项；
- [ ] 有变化时人工审核而非自动接受；
- [ ] 视觉证据与 commit 绑定。

**验收**：UI 像素级目标通过可复核基线验证，不存在功能分叉。

### W18-06 网络质量与抗干扰证据 `[ ]`

- [ ] Exit 真实访问 Google/YouTube；
- [ ] DNS、IPv4、IPv6 泄漏检查通过；
- [ ] 吞吐、延迟、丢包、并发和恢复报告；
- [ ] 高延迟、丢包、断连、端口阻断和 DNS 污染场景；
- [ ] 24 小时稳定性与资源增长；
- [ ] 测试前后系统网络清理快照；
- [ ] 报告包含拓扑、ISP、目标、时间和 commit；
- [ ] 未测试条件明确标记，不扩展为普遍结论。

**验收**：只有证据覆盖的环境可声明可用；没有永久“抗干扰合格”或“网速合格”宣传。

### W18-07 安全门禁 `[ ]`

- [ ] 公网仅 HTTPS，无明文初始化降级；
- [ ] Passkey、初始化码、并发认领和会话测试；
- [ ] CSRF、CSP、HSTS、Cookie、来源校验和限流；
- [ ] ACL 拒绝优先、本地否决和临时权限冻结；
- [ ] 路径穿越、符号链接、重放和脑裂测试；
- [ ] 秘密不进入日志、活动、URL、导出和构建产物；
- [ ] 密码学实现风险披露和审计计划；
- [ ] 安全报告保留失败与残余风险。

**验收**：任一致命安全项未通过即阻断发布，不以 UI 隐藏替代服务端控制。

### W18-08 生命周期与故障恢复门禁 `[ ]`

- [ ] 关 UI 不断网；
- [ ] control 崩溃 node 继续；
- [ ] stop node 不断 control/WebUI；
- [ ] 系统重启按配置恢复；
- [ ] TUN/路由/DNS/进程残留自动清理；
- [ ] 所有失败有硬超时和自动恢复；
- [ ] 用户不需要手工 kill；
- [ ] 完成全套故障注入证据。

**验收**：历史“每次测试断网且需手工 kill”的问题有自动化复现与通过证据。

### W18-09 平台发布门禁 `[ ]`

- [ ] Linux 实机报告；
- [ ] macOS 实机报告；
- [ ] Windows 实机报告；
- [ ] Android 真机报告；
- [ ] 安装/升级/回滚/卸载；
- [ ] 服务托管和休眠/切网；
- [ ] GPUI/WebUI/Compose 可访问性；
- [ ] 不支持平台/架构明确排除。

**验收**：平台支持表每项都有证据链接或明确“不支持”，不允许空白承诺。

### W18-10 可复现构建与发布产物 `[ ]`

- [ ] 干净工作区从锁定依赖构建；
- [ ] 构建工具链和 feature 列表固定；
- [ ] 产物签名、校验和与来源证明；
- [ ] WebUI/Android 资产哈希记录；
- [ ] SBOM/许可证清单；
- [ ] 发布说明列出能力、限制和迁移；
- [ ] 二进制不包含测试秘密或开发凭据；
- [ ] 发布产物与 commit 对应。

**验收**：第三方可从记录环境复现构建并验证发布产物来源。

### W18-11 发布阻断逐项签核 `[ ]`

- [ ] 所有 P0 工作包完成；
- [ ] 所有可见动作真实闭环；
- [ ] 无伪按钮、假图表、静态成功；
- [ ] Exit 失败关闭和泄漏检查通过；
- [ ] 临时 Coordinator 权限冻结通过；
- [ ] 无手工 kill 依赖；
- [ ] 所有平台证据齐全；
- [ ] 文档、README 和能力声明与证据一致；
- [ ] 所有未解决问题有公开影响说明；
- [ ] 所有者/维护者完成最终签核。

**验收**：签核不是形式勾选；任一阻断条件失败均阻止发布。

## 22. MVP / 第一轮实施队列

第一轮不追求先把所有页面做出来，而是先建立不会再次返工的产品地基。

### 22.1 P0 地基队列 `[ ]`

- [ ] W0-01：为现状审计补代码路径、关键符号和成熟度标记；
- [ ] W0-02～W0-06：完成页面、命令、状态、代码和保留/重写/删除映射；
- [ ] W0-07：记录脏工作区、保护文件和提交边界；
- [ ] W1-01～W1-12：冻结 Node/Network/Membership/Capability/Policy/Operation/Event 和契约门禁；
- [ ] W1 生成 schema、枚举、状态机、错误码和契约测试；
- [ ] W2-01～W2-03：先解决控制面/数据面生命周期和 UI 归属；
- [ ] W2-04～W2-09：完成启停语义、崩溃恢复、服务监督和清理看门狗；
- [x] W3：完成 HTTPS、Passkey、一次性初始化码、本地恢复和认证管理 UI；
- [ ] W4：完成权威命令、operation、SSE、快照和错误模型。

**完成标志**：UI 关闭不影响节点，control/node 可独立恢复，公网写操作有安全认证，前后端契约单一且可生成。

### 22.2 P0 UI 地基队列 `[ ]`

- [x] W5-01～W5-04：设计令牌、状态组件、布局和 i18n；
- [x] W5-05～W5-08：a11y、GPUI 适配、WebUI 新工程和视觉回归；
- [x] W6-01～W6-03：五模块外壳、唯一主状态和概览；
- [x] W6-04：设置基础页；
- [x] W6-05：可搜索的高级设置权威清单；
- [x] W6-06：差异应用与回滚；
- [x] W6-07：状态矩阵；
- [ ] 删除旧双 WebUI 的新功能入口；
- [ ] 新 React/Vite 成为唯一 WebUI 产品表面；
- [ ] GPUI 与 WebUI 使用同一状态/权限/命令契约。

**完成标志**：两个 UI 都可以承载真实功能且不再分叉，未完成页面显示真实不支持状态而非伪控件。

### 22.3 第一批垂直切片队列 `[ ]`

- [ ] W7 节点页完成；
- [ ] W8 网络创建与发现完成；
- [ ] W9 成员、组与拓扑完成；
- [ ] W10 权限、隐私、策略和活动完成；
- [ ] W11 流量路由完成；
- [ ] W12 Exit 完成；
- [ ] W13 Relay 完成；
- [ ] W14 Coordinator 自动接管完成。

**完成标志**：用户可以创建/发现/加入网络，管理员治理成员与权限，节点 owner 配置路由并选择可用 Exit，Relay 独立治理，Coordinator 可安全接管。

### 22.4 第二版能力队列 `[ ]`

- [ ] W15 File 完成；
- [ ] W16 Android 完成；
- [ ] W17 Linux/macOS/Windows 跨平台硬化；
- [ ] W18 发布门禁、实测报告和可复现构建完成。

**完成标志**：第一版范围全部有真实闭环、平台证据和发布阻断签核。

### 22.5 实施顺序规则

- W1 未冻结前，不扩展旧 UI 页面；
- W2 未完成前，不把新 UI 当作长期运行控制台；
- W3/W4 未完成前，不开放公网写操作；
- W10 未完成前，不开放 Exit/Relay/File 的成员级授权；
- W11 未完成前，不宣称翻墙、分流或失败关闭可用；
- W12 未提供 Google、泄漏和性能报告前，不宣称抗干扰或网速合格；
- W16 未完成前，不把 Android 标记为第一版支持；
- W18 任一阻断项失败，不创建“第一版已完成”发布。

## 23. 任务格式与更新规则

### 23.1 任务字段 `[ ]`

- [ ] 每个新任务包含唯一 ID；
- [ ] 每个任务写明领域/UI 范围；
- [ ] 每个任务写明依赖工作包；
- [ ] 每个任务写明验收标准；
- [ ] 每个任务写明测试证据；
- [ ] 每个任务绑定负责人或明确未分配；
- [ ] 每个任务标注 P0/P1/P2 和平台范围；
- [ ] 不创建无法验证的“优化一下”“完善体验”式任务。

### 23.2 勾选规则 `[ ]`

- [ ] `[~]` 只在已有当前分支实际工作且未验收时使用；
- [ ] `[x]` 必须有 commit、测试或实测证据；
- [ ] 代码能编译不能勾选完整功能；
- [ ] 页面能打开不能勾选产品完成；
- [ ] 单次网络连通不能勾选稳定性；
- [ ] 测试失败或被跳过时不得勾选；
- [ ] 发现回归时立即改回 `[ ]` 或 `[!]`；
- [ ] 勾选时记录证据路径或命令。

### 23.3 变更同步 `[ ]`

- [ ] 产品决策变化先更新产品文档；
- [ ] 再更新 TODO 依赖、范围和验收；
- [ ] 再实现领域契约和后端；
- [ ] 最后实现 UI 和平台适配；
- [ ] 删除能力时同时删除伪入口和文档承诺；
- [ ] schema 变化同时更新生成类型；
- [ ] 活动/错误/状态矩阵随功能更新；
- [ ] 发布记录引用实际证据而非 TODO 文本。

## 24. 发布阻断清单

以下任一项未满足均阻断第一版发布。

### 24.1 生命周期阻断 `[ ]`

- [ ] 需要用户手工 kill 进程；
- [ ] 关闭 UI 导致断网；
- [ ] stop node 关闭 control/WebUI；
- [ ] control 重启后无法识别 node 状态；
- [ ] TUN、路由、DNS 或子进程残留；
- [ ] 无硬超时、无法自动恢复；
- [ ] 节点启动隐式创建第二份 runtime。

### 24.2 翻墙与网络安全阻断 `[ ]`

- [ ] Exit 不可用时静默直连；
- [ ] DNS 泄漏；
- [ ] IPv4/IPv6 未检查；
- [ ] Google/YouTube 未通过真实 Exit 验证；
- [ ] 无吞吐、延迟、丢包、并发和 24 小时报告；
- [ ] 无高延迟、丢包、断连、DNS 污染测试；
- [ ] 固定 Exit 可被 fallback 绕过；
- [ ] 测试后系统网络未恢复。

### 24.3 认证与治理阻断 `[ ]`

- [ ] 公网无 HTTPS 或允许明文降级；
- [ ] 初始化码进入 URL/日志；
- [ ] 并发认领可产生多个 owner；
- [ ] 临时 Coordinator 可修改治理；
- [ ] 无多数分区仍可写；
- [ ] 旧 Leader 可覆盖新任期；
- [ ] 节点本地否决可被管理员覆盖；
- [ ] ACL 拒绝优先未落实。

### 24.4 UI 与产品阻断 `[ ]`

- [ ] 可见按钮只返回 `Unavailable`；
- [ ] 假数据、假图表或静态成功文案；
- [ ] 两个 WebUI 表面并行演进；
- [ ] GPUI 嵌入 WebView；
- [ ] 旧 11 页直接作为一级导航；
- [ ] 状态/权限/流程在 GPUI/WebUI 分叉；
- [ ] 缺少加载、空、错误、无权限、不支持、降级和陈旧状态；
- [ ] 右侧拓扑操作与左侧列表不一致。

### 24.5 工程与平台阻断 `[ ]`

- [ ] Rust fmt/clippy/test 未通过；
- [ ] 前端类型/Lint/单测/E2E 未通过；
- [ ] schema 漂移；
- [ ] Linux/macOS/Windows/Android 证据缺失；
- [ ] 交叉编译被当作实机通过；
- [ ] 无签名/哈希/SBOM；
- [ ] 工作区脏且包含未审阅改动；
- [ ] 发布说明与能力不符。

## 25. 产品文档章节映射

| TODO 范围 | 产品文档章节 | 主要用途 |
|---|---|---|
| W0 | 28–31、44 | 文档定位、现状审计、决策日志和被否决方案 |
| W1 | 38、40.4 | 实体、状态机、命令/事件/错误和契约冻结 |
| W2 | 19.1、32.15–32.16、40.5、43.2 | 单二进制、进程关系和生命周期验收 |
| W3 | 33.1、37、40.6、43.3 | 所有者、Passkey、HTTPS、恢复和秘密生命周期 |
| W4 | 38.6–38.10 | 命令、operation、SSE、错误和生成类型 |
| W5 | 39、35、41.4 | 设计系统、状态矩阵、前端与无障碍测试 |
| W6 | 21、32.2–32.4、35 | 五模块、概览、设置、初始化和主状态 |
| W7 | 21、32.1、34 | 节点页、平台边界和节点运行 |
| W8 | 32.5–32.7、40.7 | 创建、发现、邀请、加入和审批 |
| W9 | 32.18、33、40.8 | 多网络拓扑、成员、组和交互 |
| W10 | 33、37、38.9、41 | ACL、字段隐私、策略和审计 |
| W11 | 32.8–32.10、40.9 | 规则、分流、应用身份、失败关闭和恢复 |
| W12 | 32.9–32.10、36、40.10、43.6 | Exit、Google、泄漏、网络质量和阻断条件 |
| W13 | 30.9、33、37、38 | Relay 独立能力、授权、隐私和审计 |
| W14 | 32.11–32.12、38.5、40.11 | Coordinator 候选、任期、接管、分区和恢复 |
| W15 | 32.13、40.12 | File ACL、路径安全、续传和配额 |
| W16 | 34.6、40.13 | Android Compose、VpnService、Passkey 和 SAF |
| W17 | 34、40.14、43.5 | 平台矩阵、服务、安装和实机证据 |
| W18 | 41–43、45 | 测试、风险、完成定义、提交和发布产物 |
| 23–26 | 41.6、42、43.6、45 | 证据、风险、阻断、提交约束和更新规则 |

## 26. 当前工作区与提交约束

### 26.1 工作区提交纪律 `[x]`

- [x] `crates/client/src/cli/commands.rs` 可按任务修改和提交；
- [x] `crates/client/src/cli/mod.rs` 可按任务修改和提交；
- [x] `crates/client/src/webui/controller.rs` 可按任务修改和提交；
- [x] 提交前逐次检查 `git status --short --branch`；
- [x] 不用 `git add -A` 盲目纳入并发修改；
- [x] 不执行 reset、force push 或覆盖他人改动；
- [x] 不把 release 中的二进制当作源码提交；
- [x] 提交前逐文件审阅暂存区，排除与本任务无关的 diff。

### 26.2 secure-session 与 UDP 适配器提交顺序 `[ ]`

- [ ] 先完成 secure-session 线格式/驱动及其测试，形成独立可验证提交；
- [ ] 再提交依赖该契约的独立 UDP 适配器；
- [ ] 两个提交分别运行相关测试和 clippy；
- [ ] 不使用一个混合提交掩盖依赖顺序；
- [ ] 提交说明记录契约变化和验证命令；
- [ ] 确保不携带无关脏工作区改动；
- [ ] 若依赖尚未通过，保持未提交状态而不是提前提交；
- [ ] 完成后回报 commit hash、测试结果和剩余未提交文件。

### 26.3 文档、秘密和证据 `[ ]`

- [ ] 产品文档与 TODO 可以单独提交；
- [ ] 不写入真实密码、私钥、Passkey、初始化码、Token 或远程凭据；
- [ ] 网络测试只记录脱敏拓扑、目标和结果；
- [ ] 提交前扫描敏感凭据模式；
- [ ] 测试证据引用 commit、平台、时间和环境；
- [ ] 产物和报告不包含用户隐私；
- [ ] README 不宣称未验证的翻墙、抗干扰或网速能力；
- [ ] 发布前清理临时日志、截图和测试配置。

### 26.4 本轮验收 `[ ]`

- [ ] TODO 与产品文档主章节、工作包和依赖一致；
- [ ] 所有工作包都有可勾选任务和验收标准；
- [ ] 任务按 W0→W18 和硬依赖排序；
- [ ] 第一轮队列明确先契约、生命周期、认证/API，再做 UI；
- [ ] 发布阻断条件可逐项签核；
- [ ] Markdown 围栏配对、无尾随空白和敏感信息；
- [ ] TODO 文件保持未提交，除非另行获得提交指令；
- [ ] 最终回报文件路径、任务数量、工作包数量和剩余未提交文件。
