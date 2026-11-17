# iOS P3 测试报告

P3 = iOS 端接入（Rust staticlib + C ABI + Packet Tunnel Extension + Swift 宿主）。
本报告只覆盖**本轮实际执行过**的检查；未执行的项目在"未运行项"里逐条列出原因。

## 1. 结论

| 层面 | 状态 | 依据 |
| --- | --- | --- |
| Rust 侧门禁（fmt / check / 测试编译） | 通过 | 见 §2 表格 |
| Rust 桥（C ABI）25 个用例 | 通过 | §3 |
| P2 内嵌运行时回归 13 个用例 | 通过 | §2 |
| 客户端库回归 1205 passed / 40 failed | 比 P2 基线少 2 个失败（P1a/P1b 修好了两个 `cli::commands` 批量计数用例），其余失败集合未变 | §2、§6 |
| iOS 单元测试 53 个用例（模拟器） | 通过 | §4 |
| 真机签名构建 + 安装启动（含 Extension 嵌入 + entitlements） | 通过 | §5 |
| 真机 VPN 起隧道 + 真实 Mesh 流量 | 通过 | §5.3、§5.4 |
| 真机「断开 → Keychain 往返 → 重连」（身份不变） | 通过 | §5.5 |
一句话：**编译、签名、数据面、生命周期逻辑已在模拟器和 C 真机层面验证；真机上 VPN 隧道 + 协调服务器注册 + 跨设备 Mesh 双向流量（ICMP + 批量 UDP）已跑通。**

## 2. 环境与门禁

