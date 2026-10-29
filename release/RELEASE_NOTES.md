# fuckipv6 v0.3.0 发布说明

## 版本信息

- **当前版本**：v0.3.0
- **基础发布日期**：2026-10-29
- **文档更新时间**：2026-10-30
- **支持平台**：Linux x86_64、Windows x86_64、macOS x86_64 / arm64
- **当前阶段**：Phase 6 功能阶段，进入生产验证与发布收尾

> 仓库中已有的 `fuckipv6-windows-v0.1.0.zip` 是历史构建产物，不应视为 v0.3.0 的正式安装包。正式发布包应由 `.github/workflows/release.yml` 在对应版本标签上重新构建。

## 核心能力

### 组网与传输

- WebSocket 协调信令、虚拟 IPv4/IPv6 双栈地址分配与在线状态管理。
- 加密 UDP 直连与 QUIC Stream Mesh 双模 P2P 数据面。
- QUIC 多中继并发测速、低延迟选路与 WebSocket 信令最终回退。
- STUN/RFC 5780 NAT 类型检测、ICE 候选收集、UPnP IGD 与 RFC 6886 NAT-PMP。
- RTT、丢包率、带宽估计与连接健康阈值监控。

### 安全与治理

- Ed25519 身份签名与公钥指纹 Client ID。
- X25519 + ChaCha20-Poly1305 端到端加密与 Blake3 KDF。
- TLS 1.3 证书公钥指纹 Pinning。
- Coordinator JWT、声明式 ACL、准入黑名单与配置加密。

### Phase 6 能力

- `MeshOnly`、`BypassMainland`、`GlobalProxy` 智能分流。
- Exit Node 白名单鉴权与内核 NAT 转发。
- `fuckipv6 host` 一键建网和 `fk://` 邀请票据。
- 多网络并发、独立 TUN 设备与路由隔离。
- 自适应无头 WebUI / 桌面模式。
- 3-Hop 洋葱路由、Kademlia DHT、Blake3 分块传输基础设施。

## 客户端与运维

- CLI 支持 `host`、`join`、`up`、`down`、`status`、`peers`、`ping`、`stats`、`route`、`logs` 等命令。
- WebUI 提供拓扑、节点、流量、日志、路由和运行时配置管理。
- 支持 systemd、launchd、Docker、CI 与跨平台发布工作流。
- 运行日志已完成中文本地化。

## 当前验证基线

本仓库已通过：

```text
cargo fmt --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace --target x86_64-pc-windows-gnu
cargo check --workspace --target x86_64-unknown-linux-musl
```

工作区当前包含 **128 个通过测试**。跨平台命令应使用安装了对应 target 标准库的 Rustup 工具链。

## 已知限制

- 真实双主机、跨 NAT、跨公网环境仍需持续回归。
- macOS/Windows 实机运行测试仍需按检查清单完成。
- 更大规模 WAN 吞吐、连接数和故障切换压测仍需补充。
- 生产上线前仍建议执行独立第三方安全审计。
- Linux 创建 TUN 需要 root 或 `CAP_NET_ADMIN`；Windows 需要管理员权限。

## 当前开发分支

`CHANGELOG.md` 的 `Unreleased` 段还包含 WebUI 全面重构与静态资源分发改进。这些变更位于 v0.3.0 之后、下一版本发布之前，不应被视为 v0.3.0 稳定制品的一部分。

## 质量红线

- Linux musl 与 Windows GNU 必须保持编译通过。
- 每次修改必须创建独立 Git commit。
- 提交前执行格式、Clippy 与工作区测试。
- 不提交密钥、个人配置或构建产物。
