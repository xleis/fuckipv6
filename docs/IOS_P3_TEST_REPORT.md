# iOS P3 测试报告

P3 = iOS 端接入（Rust staticlib + C ABI + Packet Tunnel Extension + Swift 宿主）。
本报告只覆盖**本轮实际执行过**的检查；未执行的项目在"未运行项"里逐条列出原因。

## 1. 结论

| 层面 | 状态 | 依据 |
| --- | --- | --- |
| Rust 侧门禁（fmt / check / 测试编译） | 通过 | 见 §2 表格 |
| Rust 桥（C ABI）25 个用例 | 通过 | §3 |
| P2 内嵌运行时回归 13 个用例 | 通过 | §2 |
| 客户端库回归 1197 passed / 42 failed | 与 P2 基线完全一致 | §2、§6 |
| iOS 单元测试 34 个用例（模拟器） | 通过 | §4 |
| 真机签名构建 + 安装启动（含 Extension 嵌入 + entitlements） | 通过 | §5 |
| 真机 VPN 起隧道 + 真实 Mesh 流量 | **未做** | 需真机 + 协调服务器，留 P3.3 |

一句话：**编译、签名、数据面与生命周期逻辑已在模拟器和 C ABI 层面验证；真实网络连通性（设备上起 VPN、跨设备 Mesh）尚未验证。**

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
| `cargo test -p fuckipv6-client --no-default-features --lib -- --test-threads=8 --skip punch` | 1197 passed / 42 failed / 1 ignored（16.53s），失败集合与 P2 基线一致，见 §6 |

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

## 4. iOS 单元测试 34 个用例（模拟器）

命令：`xcodebuild test -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release -destination 'platform=iOS Simulator,name=iPhone 17' -derivedDataPath /tmp/fv6-dd CODE_SIGNING_ALLOWED=NO -only-testing:FuckIpv6Tests`
结果：**TEST SUCCEEDED**，34 tests / 0 failures（21.19s）。

| 套件 | 数量 | 覆盖 |
| --- | --- | --- |
| `PacketPumpTests` | 11 | 双向转发顺序、队列满重试后丢弃、非法包直丢、closed 停泵、空批视为 flow 关闭、协议族映射、stop 有界且可重复、重启换会话、stop 后陈旧回调不转发 |
| `MessageRouterTests` | 11 | 空/超长消息、版本校验、未知命令、未运行态返回 state、status/snapshot 透传、非 JSON 拒绝、execute 本地校验 + 透传、错误码保留（timeout/nodeProblem/busy）、pathChanged |
| `IdentityVaultTests` | 7 | Keychain 存取、无身份不建文件、不覆盖已有身份、capture 后删明文、无文件 no-op、capture→restore 字节一致、按网络名隔离 |
| `RustRuntimeTests` | 5 | 真实 C ABI 生命周期与失败语义（见 §6 第 2 条） |

## 5. 真机签名构建

命令：`xcodebuild build -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release -destination 'generic/platform=iOS' -derivedDataPath /tmp/fv6-dd-device`
结果：**BUILD SUCCEEDED**，扩展被嵌入 `FuckIpv6.app/PlugIns/FuckIpv6Tunnel.appex`。

- 签名身份：`Apple Development: Zhihan Zhu (9DCPP78554)`（Team R9UJHHAZQ4）
- 配置描述文件：`iOS Team Provisioning Profile: com.lingxifox.fuckipv6` 与 `...fuckipv6.tunnel`
宿主 App 含同一 App Group 与 keychain group、不含 networkextension（Provider 侧声明即可）。

安装与启动：`xcrun devicectl device install app` 成功（`bundleID: com.lingxifox.fuckipv6`），`xcrun devicectl device process launch` 后在设备进程列表里看到 `FuckIpv6.app/FuckIpv6` 持续运行，未出现启动崩溃。

## 6. 已知问题（非本轮引入）

1. **既有 42 个 macOS 主机测试失败**：`node::runtime_control` 17、`local_service::tests` 17、`cli::commands` 3、`local_ipc::tests` 3、`symmetric::actor` 2。P2 基线同为 42 个，本轮改动（`tun/mod.rs` 增加非桌面后端、`client/Cargo.toml` 扩 libc 目标、新增 ios-bridge crate）没有改变该集合，也未新增失败。
2. **`RustRuntimeTests` 只验证失败路径**：模拟器测试使用的仓库配置指向 `ws://127.0.0.1:9`，没有协调服务器，`fv6_runtime_start` 必然返回 `nodeProblem`（消息含"无法连接到协调服务器…等待虚拟 IP 分配超时"）。这正是真实行为：start 以真实初始化结果决定成败，不做假成功。running 路径由 P2 的进程内嵌入式测试（13 passed）与 P3.3 真机联调覆盖。
3. **模拟器不验证 `NEPacketTunnelProvider` 本身**：模拟器不允许装载 Network Extension，`startTunnel`/`setTunnelNetworkSettings` 的完整链路只能在真机验证。数据面用一个 `PacketFlow` 协议抽象来测，泵逻辑与真实 `NEPacketTunnelFlow` 共用同一份代码。
4. **命令请求必须用相邻标记（adjacently tagged）JSON**：`{"request_id":1,"command":{"command":"get_snapshot"}}`；写成 `{"request_id":1,"command":"get_snapshot"}` 会被 Rust 侧拒绝并回 `invalidArgument`，诊断透传到 Swift 的 `message`。

## 7. 未运行项

| 项目 | 原因 |
| --- | --- |
| 真机起 VPN 隧道 | 安装与启动已在真机验证；首次启用需人工点「允许 VPN 配置」与连接开关（系统弹窗无法脚本化），留 P3.3 |
| 真机单测（XCTest 跑在设备上） | `TEST_HOST` 改成宿主 App 后构建通过，但 xcodegen 生成的工程没有把 `XCTest.framework` / `libXCTestSwiftSupport.dylib` 嵌入 App 的 `Frameworks`，设备上 dyld 解析失败，runner 以 code 74 退出（`The test runner exited with code 74 before establishing connection.`）。模拟器同一套用例已全绿，设备侧逻辑验证改用安装启动 + 后续真机联调覆盖 |
| 真机跨设备 Mesh 连通性 / 吞吐 | 需两台设备或一台真机 + 一台桌面节点，且需可达的协调服务器，留 P3.3 |
| Linux / Windows 构建 | 仅改了 `cfg(not(any(linux, macos, windows)))` 的新分支，桌面平台代码路径未变；跨平台门禁在 P3 收尾统一跑 |
| 长时间稳定性 / 内存增长观察 | 未做，留 P3.3 |
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
  -destination 'platform=iOS Simulator,name=iPhone 17' -derivedDataPath /tmp/fv6-dd \
  CODE_SIGNING_ALLOWED=NO -only-testing:FuckIpv6Tests

# 4) 真机签名构建
xcodebuild build -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath /tmp/fv6-dd-device
```

签名与标识符细节见 `docs/IOS_SIGNING_SETUP.md`，架构与接口约束见 `docs/IOS_P3_ARCHITECTURE.md`。
