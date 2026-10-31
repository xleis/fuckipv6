# 数据面性能整改 TODO

> 来源：`docs/WIREGUARD_GO_DATAPLANE_COMPARISON.md`
> 对照：`wireguard-go` commit `b3bc704ef0ee21a36298601a9e1e5ff90377ac52`
> 建立日期：2026-10-31
> 原则：先消除确定性的正确性/调度缺陷，再优化复制、锁和 syscall；每项都必须有回归测试或可复核计数。

## 本轮八项执行清单（2026-10-31）

> 这 8 项是当前性能收口的执行顺序；总表按 PERF-1..PERF-8 记录。状态只有同时具备源码、测试和实测证据时才改为完成。

- [ ] **1. PERF-1 UDP/TUN 入站零复制。** UDP RX owned pooled buffer、单次 AEAD open 和 signaling owned ciphertext 已实现并有单测；仍缺绑定最终提交的 allocation 与端到端吞吐证据。
- [ ] **2. PERF-2 signaling fallback 复制与边界。** relay 与 signaling 复用同一逻辑密文，`SignalingEvent::RelayPacket` 到 CLI 的 `Bytes` ownership 已接通；仍缺 allocation/CPU 对吞吐和尾延迟的实测。
- [ ] **3. PERF-3 有界入站队列。** 1024 默认容量、8193 包突发 drop-newest、4097+ 积压和模型化 remove/replay/rekey 组合压力测试已完成；仍缺真实 `P2pEngine` + 密码认证组合压力测试。
- [ ] **4. PERF-4 TUN/UDP syscall 批处理。** UDP `recvmmsg`/`sendmmsg` 已在生产数据面接线，并由内外计数证明批处理；TUN read 仍是 readiness-edge 后逐包 `read(2)`，write 仍逐包，因此本大项不完整。
- [ ] **5. PERF-5 AEAD batch/worker。** 串行预留 nonce、原地批量 seal/open 和 4-worker 微基准已具备；生产批量 worker、过载策略和端到端收益仍未验收。
- [ ] **6. PERF-6 会话热路径锁。** 已加入 `try_lock` 竞争/等待/持锁指标和同 peer 批量临界区；`SecurePeerState` 仍以单一 `Mutex<SecureState>` 保护 send/open 状态，锁拆分和 profile 尚未完成。
- [ ] **7. PERF-7 buffer pool 与 GSO/GRO。** UDP receive pool 已分片并降低稳态分配；全链路统一池、Linux UDP GSO/GRO、TUN offload 和非 Linux 回退仍未实现。
- [ ] **8. PERF-8 可证明路径基准。** direct UDP/fallback=0 断言、原始 JSON、metadata、syscall 证据和三实现 smoke 已完成；正式多 run、perf 归因和可绑定最终二进制的排名仍未完成。

### 进度快照（2026-10-31）

- **大项：0/8 完整验收。** 大项要求同时具备独立提交、回归/安全测试、实测证据和同口径性能结论，当前没有一项全部满足。
- **PERF 明细：27/49 已勾选，22 项未完成。** 这是本文件 PERF-1..PERF-8 的验收条目计数，不代表代码质量或性能完成百分比。
- **最近可关闭：** PERF-4 的 UDP batch 与 64B/1420B syscall 证据、PERF-8 的 direct-path 前置断言已经完成；TUN batch 和生产 worker 不属于近期可关闭项。
- **尚未证明：** 锁拆分、GSO/GRO、全链路池、真实密码认证组合压力测试、正式多 run 三实现排名，以及任何“已追平/超过 WireGuard”的结论。

## 0. 工作约束

- [ ] 所有性能结论必须记录实际 transport、endpoint、包长和吞吐，不能把 relay/fallback 混入直连结论。
- [ ] 不牺牲 replay protection、peer 身份绑定、内层源地址校验和 ACL。
- [ ] 每个改动独立提交；不把当前工作树中的 NAT/cookie 安全改动混入数据面提交。
- [ ] 优先在 Linux 上验证，同时保持 Windows GNU 编译兼容。
- [ ] 不删除 release 产物，不在未确认时改写或清理现有工作树。

## 1. 优先级与依赖

```text
P0-1 入站 ownership / 恢复丢包正确性
  -> P0-2 TUN buffer 不再逐包清零
  -> P0-3 fallback 只 seal 一次
  -> P0-4 慢 fallback 移出中央 select 分支

P1-1 receiver index / 去 PeerInfo clone
  -> P1-1B session-scoped index（可选，追平 wireguard-go 粒度）
  -> P1-2 热路径去写锁、统计批量化
  -> P1-3 真 batch + buffer pool + per-peer queue

P2-1 crypto worker pool
P2-2 TUN/UDP GSO/GRO
P2-3 干净基准与 profiler 归因
```

## P0-1 入站 ownership：直连只解密一次，删除 4096/replay 悬崖

**现状**

- UDP receiver 在 `crates/client/src/p2p/connection.rs::dispatch_secure_data` 内先认证/解密。
- 明文被插入容量 `4096` 的 `AuthenticatedInboundCache`，队列却仍发送密文。
- 主循环 `crates/client/src/cli/commands.rs::handle_inbound_payload` 再调用 `P2pEngine::decrypt_from_peer`，靠密文哈希取缓存。
- cache 淘汰后再次 AEAD open 会撞上已经推进的 replay window，造成高负载有效包丢失。
- 正常包至少有密文复制、明文物化、入队密文复制；同一密文计算两次 BLAKE3。

**整改**

- [x] 删除 `AuthenticatedInboundCache`、容量常量、两次 BLAKE3 和所有 cache 清理逻辑。
- [x] 引入类型化入站 packet：`AuthenticatedPlaintext` 与 `SealedCiphertext` 不再共用模糊的 `(IpAddr, Vec<u8>)`。
- [x] UDP 直连认证成功后，将已解密的 plaintext ownership 直接交给主循环，不再入队密文。
- [x] 主循环对 `AuthenticatedPlaintext` 只做 ACL、peer/source 校验和 TUN 写入，不再二次解密。
- [x] QUIC relay/signaling 的 `SealedCiphertext` 仍调用 `decrypt_from_peer`，保持 relay 安全边界。
- [x] endpoint migration 只能在认证成功后发生，并保持对旧 session/revoked peer 的拒绝语义。

