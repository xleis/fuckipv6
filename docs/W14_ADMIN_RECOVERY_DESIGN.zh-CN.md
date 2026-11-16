# W14 管理员阈值恢复与本地/离线灾备设计

> 状态：功能代码与真实多节点灾难演练已完成（2026-11-16）。本文定义网络治理 roster 的管理员阈值恢复，
> 不替代 W3/J-16 的节点 owner 本机 Passkey 恢复。GPUI、WebUI、Android 和本地 CLI 已接入同一命令与状态
> 语义；`w14_admin_recovery_multinode_offline_trustee_drill` 与 root 专项 `w14_admin_recovery_netns_drill`
> 已用真实进程覆盖离线 Trustee 审批、控制面中断、执行、旧会话失效和幂等重跑。仍缺独立第三方的正式安全
> 审计结论，因此 W14-08 已勾选、W14 总体仍等待 W14-03 的审计结论。

## 1. 目标与边界

### 1.1 目标

- 在所有管理员 Passkey 丢失、管理员设备损坏、网络长期离线或可信控制面不可用时，仍能在预授权的
  Trustee 阈值内恢复网络治理。
- 恢复只能在精确匹配的网络、成员快照、目标 roster、请求有效期和撤销计划下执行。
- 不建立一个隐藏后门：本地系统权限、单个管理员、单个 Trustee、邮件、安全问题或客服代操作都不能单独
  完成恢复。
- 恢复后立即轮换权威 revision，并使旧会话、邀请和未确认治理写入失效。
- GPUI、WebUI、Android 和本地 CLI 使用相同的请求、审批、状态和错误语义。

### 1.2 非目标

- 不恢复已丢失的私钥，也不导出任何成员或 Trustee 的私钥。
- 不绕过正常治理去修改网络身份、网络 ID、成员私钥或既有数据面证书。
- 不在正常在线治理可用时把恢复流程当作快捷的“管理员重置”。
- 不允许离线 Trustee 参与普通成员、策略或邀请治理。

## 2. 信任模型

恢复涉及两类互斥策略：

1. **在线管理员策略**：未配置离线 Trustee 时，Owner 与当前管理员的严格多数审批。
2. **离线 Trustee 策略**：已配置 `recovery_authorities` 和 `recovery_threshold` 时，只计算预注册的离线
   Trustee 审批；普通管理员签名不能混入该阈值。

信任假设：

- 网络记录中的 Trustee 只有公钥；对应私钥在恢复前离线保存，不进入节点、同步盘、日志或备份明文。
- 每个审批是独立签名，绑定完整 proposal、网络、快照、目标 roster、原因、nonce 和有效期。
- 本机系统权限只证明“谁在本地发起/提交恢复”，不计作一个审批。
- 任一 Trustee 设备或单个管理员最多只能贡献一个审批，不能重复计数。
- 当阈值无法满足、网络记录已被正常治理更新或没有配置可用的离线恢复策略时，系统必须失败关闭。

没有离线 Trustee 且全部当前管理员凭据无法使用时，不得尝试隐式降级为单机恢复；网络只能由仍满足严格多数
的权威重建，否则应明确进入“不可恢复”状态并由运营者创建新网络。

## 3. 恢复请求

恢复请求是一个可离线传递的签名 bundle，包含：

- `schema_version`、`request_id` 和 `network_id`；
- 发起时的 Owner、管理员、成员和 roster digest，以及网络 revision；
- 精确目标 Owner、管理员和成员集合；
- 发起人、恢复原因、随机 nonce、签发时间和过期时间；
- 强制撤销计划：旧会话、邀请和未确认写入；
- 一个或多个独立审批及其签名时间和公钥。

约束：

- 有效期必须为正且不超过 7 天；审批签名时间必须落在请求窗口内。
- `request_id` 由完整 proposal 内容派生，任何字段变化都会产生新的请求。
- roster digest 必须与提交时权威网络记录完全匹配；成员或管理员变化会使请求过期。
- 目标 roster 必须通过领域校验，不能移除网络身份或将 Owner 与管理员重复列出。
- 审批按 signer 和公钥双重去重；伪造、跨网络、重复、过期和阈值不足均失败关闭。

## 4. 本地/离线流程

### 4.1 预先配置

1. 管理员在正常治理状态下选择恢复模式：
   - 在线管理员严格多数；或
   - 离线 Trustee 阈值。
2. 离线模式下，只导入 Trustee 公钥并设置不超过公钥数量的阈值。
3. UI 展示 Trustee 数量、阈值、缺席恢复后果和公钥指纹，要求管理员再次确认。
4. 配置通过正常签名治理写入并进入不可静默删除的审计。

### 4.2 灾难发生后的导出与审批

