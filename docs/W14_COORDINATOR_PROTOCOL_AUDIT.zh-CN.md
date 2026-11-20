# W14 Coordinator 选举与任期协议内部安全审计

> 内部审计日期：2026-11-15
> 代码基线：`79cfad6 docs(w14): record coordinator takeover runtime backoff`；增量覆盖后台故障接管 E2E、
> election commit 的旧 recovery/emergency 投影失效修复及对应回归
> 审计范围：候选配置、选举提案、投票、严格多数、任期/fencing、治理写入门禁、恢复与紧急运行权限
> 文档定位：**内部代码与协议安全审计**，不是第三方正式审计、形式化验证或发布安全认证

## 0. 结论摘要

本次内部审计未发现以下无信任凭据即可成功的代码路径：

- 单个候选伪造其他候选签名并凑出多数；
- 修改已签名提案的 Leader、任期、候选组或配置 revision 后重放；
- 使用旧任期、旧 fencing token 或旧 Leader 凭据覆盖新任期；
- 在租约仍可写时由普通候选发起新一轮选举；
- 在持久化文件未达成严格多数时安装新任期；
- 绕过治理门禁，在候选配置读取和治理写入之间利用 TOCTOU 冻结窗口。

审计确认的正常接管安全边界是：管理员签名定义候选组，候选自签名声明健康/意愿，提案人必须是当前
候选组中确定性的首选候选，投票使用独立签名并绑定完整选举上下文，Coordinator 在单一文件锁事务内校验
严格多数并推进单调任期和 fencing token，所有治理写入必须匹配当前任期、Leader、token 和可写租约。

但本结论不能勾选 W14-03 的“经过正式审计的自研设计”：

- 本次不是独立外部审计，没有审计机构签核、威胁建模评审记录和公开审计报告；
- 没有进行形式化模型检查、穷举状态验证或 Byzantine Coordinator 主机攻防演练；
- 后台节点故障接管已有进程内真实 `run_actor` + 持久化 store E2E，但尚未覆盖跨主机断网、真实进程崩溃和
  磁盘故障组合；
- 三端候选健康、Leader 和最近任期展示仍不完整，影响运营可观察性；
- 本地持久化、墙上时钟和 Coordinator 服务可用性仍存在部署级残余风险。

因此，本文可以作为 W14-03 的内部证据和正式审计输入，但不能替代正式审计结论，也不能把 W14-03 或
W14 总体标记为完成。

## 1. 审计边界

### 1.1 覆盖内容

- `crates/common/src/domain/coordinator.rs`：候选配置、声明、提案、投票、选举会话、任期、租约、
  fencing token、写凭据、恢复和紧急权限领域校验。
- `crates/coordinator/src/signaling/coordinator_candidates.rs`：持久化候选状态、选举 CAS、票收集、
  超时、任期安装/续租、治理门禁和审计。
- `crates/coordinator/src/signaling/server.rs`：信令入口的认证 actor 绑定、提案/投票/超时/任期消息处理。
- `crates/client/src/node/coordinator_takeover.rs`：客户端从权威状态到 proposal/vote/timeout/emergency
  动作的决策和轮询退避。
- `crates/client/src/node/runtime_control.rs`：运行时轮询、重连唤醒、权威快照刷新和动作失败处理。
- `crates/coordinator/tests/w14_coordinator_fault_process.rs`：真实进程及故障注入路径的存在性。

### 1.2 不覆盖内容

- 管理员阈值恢复的完整三端流程和真实灾难演练；该部分见
  [W14_ADMIN_RECOVERY_DESIGN.zh-CN.md](W14_ADMIN_RECOVERY_DESIGN.zh-CN.md)，仍属于 W14-07/W14-08。
- GPUI、WebUI、Android 的全部产品交互和可访问性。
- Windows/macOS 实机文件系统、时钟和进程行为。
- Coordinator 主机被完全攻陷后的保密性、可用性和供应链安全。
- 密码学原语实现审计。本文只审计协议如何调用签名、verify、序列化和绑定字段。
- 网络层 DoS、流量分析、QUIC/TLS 实现和依赖漏洞审计。