**验收**

- [x] 直连有效包在进入主循环前只执行一次 AEAD open。
- [x] 入队数据是 plaintext，密文不再进入 P2P receive queue。
- [x] 连续/积压超过旧的 4096 条记录后，不出现由 cache 淘汰导致的 `ReplayDetected`/`CounterTooOld` 丢包。
- [x] 重放、篡改、未认证 plaintext、旧 session 延迟包仍被拒绝。
- [x] peer 移除/重绑定后没有明文残留。
- [x] `cargo test -p fuckipv6-client p2p::connection` 通过（若测试过滤器不匹配则运行该 crate 全量测试）。

**回归测试**

- [x] 将现有 data-transfer、endpoint-migration、replay、peer-rebind 测试改为断言 queue 中为已认证明文。
- [x] relay 密文仍通过 `decrypt_from_peer` 解密；同一 relay 密文第二次提交必须失败。
- [x] 增加至少 4097 个已认证入站包的积压测试，全部有效包按序可消费且无二次 open。

## P0-2 TUN buffer 不再对 64B 包清零 64 KiB

**现状**

- `commands.rs::recycle_tun_buffer` 对回收 buffer 执行 `clear()`。
- `commands.rs::spawn_tun_reader` 下一轮执行 `resize(65536, 0)`，再 `truncate(size)`。
- 64B 包因此产生约 1024 倍无效零填充写放大。

**整改**

- [x] pooled buffer 始终保持 65536 字节长度，不再 `clear`/`resize`。
- [x] TUN reader 向主循环传递 `{ buffer: Vec<u8>, len: usize }` 或等价类型。
- [x] 出站处理只使用 `buffer[..len]`；回收时只重置 `len`，不触碰 payload 容量内容。
- [x] 保持 `TUN_BUFFER_POOL_SIZE` 有界，队列满时采取明确策略，不能重新引入无界 Vec 分配。

**验收**

- [x] 64B 包路径不再写满 64 KiB 零值。
- [x] 1420B/64B 包内容、长度、ACL、路由和发送行为不变。
- [x] buffer 回收次数稳定，池不会因错误路径泄漏。
- [x] 增加 64B/1420B 混合批次测试，验证每个包只暴露有效长度。

## P0-3 fallback 复用同一密文，禁止重复 seal

**现状**

- P2P `send_to` 已 seal 一次。
- UDP 失败后 QUIC relay 和 signaling fallback 会再次 seal 同一明文。
- 最坏情况消耗三个 nonce、执行三次 AEAD。

**整改**

- [x] 抽出“解析 peer + 生成一次密文”的出站发送接口。
- [x] UDP、QUIC relay、signaling fallback 复用同一个 sealed packet。
- [x] 保持 per-peer nonce 顺序、session revocation gate 和失败语义。
- [x] 明确只允许对同一 nonce/密文选择 transport，不能引入 AEAD 重放。

**验收**

- [x] 单次 fallback 链的 seal 计数最多为 1。
- [x] 不同业务包仍获得不同 nonce。
- [x] 三条 transport 的认证、重放窗口和 ACL 行为通过现有安全测试。
- [x] 增加 UDP 失败、QUIC 成功和全 fallback 的计数测试。

测试入口：

- `cli::commands::tests::fallback_matrix_reuses_one_ciphertext_for_relay_and_signaling`
- 矩阵覆盖 UDP 失败 + QUIC relay 成功，以及 UDP/QUIC 均失败 + signaling 成功。
- 两条路径都断言整条 fallback 链只 seal 一次、后一 transport 收到与前一 transport 完全相同的密文、对端只能认证一次且再次提交会被 replay window 拒绝。

## P0-4 慢 fallback 移出中央数据面 select 分支

**现状**

- 主事件循环直接 `await handle_tun_reader_packet()`。
- QUIC relay 发送带 2 秒 timeout，单个慢 fallback 会阻塞 TUN/UDP/control 事件。

**整改**

- [x] 中央循环只做快速校验、路由和同步入队；seal、直连、慢 relay/fallback 交给独立 owner task/channel。
- [x] per-peer 保持发送顺序，跨 peer 可以并行。
- [x] relay 断开、超时和自愈不能阻塞 TUN 读取与 UDP 接收。
- [x] relay 重连从中央 heartbeat 分支移到独立 task，并用 generation 防止旧连接清理新连接。

**验收**

- [x] relay 卡住 2 秒时，TUN 入站和已建立 UDP P2P 仍持续推进。
- [x] 控制命令和 shutdown 不被发送超时拖住。
- [x] 增加慢 transport 注入测试和中央循环心跳/次数断言。

实现位置与约束：

- `commands.rs::OutboundDispatcher::submit()` 只执行有界 `try_send`，不执行 crypto、网络 I/O 或 `await`。
- `commands.rs::run_outbound_worker()` 为每个目标 peer 串行消费一个队列；不同 peer 的 worker 并行。
- `OUTBOUND_QUEUE_CAPACITY = 256`。队列满时明确丢包并记录 `p2p_send_error`，不会无界堆积。
- 每个目标 peer 保留一个 worker task；当前没有空闲 worker 回收机制。peer 数量增长时需在 P1/P2 引入 worker 生命周期管理或统一调度。

测试入口：

- `cli::commands::tests::slow_peer_worker_does_not_block_another_peer_or_deliver_out_of_order`
- `cli::commands::tests::slow_worker_does_not_block_heartbeat_or_control_events`
- `cli::commands::tests::stalled_outbound_worker_does_not_block_tun_or_inbound_progress`
- `cli::commands::tests::outbound_queue_is_bounded_under_a_stalled_peer`

