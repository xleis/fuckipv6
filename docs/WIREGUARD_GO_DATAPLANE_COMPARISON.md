# fuckipv6 与 wireguard-go 数据面源码对比分析

> 初版分析日期：2026-10-30；固定快照复核日期：2026-10-31。
> 分析方式：只读审查固定提交的 `git archive` 快照、与 `wireguard-go` 对照，并复核已有历史基准；主结论不在活动工作树上生成。
> `fuckipv6` 固定快照：commit `39ca63730e4bad9302084272c3c6fc6c6eddb737`（2026-10-31），通过 `git archive` 导出。本文所有 `fuckipv6` 行号均指该快照，不包含工作树中的未提交改动。
> 上游对比对象：`wireguard-go` commit `b3bc704ef0ee21a36298601a9e1e5ff90377ac52`（2026-10-25），同样以固定 `git archive` 快照审查。
> 历史基准：`benchmarks/data/2026-10-30-dataplane-baseline/README.md`，被基准的 `fuckipv6` 构建提交为 `b49be00f890a46e01653f6155774816e43d98ea7`；`wireguard-go` 二进制版本为 `v0.0.20250522`。历史基准不等于固定快照的重新测量结果。
>
> 路径缩写：下文 `connection.rs` 指 `crates/client/src/p2p/connection.rs`，`secure_runtime.rs` 指 `crates/client/src/p2p/secure_runtime.rs`，`commands.rs` 指 `crates/client/src/cli/commands.rs`，`secure-session/src/lib.rs` 指 `crates/secure-session/src/lib.rs`。

## 1. 摘要

以下结论来自固定快照 `39ca63730e4bad9302084272c3c6fc6c6eddb737`：

1. **入站仍在接收端认证一次、中央循环再处理一次。** UDP receiver 把已认证明文放进 `AuthenticatedInboundCache`，却只把密文排入无界队列；中央循环再用密文 BLAKE3 取回明文，cache miss 时再次调用 `open_from_session`。共享 `recv_from` buffer 还产生 `bytes.to_vec()` 复制。
2. **4096 项 authenticated cache 与 4096 项 replay window 仍可形成确定性丢包悬崖。** 当主循环积压超过 cache 容量时，最老明文被淘汰；对应密文的 replay counter 已在第一次认证时推进，二次解密会返回 `CounterTooOld` 或 `ReplayDetected`。
3. **每包完整 `PeerInfo` clone 仍存在。** 出站 `handle_tun_reader_packet` 调用 `get_peers()` 后线性匹配目的 IP；入站还会 `peer_by_ip()` 取一份完整对象，再 `get_peers()` clone 全部对象。
4. **TUN 清零和分配式出站 seal 仍是生产路径。** TUN reader 每包 `buf.resize(65536, 0)`，回收时 `packet.clear()`；P2P `send_to()` 仍调用返回新 `Vec` 的 `seal()`。secure-session/driver 已有 `seal_in_place` 底层 API 和单元测试，但固定快照没有把它接入 P2P/TUN 生产数据面。
5. **每包统计、endpoint 和 health 写锁仍存在。** `StatsCollector` 仍使用两把 `RwLock`，`update_peer_endpoint()` 无条件获取 connections 写锁，`record_peer_payload()` 也走连接表写锁；`PeerHealth` 仍通过可变引用逐包更新。
6. **`seal_batch` / `open_batch_in_place` 仍没有并行 worker。** 它们目前是逐包串行循环，固定快照中的客户端数据面也没有调用这些批量 API；不能等同于 `wireguard-go` 的 CPU 数加解密 worker。
7. **fallback 重复 seal 仍存在。** 直连 P2P 先 seal；UDP 发送失败后 QUIC relay 的 `encrypt_for_peer()` 再 seal；signaling fallback 可再次 seal，同一明文最多消耗三个 nonce。
8. **慢 fallback 仍可阻塞中央 `select!`。** 中央循环直接 `await handle_tun_reader_packet(...)`；QUIC relay 有 2 秒 timeout，固定快照中没有 per-peer 有界 outbound queue/worker。
9. **历史 48 倍差距仍不能当作严谨排名。** 基准脚本只用 ping 证明隧道可达，没有断言 iperf3 流量实际走直连 UDP P2P；历史日志也未保留 fallback/transport 证据。因此路径差异可能混入结果。
10. **pair-scoped receiver index 只有派生原语，生产分派仍未接线。** `derive_receiver_indices()` 及其单元测试已提交，同一节点对不同 peer 会得到不同 index；但 `dispatch_secure_data()` 仍线性遍历全部 connections。固定快照中没有 100-peer 分派测试，也没有 session-scoped index 轮换；32-bit 碰撞和重握手复用同一 index 的边界仍存在。

一句话概括：

> **固定快照仍未完成数据面整合：入站 cache/二次认证、全量 peer clone、TUN 清零、分配式 seal、重复 fallback seal、中央循环阻塞和逐包锁都存在；已提交的 `seal_in_place` 与 pair-scoped index 只是底层原语，不能据此宣称生产性能、安全等级或与 WireGuard 对等/超越。**

### 1.1 固定快照数据面问题证据链

> 下面 A-I 记录固定提交 `39ca63730e4bad9302084272c3c6fc6c6eddb737` 中仍存在的路径，用于解释历史基准为何不能外推为当前性能；历史基准本身构建自 `b49be00`。

#### A. 入站每包两份 ciphertext Vec、明文搬移和两次 BLAKE3

固定快照的正常直连路径是：

```text
UDP shared bytes
  -> bytes.to_vec()                       # 1. 密文复制为 owned open buffer
  -> open_in_place + extract_opened_plaintext
                                            # 同一 Vec 内搬移明文，不再分配第二份
  -> authenticated_inbound.insert(hash)   # 对密文第一次 BLAKE3
  -> incoming_tx.send(bytes.to_vec())     # 2. 密文再次复制入队
  -> decrypt_from_peer(payload)
  -> authenticated_inbound.take(hash)     # 对密文第二次 BLAKE3
```

对应源码：

- `connection.rs:1206` 用 `bytes.to_vec()` 把共享接收 buffer 复制为可原地解密的 owned packet。
- `secure_runtime.rs:992-998`、`1069-1075` 在同一个 receive `Vec` 内调用 `open_in_place`，随后把明文 `copy_within` 到前部；这里没有第二次明文 `Vec` 分配，但仍有整包内存搬移。
- `connection.rs:1224-1232` 插入 cache，并把另一份 `bytes.to_vec()` 发送给主循环。
- `connection.rs:48-95` 在 `insert` 和 `take` 中各做一次 `blake3::hash(ciphertext)`。
- `connection.rs:58-65`、`88-93`：cache key 两次执行 `client_id.to_string()`；`insert` 还执行 `key.clone()`，让同一个 key 同时存入 `HashMap` 和顺序队列。正常入站包因此额外产生 3 次 `client_id` 字符串堆分配。
- `connection.rs:421-434` 主循环尝试从 cache 取明文。

这不是 allocator 微优化问题，而是 ownership 设计错误：解密者已经拥有明文，却只把密文发送给后续阶段，再让后者用哈希找回明文。

对比 `wireguard-go`：

- `device/receive.go:168-185` 把接收 buffer 的 ownership 放入 inbound element。
- `device/receive.go:238-265` 在同一个 buffer 上原地解密。
- `device/receive.go:439-534` 的 per-peer sequential receiver 直接消费已解密的 element，最后批量写 TUN。
- `device/pools.go:49-121` 回收 buffer/element/container。

它既不需要“密文哈希 -> 明文缓存”，也不需要在 receiver 与主循环之间制造两遍 ownership。

#### B. 4096 cache + 无界队列会形成 replay 丢包悬崖

源码事实：

- cache 容量固定为 4096：`connection.rs:27`、`43-80`。
- 超限时淘汰最老明文：`connection.rs:69-79`。
- 入站队列是无界 channel：`connection.rs:29-31`、`199`、`216`。
- 主循环 cache miss 后会回退到 `open_from_session(payload.to_vec())`：`connection.rs:490-493`。
- 第一次成功解密已经执行 replay window 标记：`crates/secure-session/src/lib.rs:552-566`。
- 同一 counter 再次进入会返回 `ReplayDetected`；落到窗口外则返回 `CounterTooOld`：`secure-session/src/lib.rs:1005-1021`、`1030-1050`。
- replay window 也恰好是 4096：`secure-session/src/lib.rs:54-55`。因此 cache 一旦因第 4097 个在途包而淘汰最老项，最老 counter 通常已经落到窗口外，典型结果是 `CounterTooOld`；其他已标记 counter 的重放则返回 `ReplayDetected`。

因此当主循环处理速度持续低于 UDP receiver，且在途包超过 4096 时：

1. receiver 已成功认证并推进 replay window；
2. 最老明文被 cache 淘汰；
3. 对应密文仍在 `incoming_tx` 中；
4. 主循环取到密文后查 cache 失败，只能再次解密；
5. replay window 拒绝同一 counter，包被丢弃。

这是**源码推断出的确定性触发条件**，不是从现有 profiler 测得的比例。它也解释了为什么饱和 UDP 下高丢包可能同时包含“队列处理不过来”和“处理了但被第二次拒绝”两类损失。修复方向不是把 cache 调大，而是让已认证明文携带 peer/source 元数据直接进入下一阶段。

#### C. 每包克隆完整 `PeerInfo`

`PeerInfo` 的字段为 `ClientId = String`、`Vec<u8>` 公钥、`Vec<SocketAddr>` endpoints 等（`crates/common/src/types.rs:5`、`81-102`）。

- 出站：`commands.rs:1402-1406` 每包调用 `p2p.get_peers()` 后线性查找目的 IP。
- `connection.rs:623-631` 的 `get_peers()` clone 所有 `peer_info`。
- 入站：`commands.rs:1081-1085` 先 `peer_by_ip()` 取一个完整对象，再 `get_peers()` 取全部 clone。
- `connection.rs:610-615` 的 `peer_by_ip()` 也整份 clone `PeerInfo`。

