# 数据面性能整改 TODO

> 来源：`docs/WIREGUARD_GO_DATAPLANE_COMPARISON.md`
> 对照：`wireguard-go` commit `b3bc704ef0ee21a36298601a9e1e5ff90377ac52`
> 建立日期：2026-10-31
> 原则：先消除确定性的正确性/调度缺陷，再优化复制、锁和 syscall；每项都必须有回归测试或可复核计数。

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

- [ ] 删除 `AuthenticatedInboundCache`、容量常量、两次 BLAKE3 和所有 cache 清理逻辑。
- [ ] 引入类型化入站 packet：`AuthenticatedPlaintext` 与 `SealedCiphertext` 不再共用模糊的 `(IpAddr, Vec<u8>)`。
- [ ] UDP 直连认证成功后，将已解密的 plaintext ownership 直接交给主循环，不再入队密文。
- [ ] 主循环对 `AuthenticatedPlaintext` 只做 ACL、peer/source 校验和 TUN 写入，不再二次解密。
- [ ] QUIC relay/signaling 的 `SealedCiphertext` 仍调用 `decrypt_from_peer`，保持 relay 安全边界。
- [ ] endpoint migration 只能在认证成功后发生，并保持对旧 session/revoked peer 的拒绝语义。

**验收**

- [ ] 直连有效包在进入主循环前只执行一次 AEAD open。
- [ ] 入队数据是 plaintext，密文不再进入 P2P receive queue。
- [ ] 连续/积压超过旧的 4096 条记录后，不出现由 cache 淘汰导致的 `ReplayDetected`/`CounterTooOld` 丢包。
- [ ] 重放、篡改、未认证 plaintext、旧 session 延迟包仍被拒绝。
- [ ] peer 移除/重绑定后没有明文残留。
- [ ] `cargo test -p fuckipv6-client p2p::connection` 通过（若测试过滤器不匹配则运行该 crate 全量测试）。

**回归测试**

- [ ] 将现有 data-transfer、endpoint-migration、replay、peer-rebind 测试改为断言 queue 中为已认证明文。
- [ ] relay 密文仍通过 `decrypt_from_peer` 解密；同一 relay 密文第二次提交必须失败。
- [ ] 增加至少 4097 个已认证入站包的积压测试，全部有效包按序可消费且无二次 open。

## P0-2 TUN buffer 不再对 64B 包清零 64 KiB

**现状**

- `commands.rs::recycle_tun_buffer` 对回收 buffer 执行 `clear()`。
- `commands.rs::spawn_tun_reader` 下一轮执行 `resize(65536, 0)`，再 `truncate(size)`。
- 64B 包因此产生约 1024 倍无效零填充写放大。

**整改**

- [ ] pooled buffer 始终保持 65536 字节长度，不再 `clear`/`resize`。
- [ ] TUN reader 向主循环传递 `{ buffer: Vec<u8>, len: usize }` 或等价类型。
- [ ] 出站处理只使用 `buffer[..len]`；回收时只重置 `len`，不触碰 payload 容量内容。
- [ ] 保持 `TUN_BUFFER_POOL_SIZE` 有界，队列满时采取明确策略，不能重新引入无界 Vec 分配。

**验收**

- [ ] 64B 包路径不再写满 64 KiB 零值。
- [ ] 1420B/64B 包内容、长度、ACL、路由和发送行为不变。
- [ ] buffer 回收次数稳定，池不会因错误路径泄漏。
- [ ] 增加 64B/1420B 混合批次测试，验证每个包只暴露有效长度。

## P0-3 fallback 复用同一密文，禁止重复 seal

**现状**

- P2P `send_to` 已 seal 一次。
- UDP 失败后 QUIC relay 和 signaling fallback 会再次 seal 同一明文。
- 最坏情况消耗三个 nonce、执行三次 AEAD。

**整改**

- [ ] 抽出“解析 peer + 生成一次密文”的出站发送接口。
- [ ] UDP、QUIC relay、signaling fallback 复用同一个 sealed packet。
- [ ] 保持 per-peer nonce 顺序、session revocation gate 和失败语义。
- [ ] 明确只允许对同一 nonce/密文选择 transport，不能引入 AEAD 重放。

**验收**

- [ ] 单次 fallback 链的 seal 计数最多为 1。
- [ ] 不同业务包仍获得不同 nonce。
- [ ] 三条 transport 的认证、重放窗口和 ACL 行为通过现有安全测试。
- [ ] 增加 UDP 失败、QUIC 成功和全 fallback 的计数测试。