## P1-1 receiver index：pair-scoped 单候选已完成，性能待验证

**现状**

- 出站热路径已改为轻量 IP 路由索引，不再每包 `get_peers()` clone 全部 `PeerInfo`。
- 入站使用 `PeerRouteInfo`/`Arc<ReceiverRoute>`，不再 clone 完整 `PeerInfo`。
- `ReceiverIndex` 使用 `(receiver_index, endpoint)` 主索引和 `receiver_index` 回退桶。
- profile 3 的 receiver index 已从“本端静态 Ed25519 公钥前 4 字节”改为“有序双方公钥 + 本端角色”的域分离 BLAKE3 派生。
- 同一节点对不同 peer 得到不同 index；同一 pair 的 initiator/responder index 也不同，双方无需新增协商字段即可得到一致映射。
- 同一 pair 的 index 在重握手/rekey 后保持不变；session 隔离仍由 SID、epoch、密钥和 replay window 完成。
- 已注册 endpoint 命中单候选；NAT rebind/未知 source 会按 pair-scoped index 查找，在当前 100-peer 场景下也是单候选。
- endpoint 和 index 都只是未认证路由提示，最终仍由 `open_from_source()` 完成 AEAD、session、epoch 和 replay 校验。

**已完成**

- [x] 建立轻量出站路由索引，不克隆完整控制面对象。
- [x] 入站使用 `PeerRouteInfo`，移除每包完整 `PeerInfo` clone。
- [x] 删除入站全连接线性扫描，增加 `(receiver_index, endpoint)` 索引。
- [x] 删除静态公钥前缀映射，改为 pair-scoped、角色分离的 BLAKE3 receiver index。
- [x] 100-peer 测试断言 100 个 `local_receiver_index()` 全部唯一。
- [x] 100-peer 测试实测：已注册 endpoint 100 次 lookup 均返回 1 个候选。
- [x] 未知 endpoint/NAT rebind 对目标 peer 只返回 1 个候选，不再返回旧实现的 100 个候选。
- [x] 固定 key 测试覆盖同一 pair 稳定、initiator/responder 分离、换 peer 后 index 变化。
- [x] 索引更新和连接移除保持原子性，旧 session/route 延迟包仍拒绝。
- [x] `CLASSIC_PROFILE_ID` 从 1 升至 3 并更新固定向量，拒绝新旧 receiver-index 映射静默混用。
- [x] 更新 `PROTOCOL_V2.md` 的 profile 3 与 index 语义。

**仍未完成**

- [ ] 32-bit 哈希碰撞仍会形成候选桶并逐个认证；增加碰撞注入和认证尝试计数测试。
- [ ] 在真实多 peer/NAT 环境中采集 lookup 候选数、AEAD open 尝试数和尾延迟。
- [ ] 用同一基线证明候选路径减少后是否带来吞吐/尾延迟收益；当前不能宣称性能提升。
- [ ] 多 peer 并发、rekey 和 endpoint migration 压力测试。

**当前状态：功能完成，性能待验证。静态共享桶的确定性 O(N) 已消除；无碰撞时为单候选，碰撞桶仍是安全回退。**

## P1-1B session-scoped receiver index（对照 wireguard-go 的剩余差异）

**目标**

当前 profile 3 是 pair-scoped deterministic index。若需要完全追平 `wireguard-go` 的隔离粒度，再引入每握手/每 rekey 可轮换的 session-scoped index；这不是修复静态共享桶的必要前置条件。

**设计前置**

- [ ] 定义 index 的 CSPRNG 生成与碰撞重试策略，或定义从已认证握手随机量派生的会话级映射。
- [ ] 定义双方如何认证绑定 local/peer index，避免未认证字段影响路由。
- [ ] 定义 rekey 窗口内旧/新 index 并存、切换和退役时机。
- [ ] 定义旧 session 延迟包、同 pair 多 active/retired epoch 和 endpoint migration 语义。
- [ ] 明确 wire profile 升级和混合版本失败关闭策略。

**实现与验收**

- [ ] 每个 handshake/rekey 的 local receiver index 独立分配。
- [ ] 未知 source/NAT rebind 仍只产生一个候选。
- [ ] 碰撞、握手重放、rekey 延迟包、旧 session 包和 peer 移除均有回归测试。
- [ ] 多 peer 并发发送、接收、rekey 和 endpoint 迁移压力测试通过。
- [ ] 用候选数、AEAD open 次数和 profile 证明查找路径改善。

**当前状态：待开始；属于可选的协议级隔离增强，不影响当前 pair-scoped 修复的结论。**

## P1-2 热路径锁与统计：payload 已改为无锁记账，控制面保留低频锁

**现状**

- `StatsCollector` 的全局计数和 per-peer 计数已改为独立原子变量。
- 已有 peer 的普通包不再获取 global stats 或 peer map 写锁；首次创建 peer 仍取一次 map 写锁。
- endpoint 未变化时只取 `connections.read()`，变化时才重新验证 session 并写回。
- `PeerHealth` 已从整个 connections 表拆成独立 `Arc<PeerHealth>`；发送路径一次取出 session/health 句柄，成功发送后直接记账，不再二次查询 connections 表。
- payload/activity 使用无锁原子状态：一个最后活动时间戳和 31 个 1 秒流量桶；每个桶用 generation 防止跨秒旧值污染。
- probe 样本、RTT、远端质量等低频控制状态仍由 `Mutex<ProbeHealth>` 保护；payload 路径不进入该 mutex，只有 snapshot 和探针事件会进入。
- 当前精确计数使用 32-bit 桶值，单 peer 单秒超过约 4 GiB 会饱和；它影响展示带宽精度，不影响数据面转发或安全状态。是否需要 64-bit 无锁桶由真实 10/40/100GbE profile 决定。
- 尚未提供 mutex/lock 等待数据，也没有证明原子化带来了吞吐提升。

**整改**