所以问题不只是 O(N) 扫描，还包括每包多次字符串、公钥和 endpoint 向量的堆分配/释放。单 peer 基准中 O(N) 扫描很轻，但 clone/allocation 仍然存在，这能解释为什么“peer 很少”也不应把它当作可忽略成本。

#### D. 两套统计、多次写锁进入每包路径

出站一包至少经历：

- `record_peer_payload()`：`connection.rs:486`、`494`、`1442-1450`，拿 `connections.write()`。
- `StatsCollector::record_p2p_tx()`：`commands.rs:1654`，在 `stats.rs:157-180` 分别拿 global 和 peers 两把写锁。

入站一包至少经历：

- `update_peer_endpoint()`：`connection.rs:1137`、`1411-1430`，即使 endpoint 未变也拿 `connections.write()`。
- authenticated cache 写锁：`connection.rs:1224-1230`。
- `record_peer_payload()`：`connection.rs:1442-1450`（调用在 `486`/`494`）。
- `StatsCollector::record_p2p_rx()`：`commands.rs:1076-1079`，同样两把写锁。
- 后续 `peer_by_ip()` 与 `get_peers()` 读锁。

`PeerHealth::record_payload()` 还会做窗口清理和带宽更新（`crates/client/src/p2p/health.rs:88-110`、`260-283`）。这些逻辑适合采样或异步聚合，不适合阻塞每个数据包。

#### E. batch API 是“批量接口”，不是“并行批处理”

- `SendCipher::seal_batch` 最后仍是 `for ... { self.seal(...) }`：`secure-session/src/lib.rs:397-440`。
- `ReceiveCipher::open_batch_in_place` 最后仍是逐包 `self.open_in_place(...)`：`secure-session/src/lib.rs:580-602`。
- `EpochSession::open_batch_in_place` 也是逐包调用：`secure-session/src/lib.rs:732-752`。

这些 API 的价值是把调用方存储布局固定、减少 per-packet allocation，并集中做布局/counter 检查；**不会自动使用多核**。若要接近 `wireguard-go`，必须在 batch 之上增加共享 worker queue 和并行 AEAD，同时保留 per-peer nonce/replay 顺序。

#### F. fallback 最多三次 AEAD、三个 nonce

路径如下：

1. `connection.rs:499-531`：P2P `send_to` 先 `seal` 一次。
2. `connection.rs:553-556`：UDP `send_to` 失败时返回错误。
3. `commands.rs` 的 TUN fallback 进入 QUIC relay。
4. `commands.rs:1640-1646`：`encrypt_for_peer()` 再次 `seal`。
5. QUIC relay 发送失败或 2 秒超时：`commands.rs:1647-1669`。
6. `commands.rs:1680-1687`：signaling fallback 第三次 `seal`。

这里每一步都在重新加密，而不是“只 seal 一次，再选择可用传输”。会话已建立、UDP `send_to` 失败且 QUIC relay 也失败时，同一明文最多执行三次 AEAD 并消耗三个 nonce。

#### G. 中央循环可以被 2 秒 relay timeout 阻塞

中央事件循环在 `commands.rs:716-801` 选中 TUN 分支后，会直接等待 `handle_tun_reader_packet(...).await`。该调用可进入 `send_tun_packet_quic_relay()`，其中：

- timeout 为 2 秒：`commands.rs:33`、`1648-1669`；
- 失败后还会关闭 relay：`commands.rs:1042-1045`。

因此 QUIC relay 慢/失败时，TUN、UDP、信令事件都不能在同一个 `select!` 循环里被处理。这个问题与“没有批处理”独立：即使每包都很快，单次慢 fallback 仍可造成明显 jitter 和队列膨胀。

#### H. 基准没有证明实际 transport

`scripts/bench-dataplane.sh:337-376` 只在启动后用 ping 验证隧道可达；`scripts/bench-dataplane.sh:592-639` 直接跑 iperf3。

脚本没有：

- 读取或打印当前 peer 的 `active_endpoint`；
- 断言流量走直连 UDP，而不是 QUIC relay/signaling fallback；
- 禁止或标记 fallback；
- 保存可用于事后确认路径的客户端日志。

因此历史结果只能证明“某条可达路径上的端到端性能”，不能证明三种实现比较的是同一 transport。固定快照中未调用 `enable_quic_mesh` 时，`send_to` 直接尝试 UDP；若 mesh 已启用，则会先尝试 QUIC mesh（`connection.rs:543-550`），再回退到 UDP（`connection.rs:553-556`）。无论哪种情况，UDP 发送失败后都存在完整 fallback 链。这个不确定性必须在下一轮基准中消除。

#### I. 64B 小包每轮仍主动清零约 64 KiB

这条不是“没有使用 zero-copy”的泛化判断，而是确定的写放大：

- buffer pool 只用 `Vec::with_capacity(65536)` 预分配容量（`commands.rs:605-610`）。
- 事件循环回收包时先 `packet.clear()`，把长度重新设为 0（`commands.rs:1049-1052`）。
- TUN reader 每轮拿到 buffer 后执行 `buf.resize(TUN_PACKET_BUFFER_SIZE, 0)`（`commands.rs:1324`）。长度从 0 变为 65536，因此 `resize` 会把全部 65,536 字节填 0。
- 随后 `tun.read(&mut buf)` 只覆盖实际包，`buf.truncate(size)` 又把长度缩回有效长度（`commands.rs:1327-1329`）；下一轮继续从头清零。

因此一个 64B 小包每轮先写约 65,536B 的零，再写 64B 有效数据，至少是 1024 倍的无效内存写放大。它独立于 AEAD、peer 数和锁竞争，直接压低小包 pps；这也解释了为什么 64B 场景比 1420B 场景更不划算。

### 1.2 数量级如何理解

历史饱和 UDP 结果对应：

```text
1420 B: 0.0382 Gbit/s / 8 / 1420 ≈ 3,363 pps
64 B:   0.00358 Gbit/s / 8 / 64  ≈ 6,992 pps
```

而密码基准中 1420B 单包 `seal`/`open` 约 `2.80/3.06 µs`；即使粗略地按同一核串行执行一次 seal 和一次 open，理论上仍可达约 `1 / (2.80 + 3.06) µs ≈ 17 万 pps`。现有端到端只有约 3-7k pps，说明大部分预算不是 AEAD，而是调度、复制、锁、查表、队列和 syscall。

这不是精确 profiler 归因，但足以否定“密码原语是数量级差距主因”的解释。

### 1.3 固定快照代码状态复核

#### A. pair-scoped index 派生已提交，但没有用于生产分派

`secure_runtime.rs:1182-1208` 的 `derive_receiver_index()` / `derive_receiver_indices()` 用双方 Ed25519 公钥和本端角色做域分离 BLAKE3，`secure_runtime.rs:1270-1289` 的单元测试验证：

- initiator 与 responder 的 index 不同；
- 相同 pair 的派生结果稳定；
- 同一本端换一个 peer 后，对应 index 发生变化。

这只能证明派生函数的局部性质。`connection.rs:1169-1190` 的 `dispatch_secure_data()` 仍遍历全部 connections 并调用 `handles_data_packet()`，固定快照中没有按 `(receiver_index, endpoint)` 的生产分派表，也没有 100-peer 或 NAT rebind 分派测试。不能把该原语写成“入站 O(1) 已完成”。

限制仍然成立：

- 它是 pair-scoped deterministic label，不是 `wireguard-go` 的 session-scoped random index；同一 pair 重握手/rekey 后 index 不变。
- 32-bit 空间仍可能碰撞；无生产索引时仍会退化为全连接扫描。
- index 只是未认证的 demultiplexing hint，最终必须通过 AEAD、epoch、session 和 replay 校验。

#### B. 入站 authenticated cache、二次 open 和共享 RX 复制仍存在

固定快照仍在 `connection.rs:27-95` 定义 4096 项 `AuthenticatedInboundCache`，在 `connection.rs:1169-1240` 线性扫描连接、复制 ciphertext、认证后写 cache，并向中央循环发送另一份 ciphertext。`commands.rs` 的入站处理再用 `decrypt_from_peer()` 取 cache 明文。第 1.1 节 A/B 的丢包悬崖和复制链仍适用，不是已整改历史。

#### C. stats、endpoint 和 payload health 仍逐包使用写锁

- `stats.rs:49-51` 仍为 `Arc<RwLock<GlobalStats>>` 和 `Arc<RwLock<HashMap<...>>>`，`record_p2p_rx()` / `record_p2p_tx()`（`stats.rs:131`、`157`）仍获取写锁。
- `connection.rs:1411-1430` 的 `update_peer_endpoint()` 仍先获取 `connections.write()`，没有未变化 endpoint 的只读 fast path。
- `connection.rs:1442-1452` 的 `record_peer_payload()` 仍获取 connections 写锁。
- `health.rs:33-110` 的 `PeerHealth` / `record_payload(&mut self, ...)` 仍要求可变访问。

因此不能标记“全局/peer atomic stats”“endpoint read fast path”或“payload health 原子记账”为已完成。

#### D. 底层原地 seal API 存在，但生产 P2P/TUN 路径未接线

固定快照确有底层原语和测试：

- `secure-session/src/driver.rs:209-222` 的 driver `seal_in_place()`；
- `secure-session/src/lib.rs:355-385` 与 `683-698` 的 session/epoch `seal_in_place()`；
- `secure-session/src/rekey_session.rs:171-181` 的 rekey wrapper；
- `secure-session/tests/driver.rs:182-241` 覆盖原地 seal、短缓冲不计 counter/stats、tamper/replay 不污染会话；
- `secure_runtime.rs:855-888` 也已有运行时 `seal_in_place()` helper。

但固定快照的生产链仍是：

- `commands.rs:1324` 每包 `buf.resize(TUN_PACKET_BUFFER_SIZE, 0)`，`commands.rs:1049` 回收时 `packet.clear()`；
- P2P `send_to()` 在 `connection.rs:499-558` 读取 peer 后调用 `seal()`，而 `secure_runtime.rs:835-851` 的 `seal()` 先分配新 packet 再调用 helper；
- fallback 在 `commands.rs:1640`、`1680` 独立调用 `encrypt_for_peer()`，仍可能重复 seal。

底层测试证明的是 API 的局部正确性，不是 TUN/UDP/QUIC/signaling 生产路径已原地加密，也不是吞吐收益。

