# TUN、路由与 DNS 平台能力矩阵（W17-04）

> 本文档逐项记录各目标平台的 TUN 生命周期、IPv4/IPv6 策略路由语义、DNS 集成与恢复、权限失败与残留清理、应用/进程识别边界、固定 Exit 失败关闭及休眠/切网/崩溃恢复能力，并与 [PLATFORM_SUPPORT.zh-CN.md](./PLATFORM_SUPPORT.zh-CN.md) 保持一致：未完成实机验证的平台显式标注 `unverified`，不支持的平台显式标注 `unsupported`，不将 Linux 行为外推为所有平台行为。

## 1. 平台总览与实测状态

| 平台 / 架构 | 发布分类 | TUN 机制 | 系统路由与 DNS 集成 | 实机验证状态 |
|---|---|---|---|---|
| Linux (`x86_64-unknown-linux-gnu` / `musl`) | `shipped`（gnu）/ `unsupported`（musl，已有实机运行证据） | `/dev/net/tun`（`IFF_TUN \| IFF_NO_PI`）由 `tun` crate 创建，netlink / `ip` 清理 | 策略路由表（`ip rule` / `ip route`，`proto openr` 标记）+ `systemd-resolved` / `resolvconf` / `/etc/resolv.conf` 原子备份恢复 | **已实机验证**：本机 Arch Linux + 远端 CentOS Stream 9 / RHEL 9 (`5.14.0-689.el9.x86_64`, `systemd`) |
| Android (`arm64-v8a`, `minSdk 28`, `targetSdk 36`) | `shipped` | `VpnService.Builder.establish()` + `detachFd()` 移交 Rust `crates/android-bridge` `TunPump`（`poll(2)` + 自管道即时退出） | `VpnService.Builder.addAddress` / `addRoute` / `addDnsServer` + `addDisallowedApplication(packageName)` 防回环 | **部分真机验证**：Samsung Galaxy S24 (`SM-S9210`, Android 16 / SDK 36) 已实机通过 ARM64 `libfuckipv6_android.so` JNI 回环控制面、5 模块无障碍、1,000 节点拓扑；真机 VPN 隧道实网 Exit/Doze/长时间移动网稳定性仍待下次连接真机补测 |
| macOS (`x86_64-apple-darwin` / `aarch64-apple-darwin`) | `shipped`（待实机） | `utun`（`PF_SYSTEM` / `SYSPROTO_CONTROL`）由内核随 fd 关闭自动回收 | `route` / `scutil` / `networksetup` 适配器 | **`unverified`**：缺少 macOS 实机与 Apple SDK 原生运行报告 |
| Windows (`x86_64-pc-windows-gnu`) | `shipped`（待实机） | Wintun 驱动适配器 + Windows IP Helper API | `netsh` / IP Helper 路由表与接口 DNS + Windows Firewall 规则 | **`unverified`**：已完成交叉检查与 Wine 文件语义测试，尚无 Windows 实机报告 |
| iOS (`crates/ios-bridge`) | `unsupported` | `NEPacketTunnelProvider` / `NEPacketTunnelNetworkSettings`（`crates/ios-bridge` 提供 FFI 桥接） | `NEPacketTunnelNetworkSettings` 声明式应用，系统随隧道停止回收 | **`unsupported`**：无打包、签名与分发路径 |
| WSL | `unsupported` | 无独立宿主路由/防火墙所有权 | 宿主网络由 Windows 主机独占，不满足独占路由与防泄漏前提 | **`unsupported`**：明确排除 |

## 2. 逐项技术能力对比

### 2.1 TUN 创建与清理机制