- [x] endpoint fast path 使用读锁/原子比较，仅变化时写。
- [x] 统计改为 per-peer atomic counter，普通包不获取全局/peer map 写锁。
- [x] 避免在持锁状态下 await 或执行加解密/网络发送。
- [x] 将 `PeerHealth` 锁从 connections 表拆出。
- [x] 删除 `record_peer_payload()` 的逐包 connections 查询和 health 写锁。
- [x] 发送路径复用已取得的 `(secure session, health)` 句柄，UDP/QUIC 成功路径不再二次查找。
- [x] 直连入站路由携带 `Arc<PeerHealth>`，认证成功后直接进行无锁 payload 记账。
- [x] 31 个秒级流量桶替代逐包 `Instant`/`VecDeque` 维护，保持 30 秒带宽窗口语义。
- [x] 覆盖并发 payload 不丢计数、probe mutex 持有期间 payload 不阻塞、并发 snapshot/probe/payload 一致性。
- [ ] 真实多 peer remove/rekey/endpoint migration 压力测试覆盖原子桶与路由句柄生命周期。
- [ ] 明确并验证锁顺序，覆盖并发 remove/rekey。
- [ ] 增加 mutex/profile 等待指标。

**验收**

- [x] 普通直连包不获取全局统计写锁。
- [x] endpoint 不变的包不获取 endpoint 写锁。
- [x] 普通 payload 不获取 per-peer health `RwLock`，也不等待 probe/RTT mutex。
- [x] 8 线程并发 80,000 次 payload 更新全部保留；probe mutex 被占用时 payload 仍可完成。
- [ ] 并发收发和 peer remove/rekey 压力测试证明无死锁、无旧 session 复活。
- [ ] 输出至少一组 mutex/lock 等待计数或 profile 证据。

**当前状态：payload 记账功能完成且已无锁；控制面低频状态仍有 mutex；复制、syscall、batch/worker 等成本仍在，性能待同口径基准确认。**

## P1-3 真 batch、buffer pool、per-peer 顺序队列

**现状**

- `seal_batch`/`open_batch_in_place` 内部仍是串行循环。
- 数据面逐包 syscall，未使用现有 batch API。
- `wireguard-go` 使用顺序 nonce 分配、CPU worker 并行 AEAD、per-peer 顺序发送。

**整改**

- [x] **P1-3a 消除不必要的整包复制/分配。**
  - [x] 认证成功后把 plaintext 原位移到 receive `Vec` 前端并截断，删除 `open_in_place(...).to_vec()` 的第二个明文分配。
  - [x] relay/fallback 的已拥有密文直接交给 in-place 认证，删除运行时路径上的 `payload.to_vec()` 兼容克隆。
  - [x] TUN reader 把池化 `TunPacket` 所有权直接交给 per-peer 发送队列，删除 `route_tun_packet()` 的 `packet.to_vec()`；发送完成、队列满或路由拒绝时归还原 buffer。
  - [x] TUN allocation 预留 `DATA_HEADER_LEN + AEAD_TAG_LEN`，生产路径通过 `send_to_or_fallback_in_place()` 在 pooled allocation 上原地 seal；direct/QUIC/relay/signaling 复用同一密文。
  - [ ] UDP receiver 仍因共享 `recv_from` buffer 执行一次 `bytes.to_vec()`；后续用可变 pooled receive ownership 消除。
  - [ ] signaling `relay_packet` 仍因 owned `Vec` API 执行一次 `encrypted.to_vec()`；后续改为 pooled/Bytes ownership 或可复用帧缓冲。该复制不影响“fallback 只 seal 一次”的结论。
  - [ ] 兼容 API `SecurePeerState::seal()`、`EpochSession::seal()` 和 P2P `send_to_or_fallback()` 仍会分配/复制；生产 TUN 数据面已不经过这些入口，后续确认调用方后再迁移或删除。
- [x] **P1-3b 已建立 TUN pool 的跨任务 ownership 和出站原地 seal。** 生产路径不再为每个 TUN 包额外分配 payload `Vec` 或第二份密文；64 个固定 buffer 形成天然背压，队列满时丢弃并回收。
- [ ] **P1-3c 完备 buffer pool。** UDP RX/TX、seal/open 输出和 fallback 仍需统一固定池，目标为稳态零逐包分配。
- [x] 定义每 peer 的顺序发送队列和有界缓冲策略（P0-4 已完成）。
- [ ] batch 读 TUN、按 peer 分组、批量 UDP send/recv。
- [ ] 保证 batch nonce 顺序分配与发送顺序一致。

**验收**

- [x] 定向测试证明 pooled TUN buffer 以原地址穿过 outbound packet 并归还池，pooled outbound payload 没有复制分配。
- [x] 定向测试证明 AEAD open 结果复用 receive allocation，没有第二份明文 `Vec`。
- [x] 定向测试证明 driver/epoch session 可在预留 header/tag 的 packet slot 中原地 seal，fallback 矩阵每条链只 seal 一次并复用同一密文。
- [ ] 峰值路径分配次数和 syscall 次数可量化下降（需要 allocation/syscall counter 或 profiler）。
- [ ] 64B/1420B 单流与多流吞吐有干净的前后对比。
- [ ] 丢包、乱序、重放和 peer 移除安全测试通过。

## P2-1 密码学 worker pool

- [ ] nonce 在串行分配阶段确定。
- [ ] AEAD 由 CPU 数 worker 并行执行。
- [ ] 每 peer 按 nonce 顺序提交发送。
- [ ] worker 队列有界，过载时丢包策略可观测且不破坏安全状态。
- [ ] 以 profile 证明 worker 并行确实改善端到端 pps，而不是只改善微基准。

## P2-2 TUN/UDP GSO/GRO

- [ ] 调研并实现 Linux TUN multi-queue/GSO/GRO 与 UDP GSO/GRO。
- [ ] 保持非 Linux fallback。
- [ ] 用同一基准记录 syscall、CPU/包和 pps。