#### E. 验证边界

本报告没有在固定快照上重新运行测试、吞吐或 profiler；历史测试计数和历史基准不能自动归因给 `39ca637`。可勾选的 `[x]` 只限于快照中可直接读到并已有对应单元测试的底层原语；生产接线、端到端安全和性能保持未验证。

### 1.2 后续提交与工作树复核（新增结论）

> 本节补充 2026-10-31 后续整改，不修改 1.1 对固定快照 `39ca637` 的历史结论。当前已提交 HEAD 为 `b3376ca6dc3a7bb7eb915fcb6585bdbeec79dd6e`；生产连接层整合仍有大量未提交修改，因此必须同时区分“已提交原语”“工作树已接线”“实测二进制”。

| 数据面结论 | 固定快照 `39ca637` | 当前状态 |
|---|---|---|
| UDP RX ownership | `recv_from` 后 `to_vec()`，认证 cache/二次 open | 工作树已用 owned pooled buffer；认证后直接把 plaintext ownership 交主循环 |
| signaling ciphertext | 有 `Bytes -> Vec` 边界 | 工作树 payload 已改为 `Bytes`，经 CLI `SignalingAction` 原样持有 |
| 入站队列 | 无界通道，积压可过 replay/cache 悬崖 | 工作树为默认 1024 有界队列，drop-newest 与指标已测；真实密码认证组合压测未做 |
| UDP syscall | 逐包 `send_to/recv_from` | 工作树生产 RX/TX 已接 Linux `recvmmsg/sendmmsg`，默认 32、上限 128 |
| TUN syscall | 逐包 read/write | 仍是每次 readiness edge 后逐包 `read(2)`，write 仍一包一次；只是减少唤醒，不是 batch syscall |
| 会话锁 | 单状态锁，batch 串行 | `SecurePeerState` 仍是单一 `Mutex<SecureState>`；已加 lock 指标和同 peer 批量原语，但未拆锁 |
| buffer/offload | 逐包分配，无 GSO/GRO | UDP receive pool 已分片；全链路统一池、GSO/GRO、TUN offload 仍未实现 |
| 基准路径证明 | direct/fallback 混入风险 | 已断言 direct 且 fallback=0；修正 UDP 1420 口径；当前只有 1 run smoke，正式结果未生成 |

**新的源码级判断：**

1. `wireguard-go` 的核心优势不只是“批量 syscall”，而是 TUN/UDP 批量、per-peer 顺序队列、crypto worker 和内存所有权的组合。当前 fuckipv6 只补齐了 UDP 消息批处理；TUN 每包一次 syscall 仍是明确差距，不能仅凭 UDP `sendmmsg` 就认为数据面路径已追平。
2. `seal_batch_in_place` 的 4-worker 微基准有约 2.4x 加速，但生产数据面仍未把批量 seal、worker 和每 peer 顺序发送完整连起来；微基准不能外推为端到端吞吐。
3. signaling `RelayPacket` 的 CLI `Bytes -> Vec` 复制已消除，这是本轮明确的复制优化；但 postcard/frame 编码和协调端 decode/re-encode 仍会物化 payload，不能宣称整个 relay 链路零复制。
4. `SecurePeerState` 仍让 send/open 共用一把状态锁。已有 `try_lock` 等待/持锁指标和批量临界区只能说明锁竞争可观测、获取次数可下降，不能说明锁已拆分或热路径已无争用。
5. UDP pool sharding 解决了同一批 acquire/recycle 的集中竞争，但不是 TUN、seal/open、fallback 共用的固定 buffer 池；没有 GSO/GRO 时，64B 小包路径仍受每包开销限制。

**当前工作树 smoke（dirty HEAD `d6fceb4`，binary SHA256 `6574f1d3...cee60`）：** 单 run 为 18/18 有效；3 run 复核为 54/54 有效，fuckipv6 的 18 次测量全部 direct-path 且 fallback delta 为 0。3 run 中位数显示 TCP 单流/8 流约为 kernel WireGuard 的 `0.391x/0.397x`、wireguard-go 的 `0.197x/0.186x`；UDP 1420 饱和约为二者的 `0.337x/0.738x`；64B 饱和 UDP 约为 `0.465x/1.015x`。这组数据与“UDP syscall 已批量、但 TCP 仍受 TUN 逐包、中央事件循环和会话锁等共同限制”的诊断一致，不支持已经追平 WireGuard 的结论。该组仍为 5s stream、未绑核、`powersave`、无 perf profile，且宿主负载不满足正式门限；3 run 只能确认方向稳定，不能量化批处理收益或形成正式排名。

**新采集的 syscall 证据：**

- 目录：`target/benchmarks/syscall-evidence-20261031/udp1420-clean/`、`udp64-clean/`；direct-path=true，fallback=0。
- 1372B iperf3 payload（1420B 隧道包）：成功 TX batch 约 31.65 包/次，RX 约 20.42 包/次。
- 64B payload：成功 TX batch 约 31.83 包/次，RX 约 17.12 包/次。
- 外部 `strace -c` 同时显示 TUN `read/write` 次数与包数近似 1:1，证明 TUN 仍逐包穿越内核。
- metadata 中的 `ec234b3a...` 是只读 strace wrapper hash；被测 binary hash 是 `720d02cc...786c1c`。

**当前性能结论边界：** 修正后的三实现 smoke 为 1 run、5s、`--no-perf`，只能证明路径、有效样本和方向性差异，不能作为最终排名。当前仍为 **0/8 大项完整验收**；近期能关闭的是 UDP batch 与 syscall 证据，不能关闭 TUN batch、锁拆分、GSO/GRO 或端到端 worker 优化。

## 2. 对比方法和适用边界

### 2.1 做了什么

- 只读审查 `fuckipv6` 固定 `git archive` 快照中的 TUN 读写、UDP 收发、Secure Session 加解密、连接索引、统计、锁和事件循环。
- 只读审查固定 `b3bc704` 快照中的 TUN/UDP 批量接口、加密/解密 worker、per-peer 队列、内存池和 Linux GSO/GRO 路径。
- 对照已有端到端基准和密码原语基准，检查“密码太慢”是否是主因。
- 只做固定快照源码和已有数据推导，没有在活动工作树上采集新的 profiler trace，也没有把历史测试结果当作 `39ca637` 的重新验证。

### 2.2 不能从本报告得出的结论

- 不能声称某个函数“占用了 40% CPU”；没有固定快照 `39ca637` 的分阶段 profiler。
- 不能声称固定快照等于历史基准二进制；历史基准来自 `b49be00`，两者之间没有同 transport、同 commit 的吞吐或尾延迟对照。
- 不能把内核 WireGuard 的差距直接等同于“用户态代码质量差距”。内核 WireGuard 与网络栈深度集成，本身是更强的基线。
- 不能拿受控 UDP 的 `50 Mbit/s` 上限测试证明峰值能力；在该测试中，多个实现都被发送速率封顶。
- 不能从行数、局部原地 API、pair-scoped index 或历史基准推导出加密等级、安全性或性能已与 WireGuard 对等或超越。

## 3. 项目代码规模

### 3.1 统计口径

- 代码规模只统计 `39ca63730e4bad9302084272c3c6fc6c6eddb737` 的 `git archive` 快照，不读取活动工作树或未提交改动。
- 物理行使用 `str.splitlines()`，有效行指去掉空行和纯注释行后的行数；行尾注释仍计入。
- `fuckipv6` 源码统计范围为快照内 `crates/*/src/**/*.rs`。“生产口径”额外排除文件末尾的顶层 `#[cfg(test)] mod tests` 块，以及 `#[cfg(test)] mod tests;` 引用的 `tests.rs`，避免和 Go 的 `_test.go` 排除口径混淆。
- `wireguard-go` 对照对象为 `b3bc704ef0ee21a36298601a9e1e5ff90377ac52`（2026-10-25），统计非 `_test.go` 文件，排除带 `Code generated ... DO NOT EDIT` 头部的 stringer 输出，保留构建标签和平台相关文件。
- 两个项目都使用同一个 Python 计数器，避免混用 `wc -l`、`NF` 和不同测试排除规则。
- 该统计用于说明工程体量，不用于直接评价性能。

### 3.2 规模结果

| 项目 | 文件数 | 物理行 | 有效行 |
|---|---:|---:|---:|
| `fuckipv6` 全部 tracked Rust（含 tests/benches/fuzz） | 124 | 78,415 | 68,147 |
| `fuckipv6` `src` Rust（含内联/外置测试） | 113 | 76,217 | 66,164 |
| `fuckipv6` `src` Rust（生产口径，排除测试） | 112 | 52,787 | 44,892 |
| `wireguard-go` 非测试 Go（排除生成文件） | 77 | 13,793 | 10,815 |
| `wireguard-go` 测试 Go（`*_test.go`） | 22 | 4,149 | 3,569 |

按生产口径计算，`fuckipv6` 有效行约为 `wireguard-go` 非测试 Go 的 **4.15 倍**；若把 `src` 内测试也算入，则为 **6.12 倍**。这些数字只对应 `39ca637` 的 archive 快照，不能与活动工作树或其他提交混算。

这不是公平的“同一功能规模”比较：

- `fuckipv6` 包含协调器、信令、路由、出口节点、UI/IPC、配置、DHT/overlay、QUIC 等更多产品功能。
- `wireguard-go` 专注 VPN 数据面和最小的 WireGuard 协议实现。
- 数据面性能由热路径复杂度决定，而不是由总代码量决定。代码更多并不能解释每包处理慢；同样，删除控制面代码也不会自动解决数据面瓶颈。

### 3.3 固定快照 Rust 代码分布

| Crate | 生产 Rust 文件数 | 物理行 | 有效行 | 占生产有效行 |
|---|---:|---:|---:|---:|
| `crates/client` | 64 | 31,804 | 27,830 | 62.0% |
| `crates/common` | 16 | 5,616 | 4,439 | 9.9% |
| `crates/coordinator` | 11 | 6,506 | 5,668 | 12.6% |
| `crates/secure-session` | 21 | 8,861 | 6,955 | 15.5% |
| **合计** | **112** | **52,787** | **44,892** | **100%** |

