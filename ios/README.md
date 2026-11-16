# iOS 客户端（占位）

iOS 客户端尚未开始实现。本目录只用于占位，后续 iOS 工作与 `android/` 平级展开，不混入桌面（`crates/client` 的 GPUI）或 `webui/`。

## 硬约束（与现有架构一致）

- UI 只做展示与输入适配，不复制网络、身份、配置、路由、Exit Node 与持久化逻辑。
- 契约来自 `contracts/schema/`：`node-command-request-v1`、`node-command-result-v1`、`node-snapshot-v1`、`node-http-v1`。
- 提交动作只走类型化 `NodeCommand`（对应 Android 的 `NodeContractV1` 通道），不新增 iOS 专属接口。
- 错误语义、校验规则与 GPUI/WebUI/Android 保持同一套，不做平台特化。

## 未定项

- 工程形态：SwiftUI 应用 + 本地节点桥接方式（是否走 Network Extension / `NEPacketTunnelProvider`）。
- 最低系统版本、签名方式、分发渠道。
- 共享 `NodeSnapshot` 的投影代码是否由 schema 代码生成，而不是手写。

## 状态

`not_started` —— 没有任何可运行代码或工程文件。