## P2-3 干净基准与 profiler 归因

- [ ] 基准启动后断言 active endpoint 和 transport，排除意外 relay/fallback。
- [ ] 固定 commit、编译 profile、CPU governor、MTU、包长、流数和时长。
- [ ] 输出 fuckipv6、wireguard-go、内核 WireGuard 原始日志和汇总。
- [ ] 至少采集 CPU profile、syscall 计数、分配计数和锁等待。
- [ ] 每完成一个 P0 复跑同一基准，记录增益和回归。

## 当前执行状态

| 项目 | 状态 | 备注 |
|---|---|---|
| P0-1 入站 ownership | 功能完成，待独立提交/性能验证 | 4097 包积压与重放测试通过 |
| P0-2 TUN buffer | 功能完成，待独立提交/性能验证 | 64B/1420B 混合测试与 clippy 通过 |
| P0-3 fallback 复用密文 | 功能完成，待独立提交/性能验证 | UDP→QUIC 与 UDP→QUIC→signaling 矩阵通过；每条链 seal=1 |
| P0-4 慢 fallback 离环 | 功能完成，待独立提交/性能验证 | per-peer 有界队列/worker；TUN、UDP、heartbeat、control、shutdown 响应测试通过 |
| P1-1 receiver index 路由 | 功能完成，性能待验证 | pair-scoped index；100-peer 全唯一，已注册/未知 endpoint 均单候选；32-bit 碰撞桶仍在 |
| P1-1B session-scoped index | 待开始（可选） | 当前不是静态共享桶阻塞项；用于追平 wireguard-go 的每握手唯一/轮换粒度 |
| P1-2 统计/endpoint/health 锁隔离 | 功能完成，性能待验证 | payload 使用原子时间戳/31 个流量桶；probe/RTT 保留低频 mutex；无锁等待指标或吞吐基准 |
| P1-3a/b | 部分完成，性能待验证 | TUN 池化 ownership、出站原地 seal、UDP owned RX、signaling `Bytes` ownership、runtime relay 去 clone 已完成；全链路仍有 frame 编码和兼容分配 |
| P1-3c/P2 | 部分完成 | UDP `sendmmsg/recvmmsg` 已接线并实测；TUN 仍逐包 syscall，完整 buffer pool、生产 crypto worker、GSO/GRO 和正式干净基准未完成 |

## 下一执行批次：8 项性能收口

这一批按“先消除确定性和可观测成本，再引入并发/offload”的顺序执行。每项完成必须同时满足源码、回归测试、度量证据和文档四项，不能只凭功能测试勾选性能完成。

### PERF-1 UDP RX 使用 owned pooled buffer

- [x] 定义可回收 UDP receive buffer pool；每个 datagram 从 `recv_from` 起持有唯一 ownership，不再复制共享 `bytes`。
- [x] 让 `open_from_source()` 直接消费 owned buffer，并在认证成功后把 plaintext 移到前端复用同一 allocation。
- [x] 为 pool 满、buffer 不足、短读、NAT rebind、relay ciphertext 和 replay 注入回归测试。
- [x] 用 allocation counter 证明直连 UDP 热路径不再调用 `bytes.to_vec()`；记录 pool 命中率和耗尽次数。
- [x] **功能验证：** 64 包稳态测试要求 hit rate ≥ 95%、allocations ≤ 4、full returns = 0；精确运行值归档到 PERF-8 原始结果。
- [ ] **最终证据：** 在包含当前数据面整合的独立提交上重跑 allocation counter 与 direct-path 端到端测试，绑定最终二进制并证明吞吐/分配收益。

### PERF-2 Signaling fallback 复制与边界

- [x] relay 与 signaling 共用一个逻辑密文；不得重新 seal、重分配 nonce 或改变 nonce 顺序。
- [x] fallback 矩阵复用同一密文，并以 allocation identity 断言 relay 层没有额外 `Vec` 复制。
- [x] 1 MiB postcard/signaling frame 上限、64 KiB relay payload 上限，以及 trailing/padding 拒绝均有测试。
- [x] signaling 断开后自动重连并重新注册、保留虚拟地址的测试通过。
- [x] 对外 QUIC relay `send_packet(Bytes)` 零复制；`&[u8]`/`&Vec<u8>` 是显式兼容复制路径。
- [x] 记录剩余复制边界：客户端 `Vec -> Bytes` 可转移 ownership；postcard 编码把 payload 写入 frame buffer；协调端 decode/re-encode 各物化一份 payload；随后 `BytesMut` 可原地认证。
- [x] **CLI ownership（工作树）：** `SignalingEvent::RelayPacket.payload` 已改为 `Bytes`，`SignalingAction::RelayedPacket` 原样持有它并交给 `InboundPacket::SealedCiphertext`，不再在该 CLI 边界执行 `Bytes -> Vec`。
- [ ] **待实测：** 记录 peaked allocation/CPU 对吞吐和尾延迟的影响；当前只有所有权、边界和回归测试证据。

### PERF-3 入站队列有界背压

- [x] 用有界入站队列替代 `UnboundedSender`，容量必须可配置且有安全上限。
- [x] 定义队列满时的策略：直连可丢最新包并计数；安全控制帧不得被普通数据淹没。
- [x] 删除 `incoming_rx` 外层不必要的锁，保证单消费者语义。
- [x] 增加突发、慢消费者、重放、4097+ 积压、shutdown 和 peer remove 测试；暴露 queue depth、drop 和阻塞次数。
- [x] **默认容量突发：** 向容量 1024 连续提交 8193 个包，严格保留最早 1024 个、drop-newest 丢弃 7169 个；消费集合与入队集合一致，最终 depth 为 0。
- [x] **组合压力：** peer remove、重放、future generation、两代 rekey 窗口混合出 10240 个 admission 候选，并发生成恰好 1024 个入队和 9216 个队列满丢弃；入队/丢弃集合互斥、消费集合一致、计数无遗漏。
- [ ] **测试边界：** 上述组合压力使用测试内 admission 等价模型，尚未覆盖真实密码认证和 `P2pEngine` 控制器；完整功能提交后仍需保留该说明，不能把模型测试表述为端到端 P2P 压测。