`client` 是数据面和控制面交汇处，也是本次分析重点。不能因为 `secure-session` 有约 6.9k 行生产有效代码就判定密码实现是瓶颈；它同时包含握手、混合密钥交换、重放窗口、epoch/rekey 和批量 API。完整、可复现的版本固定与代码规模命令见 `docs/WIREGUARD_CODE_SCALE.md`。

## 4. 已有基准说明了什么，没说明什么

### 4.1 历史端到端中位数

数据来自 `benchmarks/data/2026-10-30-dataplane-baseline/README.md`：

| 用例 | fuckipv6 | 内核 WireGuard | wireguard-go |
|---|---:|---:|---:|
| TCP 1 flow | 0.0551 Gbit/s | 1.4788 Gbit/s | 2.6436 Gbit/s |
| TCP 8 flows | 0.0497 Gbit/s | 1.4799 Gbit/s | 2.4687 Gbit/s |
| UDP 1420 controlled | 0.0405 Gbit/s，15% 丢包 | 0.0500 Gbit/s | 0.0500 Gbit/s |
| UDP 64 controlled | 0.00352 Gbit/s，27.8% 丢包 | 0.00500 Gbit/s | 0.00500 Gbit/s |
| UDP 1420 saturated | 0.0382 Gbit/s，98.3% 丢包 | 1.0381 Gbit/s | 0 Gbit/s，结果无效 |
| UDP 64 saturated | 0.00358 Gbit/s，97.4% 丢包 | 0.0870 Gbit/s | 0.0411 Gbit/s |

### 4.2 为什么不能把 3.7% 当作最终结论

- 宿主机 `load average` 约 `6.5-9.6`，不是空闲基准机。
- 没有 CPU pinning。
- 每个用例只运行 3 次，10 秒，未设置 warmup omit。
- 历史基准构建提交与固定分析快照不同，且没有绑定工作树/二进制哈希的完整对照。
- `wireguard-go` 的 1420B 饱和 UDP 结果为 0，说明至少一组采集明显失效。
- 受控 UDP 被 `50 Mbit/s`/`5 Mbit/s` 发送上限截断，同时出现明显丢包，不能作为峰值吞吐排名。

### 4.3 为什么仍然可以确认“架构落后”

不是依赖单一噪声数字，而是多条独立证据一致：

- 单流 TCP 只有 `0.055 Gbit/s`，绝对水平明显异常。
- 从 1 流增到 8 流，`fuckipv6` 没有扩容，反而略降；这符合“全局单消费者串行处理”的特征。
- 饱和 UDP 出现 `97%-98%` 丢包，说明接收/处理速度远低于发包速度。
- 源码中确实存在单包事件循环、单包系统调用、逐包分配、逐包全局锁和逐包线性扫描。

因此应表述为：

> **具体倍数需要重测；“当前数据面架构与成熟实现存在数量级差距”可以成立。**

## 5. 两边数据面架构总览

### 5.1 `wireguard-go`：显式分阶段流水线

`wireguard-go` 在 `device/send.go:23-29` 直接写明了出站流水线：

1. TUN queue
2. Routing，顺序处理
3. Nonce assignment，顺序处理
4. Encryption，并行处理
5. Transmission，按 peer 顺序处理

其核心特点：

- TUN 和 UDP 都按 batch 处理，`conn.IdealBatchSize = 128`（`conn/conn.go:18-20`）。
- 设备 batch size 取 TUN 与 UDP bind 的最大值（`device/device.go:328-339`）。
- 每种 worker 数量按 CPU 数创建：加密、解密、握手各一组（`device/device.go:309-318`）。
- 固定大小 message buffer 和 element 使用池复用（`device/pools.go`）。
- 加密在预留头部空间的原缓冲区中原地完成（`device/send.go:434-475`）。
- 解密同样原地完成（`device/receive.go:238-267`）。
- Linux 路径支持 TUN GSO/GRO 和 UDP GSO/GRO。
- 接收时通过 receiver index 表直接定位 keypair/peer，不扫描全部 peer。

### 5.2 `fuckipv6` 固定快照：以单包 `await` 为中心的事件循环

固定快照主要路径：

```text
TUN reader（逐包 read）
  -> unbounded mpsc<Vec<u8>>
  -> 中央 tokio::select! 事件循环
  -> 路由 / 查 peer / 统计
  -> 每 peer async mutex
  -> 逐包 seal 分配 Vec
  -> socket.send_to

UDP recv task（逐包 recv_from）
  -> 扫描 connections 找 receiver index
  -> 每 peer async mutex
  -> 逐包 open + 再复制 plaintext
  -> BLAKE3 哈希并写入 authenticated cache
  -> unbounded mpsc<Vec<u8>>
  -> 中央事件循环取缓存明文
  -> 统计 / ACL / peer 查询 / 校验源地址
  -> 单包 tun.write
```

它并非没有并发任务：TUN reader、UDP receiver 和主事件循环分别在任务中运行。但**每个包的完整处理仍由一个消费者串行 `await` 完成**，而且包在多个阶段之间被复制和重新查找。

## 6. `wireguard-go` 出站路径

### 6.1 TUN 批量读取与按 peer 分组

`device/send.go:206-315`：

- 预分配 `batchSize` 个 element 和 buffer。
- 调用 `device.tun.device.Read(bufs, sizes, offset)` 一次读取一批。
- 为每个包通过 allowed-IP 路由直接找到 `*Peer`。
- 同一 peer 的包放入一个 container。
- 交给该 peer 的 staged queue，再进入 outbound 和 encryption queue。

热路径没有为每个包新建 `Vec`，而是复用固定 `[MaxMessageSize]byte` 缓冲区。

### 6.2 Nonce 顺序分配、加密并行

`device/send.go:337-402` 和 `434-475`：

- nonce 在 peer 的 staged/send 路径中顺序分配，保证每 peer 顺序。
- 一个 container 被投递到共享 encryption queue。
- 每 CPU 一个加密 worker，对 container 内的包原地加密。
- transport header 直接写在预留的 buffer 前部。
- 加密结束后释放 container mutex，顺序发送端即可继续。

这是“控制顺序、并行计算”的典型流水线，而不是“一个 peer 一把 async mutex 从头锁到尾”。

### 6.3 批量 UDP 发送

`device/send.go:477-541`：

- 每个 peer 有顺序 sender。
- 一个 batch 中所有已加密包收集进 `bufs`。
- 调用 `peer.SendBuffers(bufs)`，最终进入 bind 的批量发送。
- 完成后把 buffer/element 归还池。

`conn/bind_std.go:341-434`：

- Linux 优先走 `WriteBatch`，即 `sendmmsg` 风格批量写。
- 支持 UDP GSO coalescing。
- GSO 不受支持时自动回退并重试。
- 非 Linux 才逐包 `WriteMsgUDP`。

## 7. `wireguard-go` 入站路径

### 7.1 批量接收和 O(1) receiver index 查找

`device/receive.go:72-235`：

- 一次从 bind 接收一批 UDP datagram。
- 对 transport packet 读取 receiver index。
- 通过 `device.indexTable.Lookup(receiver)` 直接得到 keypair 和 peer。
- 按 peer 分组到 inbound container。
- 同时把 container 投递到 per-peer inbound queue 和共享 decryption queue。

这里不需要遍历所有 peer，也不需要扫描连接列表。

### 7.2 并行解密、顺序消费

`device/receive.go:238-267`：

- 每 CPU 一个 decryption worker。
- AEAD 原地解密到同一 buffer。
- 解密成功后释放 container mutex。

`device/receive.go:429-535`：

- per-peer sequential receiver 等待解密完成。
- 检查 replay window，更新 endpoint、计时器和统计。
- 校验内层源 IP 属于该 peer。
- 将多个合法明文包组成一批，一次调用 `tun.device.Write(bufs, offset)`。
- 最后回收 buffer 和 element。

顺序语义只保留在每 peer 的 nonce/replay/发送阶段，不需要让全局 CPU 都串行等待。

## 8. `wireguard-go` 的关键优化点

| 能力 | 实现位置 | 效果 |
|---|---|---|
| 128 包批处理 | `conn/conn.go:18-20` | 摊薄 syscall 和调度开销 |
| CPU 数 worker | `device/device.go:309-318` | 多核并行 AEAD |
| 固定 buffer 池 | `device/pools.go:12-121` | 稳态避免逐包堆分配 |
| 原地加解密 | `device/send.go:434-475`、`device/receive.go:238-267` | 减少复制 |
| receiver index 表 | `device/receive.go:151-185` | 入站 O(1) 查 peer |
| per-peer 顺序队列 | `device/send.go:318-402`、`device/receive.go:429-535` | 并行同时保持协议顺序 |
| TUN GSO/GRO | `tun/tun_linux.go:335-448` | 减少大包拆装和 TUN syscall |
| UDP GSO/GRO | `conn/bind_std.go:226-299`、`341-434` | 批量收发，降低每包网络开销 |

这些优化不是单点 micro-optimization，而是一套相互配套的数据面架构。

## 9. `fuckipv6` 出站路径逐阶段分析（固定快照）

### 9.1 TUN reader 仍然逐包

`crates/client/src/cli/commands.rs:1312-1348`：

```rust
buf.resize(TUN_PACKET_BUFFER_SIZE, 0);
match tun.read(&mut buf).await { ... }
```

问题：

- 每次只 `read` 一个包（`commands.rs:1327`）。
- 读到包后 `buf.truncate(size)`（`commands.rs:1329`）；事件循环回收时又 `packet.clear()` 把长度设回 0（`commands.rs:1049-1052`），下一轮再 `buf.resize(TUN_PACKET_BUFFER_SIZE, 0)`（`commands.rs:1324`）。因此 `Vec::resize` 会把全部 65,536 字节填 0，而不是只填尾部；64B 小包每轮先清零约 64 KiB，达到约 1024 倍 payload 的写放大。
- 包通过 unbounded `mpsc<Vec<u8>>` 进入中央事件循环。
- Linux TUN 实现 `crates/client/src/tun/linux.rs:141-166` 只有普通 `read`/`write`，没有 batch、GSO、GRO 或多队列接口。

