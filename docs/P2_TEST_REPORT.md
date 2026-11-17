# P2 嵌入式节点运行时 — 测试报告

- 日期：2026-11-17
- 平台：macOS 27.3（Darwin arm64，Apple Silicon 10 核），rustc / cargo 1.99.1（Homebrew）
- 分支：main（未推送），HEAD 89d8200 + P2 工作区改动
- 权限：全程未使用 root、未创建真实 TUN、未修改系统路由 / DNS、未启动子进程

## 结论

P2 验收目标达成：在无 GUI、无真实 TUN、无子进程的 macOS 环境下，经 `EmbeddedNodeRuntime` 在当前进程启动真实 Rust 节点运行时，通过 `ExternalPacketDevice` 收发外部 IP 报文，并用既有 `NodeCommand` / `NodeSnapshot` 控制与观测。三个门禁全部通过，13 个嵌入式用例单线程 13/13 通过，并发全量运行中嵌入式用例 0 失败。

并发全量 `fuckipv6-client` 库测试仍有 42 项失败，均为既有 macOS 宿主机限制（IPC 路径长度、Exit Node 仅 Linux、`sendmmsg` 仅 Linux、SO_REUSEADDR 端口可复用语义），已在 HEAD 89d8200 的独立 worktree 上逐项复现，与 P2 改动无关。

## 门禁与实际结果

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过，无 diff |
| `cargo check --workspace --no-default-features` | 通过（exit 0），27 条既有 dead_code / unused 警告 |
| `cargo test --workspace --no-default-features --no-run` | 通过，生成 24 个测试可执行文件 |
| `cargo test -p fuckipv6-client --no-default-features --lib embedded::tests -- --test-threads=1` | 13 passed / 0 failed（5.91s） |
| `cargo test -p fuckipv6-client --no-default-features --lib -- --test-threads=8 --skip punch` | 1197 passed / 42 failed / 1 ignored（16.53s），嵌入式用例 0 失败 |
| `cargo test -p fuckipv6-secure-session --no-default-features` | 144 passed / 0 failed |
| `cargo test -p fuckipv6-coordinator --no-default-features` | 215 passed / 0 failed |
| `cargo test -p fuckipv6-common --no-default-features relay_client` | 1 passed / 0 failed |

## 嵌入式用例清单（crates/client/src/embedded/tests.rs）

| 用例 | 覆盖 §六 | 断言要点 |
| --- | --- | --- |
| `embedded_start_snapshot_stop_and_duplicate_host` | A / C | 真实数据面启动、权威快照、停止后无残留；同一存储根目录第二个宿主被 Conflict 拒绝 |
| `embedded_commands_preserve_authorization_operations_and_conflicts` | B | 既有命令入口、授权拒绝、稳定错误码、operation 生命周期、非法状态迁移与重复冲突操作被拒 |
| `embedded_conflicting_lifecycle_commands_are_rejected_during_real_startup` | B / C | 启动进行中重复 StartNode / StopNode 被按既有状态机拒绝，不产生第二套资源 |
| `embedded_corrupt_operation_store_fails_before_network_start_and_releases_actors` | B / C | 损坏的操作存储在启动网络前失败并释放已建 actor，错误语义稳定 |
| `embedded_cancel_start_cleans_partial_platform_and_transports` | C | 启动中取消：已初始化平台资源被关闭，端点数归零，无残留任务 |
| `embedded_platform_failure_cleans_initialized_network_core` | C | 平台 open 失败后已初始化的网络核心被回收，不留半初始化状态 |
| `embedded_platform_errors_preserve_stable_problem_fields` | A / B | 平台拒绝映射到既有 `NodeProblem` 字段（code = PermissionDenied、message_key、network_name、retryable），不新增错误类型，失败后平台 socket 已释放 |
| `embedded_failed_cleanup_retains_ownership_and_stop_can_retry` | C | 清理失败保留资源所有权与存储锁并进入 Failed，可再次 stop 重试 |
| `embedded_platform_shutdown_failure_can_retry_after_core_actors_exit` | C | 平台 shutdown 失败时核心 actor 仍被 join，重试成功后释放锁 |
| `embedded_owned_services_join_connections_and_release_listeners` | C | 内嵌 Coordinator / Relay client 的连接与监听器在停止时被 join 并释放 |
| `embedded_packet_task_failure_is_visible_and_can_restart` | C / D | 数据面任务失败反映为可见网络问题，恢复命令可重新建立数据面 |
| `embedded_real_signaling_secure_p2p_and_external_ip_packets` | D | 真实 Signaling / P2P / QUIC / Secure Session 路径上完成 IPv6 双向报文；mock 仅限系统包 I/O、系统网络配置、存储与对端 |
| `embedded_restart_after_traffic_reuses_process_crypto_pool` | C / D | 承载真实流量（触发加密批处理线程池）后 `RestartNetworks` 成功、网络回到 Online、设备被重新打开、报文继续流动 |