## P0-4 慢 fallback 移出中央数据面 select 分支

**现状**

- 主事件循环直接 `await handle_tun_reader_packet()`。
- QUIC relay 发送带 2 秒 timeout，单个慢 fallback 会阻塞 TUN/UDP/control 事件。

**整改**

- [ ] 中央循环只做快速校验、路由和直连发送；慢 relay/fallback 交给独立 owner task/channel。
- [ ] per-peer 保持发送顺序，跨 peer 可以并行。
- [ ] relay 断开、超时和自愈不能阻塞 TUN 读取与 UDP 接收。

**验收**

- [ ] relay 卡住 2 秒时，TUN 入站和已建立 UDP P2P 仍持续推进。
- [ ] 控制命令和 shutdown 不被发送超时拖住。
- [ ] 增加慢 transport 注入测试和中央循环心跳/次数断言。

## P1-1 receiver index：消除全连接扫描和 PeerInfo clone

**现状**

- 出站每包 `get_peers()` clone 全部 `PeerInfo`。
- 入站每包 `peer_by_ip()` clone 一个 `PeerInfo`，随后 `get_peers()` 再 clone 全部。
- 数据帧按 receiver index 线性扫描全部 connections。
- `PeerInfo` 含 `String`、`Vec<u8>`、`Vec<SocketAddr>`。

**整改**

- [ ] 建立 `receiver_index -> Arc<SecurePeerState>` O(1) 索引。
- [ ] 建立轻量出站路由索引，不克隆完整控制面对象。
- [ ] 入站 source/peer 校验使用 Arc/借用快照，不在包路径复制全部 peer 列表。
- [ ] 索引更新和连接移除保持原子性，旧 session 延迟包仍拒绝。

**验收**

- [ ] 每包不再 clone 全部 peer 列表。
- [ ] 已建立 peer 的直连入站查找为 O(1)。
- [ ] 多 peer（至少 100）下发送/接收正确，peer 增删无 stale route。
- [ ] 增加分配计数或可观测热路径指标。

## P1-2 热路径锁与统计

**现状**

- endpoint、health、global stats、peer stats 可能逐包取 `RwLock::write()`。
- 同一包存在 `PeerHealth` 与 `StatsCollector` 两套更新。

**整改**

- [ ] endpoint fast path 使用读锁/原子比较，仅变化时写。
- [ ] 统计改为 per-peer atomic counter 或异步批量聚合。
- [ ] 避免在持锁状态下 await 或执行加解密/网络发送。
- [ ] 明确锁顺序，消除连接索引与统计之间的潜在争用。

**验收**

- [ ] 普通直连包不获取全局统计写锁。
- [ ] endpoint 不变的包不获取 endpoint 写锁。
- [ ] 并发收发和 peer remove/rekey 压力测试无死锁。
- [ ] 输出至少一组 mutex/lock 等待计数或 profile 证据。

## P1-3 真 batch、buffer pool、per-peer 顺序队列

**现状**

- `seal_batch`/`open_batch_in_place` 内部仍是串行循环。
- 数据面逐包 syscall，未使用现有 batch API。
- `wireguard-go` 使用顺序 nonce 分配、CPU worker 并行 AEAD、per-peer 顺序发送。

**整改**

- [ ] 定义每 peer 的顺序发送队列和有界缓冲策略。
- [ ] batch 读 TUN、按 peer 分组、批量 UDP send/recv。
- [ ] 保证 nonce 顺序分配与发送顺序一致。
- [ ] 引入可复用 buffer pool，减少整包 Vec 分配。

**验收**

- [ ] 峰值路径分配次数和 syscall 次数可量化下降。
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
| P0-1 入站 ownership | 待开始 | 第一个实现项 |
| P0-2 TUN buffer | 待开始 | 依赖 P0-1 稳定后小步修改 |
| P0-3 fallback 复用密文 | 待开始 | 需要先梳理三种 transport 接口 |
| P0-4 慢 fallback 离环 | 待开始 | 与 P0-3 一起设计 owner task |
| P1/P2 | 待开始 | 以 P0 基准收益为前提继续 |

## 完成定义

一个检查项只有在以下全部满足时才能标记完成：

1. 源码改动已独立提交。
2. 对应回归/安全测试已加入并通过。
3. 文档中的验收标准有命令或测试输出证据。
4. 性能项有同一口径的前后对比；没有数据时只能标记“功能完成，性能待验证”。