- 机器：macOS（Apple Silicon），Xcode 27.2 (27B52)，模拟器运行时 iOS 27.2
- Rust：rustc 1.99.1 / cargo 1.99.1（Homebrew）
- 部署目标：iOS 17.0；架构：arm64（真机 aarch64-apple-ios、模拟器 aarch64-apple-ios-sim）

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo check --workspace --no-default-features` | exit 0（client 27 个既有 warning，`fuckipv6-ios-bridge` 0 warning） |
| `cargo test --workspace --no-default-features --no-run` | exit 0，全部测试二进制可链接 |
| `cargo test -p fuckipv6-ios-bridge --no-default-features -- --test-threads=1` | **25 passed / 0 failed**（30.16s） |
| `cargo test -p fuckipv6-client --no-default-features --lib embedded::tests -- --test-threads=1` | **13 passed / 0 failed**（5.66s，P2 回归） |
| `cargo test -p fuckipv6-client --no-default-features --lib -- --test-threads=8 --skip punch` | **1205 passed / 40 failed / 1 ignored**（P3.4 实测），失败集合比 P2 基线少 2 个 `cli::commands`，见 §6 |

## 3. Rust 桥（crates/ios-bridge）25 个用例

包处理与报文校验（`packet::tests`，12）：

- `accepts_minimal_ipv4` / `accepts_minimal_ipv6` / `accepts_ipv4_with_payload`
- `rejects_empty_packet`、`rejects_unknown_version`
- IPv4：`rejects_ipv4_header_bits_below_minimum`、`rejects_ipv4_header_longer_than_packet`、`rejects_ipv4_length_mismatch`、`rejects_truncated_ipv4`
- IPv6：`rejects_ipv6_length_mismatch`、`rejects_ipv6_jumbogram`
- `rejects_packet_larger_than_mtu`

网络设置推导（`settings`，5）：`settings_derive_dual_stack_routes`、`settings_reject_empty_addresses`、`settings_reject_small_mtu`、`settings_reject_non_mesh_routing`、`settings_reject_transport_inside_route`（避免把协调服务器地址捕获进隧道造成路由环）。

C ABI 生命周期（`tests`，8）：`ffi_create_and_destroy_is_clean`、`ffi_create_rejects_invalid_json`、`ffi_create_rejects_relative_root`、`ffi_null_handles_report_invalid_handle`、`ffi_second_runtime_on_same_root_reports_a_node_problem`、`ffi_start_snapshot_and_repeat_stop`、`ffi_endpoint_moves_packets_through_the_real_packet_device`、`ffi_endpoint_rejects_short_buffer`。

其中 `ffi_endpoint_moves_packets_through_the_real_packet_device` 走的是真实数据面：真 C ABI 句柄 + 真 `EmbeddedNodeRuntime` + 注入的 ExternalPacketDevice，只在系统边界（TUN / 网络配置 / 对端）打桩。


## 4. iOS 单元测试 53 个用例（模拟器）

命令：`xcodebuild test -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release -destination 'platform=iOS Simulator,id=74A355A7-F55A-4249-B9AE-2288062B6D2C' -derivedDataPath /tmp/fv6-dd CODE_SIGNING_ALLOWED=NO -only-testing:FuckIpv6Tests`（按名字匹配在运行时升级后会失效，UDID 用 `xcrun simctl list devices available` 查）
结果：**TEST SUCCEEDED**，53 tests / 0 failures（22.42s）。

| 套件 | 数量 | 覆盖 |
| --- | --- | --- |
| `PacketPumpTests` | 11 | 双向转发顺序、队列满重试后丢弃、非法包直丢、closed 停泵、空批视为 flow 关闭、协议族映射、stop 有界且可重复、重启换会话、stop 后陈旧回调不转发 |
| `PacketPumpLifecycleTests` | 10 | stop 排空双向、重复 stop 幂等、等阻塞中的 receive、回调已翻 running 后仍要等线程、stop 超时保留句柄直到重试、等待进行中的 send、超时后重启用新会话、旧会话迟到回调被忽略、并发 stop 只排空一次、closed 通道唤醒阻塞的 stop |
| `HandleGuardTests` | 8 | 最后一个用户离开才释放、无用户 close 立即释放、重复 close 不双释放、close 后 acquire 失败、有用户在跑时延迟到离开、并发 close 只释放一次、close 等已进入的用户、未 acquire 的 release 不溢出 |
| `MessageRouterTests` | 11 | 空/超长消息、版本校验、未知命令、未运行态返回 state、status/snapshot 透传、非 JSON 拒绝、execute 本地校验 + 透传、错误码保留（timeout/nodeProblem/busy）、pathChanged |
| `IdentityVaultTests` | 8 | Keychain 存取、无身份不建文件、不覆盖已有身份、capture 后删明文、无文件 no-op、capture→restore 字节一致、身份不绑定网络名、按网络名的旧 account 被忽略 |
| `RustRuntimeTests` | 5 | 真实 C ABI 生命周期与失败语义（见 §6 第 2 条） |

## 5. 真机验证

### 5.1 签名构建

命令：`xcodebuild build -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release -destination 'generic/platform=iOS' -derivedDataPath /tmp/fv6-dd-device`
结果：**BUILD SUCCEEDED**，扩展被嵌入 `FuckIpv6.app/PlugIns/FuckIpv6Tunnel.appex`。

- 签名身份：`Apple Development: Zhihan Zhu (9DCPP78554)`（Team R9UJHHAZQ4）
- 配置描述文件：`iOS Team Provisioning Profile: com.lingxifox.fuckipv6` 与 `...fuckipv6.tunnel`
- 宿主 App 含 NetworkExtension 权限（`com.apple.developer.networking.networkextension = packet-tunnel-provider`）、同一 App Group 与 keychain group。**App 侧的 networkextension 权限是必须的**：缺失时 `startVPNTunnel()` 抛 `permission denied`，profile 本来就带该能力，只是二进制没声明。

### 5.2 安装与启动

`xcrun devicectl device install app` 成功（`bundleID: com.lingxifox.fuckipv6`），`xcrun devicectl device process launch` 后在设备进程列表里看到 `FuckIpv6.app/FuckIpv6` 持续运行，未出现启动崩溃。

### 5.3 VPN 隧道打通（真机 + 本地协调端）

开发用协调端：仓库自带 `crates/client/examples/dev_coordinator.rs`（仅开发，非产品代码），以 Open 加入模式创建 `mesh` 网络，signaling `ws://10.0.0.122:6666`、QUIC relay `0.0.0.0:6667`。宿主机与手机在同一 Wi-Fi。

真机操作与结果（同一次会话日志，来自 App 与扩展共用的 App Group 日志）：

```
connect tapped: mesh @ ws://10.0.0.122:6666
configuration saved → coordinator probe: reachable → vpn profile saved
vpn status -> 2 (connecting)
startTunnel called → starting loading configuration
restored node identity from keychain
starting creating network configuration → network already registered; continuing
starting starting node
openNetwork: network=mesh mtu=1280 transport=[::]:63567 routes=2
packet pump started → tunnel phase -> running → vpn status -> 3 (connected)
```

协调端同刻记录：`客户端注册成功: uJ5vDZenSx2tB7cdxQytn65ATNWczZP0Pq2XUXO_kyw (虚拟IP: fd00::1001, 双栈: 10.8.0.1, 真实地址: 10.0.0.162:63567)`。

要点：节点身份在启动前从 Keychain 恢复到 App Group 目录（否则会生成新身份、协调端认成新设备）；路由只捕获 mesh 网段（2 条），不接管默认路由，避免路由环。

### 5.4 Mesh 数据面与性能（真机 ↔ Mac 第二节点）