## 2. 信任模型与安全假设

### 2.1 信任根

- 网络记录和成员身份由当前治理权威维护；候选配置必须由当前 Owner 或管理员签名，并且每个候选必须
  是当前网络成员。
- 候选声明由候选节点自己的身份密钥签名，绑定网络、配置 revision、可达性、资源、意愿和短有效期。
- 选举提案和投票由候选身份密钥签名；Coordinator 信令层另外要求签名主体等于已认证会话 actor。
- Coordinator 存储是任期与投票的串行化权威；客户端只能使用 Coordinator 返回并重新验证过的状态发起动作。

### 2.2 故障假设

- 候选和客户端可能崩溃、重启、离线、收到重复/迟到消息或持有过期快照。
- 网络可能分区、延迟、丢包和重放消息。
- 节点墙上时钟可能短时漂移；超过协议边界时系统必须进入只读而不是延长旧权威。
- Coordinator 主机可能不可用，但本审计不假设其被攻陷后仍能提供安全或活性。

### 2.3 不提供的保证

- 不保证网络分区期间的活性。没有严格多数时，系统宁可停止治理写入。
- 不保证单个或被攻陷的 Coordinator 进程不可审查、不可拒绝服务；签名和任期校验主要保护安全性，
  不能保证恶意服务持续提供服务。
- 不保证所有跨平台文件系统都满足本地文件锁、`fsync`、原子 rename 和目录同步语义。

## 3. 关键协议不变量

| 不变量 | 要求 | 当前实现位置 |
|---|---|---|
| I1 配置绑定 | 新任期必须绑定当前管理员签名配置 revision 和精确候选组 | `CoordinatorElectionProposal::validate_for_election`、`CoordinatorTerm::validate_against_candidate_config` |
| I2 提案身份 | 提案人和 Leader 必须是当前首选可参选候选，不能由其他候选代签或自指定 | `validate_for_status`、`apply_election_proposal` |
| I3 严格多数 | 票数必须大于候选组的一半，Leader 必须投给自身任期 | `majority_threshold`、`CoordinatorElectionSession::commit`、`CoordinatorTerm::validate_definition` |
| I4 单调任期 | 新任期号、fencing token 必须严格前进；同任期不同 Leader 是冲突而非合并 | `CoordinatorTerm::validate_successor`、`apply_term_install` |
| I5 单事务提交 | 决定性的票、任期安装和审计追加在同一文件锁和持久化事务中完成 | `record_election_vote`、`lock_candidate_file`、`persist_bytes_atomic` |
| I6 写权限精确匹配 | 治理写入必须匹配当前网络、任期、Leader、fencing token 和可写租约 | `CoordinatorTerm::authorize_write` |
| I7 失败关闭 | 缺失配置、过期/未来/畸形状态、旧快照和不安全恢复声明不得产生本地成功推断 | 领域校验、`decide_coordinator_takeover`、`refresh_coordinator_takeover` |
| I8 治理冻结 | 紧急运行权限不得升级为治理写权限，也不得阻止后续完整任期接管 | `CoordinatorAuthority::Emergency`、`ensure_governance_allowed`、`install_term` |

## 4. 威胁与控制审计

### 4.1 签名绑定

审计结论：**通过**。

- 候选配置签名覆盖 schema、网络、revision、候选列表、生效时间、过期时间、签名人和公钥。
- 声明签名覆盖节点公钥与 Node ID 的绑定、配置 revision、可达性、资源、意愿和有效期。
- 提案签名覆盖网络、配置 revision、任期、Leader、完整候选组、提案人、公钥和有效期；
  `proposal_id` 由签名派生，修改任一绑定字段都会导致签名或 ID 不匹配。
- 投票签名覆盖网络、配置 revision、任期、Leader、完整候选组、投票人、公钥和有效期。
- 信令入口要求 `proposal.proposer == authenticated actor`、`vote.voter == authenticated actor`，避免
  替代提交或把别人的有效签名当成当前会话的身份。