1. 发起者在任一受信节点或本地 CLI 中输入网络 ID、当前 Owner/管理员/成员快照和目标 roster。
2. 系统从权威快照生成影响预览：替换的 Owner、增删管理员、增删成员、将失效的会话/邀请/未确认写入。
3. 发起者选择原因、填写 nonce 和短有效期，导出不含私钥的恢复请求文件。
4. 每个 Trustee 在独立离线设备上校验请求摘要、网络、影响和有效期；确认后生成 detached approval。
5. 审批可通过受控介质或分别导出的文件回传，不要求 Trustee 和发起者同时在线。

### 4.3 导入、提交与执行

1. 发起者导入全部审批文件；客户端在本地先验证签名、窗口、网络、快照、目标 roster 和阈值。
2. UI 显示“已有/所需”审批数、参与者、影响、剩余有效期、撤销路径和不可逆确认。
3. 请求提交到 Coordinator 后由服务端重新读取权威网络和当前恢复策略，禁止信任客户端结论。
4. 仅当请求仍为 Pending、阈值满足、快照未变化且当前操作者获准时，才允许执行。
5. 执行必须在服务互斥锁内原子完成 roster/revision 更新和旧权威失效编排；部分失效失败可幂等续跑。
6. 执行连接必须先收到 `AdminRecoveryExecuteAck { Executed }`，再收到 Close；不得以裸断连表示成功。
7. 其他旧权威会话立即失效；不在新成员快照中的 pending applicant 保持信令连接，但其旧治理状态不可继续。

### 4.4 撤销

- Pending 请求可由具备恢复权限的操作者撤销，必须填写原因并持久化审计。
- 已执行请求不可撤销；只能通过新的正常治理或新的灾难恢复请求修正。
- 已过期、roster 变化或阈值失效的请求显示为不可执行，不自动删除历史记录。

## 5. 本地 CLI 约定

以下命令是待实现的产品接口草案，参数和输出必须与三端共享同一领域类型：

```text
fuckipv6 network recovery request --network <id> --reason <reason> --output <file>
fuckipv6 network recovery approve --request <file> --output <approval-file>
fuckipv6 network recovery verify --request <file> --approval <file>...
fuckipv6 network recovery submit --request <file> --approval <file>...
fuckipv6 network recovery status --network <id> --request <id>
fuckipv6 network recovery revoke --network <id> --request <id> --reason <text>
fuckipv6 network recovery execute --network <id> --request <id>
```

CLI 规则：

- 默认只输出公开 ID、指纹、计数、状态、时间和影响，不输出私钥或完整审批材料。
- `approve` 必须要求本地显式确认请求摘要；非 TTY 环境必须提供确认参数。
- `submit` 和 `execute` 是独立步骤，提交成功不等于执行成功。
- 退出码和稳定错误键按 `admin_recovery.error.*` 映射，脚本可区分阈值不足、过期、stale snapshot、
  无权限、已执行和已撤销。

## 6. 三端 UI 要求

GPUI、WebUI 和 Android 必须实现同一流程，允许平台差异，但不允许语义差异：

1. **影响页**：显示 Owner 变更、管理员增删、成员增删、旧会话/邀请/未确认写入失效范围。
2. **参与者页**：显示已有审批、所需阈值、匿名化 Trustee 指纹、缺失审批和签名有效性。
3. **有效期页**：显示签发/过期时间和剩余时长；过期后所有执行入口禁用。
4. **撤销页**：说明 Pending 可撤销、Executed 不可撤销，以及正确补救路径。
5. **最终确认**：要求近期本地认证；明确“恢复后当前会话将失效”，不能使用默认确认按钮。
6. **执行结果**：等待并展示服务端 Ack；断线后只能查询状态，不得在本地推断成功。
7. **审计页**：显示请求、审批、提交、撤销、执行、失效完成和失败重试事件，但不泄露秘密材料。

公共投影至少包含 `AdminRecoveryImpact`、`AdminRecoveryStatusView`、`participants`、
`signature_count`、`required_signatures`、`expires_at_unix_ms`、`revocation`、`can_execute` 和
稳定 `denial_key`。三端不得各自推导“可执行”状态。

## 7. 状态机与失败处理

```text
Pending ──threshold+valid──> Executed
   │                            │
   ├──expiry──────────────────> Expired
   ├──roster change───────────> Blocked/Stale
   └──operator revoke─────────> Revoked

Pending ──execute accepted──> Executing ──partial failure──> Pending/Incomplete
                                             └──retry──────> Executed
```

- 本地和服务端都必须重新校验，客户端状态只用于展示。
- 重复提交同一请求应幂等返回当前状态；不同请求内容不能复用同一 request ID。
- 执行 Ack 丢失时，客户端查询权威状态，不能重新生成不同请求或重复应用 roster。
- 失效编排部分失败时保留不可执行真相和失败原因，允许使用同一请求幂等续跑。
- 时钟漂移超过边界、无法读取权威网络或恢复策略不一致时失败关闭。

## 8. 审计与隐私