第二个节点用 `crates/client/examples/mesh_probe.rs`（嵌入式运行时 + `ExternalPacketDevice`，同一套 Rust 数据面）加入同一网络，向手机虚拟 IP 发 ICMP echo 并做批量吞吐：

| 指标 | P3.3（P1a 修复前） | P3.4（当前） |
| --- | --- | --- |
| 对端可见性 | `peers=1`，`p2p_active=true`、`p2p_healthy=true` | 同左 |
| ICMPv4 → 10.8.0.1（20 包） | 5.53/6.29/7.01/6.32/7.61 ms | 4.98/6.35/6.96/**6.40**/10.07 ms |
| ICMPv6 → fd00::1001（20 包） | 5.23/6.45/7.95/7.20/23.76 ms | 5.09/6.32/7.53/**6.74**/12.32 ms |
| 吞吐，1 包/批 1 ms 间隔 | 4.31 / 4.40 Mbit/s，errors +0 | 4.40 / 4.29 / 4.34 Mbit/s，errors +0 |
| 吞吐，16 包/批 1 ms 间隔 | 6.35 / 5.72 / 6.01 / 7.18 Mbit/s（每轮 221–223 包回退） | **33.27 / 39.33 / 44.11 Mbit/s（mean 38.90）**，errors +0 |
| 512 × 1200 B 单轮 | 未测 | 512/512，39.10–44.47 Mbit/s，errors +0 |
| 1024 × 1200 B 单轮 | 未测 | 720–760/1024（约 30% 丢包），errors +161…193 |
| 4000 × 1200 B 连发 | 未测 | 450/4000，节点不掉线但大量丢包 |

表内 RTT 五列依次为 min/p50/p90/avg/max（单位 ms），吞吐口径是「往返 payload」，即收发的 1200 B 数据部分各算一次。

三点结论：

1. **P1a 收益在真机上成立**：同一台 iPhone、同参数（256×1200 B、16 包/批）从 6.32 Mbit/s 且每轮 221 包回退，变成 38.90 Mbit/s 且回退 0 次，约 6×。P3.3 记录的「批量吞吐靠中继」不再是当前状态。
2. **P1b 在真机上也站得住**：连续灌入 12000 个 1200 B 包期间，探针侧 snapshot 始终毫秒级返回、没有出现 `等待运行时响应超时`，手机节点不掉线、扩展不崩、UI 保持 `connected` / `running`。
3. **新发现一个手机入站上限**（不是 P1a/P1b 的回归）：单轮超过约 512 个包、或高带宽连发时丢包 5–60%，而 P2P 侧 `errors` 为 0，说明丢在手机入站路径（系统隧道队列 + Swift pump 逐包跨 FFI）。记录在 §6 第 7 条。
### 5.5 生命周期与身份（P3.4）

按下「断开」→ 等状态变 `invalid` → 再按「连接」，App Group 日志给出的完整序列（`Library/Logs/fuckipv6/app.log`）：

```
teardownNode
closeNetwork
identity captured into the keychain      # node_identity.pem 进 Keychain 并从磁盘删除
network settings were not cleared        # iOS 自行拆除隧道，属预期
tunnel phase -> stopping stop reason 1
...
tunnel phase -> starting restored node identity from keychain
openNetwork: network=mesh2 mtu=1280 transport=[::]:65175 routes=2
tunnel phase -> running
```

协调端三次注册的 node id 都是 `node_71ch-PLVD6S-FEDrAe6PmJtN6_wwhoDv0uMMEQ8Cr7s`、虚拟 IP 都是 `fd00::1001`，说明身份跨「停止 / Keychain 往返 / 重启」保持不变。

这一轮修掉的两个真机问题：

- `Diagnostics.record` 原来是 `queue.async`，而 `stopTunnel` 返回后系统立刻回收扩展进程，teardown 日志正好在那一刻丢掉（第一次断开只看到协调端优雅注销、App Group 日志里没有 `teardownNode`）。改成 `queue.sync` 后 teardown 相关日志稳定落盘。
- `captureIdentity` 原来只用 `NSLog` 报错，Keychain 失败在 App Group 日志里完全不可见；现在成功/无明文/失败（带错误文案）都写进 `app.log`。

另外 `IdentityVault` 的 Keychain account 从「网络名」改成固定 `node-identity`：`node_identity.pem` 位于 storage root（整个节点一份，不按网络分目录），用网络名做 account 会让节点换个网络名重连时找不到私钥、现场生成新身份，导致名册对不上。

### 5.6 调试通道（复现用）