测试证据：

- `election_votes_reject_forgery_and_impersonation`
- `election_votes_bind_network_revision_term_leader_and_candidate_group`
- `election_proposal_is_signed_and_bound_to_its_identity`
- `candidate_config_signatures_are_bound_to_permissions_and_membership`
- `candidate_declarations_are_self_signed_and_revision_bound`

### 4.2 持久化 CAS 与原子性

审计结论：**通过，但依赖本地持久化文件系统假设**。

- `CoordinatorCandidateStore` 在进程内持有 mutex，并通过 profile lock、候选文件 lock 和 governance gate
  串行化跨线程/跨进程读改写。
- 候选配置更新使用 `expected_revision` 做逻辑 CAS：相同 revision 的完全相同内容幂等返回，不同内容拒绝；
  旧 revision、revision 跳号和并发单胜者均失败关闭。
- 投票事务在文件锁内重新读取当前配置和任期，再追加票；达到严格多数时，在同一事务中安装任期、
  清空不再匹配的恢复投影并持久化。
- 持久化采用临时文件写入、文件 `sync_all`、原子 rename 和父目录同步；审计记录与状态变更一起落盘。

测试证据：

- `config_cas_idempotency_and_restart_are_durable`
- `concurrent_revision_updates_have_exactly_one_winner`
- `persisted_election_commits_only_after_strict_majority`
- `concurrent_split_brain_successors_install_exactly_one_authority`
- `legacy_v2_candidate_store_loads_and_upgrades_on_next_write`

残余风险：若部署在 NFS/SMB 等不保证 POSIX advisory lock 或 rename 语义的文件系统上，上述保证不成立。
当前没有网络文件系统验收，也没有跨平台掉电恢复矩阵。

### 4.3 严格多数和候选确定性

审计结论：**通过**。

- 阈值为 `candidate_group.len() / 2 + 1`，不是简单相对多数。
- 任期 majority proof 必须非空、无重复、全部属于当前候选组、与保留的签名票一一对应，并且数量大于
  候选组的一半。
- Leader 必须属于候选组且必须包含在多数票中，不能只靠 proposer 自荐。
- 只有当前有效配置、新鲜且愿意参选的健康候选参与首选候选选择；旧 Leader 在租约不可写后不会因
  过期快照直接重新成为首选。
- 客户端只能为首选候选生成提案，或者为当前候选状态的合法提案独立投票；malformed 提案在签名前失败关闭。

测试证据：

- `three_candidate_election_commits_only_after_strict_majority`
- `persisted_election_commits_only_after_strict_majority`
- `election_session_requires_strict_majority_and_is_idempotent`
- `majority_proof_and_leader_are_checked`
- `election_votes_require_unique_voters_and_matching_proof`
- `proposals_validate_against_authoritative_candidate_status_before_voting`
- `preferred_candidate_proposes_the_next_term_after_lease_expiry`
- `non_preferred_candidate_waits_instead_of_signing_a_conflicting_proposal`

### 4.4 迟到票、重复票和冲突票

审计结论：**通过**。

- 完全相同投票重复提交是幂等的，不重复计数。
- 同一 voter 在同一选举中提交不同内容会产生
  `domain.coordinator_election_conflicting_vote`，不能替换原票。
- 超过 proposal deadline 的票、已过期票、来自未来的票和 terminal election 后的票均拒绝。
- 选举超时是持久状态；超时后的票不能使旧会话恢复，后续 `latest_term` 也不会被回滚。

测试证据：

- `election_session_rejects_conflicting_duplicate_votes_and_expiry`
- `election_timeout_is_durable_and_rejects_late_votes`
- `minority_loss_and_delay_recover_without_rolling_back_the_term`

### 4.5 配置 revision

审计结论：**通过**。