验证命令（工作树）：

```text
cargo test -p fuckipv6-client p2p::inbound_queue::tests --lib
# 10 passed; 375 filtered out
cargo test -p fuckipv6-client p2p::inbound_queue::tests --lib --no-default-features
# passed
```

### PERF-4 TUN/UDP syscall 批处理

- [ ] Linux TUN 路径实现 batch read/write（优先 multi-buffer 或等价接口），非 Linux 保留逐包 fallback。当前只完成 readiness-edge drain，未实现多包 syscall。
- [x] UDP 使用 Linux `recvmmsg`/`sendmmsg`，默认 batch 32、上限 128、非 Linux 逐包 fallback；生产 UDP receiver 与同 peer sender 已接线。
- [x] 批量发送在 syscall 前完成整个 prefix 的 seal，保持每 peer nonce 顺序；短写只把未发送 suffix 交 fallback，不重新 seal。对应测试为 `short_sendmmsg_falls_back_only_unsent_suffix_without_resealing`。
- [x] 用内建 counter 与外部 `strace -c` 证明成功包/批和真实 syscall 数，归档 1420B/1372B-payload 与 64B 两组。

**当前边界（2026-10-31）：**

- Linux TUN 已有一条 readiness edge 后连续 drain 的 `read_batch`，但它仍是一包一次 `read(2)`；收益是减少唤醒和 task handoff，不能写成 syscall 批处理。
- TUN 字符设备的一次 `write(2)` 对应一个 L3 packet；直接拼接多个 packet 的 `writev(2)` 会改变包边界，不能作为当前模型的 batch write。真正的多包写入需要 `io_uring` 多请求、multi-queue 或内核 ABI 层面验证。
- UDP `recvmmsg`/`sendmmsg` 已在工作树的生产 UDP receiver/sender 上线；`d38986d` 提交的是底层原语，当前连接层整合尚未独立提交，因此只能记为“工作树已接线并实测，未进入完整验收”。
- 外部证据位于 `target/benchmarks/syscall-evidence-20261031/udp1420-clean` 与 `udp64-clean`。内建 counter 的成功包/次约 31.65 TX、20.42 RX（1420B），以及 31.83 TX、17.12 RX（64B）；`strace -c` 的真实调用还包含 readiness 探测/失败尝试，不能直接用成功 batch 数替代。
- 同组 strace 中 TUN 约每包一次 `read(2)`/`write(2)`，证明不能把当前 read drain 宣传成 TUN 批处理。PERF-4 因 TUN 多包 syscall 未实现而保持不完整。

### PERF-5 密码批处理接线与 worker 化

- [x] **底层同 peer 原地批处理：** `0e316e1` 提供 `seal_batch_in_place`、独立 buffer 的逐包 open 结果和 `Bytes` 原地 open；完整 nonce 在 AEAD 前预留。
- [ ] 在 TUN/UDP batch 之上按 peer 构造连续 packet batch，明确 header/tag/stride 布局。
- [ ] 串行预留 counter/nonce 后并行执行 AEAD，保持每 peer 发送顺序和失败关闭语义。
- [ ] worker 数按可用 CPU 配置，队列有界，过载策略可观测。
- [ ] 增加 batch 重排、部分失败、counter 耗尽、rekey 竞争、重放和乱序测试。
- [ ] 用同一基准比较 serial batch 与 worker batch 的 pps、CPU/包和尾延迟。
- [x] **微基准：** 同口径处理 64 包/批、固定 4 workers，覆盖 64B、1280B、1420B；基准启动断言实际进入 worker 且初始化失败数为 0，避免静默 fallback。
  - 环境：AMD Ryzen 7 5800H，16 逻辑 CPU，Linux，rustc 1.99.1。
  - 命令：`cargo bench -p fuckipv6-secure-session --bench dataplane -- 'secure_session_v2_batch_(64|1280|1420)_w4' --warm-up-time 1 --measurement-time 3 --sample-size 20`

| Payload | Serial seal | Worker seal | Serial open | Worker open |
|---|---:|---:|---:|---:|
| 64B | 93.854 µs | 40.004 µs | 98.498 µs | 45.243 µs |
| 1280B | 157.75 µs | 59.841 µs | 146.81 µs | 61.015 µs |
| 1420B | 161.52 µs | 64.680 µs | 163.66 µs | 67.047 µs |

  - 1420B seal：约 536.6 → 1308.6 MiB/s；open：约 529.6 → 1262.4 MiB/s。
  - **边界：** 这是 4-worker AEAD 微基准，不是端到端 P2P 性能，不能据此宣称吞吐超过 WireGuard；端到端收益仍需 PERF-8 在 direct UDP、fallback=0 的固定环境验证。

### PERF-6 移除会话热路径 async mutex

- [x] **锁等待/持锁指标：** `0e316e1` 增加真实 `try_lock` 竞争计数、wait/hold 总时长与最大值，并覆盖并发 seal 的线性化测试。
- [x] **同 peer 批量临界区：** `seal_batch_in_place` 一次持锁预留完整 nonce 范围并完成批量 AEAD；这是缩小锁获取次数，不是移除热路径锁。
- [ ] 拆分 `SecurePeerState` 的 admission、session/epoch、driver 和统计状态，缩小临界区。
- [ ] 采用每 peer actor、同步短锁或所有权转移，禁止在持锁期间执行网络 I/O 或 await。
- [ ] 保持 revoke/rekey/transport handoff 的线性化和旧 session 拒绝语义。
- [ ] 增加并发 seal/open、rekey、revoke、endpoint migration 和 peer remove 压力测试；采集 lock wait/hold 分布。

### PERF-7 完整 buffer pool、CPU worker 与 GSO/GRO

