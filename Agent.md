# Agent Working Rules

本文件适用于本仓库内的所有自动化代理与开发人员。

## 强制工作流

1. 每次修改文件后，必须立即创建独立 Git commit；不得把无关修改混入同一个 commit。
2. 开始修改前先检查 `git status`，结束时确认工作区干净。
3. commit message 使用 Conventional Commits，例如：
   - `feat: add network diagnostics`
   - `fix: handle relay reconnect`
   - `docs: sync release version`
   - `style: format workspace`
   - `chore: initialize repository`
4. 提交前至少执行与改动直接相关的检查；涉及 Rust 全局变更时优先执行：
   - `cargo fmt --check`
   - `cargo check --workspace`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --workspace`
5. 未经用户明确同意，不得改写已经推送或共享的 Git 历史。
6. 不提交密钥、证书、个人配置、构建产物或其他敏感/生成文件。

## 质量要求

- 保持 Linux musl 与 Windows GNU 编译兼容。
- 修改协议、加密、路由或持久化格式时，必须同步更新测试与文档。
- 优先修复测试、格式、Clippy 和文档一致性问题，再扩展新功能。
- 如果验证失败，必须在最终说明中明确失败命令、原因和影响。

## 产品架构约束

- 唯一产品身份是“节点”。只发布一个用户入口 `fuckipv6`，不得把协调服务实现成第二个用户需要选择或启动的产品。
- 网络创建、加入、发现、地址分配、信令、中继和 Exit Node 都是节点能力或节点在某个网络中的职责，不是客户端/服务端身份。
- 有桌面环境时使用 GPUI 原生 GUI；无桌面环境时使用 WebUI。两套 UI 必须消费同一命令层、状态模型、校验规则和错误语义。
- UI 只能做展示与输入适配，不得复制网络、身份、配置、路由、Exit Node 或持久化业务逻辑。
- 协议和 TLS 中的 `client`/`server`、Initiator/Responder 可以作为会话技术术语保留，但不得出现在用户可见的产品身份和启动流程中。
- 统一节点迁移的验收清单以 `UNIFIED_NODE_TODO.md` 为准；未完成对等性、单发布物和跨平台验证前，不得宣称目标已完成。

## 记录

每次提交应保持单一目的，并在必要时同步更新 `CHANGELOG.md`。