测试替身边界：`TestPlatform` 只用回环 UDP / TCP 与内存 `ExternalPacketDevice`，存储在临时目录；不触碰系统虚拟接口、路由和 DNS，因此无需系统网络权限。节点运行时、签名与密钥协商、会话加解密、审计与授权都是生产代码路径。

## 本轮发现并修复的问题

`crates/secure-session/src/crypto_batch.rs` 的 `configure_crypto_workers` 原本严格首次调用生效；一旦进程内加密批处理线程池被真实流量初始化（`crates/secure-session/src/crypto_batch.rs:207`：`packet_count >= 4 && plaintext_bytes >= 4096`），任何同进程内的网络重启都会失败：

```
Rejected(NodeProblem { code: Unavailable, category: Dependency,
  message: "重启网络: 已重启 0 / 1 个网络；1 个网络重启失败 (mesh [start]: 网络 [mesh] 初始化失败: 无法配置数据面加密 worker: crypto worker pool was initialized before configuration)",
  message_key: "error.operation_unavailable", retryable: true })
```

这直接违反 §三(7) 重复 start / stop 的要求（对长驻 iOS 宿主是常态）。修复方式：池已初始化时，若 `CONFIGURED_WORKERS == requested_workers` 则返回 Ok（幂等空操作），不同值仍返回原错误。理由：worker 数量属于进程级 sizing，不涉及协议或密钥状态，重复同一请求不应打断已有数据面；配置不同的值仍显式失败。修复后该用例从失败转为通过，并发全量运行中另外 11 个嵌入式用例的连带失败（同一进程全局池被其他模块先行初始化）也一并消失。

## 未运行项及原因

| 项 | 原因 |
| --- | --- |
| `p2p::punching::tests`（并行全量中以 `--skip punch` 排除） | 需要真实 NAT 环境的打洞探测；并行执行时在本机挂起（`nat_probe_and_engine_reuse_the_same_transport_port`），与 P2 数据面路径无关，本轮未单独运行 |
| `tests/*_linux.rs`、netns、TUN / 路由 / DNS / Exit Node 系统测试 | Linux 专属且需要 root 与网络命名空间；宿主机为 macOS |
| 真实公网 P2P / 公网 Relay / 跨 NAT QUIC | 需要外部网络与对端，本轮只用本机回环 Signaling / Relay |
| iOS / Android 整仓交叉编译 | P2 不含移动目标与 Apple 适配层（NetworkExtension / App Group / Keychain） |
| `GetDiagnostics` / `ApplyTrafficPolicy` / `RollbackTrafficPolicy` | 移动侧尚无对应适配器，嵌入式模式按原语义返回 Unsupported，未伪造成功 |

## 已知问题（既有，非本阶段引入）

并发全量 `fuckipv6-client` 的 42 项失败，逐项与 HEAD 89d8200 干净 worktree 的结果一致：

| 模块 | 失败数 | 原因 |
| --- | --- | --- |
| `node::runtime_control::tests` | 17 | macOS `sun_path` 104 字节上限：`local socket name length exceeds capacity of sun_path of sockaddr_un`；Exit Node 仅 Linux |
| `local_service::tests` | 17 | 同上（本地服务 IPC 路径与 Linux 专属行为） |
| `local_ipc::tests` | 3 | 同步调用路径依赖的 IPC 行为在 macOS 不可用 |
| `cli::commands::tests` | 3 | `sendmmsg` 仅 Linux（`udp_batch_*`，断言 1 vs 8）；Exit Node 仅 Linux（`runtime_options_update_and_reset_exit_node`，`crates/common/src/routing.rs:230`） |
| `symmetric::actor::tests` | 2 | macOS `SO_REUSEADDR` 允许失败后重新绑定同一端口，测试断言“Signaling 端口仍被占用”不成立（`crates/client/src/symmetric/actor.rs:1228`、`:1430`） |

其他：`fuckipv6-client` 库仍有 27 条既有 dead_code / unused 警告，本轮未清理（改动范围外的模块）。

## 复现

```bash
cargo fmt --all -- --check
cargo check --workspace --no-default-features
cargo test --workspace --no-default-features --no-run
cargo test -p fuckipv6-client --no-default-features --lib embedded::tests -- --test-threads=1
cargo test -p fuckipv6-client --no-default-features --lib -- --test-threads=8 --skip punch
```