需要修正一个容易夸大的点：TUN channel 名义上是 unbounded，但 reader 必须先从一个只有 64 个 buffer 的池中取得 buffer（`commands.rs:31-32`、`605-618`、`1312-1348`），所以出站 TUN 在途上限约为 64 包，并不是无限堆积。真正无容量上限的是 P2P 入站 channel；它与固定 4096 的 authenticated cache 组合后才形成后面的丢包悬崖。两者不能混为同一个队列问题。

### 9.2 中央事件循环一次只处理一个完整包

`crates/client/src/cli/commands.rs:716-801` 的 `tokio::select!` 同时接收：

- TUN 包
- P2P 入站包
- QUIC relay 包
- 信令事件
- 控制命令
- 心跳
- shutdown

分支随后 `await` 完整处理函数，例如：

```rust
Some(packet) = runtime.tun_packet_rx.recv() => {
    handle_tun_reader_packet(runtime, &packet).await;
    recycle_tun_buffer(runtime, packet);
}
```

在 `handle_tun_reader_packet` 返回前，主循环不能处理下一个数据包。虽然底层 reader 可以继续收包，但实际数据面消费者只有一个。这与 TCP 8 流没有提升吞吐的现象一致。

### 9.3 每个出站包克隆整个 peer 列表

`commands.rs:1402-1406`：

```rust
let peers = p2p.get_peers().await;
let is_peer_ip = match dst_ip {
    IpAddr::V6(v6) => peers.iter().any(|p| p.virtual_ip == v6),
    IpAddr::V4(v4) => peers.iter().any(|p| p.virtual_ips.ipv4 == Some(v4)),
};
```

`get_peers()`（`crates/client/src/p2p/connection.rs:623-631`）会对整个连接表做读锁并 clone 所有 `PeerInfo`。后续出口节点决策还继续使用这个克隆列表。

这里已经存在 `ip_index`，但热路径仍通过“复制 peer 集合再线性查找”做判断。peer 越多，成本越大。

### 9.4 每包多次获取连接状态和统计锁

出站链路中至少涉及：

- `ip_index` 读锁并 clone `String`
- `connections` 读锁并 clone `Arc<SecurePeerState>`
- `StatsCollector::record_p2p_tx` 获取 global 和 peers 两把写锁
- `record_peer_payload` 再获取 `connections` 写锁
- `SecurePeerState` 的每 peer async mutex

`record_peer_payload` 位于 `connection.rs:1442-1450`；`StatsCollector` 位于 `crates/client/src/stats.rs:117-180`。这些锁在控制面可接受，放进每包热路径会形成不必要的全局竞争。

### 9.5 单包加密和分配

`crates/client/src/p2p/secure_runtime.rs:835-851`：

- `SecurePeerState` 持有 `state: Mutex<SecureState>`（`secure_runtime.rs:180`）。
- 每包 `vec![0u8; packet_len]` 分配密文并复制 plaintext。
- 随后调用已存在的 `seal_in_place()`，但分配已经发生在兼容入口 `seal()` 内。

同一个 peer 的所有包都被单把 async mutex 串行化。per-peer nonce 顺序确实需要控制，但成熟实现采用“批量顺序分配 nonce + 并行加密 + 顺序发送”，而不是“每包加密期间锁住全部 peer 状态”。

### 9.6 每包单次 `send_to`

直连路径在 `connection.rs:553-556` 对每个包调一次 `self.socket.send_to(&data, ...)`；没有 `sendmmsg`、UDP GSO 或按 peer 聚合发送。

### 9.7 现有批量密码 API 未被数据面使用

`crates/secure-session/src/lib.rs` 已实现：

- `SendCipher::seal_batch`：`secure-session/src/lib.rs:397-440`
- `ReceiveCipher::open_batch_in_place`：`580-602`
- `EpochSession::seal_batch`：`705-724`
- `EpochSession::open_batch_in_place`：`732-752`

这些 API 支持预分配连续 buffer、先整体校验布局/counter，再做原地加解密，并声称调用方提供存储时稳态无逐包分配。但必须看清：`seal_batch` 内部仍是逐包 `for { self.seal(...) }`，`open_batch_in_place` 也仍是逐包调用 `open_in_place`，它们不是多核并行批处理。`rg` 还显示客户端数据面没有调用它们，只有 `secure-session` 内部测试/benchmark 使用。

这是一个强信号：项目已经具备一部分正确的底层能力，但上层数据面尚未接线；即使直接接线，也还需要另行设计 worker pool、nonce 批量分配和每 peer 顺序发送，才能得到 `wireguard-go` 的并行模型。

## 10. `fuckipv6` 入站路径逐阶段分析（固定快照）

### 10.1 UDP receiver 逐包接收并完整处理

`crates/client/src/p2p/connection.rs` 中的逐包 UDP receiver：

```rust
let mut buf = vec![0u8; 65536];
loop {
    match socket.recv_from(&mut buf).await {
        Ok((size, addr)) => {
            dispatch_udp_packet(...).await;
        }
    }
}
```

问题：

- 没有 `recvmmsg`/batch receive。
- `dispatch_udp_packet` 在下一包 `recv_from` 前完整 `await`。
- 所有包共享一个 64 KiB buffer，虽然避免了每包分配，但无法形成接收队列流水线。

### 10.2 入站数据先线性扫描所有连接

`connection.rs:1169-1240` 的 `dispatch_secure_data`：

```rust
let candidates = {
    let connections = connections.read().await;
    connections
        .iter()
        .filter(|(_, connection)| connection.secure.handles_data_packet(bytes))
        .map(|(client_id, connection)| {
            (client_id.clone(), connection.secure.clone(), src_ip)
        })
        .collect::<Vec<_>>()
};
```

每个 UDP 数据包都：

- 遍历所有连接。
- 对每个连接解析数据头并比较 receiver index。
- 对匹配项 clone client ID、`Arc` 和源 IP。
- 构造临时 `Vec`。

`SecurePeerState::handles_data_packet`（`secure_runtime.rs:986-989`）本身只是轻量 header 检查，但这层 `O(N)` 扫描在大 peer 数下会明显放大。

`wireguard-go` 使用 `indexTable.Lookup(receiver)` 直接 O(1) 查找，应该作为整改方向。

### 10.3 解密后在同一 Vec 内搬移明文，并把第二份密文入队

`connection.rs:1206-1232`：

```rust
match secure.open_from_source(addr, bytes.to_vec()).await {
    Ok(plaintext) => {
        let mut active_connections = connections.write().await;
        // ...revalidate connection, then update endpoint...
        authenticated_inbound.write().await.insert(
            &client_id,
            secure_cache_id(&secure),
            src_ip,
            bytes,
            plaintext,
        );
        drop(active_connections);
        let _ = incoming_tx.send((src_ip, bytes.to_vec()));
    }
}
```

这里至少发生：

1. `bytes.to_vec()`：ciphertext 复制给 `open_from_source`。
2. `extract_opened_plaintext()` 在同一个 `Vec` 内把明文搬到头部，然后截断（`secure_runtime.rs:1069-1075`）；没有第二份明文分配，但有内存搬移。
3. `bytes.to_vec()`：ciphertext 再复制进 incoming channel。
4. `authenticated_inbound.insert` 对 ciphertext 计算 BLAKE3。
5. 主循环 `decrypt_from_peer` 再次对 ciphertext 计算 BLAKE3 来 `take` 缓存（`connection.rs:48-95`、`470-484`）。

一个普通数据包因此经历不必要的多次复制和两次相同哈希。密码运算本身可能不是主因，但这些内存带宽和分配成本会叠加在每包关键路径上。

### 10.4 authenticated cache 设计绕过了 ownership

当前设计的目标可以理解：UDP receiver 已经完成 AEAD/replay 验证，不能把 ciphertext 再交给主循环二次 `open`，否则 replay window 会拒绝有效包。

实际做法是 receiver 把明文放进“`(源 IP, ciphertext BLAKE3)` -> plaintext”缓存，只把 `(源 IP, ciphertext)` 投递给主循环，主循环再用同一密文的 BLAKE3 找回明文。这条链路不仅产生哈希和复制，
还把两个容量恰好都是 4096 的机制拼成了一个丢包悬崖：

1. `AUTHENTICATED_INBOUND_CACHE_CAPACITY = 4096`（`connection.rs:27`），超限淘汰最老明文（`connection.rs:69-79`）。
2. replay window 也是 4096（`secure-session/src/lib.rs:54-55`）。
3. P2P 入站队列是无界 `mpsc`（`connection.rs:29-31`、`199`、`216`），所以 receiver 可以继续认证并把包排入队列。
4. 当积压达到第 4097 个包时，最老明文被 cache 淘汰，但对应密文仍在队列中。
5. 主循环执行 `authenticated_inbound.take(...)` 失败（`connection.rs:470-484`），随后调用 `open_from_session(payload.to_vec())` 尝试第二次解密（`connection.rs:490-493`）。
6. 该 counter 在第一次成功解密时已经由 `mark_authenticated` 推进 replay window（`secure-session/src/lib.rs:552-566`）。第二次检查会在 AEAD 前失败：窗口外是 `CounterTooOld`，窗口内已标记是 `ReplayDetected`（`secure-session/src/lib.rs:1005-1021`、`1030-1050`）。

因此这是**源码可证明的正确性/丢包缺陷**，不只是“缓存实现复杂”。在单会话连续流量、主循环持续落后 4097 包时，最老包通常已经落到 4096 窗口之外，首发错误应为 `CounterTooOld`；若触发条件使 counter 仍在窗口内，则表现为 `ReplayDetected`。现有基准没有记录这个错误码，所以不能给出它占 98% 丢包的精确比例，但该路径确实存在。

调大 cache 不是修复：它只会把悬崖向后推，并增加明文常驻内存；消除 queue/cache 双通道才是。正确结构是让 UDP receiver 把“已认证的 owned plaintext + peer/source 元数据”直接交给后续队列，replay window 是唯一的认证状态来源，后续阶段不再解密、不再用密文哈希找回明文。

当前缓存还额外造成：