每个恢复事件至少记录：

- event ID、network ID、request ID、阶段和结果；
- 发起人/审批人 Node ID 与公钥指纹；
- 快照/目标 roster digest、网络 revision、阈值和过期时间；
- 原因、撤销原因、错误键、重试次数和 Ack 状态；
- 旧会话、邀请和未确认写入的失效计数。

审计不得记录私钥、恢复码、Passkey 材料或其他可直接用于冒充的敏感材料。若审计需要引用审批，
只记录 signer、公钥指纹和请求摘要，不复制完整审批包。恢复审计必须进入与网络治理相同的长期审计链，
不能被普通管理员静默删除。

## 9. 当前实现映射

已有实现：

- `crates/common/src/domain/admin_recovery.rs`：请求、快照、目标 roster、审批、阈值、影响投影和
  `apply_to_network`。
- `crates/coordinator/src/signaling/admin_recovery.rs`：持久化提交/状态/列表/撤销/执行、并发单胜者和
  失效编排。
- `crates/client/src/signaling/client.rs`：提交、状态、列表、撤销和执行的请求/响应关联及稳定错误键。
- `websocket_admin_recovery_ack_precedes_old_session_close`：真实 WebSocket Ack 先于旧会话关闭。
- node 命令层、本地 CLI、GPUI、WebUI 和 Android Compose：提交、状态、列表、撤销和执行的等价流程；
  execute 在提交前强制读取同 request ID 的最新权威状态，仅当 Pending 且 `can_execute == true` 时发出一次，
  Ack 丢失后只查询状态而不自动重放。
- `crates/coordinator/tests/w14_admin_recovery_multinode.rs`：真实多节点灾难演练。Coordinator 与每个离线
  Trustee 都是独立进程，Trustee 只持有自己目录下的私钥并输出一个 detached approval；演练覆盖 2/3 阈值、
  单审批与“旧管理员 + Trustee”混合请求的失败关闭、两次控制面 SIGKILL/重启后的请求持久化、执行 Ack 先于
  Close、旧会话与旧邀请失效、快照外 pending applicant 存活、新 Owner 可用以及重复执行幂等。报告 schema 为
  `fuckipv6.w14.admin-recovery.drill.v1`，可用 `FUCKIPV6_W14_ADMIN_RECOVERY_REPORT` 导出。
- `scripts/test-w14-admin-recovery-netns.sh`：root 专项，在独立网络 namespace 中运行同一演练并用 nftables
  切断 Operator 到 Coordinator 的路径，验证中断期间不推断成功、恢复后流程继续。
- 服务端错误映射：`admin_recovery_protocol_error` 现在把 `domain.admin_recovery_threshold_not_met`、
  `_expired`、`_stale_snapshot`、`_network_mismatch` 显式映射到公开稳定键，阈值不足不再退化为
  `admin_recovery.error.internal`；`admin_recovery_domain_rejections_keep_stable_public_keys` 固定该边界。

仍需实现：

- 正式外部安全审计：需要独立第三方对候选人 threshold 恢复、持久化事务和失效编排出具审计结论。

## 10. 验收矩阵

| 场景 | 预期 |
|---|---|
| 在线管理员达到严格多数 | 可提交并由服务端验证后执行 |
| 在线管理员少于严格多数 | `admin_recovery.error.threshold_not_met` |
| 配置离线 Trustee | 只接受 Trustee 审批，普通管理员不能混入 |
| 伪造、重复或跨网络审批 | 失败关闭且不改变 roster |
| 请求过期或 roster 变化 | 不可执行，不更新 revision |
| 执行成功 | revision 前进，旧会话/邀请/未确认写入失效，Ack 先于 Close |
| 执行 Ack 丢失 | 查询得到 Executed，不重复应用 |
| 部分失效失败 | 保持受限/未完成状态，可幂等续跑 |
| Pending 撤销 | 后续执行失败，撤销事件可审计 |
| 本地系统权限单独操作 | 不计入审批，不能完成恢复 |
| 三端操作 | 影响、参与者、有效期、撤销路径和错误键一致 |

**完成标准**：上述矩阵由领域、Coordinator、客户端、真实 WebSocket、三端 UI/Android 和至少一次真实
多节点离线 Trustee 演练共同覆盖后，W14-08 才可整体勾选。

该标准已于 2026-11-16 满足：领域与存储测试覆盖全部行，`websocket_admin_recovery_ack_precedes_old_session_close`
覆盖真实 WebSocket 的 Ack/Close 顺序，`w14_admin_recovery_multinode_offline_trustee_drill` 覆盖真实多进程离线
Trustee 演练，`w14_admin_recovery_netns_drill` 覆盖真实网络中断。矩阵中的“执行 Ack 丢失”“部分失效失败”两行由
存储层测试与演练中的重复执行幂等断言共同覆盖；正式外部安全审计仍归 W14-03，不影响本项验收。
