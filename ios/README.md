# iOS 客户端（骨架）

iOS 客户端尚未实现。本目录只有一个能打开的 Xcode 骨架，没有任何可运行功能，状态 `not_started`。

## 工程形态

- SwiftUI 应用，target `FuckIpv6`，bundle id `cc.xlei.fuckipv6.ios`，最低 iOS 17.0，Swift 6 语言模式。
- 工程由 XcodeGen 从 `ios/project.yml` 生成，`FuckIpv6.xcodeproj` 一并提交。改结构时改 `project.yml`，再执行：

  ```bash
  cd ios && xcodegen generate --spec project.yml --project .
  ```

- `FuckIpv6/App/` 是 App 入口与占位 `RootView`；`FuckIpv6/Resources/Assets.xcassets` 是资源目录（AppIcon 尚无实际图片）。

## 硬约束（与现有架构一致）

- UI 只做展示与输入适配，不复制网络、身份、配置、路由、Exit Node 与持久化逻辑。
- 契约来自 `contracts/schema/`：`node-command-request-v1`、`node-command-result-v1`、`node-snapshot-v1`、`node-http-v1`。
- 提交动作只走类型化 `NodeCommand`（对应 Android 的 `NodeContractV1` 通道），不新增 iOS 专属接口。
- 错误语义、校验规则与 GPUI/WebUI/Android 保持同一套，不做平台特化。

## 尚未验证 / 未做

- 未在本机或 CI 编译。`xcodebuild -list` 只能证明工程可解析，不代表可构建或可运行。
- 未配置签名、开发者团队、分发渠道。
- 未接入 Network Extension / `NEPacketTunnelProvider`，也未写任何 IPC 或契约代码。