- 配置 revision 必须为正；更新必须精确等于当前 revision + 1，期望 revision 不匹配时拒绝。
- 提案、投票和任期都绑定配置 revision 与完整候选组；配置更新后旧声明、旧恢复投影和旧紧急租约失效。
- 服务端在提案和投票时重新读取当前配置，不能仅相信客户端状态。
- 配置变化会使旧成员快照下的恢复报告和待执行治理状态无法继续使用。

测试证据：

- `config_cas_idempotency_and_restart_are_durable`
- `declarations_require_self_signature_current_candidate_and_revision`
- `candidate_declarations_are_self_signed_and_revision_bound`
- `old_leader_recovery_enters_read_only_before_rejoining`
- `rejoin_as_member_when_config_changes_candidate_group`

### 4.6 租约与 fencing token

审计结论：**通过**。

- 任期号和 fencing token 必须为正；后继任期号和 token 必须严格增大。
- 任期定义要求所有 majority vote 覆盖任期窗口；票的有效期不能短于任期。
- 续租不能改变 term/Leader/token，不能缩短租约，不能超过最大租约时长，也不能超过选举票的有效期。
- 写凭据必须精确匹配当前网络、任期、Leader 和 token；旧任期、未来任期、旧 token、未来 token、
  过期租约和超界时钟回拨均拒绝。
- 旧 Leader 恢复只恢复健康检查和数据面能力，不恢复写权限；恢复报告需要重新匹配权威快照和审计游标。

测试证据：

- `terms_survive_restart_and_old_writes_are_rejected_and_audited`
- `lease_renewal_extends_authority_and_expiry_enters_read_only`
- `stale_terms_and_fencing_tokens_are_rejected_after_recovery`
- `stale_and_expired_write_credentials_fail_closed`
- `lease_projection_distinguishes_renewal_expiry_and_clock_skew`
- `bounded_clock_rollback_marks_authority_read_only`
- `recovered_old_credential_remains_rejected`

残余风险：租约判断使用节点墙上时间。协议对超过上限的回拨失败关闭，但不能消除 NTP 欺骗、系统时钟被
高级攻击者修改或跨主机时间源不一致带来的活性问题。

### 4.7 TOCTOU 与治理竞态

审计结论：**通过当前实现的串行化模型**。

- `with_governance_allowed` 在读取候选状态前获取 governance gate，并把 gate/permit 持有到治理写入完成；
  候选配置更新和声明更新走同一进程锁，不能插入“检查通过后又被冻结”的窗口。
- 服务端在排队的治理操作真正执行前重新检查当前权威，而不是只依赖入队时的快照。
- 并发 successor 安装由文件锁和单调任期校验收敛为单胜者；失败安装不撤销已有紧急租约或制造部分状态。

测试证据：

- `governance_gate_serializes_governance_writes_with_candidate_updates`
- `governance_gate_fails_closed_until_a_current_candidate_is_healthy`
- `governance_gate_freezes_pending_configuration_without_actor_membership`
- `queued_old_authority_is_rechecked_after_authority_replacement`
- `failed_successor_install_preserves_emergency_lease_and_freeze`
- `concurrent_split_brain_successors_install_exactly_one_authority`

### 4.8 恢复、紧急权限与失败关闭

审计结论：**通过安全性检查，仍需更多真实演练**。

- 紧急 Leader 只有在服务端观察到所有配置候选不可用时才能产生，只能执行运行协调类动作，不能进行
  候选配置、成员、策略等治理写入，也不能把自己提升为正常 Leader。
- 候选恢复、配置 revision 变化或 successor term 安装会原子撤销或失效紧急租约；截断/重启不会延长
  原硬截止时间。
- 恢复响应中的 `governance_writable`、健康检查和数据面能力必须符合失败关闭约束；客户端发现不安全
  声明时不会据此授予本地写权限。
- 客户端每轮先读取权威候选状态和恢复快照，再签名或提交动作；动作失败不会被本地推断为成功。

测试证据：