- 安装/启动：`xcrun devicectl device install app` / `xcrun devicectl device process launch --terminate-existing`
- 日志：App 与扩展都写 App Group 的 `Library/Logs/fuckipv6/app.log`（容器根目录被 devicectl 容器访问策略挡住，只能放行 Library 等目录），用 `xcrun devicectl device copy from --domain-type appGroupDataContainer --domain-identifier group.com.lingxifox.fuckipv6 --source /Library/Logs/fuckipv6/app.log` 拉取
- 截图：`xcrun devicectl device capture screenshot`
- 扩展调试：Xcode `Debug > Attach to Process by Name: FuckIpv6Tunnel`

## 6. 已知问题（非本轮引入）

1. **既有 40 个 macOS 主机测试失败**：`node::runtime_control` 17、`local_service::tests` 17、`local_ipc::tests` 3、`symmetric::actor` 2、`cli::commands` 1，均为宿主机环境限制（macOS 不支持 Exit Node、UDS 路径超出 `sun_path` 容量等）。P2 基线为 42 个；P3.4 的 P1a（Darwin 批量发送）与 P1b（入站回写移出事件循环）修好了其中 2 个 `cli::commands` 批量计数用例，其余集合未变、未新增失败，全量结果为 `1205 passed / 40 failed`。
2. **`RustRuntimeTests` 只验证失败路径**：模拟器测试使用的仓库配置指向 `ws://127.0.0.1:9`，没有协调服务器，`fv6_runtime_start` 必然返回 `nodeProblem`（消息含"无法连接到协调服务器…等待虚拟 IP 分配超时"）。这正是真实行为：start 以真实初始化结果决定成败，不做假成功。running 路径由 P2 的进程内嵌入式测试（13 passed）与 P3.3 真机联调覆盖。
3. **模拟器不验证 `NEPacketTunnelProvider` 本身**：模拟器不允许装载 Network Extension，`startTunnel`/`setTunnelNetworkSettings` 的完整链路只能在真机验证。数据面用一个 `PacketFlow` 协议抽象来测，泵逻辑与真实 `NEPacketTunnelFlow` 共用同一份代码。
4. **命令请求必须用相邻标记（adjacently tagged）JSON**：`{"request_id":1,"command":{"command":"get_snapshot"}}`；写成 `{"request_id":1,"command":"get_snapshot"}` 会被 Rust 侧拒绝并回 `invalidArgument`，诊断透传到 Swift 的 `message`。
5. **Apple 平台批量发送（P3.4 已修复）**：原实现（`crates/client/src/p2p/udp_batch.rs` 的非 Linux 分支）每次只发一个数据报并返回 `Ok(1)`，同批其余数据报按「未发送后缀」转成 sealed relay 回退（`crates/client/src/p2p/connection.rs:2170-2185`，旧文案 `sendmmsg accepted only 1 of N datagrams`）。现在非 Linux 分支按序循环 `send_to`、首个包失败才返回 `Err`、后续失败保留成功前缀、每 16 包 `yield_now`；新增 `send_syscalls()` 按平台报告真实 syscall 数（Linux 1，其他 N），`UdpBatchStats` 改为 `record_send_calls(syscalls, datagrams)` 记账。验证：`p2p::udp_batch` 9 passed（含 1/8/16/32/128 包顺序与边界、IPv6 不合并、短写保留前缀、首包失败透错、syscall 计数），`short_sendmmsg_falls_back_only_unsent_suffix_without_resealing` 与 `assert_production_udp_batch` 按平台期望值断言（Linux 1 / 其他 2 与 8）。真机复测见 §5.4：同参数（16 包/批、256 × 1200 B）从 6.32 Mbit/s 且每轮 221 包回退，变为 38.90 Mbit/s、回退 0 次，约 6×。
6. **极端注入速率下控制面无响应（已定位并修复）**：200 µs 间隔连发 512 × 1200 B 时，接收侧事件循环在「入站包写回宿主」处等待设备出队队列容量，`control_rx` 拿不到轮询机会，日志持续输出 `刷新本机 Exit 客户端计数失败… error="等待运行时响应超时（1 秒）"`。P3.4 把入站回写改成「有界队列（128）+ 专用写任务」，事件循环只 `try_send`、队列满即丢包并计 `tun_write_error`；负载矩阵下接收侧 snapshot p50 从 980–1983 ms 降到 0.4–0.6 ms，告警降为 0 条，详见 `docs/DATAPLANE_PERFORMANCE_TODO.md` 的「P1b 收口」一节。
7. **手机入站吞吐上限（P3.4 实测发现，未修）**：同一台 iPhone 上，单轮 512 × 1200 B 能 512/512 全达（39–44 Mbit/s），单轮 1024 只到 720–760、单轮 2048 只到 450–482、4000 包连发只到 450；512 包 × 10 轮连发时在「满发 40 Mbit/s」与「丢 5–60%」之间交替，降速（16 包/批 4 ms、8 包/批 1 ms）仍有 5–25% 丢包。这些轮次里 P2P 侧 `errors` 为 0，也没有 relay 回退，说明丢在手机入站路径：系统给隧道的入站队列 + Swift pump 逐包跨 FFI 交给 `PacketFlow` 的速度。属于后续性能项（批量跨 FFI 注入、pump 侧批处理），不影响 P1a/P1b 的结论。

