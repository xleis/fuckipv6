# W14 Coordinator 接管：外部安全审计包

> 文档定位：**送审材料清单与复现指南**，不是审计结论。本文自 2026-11-20 起维护，用于把自研选举/任期协议
> 交给独立第三方审计。W14-03 的“经过审计的自研设计”只有在该第三方出具正式报告后才能勾选。
> 内部审计结论见 [W14_COORDINATOR_PROTOCOL_AUDIT.zh-CN.md](W14_COORDINATOR_PROTOCOL_AUDIT.zh-CN.md)；
> 管理员阈值恢复见 [W14_ADMIN_RECOVERY_DESIGN.zh-CN.md](W14_ADMIN_RECOVERY_DESIGN.zh-CN.md)。

## 1. 审计请求

请独立审计方对下列问题给出**可否**（而不是“看起来没问题”）的结论：

| 编号 | 请求审计方判定 |
|---|---|
| Q1 | 是否存在无需当前管理员签名配置、无需严格多数票即可安装任期的路径？ |
| Q2 | 是否存在两个 Leader 同时持有可写治理权限的时间窗？ |
| Q3 | 旧任期、旧 fencing token、旧快照在恢复/重放/迟到消息下是否可能重新获得写权限？ |
| Q4 | 候选配置 CAS、选举提交、任期安装和审计追加是否在同一持久化事务内，崩溃后能否出现“已提交但审计缺失”或反向不一致？ |
| Q5 | 紧急临时 Leader 是否在任何路径上能升级为治理写权限或静默延长？ |
| Q6 | 客户端是否可能仅凭本地状态推断出接管成功？ |
| Q7 | 上述性质是否只对“Coordinator 主机诚实但会崩溃、网络会分区”的模型成立，以及在主机被完全攻陷时的边界是什么？ |

本仓库不主张自研协议已被审计。审计方给出否定结论（发现缺陷）同样是可交付结果，我们会据此修复并重跑证据。

## 2. 送审制品

### 2.1 协议实现

| 层 | 路径 | 内容 |
|---|---|---|
| 领域规则 | `crates/common/src/domain/coordinator.rs` | 候选配置、候选声明、提案、投票、选举会话、任期、租约、fencing token、写凭据、恢复报告、紧急权限 |
| 持久化与事务 | `crates/coordinator/src/signaling/coordinator_candidates.rs` | CAS 配置更新、提案/投票/超时的文件锁事务、任期安装/续租、紧急签发与撤权、治理门禁、审计追加 |
| 信令入口 | `crates/coordinator/src/signaling/server.rs` | 认证 actor 绑定、候选/选举/任期消息处理、治理冻结拦截 |
| 客户端决策 | `crates/client/src/node/coordinator_takeover.rs` | 从权威状态到 proposal/vote/timeout/emergency 动作的决策与退避 |
| 运行时轮询 | `crates/client/src/node/runtime_control.rs` | 权威状态刷新、重连唤醒、动作失败处理 |

### 2.2 设计文档

- 本文与内部审计文档（威胁模型、不变量、发现处置）。
- `docs/W14_ADMIN_RECOVERY_DESIGN.zh-CN.md`（灾难恢复边界，供 Q3/Q5 参考）。
- `docs/QUIC_IDENTITY_THREAT_MODEL.md`、`docs/SECURITY_MODEL.md`（传输与身份前提）。

### 2.3 已冻结的不变量

内部审计 §3 定义的 I1–I8 就是本次送审的性质清单（配置绑定、提案身份、严格多数、单调任期、单事务提交、
写权限精确匹配、失败关闭、治理冻结）。本轮新增的穷举扫描（§5）直接对这 8 条中的 I3/I4/I6/I8 做状态级检查。

## 3. 协议规格摘要

### 3.1 参与者与凭据