- [x] UDP receive buffer pool 已按 shard 分配/回收，生产 smoke 的 buffer-pool `full_returns=0`；这不是全链路零分配证明。
- [ ] 统一 TUN RX/TX、UDP RX/TX、seal/open 和 fallback frame 的固定大小 buffer pool，稳态零逐包分配。
- [ ] 接入 per-CPU crypto workers，并让每 peer 的 nonce 排序与 worker 完成顺序解耦。
- [ ] Linux UDP GSO/GRO 和 TUN offload 按 capability 探测，失败时自动回退。
- [ ] 增加 pool 泄漏、double-return、OOM、worker backlog、GSO 边界和非 Linux fallback 测试。
- [ ] 用 profile 证明 worker/offload 改善端到端 pps，而不是只改善微基准。

### PERF-8 可证明路径的干净基准

- [x] 基准每轮采集 direct-path/endpoint 与 fallback counter；修正后的 smoke 18/18 样本有效且 `fallback_packets_delta=0`。这些 smoke 不构成正式排名。
- [ ] 固定 commit/工作树 diff、二进制 SHA256、编译 profile、CPU governor、MTU、包长、流数、warmup 和时长。
- [ ] 采集 iperf3 原始日志、CPU profile、syscall、allocation、队列深度、锁等待和丢包原因。
- [ ] 对比 fuckipv6、wireguard-go 和内核 WireGuard；异常路径样本不得进入排名。
- [ ] 输出机器可读汇总与 Markdown 报告；明确哪些收益有统计显著性，哪些只是功能完成。

## 性能与 syscall 证据（2026-10-31）

### 1. UDP 1420B 口径修正

此前 benchmark 将 TUN MTU=1420 与 iperf3 UDP payload=1420 混用。内层 IPv6+UDP 头占 48B，实际隧道包为 1468B，触发分片并让 iperf3 receiver `sum_received.bytes=0`；summarizer 又回退到 sender `sum`，掩盖了接收端零有效载荷。

现已修正：

- `udp_1420_*` 的 iperf3 payload 为 `MTU - 48 = 1372B`，隧道内层 L3 packet 为 1420B；
- UDP 吞吐只读取 `end.sum_received`，禁止回退到 sender `sum`；
- receiver bytes 为 0 而 sender 有包时标记 `valid=false`、`invalid_reason=zero_received_payload`，并让 benchmark 失败；
- 回归测试覆盖 zero-receive 异常；旧目录 `target/benchmarks/dataplane-smoke-peerfix-20261031/` 保留为反例。

### 2. 三实现 smoke（只作阶段证据）

目录：`target/benchmarks/dataplane-smoke-udpfix-20261031/`。参数为 1 run、5s、omit 0、`--no-perf`，18/18 样本有效，direct 校验全部通过。吞吐单位 Gbit/s：

| case | fuckipv6 | kernel WireGuard | wireguard-go |
|---|---:|---:|---:|
| TCP 1 flow | 0.630 | 1.373 | 3.055 |
| TCP 8 flow | 0.634 | 1.418 | 3.169 |
| UDP 1420 saturated | 0.696 | 1.798 | 0.813 |
| UDP 64 saturated | 0.041 | 0.082 | 0.035 |
| UDP 1420 controlled | 0.050 | 0.050 | 0.050 |
| UDP 64 controlled | 0.005 | 0.005 | 0.005 |

**证据边界：** metadata 记录提交 `d38986d`、工作树 dirty、二进制 SHA256 `720d02cc...786c1c`；该二进制时间早于当前 HEAD，且只有 1 run/5s，不能作为正式排名、显著性结论或最终提交验收。受控 UDP 的 50/5 Mbit/s 是发送上限，不测峰值能力。

### 2.1 当前工作树复核 smoke（2026-10-31）

目录：`target/benchmarks/dataplane-smoke-current-20261031/`。参数仍为 1 run、5s、omit 0、`--no-perf`、MTU 1420；18/18 样本有效，6 个 fuckipv6 case 的 `direct_path=true` 且 `fallback_packets_delta=0`。吞吐单位 Gbit/s：

| case | fuckipv6 | kernel WireGuard | wireguard-go |
|---|---:|---:|---:|
| TCP 1 flow | 0.666 | 1.507 | 3.242 |
| TCP 8 flow | 0.626 | 1.457 | 3.258 |
| UDP 1420 saturated | 0.711 | 1.972 | 0.911 |
| UDP 64 saturated | 0.044 | 0.097 | 0.044 |
| UDP 1420 controlled | 0.050 | 0.050 | 0.050 |
| UDP 64 controlled | 0.005 | 0.005 | 0.005 |

该二进制由当前 dirty 工作树重建，绑定提交 `13d83ee`、二进制 SHA256 `6574f1d3...cee60`、worktree snapshot SHA256 `ca06ad2a...54ff`，wireguard-go 固定为 `b3bc704`。

**新增结论：** TCP 单流吞吐是 kernel WireGuard 的约 `0.44x`、wireguard-go 的约 `0.21x`；8 流没有改善，分别约 `0.43x` 和 `0.19x`。UDP 1420 已达到 wireguard-go 的约 `0.78x`，64B 饱和 UDP 与 wireguard-go 基本持平，但仍分别只有 kernel WireGuard 的 `0.36x` 和 `0.46x`。已接线的 UDP syscall 批处理没有单独消除 TCP 路径的数量级差距，也没有证明 TUN/事件循环/锁竞争已经追平；单 run 无法量化批处理本身的收益。

为检查方向是否由单次噪声造成，同一二进制又执行了 3 run 重复 smoke，目录为 `target/benchmarks/dataplane-smoke-current-3run-20261031/`。18 个 case/实现组合共 54/54 样本有效，fuckipv6 的 18 次测量全部 direct-path 且 fallback delta 为 0；以下为中位数：