| 平台 | 创建路径 | 正常关闭清理 | 异常崩溃 / 残留清理 |
|---|---|---|---|
| **Linux** | `NetworkSetup` 在特权上下文（`CAP_NET_ADMIN` + `CAP_NET_RAW`）打开 `/dev/net/tun`，按网络名分配 `fk-<name>`，设置 MTU 与虚拟 IPv4/IPv6 | 停止网络时先移除策略路由与 DNS 覆盖，关闭 TUN fd 使内核同步销毁接口 | 启动前与停止后执行残留检测（`test-lifecycle-cleanup-netns.sh` 与 `cleanup_watchdog`），清理遗留 `fk-*` 接口、`proto openr` 规则与 `FKV6N-*` NAT 链；在 CentOS Stream 9 实测 `stop_networks` 与 `service stop` 后零接口与进程残留 |
| **Android** | Rust `AndroidEmbeddedPlatform::open_network` 派生 `AndroidTunnelSettings` JSON，经 JNI 回调 `FuckIpv6VpnService.openNetwork` 调用 `VpnService.Builder.establish()` 并 `detachFd()` 给 `TunPump` | `AndroidNetworkLease::close()` 向 `TunPump` 写自管道字节立即唤醒 `poll(2)`、关闭 TUN fd，系统内核自动移除 `tun0`、VPN 路由与 DNS；`closeNetwork` 更新前景通知 | `onRevoke()` 与 `onDestroy()` 强制调用 `EmbeddedNodeHost.stopAll()`、`stopForeground(STOP_FOREGROUND_REMOVE)` 与 `stopSelf()`；`ACTION_STOP_DATA_PLANE`（`stop_node`）仅释放 TUN/路由/DNS 而保留回环控制面 |
| **macOS** | `utun` fd（待实机验证） | 关闭 fd 自动销毁 `utunX`，恢复 `scutil` DNS 与路由（待实机验证） | 待 macOS 实机验证 |
| **Windows** | Wintun session + 适配器句柄（待实机验证） | 销毁 Wintun session 并还原接口路由/DNS（待实机验证） | 待 Windows 实机验证 |

### 2.2 IPv4 / IPv6 路由语义与冲突处理

- **MeshOnly**：仅注入虚拟网段路由（IPv4 `10.8.0.0/24`、IPv6 网络分配前缀 `fd00::/64` 等），**不修改默认路由**；冲突地址在分配与应用阶段失败关闭（`AddressConflict`）。
- **BypassMainland / GlobalProxy**：
  - **Linux**：使用独立策略路由表与 `fwmark`/control-endpoint 绕过规则，避免节点自身信令、QUIC Relay 与 P2P UDP 流量回环进 TUN；主表默认路由保持可回滚快照。
  - **Android**：`AndroidTunnelSettings::derive` 在 `BypassMainland`/`GlobalProxy` 下写入 `0.0.0.0/0` 与 `::/0`，并通过 `VpnService.Builder.addDisallowedApplication(packageName)` 将本应用自身的所有 UDP/QUIC/TCP/WS 套接字排除在 VPN 之外，**从内核层杜绝路由回环**；若系统拒绝排除自身包名，直接返回错误码 `-2` 失败关闭。
  - **macOS / Windows**：按同源路由契约规划，标为 `unverified` 待实机验收。

### 2.3 DNS 系统集成、DoH/DoT 与恢复

- **Linux**：通过 `DnsManager` 管理 `systemd-resolved`（`resolvectl`）与直接 `/etc/resolv.conf` 覆盖；停止网络或故障退出时自动还原原始解析配置（CentOS Stream 9 与 netns 实测 `cmp -s resolv.before resolv.conf` 字节一致）。
- **Android**：`Exit` 模式下若操作者未显式配置解析器，`AndroidTunnelSettings` 默认向 `VpnService.Builder.addDnsServer` 注入隧道内递归解析器（`1.1.1.1`、`8.8.8.8`），使 DNS 查询随 `0.0.0.0/0` 经 `tun0` 穿越 Exit，避免经运营商 DNS 泄漏；系统在 `tun0` 关闭时自动恢复原网络 DNS。设备端 Private DNS（DoT）受 Android 系统级策略管辖，由 VPN 默认路由封装其底层 TCP 853 流量。
- **macOS / Windows**：标为 `unverified`。