- **管理员签名配置**：网络 Owner/管理员签名的版本化候选组（成员、优先级、生效窗口、revision）。
- **候选声明**：候选自签名的健康/资源/意愿/可达性声明，绑定网络与配置 revision，短有效期。
- **选举提案**：候选自签名，绑定网络、配置 revision、任期号、目标 Leader、精确候选组、时间窗。
- **选举投票**：候选自签名，绑定提案的全部上下文字段；verifier 要求投票人属于该候选组且票不重复。
- **任期**：`{network_id, term, leader, candidate_group, candidate_config_revision, majority_votes,
  fencing_token, issued_at, expires_at, authority}`；治理写凭据为 `{network_id, term, leader, fencing_token}`。
- **紧急租约**：候选全失时签发的有硬期限运行权限，`governance_writable=false`，到期或用后继任期接管后撤销。

### 3.2 状态机

```text
（无任期 / bootstrap）
   └─ propose(term n) ─> Voting ─(严格多数)─> Committed ─> install(term n) ─> Stable(可写租约)
            └─ timeout ─> TimedOut ─> 仅允许更高任期重新提案
   Stable ──lease 过期/时钟漂移──> ReadOnly ─> 需更高任期 install
   Stable ──候选全失──> emergency issued(有硬期限, 治理冻结)
   emergency ──后继任期 install(严格多数+配置 revision 匹配)──> emergency 撤销 + 治理恢复
```

### 3.3 持久化布局

单一 JSON 文件 + 进程内互斥 + 跨进程 `flock` + 原子 rename + `fsync`：

```text
{ networks: { <network_id>: { config, candidates{declaration}, latest_term, latest_election,
                              emergency_leader, emergency_epoch, audit_sequence } },
  consumed_nonces, audit_log: [ ... ] }
```

所有写路径在**同一个文件锁**内完成“读-校验-改-落盘-审计”，审计追加与状态变更不可分离（I5）。
管理员配置更新走 revision CAS（`update_config(expected_revision)`），重复提交幂等。

## 4. 威胁清单与现有控制

| # | 攻击/故障 | 控制 | 证据（§6 表格 ID） |
|---|---|---|---|
| T1 | 单候选伪造他人签名凑多数 | 每个投票独立 Ed25519 验签 + 候选组成员校验 | E-D1、E-D3 |
| T2 | 篡改已签名提案的 Leader/任期/组后重放 | 签名覆盖全部上下文字段；proposal_id 由内容派生 | E-D1、E-F1 |
| T3 | 旧任期/旧 token 覆盖新任期 | 单调任期与 fencing token 高水位；写凭据精确匹配 | E-S1、E-S5 |
| T4 | 租约仍可写时发起新选举 | 提案要求当前任期不可写/过期 | E-S2 |
| T5 | 持久化达到多数前的任期安装 | install 前校验 majority_proof 与配置 revision | E-S3、E-X1 |
| T6 | 并发同任期双 Leader 安装 | 文件锁内 CAS + 同任期不同 Leader 判冲突 | E-S4 |
| T7 | 配置读取与治理写入之间的 TOCTOU | 治理门禁在读配置后保持持有直到写入结束 | E-G1 |
| T8 | 紧急 Leader 越权治理或静默延长 | `authority=Emergency` 禁止治理写；硬期限与单次续租 | E-E1、E-E2 |
| T9 | 崩溃/断电后状态与审计不一致 | 单事务写入 + 原子 rename + `fsync`；断电演练 | E-C1、E-C2 |
| T10 | 网络分区/无多数时期写入部分成功 | 失败关闭，只读；恢复后按任期对齐 | E-F2、E-N1 |
| T11 | 客户端凭本地状态推断成功 | 决策只消费权威状态；动作失败不本地推断 | E-K1、E-K2 |
| T12 | 恶意 Coordinator 主机 | **不提供保证**（超出模型） | 见 §7 残余风险 R1 |
| T13 | DoS 导致无新任期 | 安全性保持、活性不保证 | R2 |
| T14 | 时钟回拨延长旧租约 | 超过 30 秒漂移边界转只读并告警 | E-S6 |

## 5. 有界穷举状态扫描（本轮新增）