- `emergency_leader_cannot_write_governance_or_promote_itself`
- `emergency_lease_requires_all_candidates_offline_and_selects_leader_stably`
- `candidate_recovery_revokes_emergency_lease_and_is_audited`
- `successor_term_install_atomically_revokes_emergency_authority`
- `recovery_rejects_future_term_and_audit_cursor`
- `concurrent_recovery_is_durable_and_audited_once`
- `emergency_issue_ack_for_remote_leader_does_not_enable_local_authority`

### 4.9 客户端重连与轮询退避

审计结论：**通过当前单机运行时测试**。

- 正常轮询基础间隔为 5 秒；失败后按 5/10/20/40/60 秒有界退避，最大 60 秒。
- 抖动由稳定 identity 和失败次数确定性地生成 ±10%，避免大量客户端同步重试，同时保持测试和事故复现可重复。
- 刷新成功或没有可轮询会话时重置退避；失败时推进退避。
- 信令 `Reconnected` 会通过 unbounded wakeup channel 立即触发权威状态和恢复快照刷新，不等待下一轮常规轮询。
- 重连唤醒测试使用真实 actor、测试信令会话和 wakeup channel，在 2 秒内观察到
  `CoordinatorCandidateStatusRequest` 与 `CoordinatorRecoveryRequest`，证明无需等待至少 4.5 秒的首轮常规轮询。

测试证据：

- `takeover_poll_backoff_is_deterministic_bounded_and_resettable`
- `takeover_poll_jitter_differs_by_stable_seed`
- `coordinator_takeover_reconnect_wakeup_refreshes_without_waiting_for_poll`
- `coordinator_takeover_poll_preserves_the_live_data_plane_session`

## 5. 本轮实际执行证据

2026-11-15 在本 worktree 实际执行：

```text
cargo fmt --package fuckipv6-client
git diff --check
cargo check -p fuckipv6-client
cargo test -p fuckipv6-client node::coordinator_takeover --lib
  14 passed
cargo test -p fuckipv6-client node::runtime_control::tests::coordinator_takeover --lib
  2 passed
```

此前同一代码线上还执行过：

```text
cargo test -p fuckipv6-common domain::coordinator --lib
  33 passed
cargo test -p fuckipv6-coordinator signaling::coordinator_candidates --lib
  37 passed
```

其中 `coordinator_takeover_reconnect_wakeup_refreshes_without_waiting_for_poll` 和
`coordinator_takeover_poll_preserves_the_live_data_plane_session` 本轮均通过。

`coordinator_takeover_background_candidate_failure_e2e` 使用两个真实后台 `run_actor`、两套独立节点身份和
同一个持久化 candidate store：候选 A 的声明过期后，B 自动提案 term 2，B/C 自动投票并提交严格多数结果；
测试验证 fencing token 2、B 为新 Leader、lease 可写且两个 runtime 保持存活。该场景发现旧 recovery
投影未随 election commit 原子失效的问题，`election_commit_invalidates_recovery_projections_from_the_previous_term`
已覆盖修复后的持久化读取和重新恢复。

## 6. 审计发现与处置

| ID | 级别 | 发现 | 处置 |
|---|---|---|---|
| IA-01 | 发布门禁 | 当前结论不是独立正式审计，缺少第三方报告和签核 | 保持 W14-03 审计项未勾选，安排外部安全审计 |
| IA-02 | 已关闭 | 后台节点故障接管缺少真实运行时 E2E | `coordinator_takeover_background_candidate_failure_e2e` 验证两个真实 `run_actor` 自动完成 term 2 接管；旧 recovery 投影失效缺陷由 `election_commit_invalidates_recovery_projections_from_the_previous_term` 回归覆盖 |
| IA-03 | 中 | UI 未完整展示候选健康、Leader、最近任期和选举轨迹 | 补齐 GPUI/WebUI/Android 等价投影和运营视图 |
| IA-04 | 环境假设 | 持久化正确性依赖本地文件锁、`fsync`、原子 rename 和目录同步 | 明确只支持已验证文件系统；禁止网络盘部署，补跨平台测试 |
| IA-05 | 环境假设 | 租约/超时依赖墙上时钟和有限漂移边界 | 部署可信时间源，持续暴露 `clock_skew` 告警并演练回拨 |
| IA-06 | 可用性 | Coordinator 服务被拒绝服务时可能无新任期；安全性可保持但活性不保证 | 监控控制面中断、保留数据面并补多节点切换/恢复演练 |
| IA-07 | 范围 | 管理员阈值恢复、三端流程和灾难演练未纳入本次审计 | 继续按 W14-07/W14-08 独立验收 |