### 2.4 权限失败、接口消失与残留清理

- **Linux**：无 `CAP_NET_ADMIN`/`CAP_NET_RAW` 时在 `NetworkSetupStage::TunCreate` 明确返回结构化 `PermissionDenied` 并回滚已分配资源；无桌面环境与 systemd 单元（`ProtectSystem=strict`、`ProtectHome=true`、`PrivateTmp=true`、`AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW`、`CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW`、`UMask=0077`）已在 CentOS Stream 9 实测 `systemd-analyze verify` 零告警。
- **Android**：未获用户 VPN 授权时 `VpnService.Builder.establish()` 返回 `null`，`openNetwork` 返回 `-3`，上层不伪造 `Running`；UI 在拉起系统 VPN 授权前必先展示用途与影响说明弹窗（`AndroidShellAndAccessibilityTest` 在 Galaxy S24 真机通过）；系统撤销 VPN 权限时 `onRevoke()` 立即清理数据面并转入可见错误态。

### 2.5 应用 / 进程识别的平台能力

| 平台 | 应用 / 进程级路由匹配能力 | 当前实现状态与失败关闭策略 |
|---|---|---|
| Linux | 需 eBPF / cgroup v2 / socket UID 挂载，当前 TUN 逐包引擎不具备进程身份归因 | `TrafficPolicy` 遇 `app` / `process` 规则在校验与运行时明确返回 `Unsupported` 失败关闭，不静默直连（W11-08） |
| Android | 支持应用级 `addDisallowedApplication`（目前用于防回环排除自身包名）；按应用分流策略暂未在策略引擎暴露 | 策略引擎同源失败关闭，不伪造按应用规则生效 |
| macOS / Windows | 需 Network Extension / WFP 驱动 | `unsupported` / `unverified`，同源失败关闭 |

### 2.6 固定 Exit、失败关闭与 Fallback

- **Linux & Android 共用同一 Rust 权威状态机**（`crates/client/src/cli/commands.rs`）：
  - 固定 Exit（`mode = fixed`）与非 `MeshOnly` 流量会话启动时处于关闭态，通过 `ExitConnectionWatchdog` 监测 Exit Secure Session 密钥确认后发起带随机 16 字节 nonce 的签名 `ExitReadinessFrame` 探针，**全部探针通过后才将 `traffic_session` 推进至 `Protected`**；
  - 任一阶段超时、对端不匹配、目标变化或 Exit 离线时立即调用 `enter_fail_closed`，**绝不静默回退直连**，且固定 Exit 不被 fallback 绕过；
  - CentOS Stream 9 实机与 Linux netns 故障注入套件（12/12）已实测验证；Android 侧已在 `crates/client/src/cli/commands.rs`（`#[cfg(target_os = "android")]`）与 `crates/android-bridge` 接通同源状态机。

### 2.7 休眠、网络切换与强制崩溃恢复

- **Linux**：`test-recovery-netns.sh`、`fault-injection-suite.py`（12/12）与 CentOS Stream 9 实测覆盖网卡闪断（`ip link set down/up` 后 `GOOGLE_AFTER_FLAP=204`）、`SIGKILL __node` 进程监督自动拉起、`SIGKILL __control` 不中断节点、`systemctl restart` 状态对账与 `stop_node` 保留 WebUI。
- **Android**：`AndroidNetworkRecoveryObserver`（`ConnectivityManager.NetworkCallback` + Doze/锁屏/飞行模式广播）驱动 `BoundedRecoveryStateMachine`（最多 3 次重试、10 秒硬超时、飞行模式立即中止清理），并通过 `NativeNodeBridge.notifyNetworkPathChanged` 触发 Rust 侧有界恢复；JVM 单测与 Galaxy S24 JNI 回调均已通过，真机真实 Doze/Wi-Fi 切网/长时间移动网实网验证仍标注待下次连接真机补测。
- **macOS / Windows**：`unverified`（缺少实机睡眠/唤醒与网络切换验证）。