自研协议最大的审计风险是“单条规则都对，但交错顺序出错”。为此新增
`bounded_exhaustive_interleavings_hold_authority_invariants`：
对真实 `CoordinatorCandidateStore`（真实文件锁、真实持久化、真实验签）枚举**有序动作序列**，
每步后重新读取权威状态并检查不变量。

- 选举字母表（10 个动作）：提案（两个候选各一）、单票、第二票、超时、缺多数安装、合法安装、后继任期安装、
  续租、治理写。
- 紧急字母表（6 个动作）：签发紧急租约、紧急下治理写、合法安装、后继任期安装、续租、紧急下更新候选配置。
- 深度由 `FUCKIPV6_W14_INTERLEAVING_DEPTH` 控制：默认 `2`（全排列有序对），审计/发布证据使用 `3`
  （有序三元组：1000 + 216 条序列）；无论深度如何，额外固定执行 14 条三步危险交错（提交后超时、
  超时后安装、提交后并发续租、后继任期与旧凭据、紧急撤销顺序等）。
- 每步断言：
  1. 任一已安装任期都有**严格多数**且投票人互不相同且都在候选组内；
  2. 任期号单调不减，同任期 fencing token 不回退，换任期时必须前进；
  3. `Committed` 选举必须满足严格多数；
  4. 紧急租约 `Active` 时不得同时存在可写任期；
  5. 紧急租约 `Active` 时 `ensure_governance_allowed` 必须失败关闭；
  6. **被拒绝的操作不得改变任何持久权威字段**（任期、租约到期、选举阶段/票数、紧急 epoch）。
- 该扫描只断言协议不变量，不断言落盘耐久性，因此用内存盘（`/dev/shm`，不可用时回退临时目录）运行；
  耐久性由 E-C1/E-C2 的断电与独立进程测试覆盖。
- 默认测试运行只跑 5 条危险交错：未优化构建里每次 store 调用都要重新验证 Ed25519，全量枚举会拖慢常规
  测试。完整覆盖用 `FUCKIPV6_W14_INTERLEAVING_DEPTH=3 cargo test --release -p fuckipv6-coordinator --lib
  coordinator_candidates::tests::bounded_exhaustive`（实测 8.3 s）。

扫描是**有界**的：它不覆盖任意长度的协议历史，也不替代形式化模型检查（TLA+/Apalache 等）。审计方如要求
无界证明，需要在 §7 R4 中另立任务。

## 6. 证据追踪矩阵

运行 `scripts/w14-coordinator-audit-evidence.py` 会重跑下列分组、记录通过数与日志，并绑定 Git commit。
运行本地产出见 `target/w14-coordinator-audit/evidence.json`。

2026-11-20 在本 worktree 实测（debug 构建，`--skip-slow --depth 0`，8/8 分组通过，122 个测试通过、3 个 ignored）：

| 分组 | 通过 | 耗时 |
|---|---|---|
| domain-invariants | 33 | 50.1 s |
| candidate-store | 40 | 12.7 s |
| interleaving-sweep（depth 0 冒烟） | 1 | 5.8 s |
| client-takeover | 14 | 39.6 s |
| client-runtime | 3 | 8.8 s |
| fault-process | 28（2 ignored，需特权/环境） | 32.7 s |
| admin-recovery | 2（1 ignored，netns 需 root） | 6.9 s |
| admin-recovery-websocket | 1 | 5.4 s |

交错扫描的分层耗时（同一 worktree）：默认冒烟（5 条三步危险交错）5.3 s；`DEPTH=1` 22.2 s；
`DEPTH=2` 111.4 s（debug）；`DEPTH=3` 全三元组 **release 构建 8.3 s**，非 release 约 15 分钟以上。
因此 `DEPTH=3` 应始终配 `--release` 使用。