- 每包哈希。
- 全局 `RwLock`。
- plaintext 额外存储。
- 取出时再次哈希。
- ciphertext 和 plaintext 同时持有。
- cache 与 replay window 生命周期耦合错误，形成确定性丢包。

### 10.5 每包无条件获取 endpoint 写锁

`connection.rs:1411-1430`：

```rust
let mut connections = connections.write().await;
...
if conn.active_endpoint != Some(endpoint) {
    conn.active_endpoint = Some(endpoint);
}
```

即便 endpoint 没有变化，也先获取全局连接表写锁。正确做法可以是：

- 先用读锁/原子快照快速判断是否一致。
- 仅在确实迁移 endpoint 时获取写锁。
- 把 endpoint 状态从 peer 元数据表中拆出，使用独立的每 peer 同步原语。

### 10.6 主循环还要做 peer 查询和全量 clone

`commands.rs:1054-1120`：

- `decrypt_from_peer` 取缓存明文。
- `record_p2p_rx` 获取两把统计写锁。
- `peer_by_ip` 查一次。
- `get_peers()` 又 clone 全部 peer。
- 再做源地址校验和单包 TUN 写入。

入站并未真正做到“UDP 接收完成即可快速交给并行解密 worker”；相反，包在多个任务之间往返，且每个阶段都重新查找或克隆状态。

### 10.7 固定快照状态核对

| 检查项 | `39ca637` 固定快照状态 | 证据 |
|---|---|---|
| authenticated cache + 二次 open | **仍存在**：4096 项明文 cache、无界 ciphertext queue、cache miss 后再次 `open_from_session` | `connection.rs:27-95`、`199`、`478-493`、`1224-1232` |
| 64B 包清零 64 KiB | **仍存在**：reader 每包 `resize(65536, 0)`，回收先 `clear()` | `commands.rs:1049`、`1324` |
| 出站分配式 seal | **仍在生产路径**：`send_to()` 调用返回新 `Vec` 的 `seal()` | `connection.rs:499-531`、`secure_runtime.rs:835-851` |
| 底层原地 seal 原语 | **已提交并有局部测试**：driver/session/epoch/rekey API 存在 | `driver.rs:209-222`、`lib.rs:355-385`、`683-698`、`rekey_session.rs:171-181`、`tests/driver.rs:182-241` |
| fallback 重复 seal | **仍存在**：直连、QUIC relay、signaling 各自调用 seal | `connection.rs:499-531`、`commands.rs:1640-1646`、`1680-1687` |
| 慢 fallback 阻塞中央循环 | **仍存在**：中央 `select!` 直接 await TUN 处理，relay timeout 2 秒 | `commands.rs:33`、`723`、`1647-1669` |
| 每包 clone 全部 `PeerInfo` | **仍存在**：出站 `get_peers()`；入站 `peer_by_ip()` + `get_peers()` | `commands.rs:1081-1085`、`1402-1406`、`connection.rs:610-631` |
| 入站全 connections 扫描 | **仍存在**：`dispatch_secure_data()` 线性遍历全部连接 | `connection.rs:1169-1190` |
| pair-scoped receiver index | **仅派生原语存在**：派生函数和单测已提交，生产分派未接入 | `secure_runtime.rs:1182-1208`、`1270-1289` |
| 全局/peer 统计写锁 | **仍存在**：`RwLock<GlobalStats>` 和 peer map | `stats.rs:49-51`、`131-180` |
| endpoint 无条件写锁 | **仍存在**：`update_peer_endpoint()` 先获取 `connections.write()` | `connection.rs:1411-1430` |
| per-peer health 写锁 | **仍存在**：`PeerHealth::record_payload(&mut self, ...)` 经 connections 写锁调用 | `health.rs:33-110`、`connection.rs:1442-1450` |

固定快照的真实结构是：

```text
出站 TUN read（每包 resize 64 KiB）
  -> 中央 select! 串行 await
  -> get_peers() clone + 线性路由
  -> per-peer mutex + seal() 新分配 Vec
  -> QUIC/UDP；失败后 relay/signaling 可能再次 seal

UDP recv（共享 buffer）
  -> 线性扫描全部 connections / receiver index hint
  -> bytes.to_vec() + open_from_session
  -> AuthenticatedInboundCache + 另一份 ciphertext queue
  -> 中央循环 BLAKE3 找回 plaintext
  -> ACL / peer_by_ip / get_peers / 单包 TUN write
```

因此可确认的是：固定快照包含一些安全/性能相关的底层原语和局部测试；不能确认这些原语已经进入生产数据面，也不能确认吞吐、尾延迟、安全性或加密等级与 WireGuard 对等或超越。

## 11. 根因排序（固定快照）

以下按对固定快照吞吐/丢包的预期影响排序，但精确比例仍需 profiler 验证。

### P0：入站 ownership 错误与 4096/replay 丢包悬崖

这是唯一一条同时破坏正确性和性能的主线：

- receiver 已拥有明文，却把它放进旁路 cache，再把密文排入另一个无界队列。
- cache 只有 4096，replay window 也只有 4096；超限后主循环二次解密必然被 replay 拒绝。
- 每包额外产生两份 owned ciphertext（解密输入和队列副本）、一次同 `Vec` 内明文搬移和两次 BLAKE3。
- 两条队列/cache 的生命周期无法形成可靠背压。

修复优先级应高于任何 AEAD、batch 或 GSO 优化。`wireguard-go` 的 inbound element 在解密后直接交给 per-peer sequential receiver，不存在“已认证明文 -> 密文哈希 -> 二次解密”的反向传递（`device/receive.go:168-185`、`238-265`、`429-535`）。

### P0：中央循环可被慢 fallback 阻塞

中央 `select!` 选中 TUN 分支后，会一直 `await` 到完整的 `handle_tun_reader_packet` 返回（`commands.rs:721-730`）。该调用在 P2P 失败后可能进入 QUIC relay，后者有 2 秒 timeout（`commands.rs:33`、`1648-1669`）。因此一次慢 relay/fallback 就能让 TUN、UDP、信令和控制事件全部停摆。

### P0/P1：fallback 重复加密，且路径未被基准确认

P2P `send_to` 先 seal 一次（`connection.rs:499-533`），UDP 发送失败后 QUIC relay 再 seal（`commands.rs:1641-1647`），QUIC 失败/超时后 signaling 再 seal（`commands.rs:1681-1687`）。同一明文最多消耗三个 nonce。基准脚本又只检查 ping 可达，没有断言实际走 UDP P2P、QUIC relay 还是 signaling relay（`scripts/bench-dataplane.sh:337-376`、`592-639`），所以历史倍数既混有路径差异，也没有排除重复 seal 和 2 秒阻塞。

### P1：中央单消费者与单包 syscall

中央事件循环一次只推进一个完整数据包；TUN/UDP 都是逐包 `read`/`recv_from`/`send_to`。即使每包都很快，这个消费者本身也限制多核扩展，并解释 8 条 TCP 流为何没有带来吞吐提升。`wireguard-go` 采用 128 包接口、批量 UDP、TUN 批量读写和 CPU 数 worker（`conn/conn.go:18-20`、`device/device.go:309-318`、`device/send.go:206-315`、`device/receive.go:72-235`）。

### P1：每包复制、分配和重复哈希

- 出站 seal 每包新分配 `Vec`（`secure_runtime.rs:835-851`）。
- 入站正常包产生两份 owned ciphertext `Vec`（解密输入和入队副本）、一次同 `Vec` 内明文搬移、两次同一密文 BLAKE3，并额外做 3 次 cache key 的 `client_id` 字符串分配。
- TUN reader 每包 `resize(65536, 0)`；64B payload 会被约 64 KiB 清零成本主导（`commands.rs:1324-1329`）。
- `wireguard-go` 使用 pooled buffer/element，接收后原地解密，按批写 TUN（`device/pools.go:49-121`、`device/receive.go:429-535`）。

这些是每包固定成本；在高 pps 和小包场景会被线性放大。

### P1：每包克隆控制面对象并拿多把写锁

- 出站每包 `get_peers()` 克隆全部 `PeerInfo`（`commands.rs:1402-1406`、`connection.rs:623-630`）。
- 入站先 `peer_by_ip()` 克隆一个完整 `PeerInfo`，再 `get_peers()` 克隆全部（`commands.rs:1081-1085`、`connection.rs:610-620`）。
- `PeerInfo` 含 `String`、公钥 `Vec<u8>` 和 endpoint `Vec<SocketAddr>`（`crates/common/src/types.rs:5`、`81-102`），不是纯拷贝快路径。
- `PeerHealth` 与 `StatsCollector` 两套统计都逐包更新，涉及 global/peer 两把写锁；`record_peer_payload` 和 endpoint 更新还会分别获取 `connections.write()`（`connection.rs:1411-1450`、`stats.rs:131-180`）。

在没有高争用时，单次 `tokio::RwLock` 成本也可能看似不大；但所有这些锁、分配和调度点都在同一个逐包消费者上叠加，形成缓存未命中、尾延迟和固定 CPU 预算。

### P1：密码学 state 每包串行，batch API 也仍串行

`SecurePeerState` 一把 async mutex 覆盖会话状态和 driver（`secure_runtime.rs:167-181`、`835-851`、`992-1075`）。客户端没有 CPU 数 worker；现有 `seal_batch` 和 `open_batch_in_place` 内部也只是串行 `for` 循环（`secure-session/src/lib.rs:397-440`、`550-558`、`672-680`）。`wireguard-go` 则先按 peer 固定 nonce 顺序，再把 batch 交给 CPU 数 encryption/decryption workers（`device/send.go:434-475`、`device/device.go:309-318`）。

### P1/协议：pair-scoped receiver index 只有派生函数和单测

`secure_runtime.rs:1182-1208` 新增了 pair-scoped、role-separated 的 deterministic receiver-index 派生，`secure_runtime.rs:1270-1289` 的单个单元测试验证了同 pair 稳定、收发角色不同、同一本端换 peer 后 index 不同。这只能证明派生函数的局部性质：

- 该 index 仍由双方公钥和角色确定；同一 pair 的重握手/rekey 复用同一 index，并非 `wireguard-go` 的随机 session-scoped index。
- 固定快照没有 `receiver_index -> SecurePeerState` 的生产分派表；`connection.rs:1169-1243` 的 `dispatch_secure_data()` 仍线性扫描全部 connections。
- 没有 100-peer、NAT rebind、碰撞桶或重握手轮换测试，也没有吞吐证据。

