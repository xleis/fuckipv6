# iOS 签名与工程配置

本文记录 P3 阶段 iOS 工程的标识符、权限、签名方式与构建步骤，确保换机或换团队时可复现。

## 1. 固定标识符

| 项 | 值 | 说明 |
| --- | --- | --- |
| 主 App Bundle ID | `com.lingxifox.fuckipv6` | 不得更改 |
| Extension Bundle ID | `com.lingxifox.fuckipv6.tunnel` | 必须是主 App 的前缀扩展 |
| App Group | `group.com.lingxifox.fuckipv6` | 配置、状态、网络列表 |
| Keychain Access Group | `$(AppIdentifierPrefix)com.lingxifox.fuckipv6` | 主 App 侧声明；Extension 用默认组 |
| Development Team | `R9UJHHAZQ4` | 自动签名实测可用 |

Extension 的 Bundle ID 与主 App 的 App Group / Keychain 组必须一致，否则
`NETunnelProviderManager` 保存配置时会报 `NEVPNErrorConfigurationInvalid`。

## 2. Capabilities 与 Entitlements

主 App（`ios/FuckIpv6/FuckIpv6.entitlements`）：

```xml
<key>com.apple.security.application-groups</key>
<array><string>group.com.lingxifox.fuckipv6</string></array>
<key>keychain-access-groups</key>
<array><string>$(AppIdentifierPrefix)com.lingxifox.fuckipv6</string></array>
```

Extension（`ios/FuckIpv6Tunnel/FuckIpv6Tunnel.entitlements`）：

```xml
<key>com.apple.developer.networking.networkextension</key>
<array><string>packet-tunnel-provider</string></array>
<key>com.apple.security.application-groups</key>
<array><string>group.com.lingxifox.fuckipv6</string></array>
<key>keychain-access-groups</key>
<array><string>$(AppIdentifierPrefix)com.lingxifox.fuckipv6</string></array>
```

`com.apple.developer.networking.networkextension` 只声明在 Extension 上：Apple 只要求 Provider
具备该权限，主 App 不需要。App Group 两边都必须有，Keychain 组用于后续共享访问。

Xcode 侧需要的 Capabilities（自动签名会自动创建对应的 App ID 配置）：

- 主 App：App Groups、Keychain Sharing。
- Extension：App Groups、Keychain Sharing、Network Extensions（Packet Tunnel）。

## 3. 工程生成

工程文件由 `xcodegen` 从 `ios/project.yml` 生成，不手工改 `.pbxproj`：

```bash
cd ios
xcodegen generate --spec project.yml --project .
```

关键设置：

- `SWIFT_VERSION = 6.0`、`SWIFT_STRICT_CONCURRENCY = complete`（Swift 6 并发检查全开）。
- `ENABLE_USER_SCRIPT_SANDBOXING = NO`：构建期需要运行 `cargo`。
- `EXCLUDED_ARCHS[sdk=iphonesimulator*] = x86_64`：Rust 只产出 arm64 切片。
- Extension 与测试 target 都设置 `SWIFT_OBJC_BRIDGING_HEADER`、
  `HEADER_SEARCH_PATHS = $(SRCROOT)/../crates/ios-bridge/include`、
  `LIBRARY_SEARCH_PATHS = $(SRCROOT)/Rust/lib/$(PLATFORM_NAME)`、`OTHER_LDFLAGS = -lfuckipv6_ios`。
- 每个需要 Rust 的 target 都有 pre-build script `ios/scripts/build-rust.sh`。

`scripts/build-rust.sh` 行为：

- `PLATFORM_NAME=iphoneos` → `--target aarch64-apple-ios`，产物 `ios/Rust/lib/iphoneos/libfuckipv6_ios.a`。
- `PLATFORM_NAME=iphonesimulator` → `--target aarch64-apple-ios-sim`，产物 `ios/Rust/lib/iphonesimulator/libfuckipv6_ios.a`。
- `CONFIGURATION=Release` 用 `--release`，其余用 debug。
- `ARCHS` 含非 arm64 时直接报错退出，避免链接缺失切片。
- 头文件统一导出到 `ios/Rust/include/fuckipv6_ios.h`。

## 4. 构建命令

模拟器（无需签名）：

```bash
cd ios
xcodebuild -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release \
  -destination 'platform=iOS Simulator,name=iPhone 17' \
  -derivedDataPath /tmp/fv6-dd CODE_SIGNING_ALLOWED=NO build
```

单元测试（模拟器，host-less）：

```bash
cd ios
xcodebuild test -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release \
  -destination 'platform=iOS Simulator,name=iPhone 17' \
  -derivedDataPath /tmp/fv6-dd CODE_SIGNING_ALLOWED=NO -only-testing:FuckIpv6Tests
```

真机（自动签名）：

```bash
cd ios
xcodebuild -project FuckIpv6.xcodeproj -scheme FuckIpv6 -configuration Release \
  -destination 'generic/platform=iOS' \
  -derivedDataPath /tmp/fv6-dd-device -allowProvisioningUpdates build
```

真机安装需要 Apple ID 已登录 Xcode、设备已在「Windows → Devices and Simulators」中配对，
并且该 Team 下 App ID 具备上述 Capabilities。

## 5. 签名校验

构建产物检查：

```bash
codesign -d --entitlements :- /tmp/fv6-dd-device/Build/Products/Release-iphoneos/FuckIpv6.app
codesign -d --entitlements :- /tmp/fv6-dd-device/Build/Products/Release-iphoneos/FuckIpv6.app/PlugIns/FuckIpv6Tunnel.appex
```

预期：

- 主 App：`application-identifier = R9UJHHAZQ4.com.lingxifox.fuckipv6`，
  含 `group.com.lingxifox.fuckipv6` 与 `R9UJHHAZQ4.com.lingxifox.fuckipv6` Keychain 组。
- Extension：`application-identifier = R9UJHHAZQ4.com.lingxifox.fuckipv6.tunnel`，
  含 `com.apple.developer.networking.networkextension = [packet-tunnel-provider]` 和相同 App Group。

查看描述文件：

```bash
security find-identity -v -p codesigning
ls ~/Library/MobileDevice/Provisioning\ Profiles/ | head
```

## 6. 常见问题

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `only arm64 simulators are supported (got ARCHS=arm64 x86_64)` | Rust 无 x86_64 切片 | 保持 `EXCLUDED_ARCHS[sdk=iphonesimulator*]` |
| `Sandbox: bash deny file-write-data` | 脚本沙盒阻止 cargo | `ENABLE_USER_SCRIPT_SANDBOXING = NO` |
| `NEVPNErrorConfigurationInvalid` | Bundle ID / App Group 不匹配 | 核对 §1、§2 |
| 真机启动隧道立即失败 | 描述文件缺 Network Extension 权限 | 在开发者后台为 `.tunnel` App ID 勾选 Packet Tunnel |
| Keychain 读取 `errSecMissingEntitlement` | 该进程无对应 Keychain 组 | 用默认组，或补齐 `keychain-access-groups` |
| `cargo-fmt is not installed` | 工具链缺 rustfmt | `rustup component add rustfmt --toolchain <toolchain>` |

## 7. 安全约定

- 不提交任何证书、私钥、描述文件。
- 不修改 Apple 正式标识符；如需自测，只在本地临时改为自己的 Team / Bundle ID，且不提交。
- 节点身份私钥只放 Keychain，App Group 与日志中不得出现明文。