| ID | 目的 | 命令/测试 |
|---|---|---|
| E-D1 | 配置签名绑定权限与成员 | `candidate_config_signatures_are_bound_to_permissions_and_membership` |
| E-D2 | 声明自签名且绑定 revision | `candidate_declarations_are_self_signed_and_revision_bound` |
| E-D3 | 不健康/不愿参选者不获得资格 | `unwilling_or_missing_candidates_never_gain_election_eligibility` |
| E-D4 | 配置并发 CAS 单一胜者 | `concurrent_revision_updates_have_exactly_one_winner` |
| E-D5 | 配置 CAS 幂等与重启持久 | `config_cas_idempotency_and_restart_are_durable` |
| E-D6 | 旧版存储升级 | `legacy_v2_candidate_store_loads_and_upgrades_on_next_write` |
| E-S1 | 任期跨重启、旧写拒绝且审计 | `terms_survive_restart_and_old_writes_are_rejected_and_audited` |
| E-S2 | 租约续期与到期只读 | `lease_renewal_extends_authority_and_expiry_enters_read_only` |
| E-S3 | 严格多数才提交（持久化） | `persisted_election_commits_only_after_strict_majority` |
| E-S4 | 同任期并发安装单一胜者 | `concurrent_split_brain_successors_install_exactly_one_authority` |
| E-S5 | 旧任期/旧 token 恢复后拒绝 | `stale_terms_and_fencing_tokens_are_rejected_after_recovery`、`stale_and_expired_write_credentials_fail_closed` |
| E-S6 | 时钟回拨边界与租约投影 | `bounded_clock_rollback_marks_authority_read_only`、`lease_projection_distinguishes_renewal_expiry_and_clock_skew` |
| E-X1 | 三候选严格多数 2/3 | `three_candidate_election_commits_only_after_strict_majority` |
| E-X2 | 穷举交错扫描 | `bounded_exhaustive_interleavings_hold_authority_invariants`（`FUCKIPV6_W14_INTERLEAVING_DEPTH=3`） |
| E-X3 | 选举提交使旧 recovery/emergency 投影失效 | `election_commit_invalidates_recovery_projections_from_the_previous_term` |
| E-E1 | 紧急签发、续租与硬期限 | `emergency_lease_is_bounded_and_restart_never_extends_it`、`emergency_lease_expiry_cannot_be_renewed_or_revived_by_restart` |
| E-E2 | 紧急权限不得治理 | `successor_term_install_atomically_revokes_emergency_authority`、`failed_successor_install_preserves_emergency_lease_and_freeze` |
| E-G1 | 治理门禁与配置更新互斥 | `governance_gate_fails_closed_until_a_current_candidate_is_healthy`、`governance_gate_serializes_governance_writes_with_candidate_updates` |
| E-K1 | 客户端接管决策 | `cargo test -p fuckipv6-client node::coordinator_takeover --lib` |
| E-K2 | 运行时轮询/重连/退避 | `cargo test -p fuckipv6-client node::runtime_control::tests::coordinator_takeover --lib` |
| E-K3 | 后台候选故障自动接管 E2E | `coordinator_takeover_background_candidate_failure_e2e` |
| E-C1 | 独立进程故障矩阵（SIGKILL/SIGSTOP/并发安装） | `cargo test -p fuckipv6-coordinator --test w14_coordinator_fault_process` |
| E-C2 | QEMU 断电后持久状态 | `scripts/test-w14-coordinator-powerloss-vm.sh` |
| E-F1 | 逻辑故障注入与状态机轨迹 | `w14_fault_injection_matrix_records_state_machine_trajectory` |
| E-F2 | 真实 netns/nftables 分区 | `scripts/test-w14-coordinator-fault-netns.sh` |
| E-A1 | 管理员阈值恢复（真实多节点） | `w14_admin_recovery_multinode_offline_trustee_drill`、`w14_admin_recovery_netns_drill` |
| E-A2 | 恢复错误键稳定 | `admin_recovery_domain_rejections_keep_stable_public_keys` |

## 7. 残余风险与审计关注点