因此不能把该原语写成“静态共享桶已消除”“入站 O(1) 已完成”或“生产 receiver index 已整改”。32-bit 碰撞、会话级隔离和 rekey 生命周期仍是未实现/未验证边界。

### P2：缺少 Linux offload

没有 TUN GSO/GRO、UDP GSO/GRO，也没有 TUN multi-queue。对小包，这会显著提高每字节 syscall 和协议栈开销。

## 12. 密码学是否是主因

### 12.1 现有密码基准

`benchmarks/data/2026-10-30-v2-crypto-baseline/README.md`：

- 1420B 单包 `seal`：约 `2.80 µs`
- 1420B 单包 `open`：约 `3.06 µs`
- 批量 fast path：约 `486-507 MiB/s`
- 已认证重复包在 AEAD 前被 replay window 拒绝，约 `4 ns`

该基准自身也受高负载影响，且使用 `--quick`，不能作为发布门禁；但它的数量级仍能说明问题。

### 12.2 与端到端差距的数量级对照

`0.0551 Gbit/s` 的 1420B 包吞吐约为：

```text
0.0551e9 / 8 / 1420 ≈ 4,850 packets/s
```

对应每包约 `206 µs` 的单消费者预算。单包 seal 约 `2.8 µs`，即密码原语只占这个粗略预算的一小部分。即使考虑双向、队列和内存访问，也不足以解释约 48 倍的差距。

这里不应声称“密码只占 1.4%”；TCP 包长、双工、并发和 profiler 缺失都会影响精确比例。可靠的结论是：

> **密码原语不是当前数量级差距的首要解释；调度、syscall、复制、锁和队列结构的上限更低。**

### 12.3 批量实现已经存在但没接入

`seal_batch`/`open_batch_in_place` 的存在意味着底层密码学实现已经考虑到：

- 连续预分配 buffer。
- 调用前整体校验长度、布局、epoch 和 counter 余量。
- 原地 open，以及调用方提供输出 buffer 的 seal。
- 稳态无逐包分配（前提是调用方正确池化）。

但源码明确显示它们仍是串行循环：`SendCipher::seal_batch` 逐个调用 `seal`（`secure-session/src/lib.rs:397-440`），`ReceiveCipher::open_batch_in_place` 逐个调用 `open_in_place`（`550-558`），`EpochSession::open_batch_in_place` 也是逐包调用（`672-680`）。因此接入 batch 只会降低调用边界、布局和分配成本，**不会自动获得多核并行**。

当前瓶颈不是“要重写密码原语”，而是先把数据面改成正确的 owned buffer/peer 元数据传递，再在 batch 之上设计 nonce 顺序分配、worker pool 和 per-peer 顺序发送。

## 13. 公平性说明

必须承认以下不对称：

- Linux 内核 WireGuard 与内核网络栈集成，具有天然优势。
- `wireguard-go` 是运行多年的成熟用户态实现，批处理、池化、GSO/GRO 和队列语义经过大量优化。
- `fuckipv6` 多了协调器、信令、NAT 检测、QUIC relay、路由、出口节点、Web/UI/IPC、DHT/overlay 等功能。
- 当前协议和安全层并非 WireGuard 的复刻；两者安全模型、握手和产品目标不同。
- 历史基准的 `wireguard-go` 二进制版本早于本次 clone 的最新源码，但相关数据面结构多年稳定。
- 历史基准的 `fuckipv6` 构建提交为 `b49be00`，本报告的固定分析快照为 `39ca637`；两者不同，且没有同 transport、同 commit 的新测量，因此历史倍数不能直接归因给固定快照。

因此，本报告比较的是 **数据面工程设计成熟度**，不是“同等功能、同等安全模型下所有代码的绝对优劣”。

## 14. 分阶段整改建议

### P0：先让基准可复现，并能证明实际路径

在有条件时使用空闲机器、固定 CPU、冷/热 cache 策略一致、至少 5 次运行并 omit 前 2 次：

```bash
sudo env WG_GO_BIN=/path/to/wireguard-go \
  scripts/bench-dataplane.sh \
  --impls all --runs 5 --duration 10 --omit 2 \
  --client-cpus 2 --server-cpus 3 --control-cpus 0 \
  --output /tmp/fuckipv6-bench-clean
```

同时记录：

- 固定 commit、使用的补丁/二进制 SHA256；若为临时实验，记录补丁内容哈希，但不从活动工作树生成文档主结果。
- 每个用例实际使用的 transport/endpoint：直连 UDP、QUIC relay、signaling relay 或本地路由。
- fallback 次数、`CounterTooOld`/`ReplayDetected` 次数、QUIC timeout 次数。
- 每阶段 packets/s。
- TUN/UDP syscall 数。
- 队列深度和丢包位置。
- 每 task CPU 使用率。
- 每包 allocation/copy 数。
- 发送/接收端 `perf stat` 与火焰图。

验收原则：先证明测试本身可重复，再讨论优化收益；不要继续基于高负载、无 pinning、3 次运行的绝对倍数做发布判断。

基准脚本当前只 ping 通就开跑，不能证明三种实现比较的是同一条数据路径。正式重测至少应让 agent 暴露当前 `active_endpoint`/transport，或在测试网络里显式禁掉 relay fallback；如果无法断言，就必须把 fallback 次数和丢包错误码一起报告。

固定快照执行状态（2026-10-31）：

- [ ] 入站 owned plaintext、删除 authenticated cache/二次 open、有界入站背压：固定快照中均未完成。
- [ ] TUN 有效长度复用、生产出站原地 seal、fallback 单次 seal、慢 fallback 独立队列：固定快照中均未完成；只能确认底层 `seal_in_place` 原语和局部测试存在。
- [ ] 出站轻量 IP 路由、入站避免完整 `PeerInfo` clone、endpoint read fast path、原子统计、payload health 原子记账：固定快照中均未完成。
- [ ] pair-scoped receiver index 的生产分派：固定快照中未接线；只有派生函数和一个局部单测。
- [ ] 入站队列背压、共享 UDP RX buffer ownership、完整 buffer pool、真实 batch、CPU worker、GSO/GRO 和干净基准：固定快照中均未完成。

### P0：修复入站 ownership 和确定性丢包

这些修改优先于性能微调：

1. 让已认证入站队列直接携带 `owned plaintext + peer_id/内层源 IP/outer source/metadata`，后续阶段不再接收 ciphertext。
2. 删除 `AuthenticatedInboundCache` 以及两次 BLAKE3；replay window 只由真正解密成功的那一次更新。
3. 给入站队列建立有界背压或明确丢弃策略，不能一边无限收包、一边只保留 4096 个已认证结果。
4. 为“积压超过 4096”增加回归测试，断言有效包不会因第二次 replay 检查被丢弃。
5. `incoming_rx` 只有一个消费者，不需要 `RwLock`；让事件循环直接持有 receiver。
6. 将 relay 发送移出中央数据面 `select!`，避免 2 秒 relay timeout 阻塞 TUN/UDP 推进。
7. fallback 改为“先选路径再 seal”或“seal 一次后复用同一密文”；当前最多三次 seal 且消耗三个 nonce，不应作为稳态路径。

验收标准不是“吞吐提高了一点”，而是：受控流量下不再出现由 authenticated cache 淘汰触发的 `CounterTooOld`/`ReplayDetected`，relay 慢时 TUN/UDP 仍能继续推进。

### P1：低风险热路径清理

以下生产项在固定快照 `39ca637` 中均未完成，不能勾选：

1. [ ] endpoint 未变化时不取 `connections.write()`；`connection.rs:1411-1430` 仍先拿写锁。
2. [ ] 统计改为原子计数器或分片计数器；`stats.rs:49-51`、`131-180` 仍使用 `RwLock`。
3. [ ] payload health 删除逐包写锁；`health.rs:33-110`、`connection.rs:1442-1450` 仍是可变引用加连接表写锁。
4. [ ] 建立生产 `receiver_index -> SecurePeerState` 分派；固定快照仍由 `connection.rs:1169-1243` 线性扫描。
5. [x] **仅限底层原语**：`secure_runtime.rs:1182-1208` 的 pair-scoped index 派生函数存在，`secure_runtime.rs:1270-1289` 有局部单测；这不代表生产索引或 O(1) 分派已完成。
6. [ ] 出站直接查 `ip_index`/peer 索引并删除每包 `get_peers()` clone；`commands.rs:1402-1406`、`connection.rs:623-630` 仍克隆全部 peer。
7. [ ] 已认证明文直接携带元数据进入后续阶段；固定快照仍写 `AuthenticatedInboundCache` 并发送第二份 ciphertext。
8. [ ] 出站 TUN 在生产池化 allocation 上原地 seal；固定快照仍调用分配式 `seal()`（`secure_runtime.rs:835-851`）。
9. [ ] 避免 TUN 每包 `resize(65536, 0)`；固定快照仍执行该清零（`commands.rs:1324`）。
10. [ ] 消除共享 UDP RX buffer 的 `bytes.to_vec()`，并用可变 pooled receive ownership 原地 open。

**底层原语不等于生产接线，功能存在不等于吞吐改善。** 当前仍需同一 transport、同一 commit 口径的前后基准；固定快照没有这样的结果。

### P2：批处理和池化

1. TUN 引入 batch read/write API。
2. UDP 引入 batch receive/send（Linux `recvmmsg`/`sendmmsg` 或等价机制）。
3. 定义固定大小、预分配的 packet pool，稳态不逐包分配。
4. 接入现有 `seal_batch` / `open_batch_in_place`，但不要把“batch API”误认为“并行”；其内部目前仍是串行循环。
5. 按 peer 聚合出站包；在每 peer 顺序分配 nonce 后，把独立 batch 交给并行 worker 加密。
6. 入站按 peer 聚合后并行 open；replay 检查和 per-peer 顺序消费必须在明确的 ownership 协议下完成。
7. 将 TUN RX、TUN TX、UDP RX、UDP TX 拆成独立队列任务，避免中央 `select!` 成为唯一消费者。

