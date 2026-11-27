# 平台支持表

> 规则：每个随发布产出的 target 必须有一行且分类为 `shipped`；每一行必须给出**可解析的证据**或**明确的排除理由**，
> 不允许空白承诺。证据格式：`path:<仓库内存在的文件>`、`doc:<存在的文档>#<章节关键词>`、`unsupported:<理由>`、
> `unverified:<缺什么证据>`。`scripts/check-platform-matrix.py` 会逐行校验这张表。
> 各平台 TUN、策略路由与 DNS 详细能力见 [PLATFORM_TUN_MATRIX.zh-CN.md](./PLATFORM_TUN_MATRIX.zh-CN.md)。

| 平台 / 架构 | 分类 | 证据或排除理由 |
|---|---|---|
| `x86_64-unknown-linux-gnu` | shipped | doc:docs/PRODUCT_UI_REDESIGN_TODO.zh-CN.md#W17-01 Linux 实机基线与 W18-02 门禁整绿 |
| `x86_64-pc-windows-gnu` | shipped | unverified:交叉检查与 Wine 文件语义测试通过，但未在 Windows 实机运行（W17-03 / W18-09 第 3 条） |
| `x86_64-apple-darwin` | shipped | unverified:未在 Intel Mac 实机运行（W17-02 / W18-09 第 2 条） |
| `aarch64-apple-darwin` | shipped | unverified:未在 Apple Silicon 实机运行（W17-02 / W18-09 第 2 条） |
| Android 通用 APK（minSdk 28） | shipped | doc:docs/PRODUCT_UI_REDESIGN_TODO.zh-CN.md#W16-08 Samsung Galaxy S24（SM-S9210，Android 16）真机 JNI、无障碍与 1,000 节点拓扑通过；实网 VPN/Doze/长时间移动网待补测 |
| `aarch64-unknown-linux-gnu` | unsupported | unsupported:发布矩阵不含该 target，仓库也没有对应 runner 或交叉产物 |
| `x86_64-unknown-linux-musl` | unsupported | doc:docs/PRODUCT_UI_REDESIGN_TODO.zh-CN.md#W17-01 CentOS Stream 9（5.14.0-689.el9.x86_64）静态二进制实机通过 Exit/systemd 全生命周期验收，但尚未纳入发布产物矩阵 |
| WSL | unsupported | unsupported:Linux 运行时独占宿主路由/防火墙与 DNS，WSL 的宿主网络不归该进程所有 |
| iOS | unsupported | unsupported:`crates/ios-bridge` 是可编译的桥接库，但没有打包与分发路径 |