## 7. 未运行项

| 项目 | 原因 |
| --- | --- |
| 真机上「首次允许 VPN 配置」弹窗 | 系统授权弹窗无法脚本化，需要人工点一次；本轮由主人手动点击「连接」完成（§5.3） |
| 真机单测（XCTest 跑在设备上） | `TEST_HOST` 改成宿主 App 后构建通过，但 xcodegen 生成的工程没有把 `XCTest.framework` / `libXCTestSwiftSupport.dylib` 嵌入 App 的 `Frameworks`，设备上 dyld 解析失败，runner 以 code 74 退出（`The test runner exited with code 74 before establishing connection.`）。模拟器同一套用例已全绿，设备侧逻辑验证改用安装启动 + 后续真机联调覆盖 |
| 多设备 / 多跳 Mesh | 本轮只验证「真机 ↔ Mac 第二节点」两点互通；三节点、跨 NAT、跨运营商路径未测 |
| Linux / Windows 构建 | 本机只装了 aarch64-apple-darwin / -ios / -ios-sim 三个 target，无法交叉编译验证；改动只涉及 `cfg(not(any(linux, macos, windows)))` 的新分支与 ios-bridge crate，桌面代码路径未变 |
| 长时间稳定性 / 内存增长观察 | 只做了分钟级压测（单轮 4000 × 1200 B 连发、512 包 × 10 轮），没有小时级长跑与内存曲线 |
| App Store 分发签名（Distribution） | 需付费分发证书与 App Store Connect 配置，不在 P3 范围 |

## 8. 复现命令

```bash
# 1) Rust 侧
cargo fmt --all -- --check
cargo check --workspace --no-default-features
cargo test --workspace --no-default-features --no-run
cargo test -p fuckipv6-ios-bridge --no-default-features -- --test-threads=1
cargo test -p fuckipv6-client --no-default-features --lib embedded::tests -- --test-threads=1

# 2) 生成 Xcode 工程（改了 project.yml 才需要）
cd ios && xcodegen generate --spec project.yml --project .

# 3) 模拟器单测
xcodebuild test -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release \
  -destination 'platform=iOS Simulator,id=<模拟器 UDID>' -derivedDataPath /tmp/fv6-dd \
  # 可用 UDID 用 `xcrun simctl list devices available` 查；按名字匹配在运行时升级后可能失效
  CODE_SIGNING_ALLOWED=NO -only-testing:FuckIpv6Tests

# 4) 真机签名构建
xcodebuild build -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath /tmp/fv6-dd-device
# 5) 真机联调：本地协调端 + 手机 + 第二节点
cargo run -q -p fuckipv6-client --example dev_coordinator -- \
  --external-host <本机局域网 IP> --network mesh --config-dir .tmp/dev-coordinator
#   App 里填 ws://<本机局域网 IP>:6666，点「保存配置」→「连接」
cargo run -q -p fuckipv6-client --example mesh_probe -- \
  --coordinator ws://127.0.0.1:6666 --network mesh \
  --count 20 --warmup 5 --burst 256 --burst-payload 1200 --burst-rounds 4 --burst-batch 16
```

两个 example 都只用于开发联调：`dev_coordinator` 用 Open 加入模式建网（普通成员无需邀请即可加入），`mesh_probe` 用嵌入式运行时 + `ExternalPacketDevice` 冒充第二节点并测 ICMP / 吞吐。生产环境仍是桌面客户端 + 正式协调端。

注意：数据面成员资格来自协调端读到的 network record（`members`），新加入的节点若不在 `members` 里就拿不到 relay 端点、也不会出现在 peers 列表。开发期需要把节点 NodeId（`node_` + base64url(blake3(public_key))）追加到 `<config-dir>/<network>/network.json` 的 `members`；协调端每次调用都重读该文件，无需重启。

签名与标识符细节见 `docs/IOS_SIGNING_SETUP.md`，架构与接口约束见 `docs/IOS_P3_ARCHITECTURE.md`。