### P3：并行数据面和 offload

1. 加密、解密、握手 worker 按 CPU 数创建。
2. per-peer inbound/outbound queue，保留必要的每 peer 顺序。
3. TUN multi-queue。
4. Linux TUN GSO/GRO。
5. UDP GSO/GRO，并对不支持的情况自动回退。
6. 压测不同 peer 数和包长，确认扩展性从“单核串行”变为“多核近似线性”。

### 安全约束不能牺牲

任何优化都必须继续保证：

- AEAD 成功后才允许 endpoint migration。
- replay window 在正确阶段更新，不能为了批量并行而放宽重放语义。
- nonce 分配和发送顺序保持每 peer 一致。
- 解密后仍校验内层源地址、ACL 和 peer 权限。
- 批量失败时正确回滚/丢弃，不能把未认证数据送入 TUN。

## 15. 建议的验收标准

在干净基准上，按以下顺序验收，而不是一次性追求内核 WireGuard：

1. **可重复性**：同一 commit 多次运行的中位数波动可接受，`wireguard-go` 不再出现 0 吞吐结果。
2. **无崩溃、无异常丢包**：受控 50 Mbit/s UDP 和 5 Mbit/s 小包测试达到 0% 应用层丢包。
3. **单流绝对提升**：TCP 单流先以 `>= 0.5 Gbit/s` 作为阶段性工程门槛，再逐步接近 `wireguard-go`。
4. **多流扩展**：8 流吞吐应显著高于 1 流，证明全局串行瓶颈已解除。
5. **批量/锁指标**：稳态下每包 allocation 数接近 0；统计和 endpoint 快路径不再触发全局写锁。
6. **CPU 利用**：加密/解密 worker 能在多核并行；不是单个 tokio task 占满一个核而其他核空闲。
7. **路径可证明**：每次结果都记录 `active_endpoint`/transport、fallback 次数和 replay 错误次数；性能比较只使用实际 transport 相同且无异常 fallback 的样本。

这些是建议目标，不是对当前未测版本的事实陈述。

## 16. 源码索引

> 下表的 `fuckipv6` 行号只对应 `39ca637` 的 `git archive` 快照；`wireguard-go` 行号对应 `b3bc704`。固定快照中的生产整改仍未完成。

### `fuckipv6`

| 主题 | 固定快照位置 |
|---|---|
| 中央单包事件循环与直接 await | `crates/client/src/cli/commands.rs:715-723` |
| 出站 TUN 包处理 | `crates/client/src/cli/commands.rs:1020-1052`、`1387-1473` |
| 入站包处理 | `crates/client/src/cli/commands.rs:1054-1120` |
| TUN 逐包 reader 与 64 KiB 清零 | `crates/client/src/cli/commands.rs:1312-1348` |
| TUN buffer pool（64 × 64 KiB，出站在途上限） | `crates/client/src/cli/commands.rs:31-32`、`605-618` |
| Linux TUN 单包 read/write | `crates/client/src/tun/linux.rs:141-166` |
| P2P engine 结构 | `crates/client/src/p2p/connection.rs:131-149` |
| authenticated cache 与无界入站 channel | `crates/client/src/p2p/connection.rs:27-95`、`199`、`216` |
| UDP 接收入口与逐包分派 | `crates/client/src/p2p/connection.rs:1007-1042`、`1044-1167` |
| 入站全 connections 扫描、复制与缓存 | `crates/client/src/p2p/connection.rs:1169-1243` |
| cache miss 后二次解密 | `crates/client/src/p2p/connection.rs:447-495` |
| replay window 与第二次拒绝 | `crates/secure-session/src/lib.rs:54-55`、`552-566`、`1005-1021`、`1030-1050` |
| 出站分配式 seal 与底层原地 seal | `crates/client/src/p2p/secure_runtime.rs:835-851`、`855-885` |
| 固定快照已有 driver/session 原地 seal 原语及测试 | `crates/secure-session/src/driver.rs:209-222`、`crates/secure-session/src/lib.rs:355-385`、`683-698`、`crates/secure-session/src/rekey_session.rs:171-181`、`crates/secure-session/tests/driver.rs:182-241` |
| pair-scoped index 派生与局部单测 | `crates/client/src/p2p/secure_runtime.rs:1182-1208`、`1270-1289` |
| 无条件下 endpoint 写锁 | `crates/client/src/p2p/connection.rs:1411-1430` |
| 每包 peer payload 写锁 | `crates/client/src/p2p/connection.rs:1442-1450` |
| 每 peer state mutex 与接收明文搬移 | `crates/client/src/p2p/secure_runtime.rs:167-181`、`992-1075` |
| 统计全局写锁 | `crates/client/src/stats.rs:49-51`、`117-180` |
| QUIC relay 2 秒 timeout | `crates/client/src/cli/commands.rs:33`、`1647-1669` |
| fallback 重复 seal | `crates/client/src/p2p/connection.rs:499-533`、`crates/client/src/cli/commands.rs:1640-1646`、`1680-1687` |
| 批量密码 API（内部逐包串行） | `crates/secure-session/src/lib.rs:397-440`、`580-602`、`705-752` |
| 基准未断言实际 transport | `scripts/bench-dataplane.sh:337-376`、`592-639` |

### `wireguard-go`

| 主题 | 位置 |
|---|---|
| 出站流水线定义 | `device/send.go:23-29` |
| TUN 批量读取/路由/分组 | `device/send.go:206-315` |
| staged packet 与 nonce 顺序 | `device/send.go:318-402` |
| 每 CPU 加密 worker | `device/send.go:434-475` |
| per-peer 批量发送 | `device/send.go:477-541` |
| 批量 UDP receive | `device/receive.go:72-235` |
| 每 CPU 解密 worker | `device/receive.go:238-267` |
| per-peer 顺序接收/TUN 批量写 | `device/receive.go:429-535` |
| CPU worker 初始化 | `device/device.go:309-318` |
| device batch size | `device/device.go:328-339` |
| buffer/element 池 | `device/pools.go:12-121` |
| 128 batch 常量 | `conn/conn.go:18-20` |
| UDP batch receive | `conn/bind_std.go:218-299` |
| UDP batch send/GSO | `conn/bind_std.go:341-434` |
| TUN GSO/GRO write | `tun/tun_linux.go:335-369` |
| TUN GSO read | `tun/tun_linux.go:372-474` |
| TUN offload 探测 | `tun/tun_linux.go:501-547` |

## 17. 最终判断

### “性能差这么多吗？”

- **历史答案：旧实现确实有确定性架构缺陷，但不能用现有数据把差距精确写成 48 倍或内核的 3.7%。** 入站 cache/replay 悬崖、逐包 ownership 错误、全量 `PeerInfo` clone、64B 包每轮清零 64 KiB、热路径写锁、中央单消费者、fallback 重复加密和最多 2 秒阻塞，解释了旧基准为何先卡在每包固定成本上。
- **固定快照答案：上述正确性和调度缺陷仍存在于生产路径，不能写成已完成整改。** TUN 每包清零、分配式 seal、authenticated cache/二次 open、fallback 重复 seal、中央循环阻塞、逐包锁和线性 peer 扫描都仍可从 `39ca637` 源码直接确认；底层 `seal_in_place` 与 pair-scoped index 派生尚未接入对应生产路径。固定快照也没有同一 transport、同一 commit 口径的新吞吐/尾延迟数据。
- **精确答案仍待实测：** 下一轮必须记录并断言 active endpoint/transport，保存 fallback/replay 计数，再决定差距是一个数量级还是更小。

### “我们的代码这么烂吗？”

更准确的评价是：

> **不能因为总代码量大就评价“烂”；但数据面确实存在具体、可定位、可验收的设计错误，不是单纯“还没优化好”。**

固定快照中可以确认的基础是：

- secure-session/driver 已有原地 seal 原语、短缓冲和 tamper/replay 局部测试。
- 固定 TUN buffer pool 和认证成功后再迁移 endpoint 的逻辑存在，但前者仍每包清零，后者仍走连接表写锁。
- pair-scoped receiver-index 派生函数及一个局部单测存在。
- 控制面和产品能力较完整。

固定快照的未完成项必须与“底层 API 存在”分开记录：

- [x] **仅限底层原语**：driver/session/epoch/rekey `seal_in_place` 存在，`tests/driver.rs:182-241` 覆盖短缓冲、tamper/replay 不污染会话；没有生产 TUN/P2P 接线或端到端性能证据。
- [x] **仅限派生函数**：pair-scoped receiver-index 派生和角色/稳定性单测存在；没有生产索引表、100-peer/NAT rebind 测试或 O(1) 分派。
- [ ] 已认证明文直接交给后续阶段，删除 cache/replay 双状态。
- [ ] relay fallback 复用已 seal 密文，并把慢发送移出中央循环。
- [ ] TUN allocation 预留 header/tag，生产路径原地 seal；direct/QUIC/relay/signaling 共享同一密文。
- [ ] 生产 receiver index 分派、不可变 peer 快照和避免逐包全量 clone。
- [ ] 统计、endpoint 和 payload health 从数据面主锁中拆出。
- [ ] 真正的批处理、worker pool、per-peer 顺序队列、完整 buffer pool 和 GSO/GRO。
- [ ] 路径可证明的干净基准与 profiler 归因。

优先级建议：

```text
先用固定 commit + 实际 transport + replay/fallback 计数建立可复现基准
  -> 待完成：owned plaintext，删除 4096 cache/replay 悬崖
  -> 待完成：relay 发送移出中央循环，fallback 不重复 seal
  -> 待完成：去全量 clone、生产 receiver-index 分派、全局统计写锁、payload health 写锁
  -> 待完成：出站 TUN 原地 seal，fallback 复用同一密文
  -> 待完成：UDP RX ownership、入站背压和会话热路径锁
  -> 待完成：真 batch + pool + 独立队列
  -> P3 crypto worker + per-peer queue + GSO/GRO
  -> 安全回归和干净基准验收
```

下一步应基于固定快照和可复现基准定位剩余瓶颈，而不是把底层原语或局部单测当作生产整改、性能提升、加密等级或安全等级已经达到某个结论。