| ID | 风险 | 现状 | 请审计方确认 |
|---|---|---|---|
| R1 | Coordinator 主机被完全攻陷 | 不在模型内；被控主机可拒绝服务或伪造自身状态 | 明确“诚实验证者 + 崩溃/网络故障”模型边界是否可接受 |
| R2 | 活性 | 无多数、单点 Coordinator 时不保证新任期 | 是否需要在文档中进一步强化“只保证安全性”的表述 |
| R3 | 文件系统假设 | 依赖 `flock`、`fsync`、原子 rename、目录同步；不支持网络文件系统 | 是否需要运行时探测并在不支持的挂载点上拒绝启动 |
| R4 | 无界证明缺失 | 只有有界穷举（深度 3）与真实进程故障测试 | 是否需要 TLA+/Apalache 模型检查，作为后续独立交付 |
| R5 | 时钟 | 依赖墙上时钟与 30 秒漂移边界 | 边界值与部署要求是否需要收紧 |
| R6 | 审计链 | 追加式、单文件；无外部不可篡改锚定 | 是否需要外部锚定（如定期导出摘要） |
| R7 | 升级路径 | 旧版存储按需升级（E-D6） | 升级期间的 partial write 是否需要额外门禁 |
| R8 | 密钥与信任根 | 网络身份与管理员签名由现有治理链维护 | 是否需要审计治理链本身（W10 范围） |

## 8. 复现步骤

```bash
# 1) 协议不变量穷举（含三步危险交错；深度 3 为全三元组）
FUCKIPV6_W14_INTERLEAVING_DEPTH=3 \
  cargo test -p fuckipv6-coordinator --lib coordinator_candidates::tests::bounded_exhaustive

# 2) 领域规则与存储事务
cargo test -p fuckipv6-common domain::coordinator --lib
cargo test -p fuckipv6-coordinator signaling::coordinator_candidates --lib

# 3) 客户端决策与运行时
cargo test -p fuckipv6-client node::coordinator_takeover --lib
cargo test -p fuckipv6-client node::runtime_control::tests::coordinator_takeover --lib

# 4) 真实进程与真实网络
cargo test -p fuckipv6-coordinator --test w14_coordinator_fault_process
sudo ./scripts/test-w14-coordinator-fault-netns.sh          # 需要 root + nftables
sudo ./scripts/test-w14-coordinator-powerloss-vm.sh         # 需要 root + QEMU/KVM

# 5) 一次性收集全部证据（写 target/w14-coordinator-audit/）
python3 scripts/w14-coordinator-audit-evidence.py --depth 3
```

## 9. 审计方检查清单

- [ ] 独立复现 §8 全部命令，并核对通过数不低于 §6 记录的基线；
- [ ] 抽查 `coordinator_candidates.rs` 的每个 `apply_*` 函数：校验顺序、错误路径是否落审计、是否可能先落盘后校验；
- [ ] 确认 `lock_profile`/`lock_candidate_file`/`lock_governance_gate` 的获取顺序不存在环（含嵌套 store 的 `GovernancePermit`）；
- [ ] 确认 `persist_bytes_atomic` 的 rename/fsync 顺序在目标文件系统上满足 I5；
- [ ] 尝试构造 Q1–Q6 的否证：跨网络重放、revision 回退、过期投票、租约边界、并发安装、紧急续租；
- [ ] 对 §5 的扫描补充或反驳：指出未覆盖的动作/参数组合（例如多于三候选、多网络并存）；
- [ ] 评审 §7 残余风险，逐条给出“接受/需缓解/阻断发布”的判断。

## 10. 结论使用约定

- 本包不构成审计结论；引用时必须同时说明“内部自审 + 有界穷举 + 真实进程/断电演练”，不得表述为“已通过安全审计”。
- 第三方报告出具后，应把报告链接、覆盖范围、发现编号与处置状态补入本文 §11，并在
  `docs/PRODUCT_UI_REDESIGN_TODO.zh-CN.md` 勾选 W14-03 的审计项。
- 若报告发现缺陷，先在 §7 增加风险行，再修复并重跑 §8，禁止用文档措辞替代修复。

## 11. 外部审计记录

| 日期 | 审计方 | 范围 | 结论 | 发现 | 处置 | 报告链接 |
|---|---|---|---|---|---|---|
| — | 待定 | — | — | — | — | — |