| case | fuckipv6 | kernel WireGuard | wireguard-go | 对 kernel | 对 wireguard-go |
|---|---:|---:|---:|---:|---:|
| TCP 1 flow | 0.628 | 1.607 | 3.181 | 0.391x | 0.197x |
| TCP 8 flow | 0.599 | 1.510 | 3.212 | 0.397x | 0.186x |
| UDP 1420 saturated | 0.662 | 1.964 | 0.897 | 0.337x | 0.738x |
| UDP 64 saturated | 0.044 | 0.094 | 0.043 | 0.465x | 1.015x |

3 run 仍未改变方向：TCP 从单流增加到 8 流没有提升，wireguard-go 约快 `5.1x/5.4x`；UDP 1420 约为 wireguard-go 的 `0.74x`，64B 饱和 UDP 基本持平。metadata 绑定 HEAD `d6fceb4`、dirty worktree snapshot `ca06ad2a...54ff`、二进制 SHA256 `6574f1d3...cee60` 和 wireguard-go `b3bc704`。

**证据边界：** 仍只有 5s stream、未绑核、`powersave` governor、无 CPU profile，且测试时宿主负载约 `2.9-6.2`，不满足正式 harness 门限。3 run 只能说明方向在当前机器上稳定复现，不能当作统计显著排名或正式验收。

### 3. `strace -c` 与内建 batch counter

正式采用的 clean 证据：

- `target/benchmarks/syscall-evidence-20261031/udp1420-clean/`
- `target/benchmarks/syscall-evidence-20261031/udp64-clean/`

两组的 direct-path 均为真且 `fallback_packets_delta=0`。实测 binary SHA256 为 `720d02cc1a286c625c0a3f4ce7a6b261f3dfcc1ffd1a57a8fb4e42b922786c1c`。metadata 中的 SHA256 `ec234b3a...71a30` 是只读 strace wrapper 的 hash，不是被测二进制 hash。

| 流量 | 方向 | 成功 datagrams / batch syscall | 峰值 batch | 外建 strace（含失败尝试） |
|---|---|---:|---:|---|
| 1420B | TX | 35,795 / 1,131 = 31.65 | 32 | `sendmmsg=1131` |
| 1420B | RX | 35,878 / 1,757 = 20.42 | 32 | `recvmmsg=2464, errors=696` |
| 64B | TX | 63,439 / 1,993 = 31.83 | 32 | `sendmmsg=1993` |
| 64B | RX | 63,523 / 3,710 = 17.12 | 32 | `recvmmsg=5718, errors=1996` |

strace 的手工调用数高于内建“成功 batch syscall”数，因为 `async_io` readiness 探测、非消费返回和失败尝试也会形成系统调用；两组数字统计口径不同，不能混算。

同一批 strace 还显示 TUN 约每包一次 syscall：

- 1420B：发送端 `read=36037`、接收端 `write=35901`；
- 64B：发送端 `read=63679`、接收端 `write=63534`。

因此当前准确结论是：**应用层 UDP TX/RX 已批量；TUN 仍是逐包 `read(2)`/`write(2)`。** TUN reader 的 readiness-edge 后 drain 降低了唤醒次数，但不是 syscall batch；Linux TUN 当前 ABI 也没有可直接照搬的 multi-message read。

### 4. 当前未闭环项

- TUN 多包写入/读取仍无实现；`writev` 拼接会改变 L3 packet 边界，不能直接宣称可行。
- `seal_batch_in_place` 仍是底层/工作树能力，生产 crypto worker、过载策略和端到端收益未验收。
- `SecurePeerState` 仍只有一把 `Mutex<SecureState>`，send/open 共用；已有等待/持锁指标不等于完成锁拆分。
- UDP GSO/GRO 和 TUN offload 尚未实现；buffer pool 只覆盖 UDP receive。
- 尚缺真实 `P2pEngine` + 密码认证组合压力测试、正式多 run/perf 归因，以及与最终独立提交绑定的前后对比。

## 本轮验证记录

日期：2026-10-31

### secure-runtime 独立提交 `0e316e1`

```text
cargo fmt --all -- --check
cargo clippy -p fuckipv6-client --all-targets -- -D warnings
cargo test -p fuckipv6-client p2p::secure_runtime --lib -- --nocapture
```

结果：29 个 secure-runtime 测试通过，Clippy 无项目警告。独立树上保留旧连接 API 到 P2P 接线提交，并仅对新 API 标注临时 dead-code 原因；连接层切换后应删除这些兼容标记。

### 数据面整合工作树

```text
cargo fmt --all -- --check
cargo clippy -p fuckipv6-client --all-targets -- -D warnings
cargo test --workspace
python3 scripts/test_bench_summarize.py -v
python3 scripts/benchmarks/test_harness.py -v
git diff --check
```

结果：`cargo test --workspace` 全部通过，其中 client 392 passed、2 ignored；
client Clippy 无项目警告；Python 基准测试 28 passed；fmt 与 `git diff --check`
通过。依赖 `proc-macro-error2 v2.0.1` 仍有 future-incompatibility 提示。
本轮没有执行 workspace 全量 Clippy、musl/Windows 交叉编译、特权 netns、
WireGuard 对照或性能基准，因此不能把这些结果表述为完整平台门禁或性能收益。

注意：这些是功能/回归证据，不是吞吐收益证据。P0 与 P1 的已完成项仍需在独立提交后使用同一基准测量；UDP owned RX、signaling `Bytes` ownership 和 `sendmmsg/recvmmsg` 仍在未提交工作树，TUN 仍逐包 syscall，生产 crypto worker、GSO/GRO、会话锁拆分和真实认证压力测试仍未完成。receiver index 已消除静态共享桶并恢复未知 endpoint 单候选，但仍有 32-bit 碰撞桶，且没有吞吐/尾延迟数据。

## 完成定义

一个检查项只有在以下全部满足时才能标记完成：

1. 源码改动已独立提交。
2. 对应回归/安全测试已加入并通过。
3. 文档中的验收标准有命令或测试输出证据。
4. 性能项有同一口径的前后对比；没有数据时只能标记“功能完成，性能待验证”。