未发现需要在本审计文档中标记为“可绕过严格多数或直接制造双 Leader 写入”的严重代码缺陷。该表述只反映
当前代码审查范围和测试覆盖，不构成对外安全保证。

## 6.1 本轮补充：有界穷举交错扫描与送审包

2026-11-20 增补两项与 W14-03 直接相关的内容，均不改变“未经第三方审计”的事实：

- `bounded_exhaustive_interleavings_hold_authority_invariants`：在真实 `CoordinatorCandidateStore`（真实文件锁、
  真实持久化、真实验签）上枚举有序动作序列，默认执行 5 条三步危险交错；`FUCKIPV6_W14_INTERLEAVING_DEPTH=3`
  时枚举全部 1000 条选举有序三元组和 216 条紧急有序三元组。每步校验 I3（严格多数）、I4（任期/fencing 单调）、
  单一 `Committed` 选举、紧急权限与可写任期互斥、紧急期治理必须失败关闭，以及被拒绝操作不得改变任期、选举阶段、
  紧急 epoch 等权威字段。该扫描是有界的，不替代形式化模型检查。
- `docs/W14_COORDINATOR_AUDIT_PACKAGE.zh-CN.md` + `scripts/w14-coordinator-audit-evidence.py`：把送审问题
  （Q1–Q7）、制品清单、威胁/控制映射（T1–T14）、证据追踪矩阵（E-*）和复现命令固化；证据脚本会把每个分组绑定到
  Git commit 并写出 `target/w14-coordinator-audit/evidence.json`，其 `verdict` 固定为 `collected`，不会声明审计通过。

## 7. W14-03 审计判定

可以确认的实现项：

- [x] 选举过程有超时、退避和幂等重试；
- [x] 客户端重连后刷新 Leader 与任期中继信息；
- [x] 记录选举阶段、参与者集合和结果；
- [x] 真实后台客户端运行时完成候选故障后的自动接管 E2E；
- [x] 严格多数、旧任期、旧 token、迟到/重复/冲突票和配置 revision 均有失败关闭路径。

不能确认的发布项：

- [ ] 自研设计已经过正式、独立、可公开复核的安全审计；
- [ ] 三端运营界面完整展示候选健康、Leader 和最近任期；
- [ ] 管理员阈值恢复和三端灾难流程完成。

结论：W14-03 继续保持 `[ ]`，本文只作为内部审计证据，不改变总体完成状态。

## 8. 复现命令

```bash
cargo test -p fuckipv6-common domain::coordinator --lib
cargo test -p fuckipv6-coordinator signaling::coordinator_candidates --lib
cargo test -p fuckipv6-client node::coordinator_takeover --lib
cargo test -p fuckipv6-client node::runtime_control::tests::coordinator_takeover --lib
cargo test -p fuckipv6-coordinator --test w14_coordinator_fault_process

# 有界穷举交错（深度 3 为全三元组，release 构建下为分钟级）
FUCKIPV6_W14_INTERLEAVING_DEPTH=3 \
  cargo test --release -p fuckipv6-coordinator --lib \
  signaling::coordinator_candidates::tests::bounded_exhaustive

# 或一次性收集全部送审证据
python3 scripts/w14-coordinator-audit-evidence.py --depth 3 --release --skip-slow
```

最后一组真实进程故障测试需要按仓库脚本和环境要求执行，可能依赖 Linux 权限、netns/nftables 或 QEMU；
未在本轮无条件重跑的项目不能作为本轮 E3/E4 证据。
