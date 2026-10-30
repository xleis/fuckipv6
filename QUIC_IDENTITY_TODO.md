# QUIC 统一身份执行清单（EQ-14）

> 范围：中继 QUIC 的双向 TLS、应用身份绑定和 0-RTT 策略。
> 威胁模型：[`QUIC_IDENTITY_THREAT_MODEL.md`](QUIC_IDENTITY_THREAT_MODEL.md)。
> 边界：本清单只涉及 `crates/common`、`crates/coordinator`、非受保护的 P2P QUIC/连接代码与文档。不得修改用户正在处理的 `crates/client/src/cli/commands.rs`、`crates/client/src/cli/mod.rs`、`crates/client/src/webui/controller.rs`。
> 结论口径：代码和测试通过不等于独立安全审计；在该清单全部完成且独立复核前，不得宣称 QUIC 身份层或项目安全性超过 WireGuard。

## 执行清单

- [x] **QI-01 威胁模型与版本边界**：明确 QUIC TLS、应用 Ed25519 身份、relay 注册和 0-RTT 的攻击面；记录兼容边界与不承诺项。
- [x] **QI-02 身份证书生成**：由长期 Ed25519 身份确定性派生 QUIC 客户端证书，证书公钥必须与应用签名公钥一致，私钥不落盘、不打印。
- [x] **QI-03 强制客户端证书校验**：服务端启用 mandatory client auth，只接受满足客户端用途且能完成 TLS proof-of-possession 的证书。
- [x] **QI-04 客户端认证配置**：提供带客户端证书的 pinned 客户端配置，并保持现有无客户端证书配置仅供显式 legacy/自检路径使用。
- [x] **QI-05 TLS channel binding**：定义稳定的 exporter label/context，客户端与服务端使用同一 helper 派生并比较绑定材料。
- [x] **QI-06 Relay 注册绑定**：Register 签名覆盖 client id、虚拟 IP、应用公钥、时间戳、服务端 pin、客户端证书 pin 和 TLS exporter。
- [x] **QI-07 证书与应用身份一致**：服务端将 TLS 对端证书 SPKI 与 Register 中的 Ed25519 公钥逐字节比较，错绑时在注册前失败关闭。
- [x] **QI-08 缺失/错误客户端证书测试**：覆盖无客户端证书、错误客户端证书和错误服务端 pin 均无法完成连接。
- [x] **QI-09 应用身份错绑测试**：覆盖合法客户端证书搭配另一应用身份、篡改 Register 字段和缺失绑定信息的拒绝路径。
- [x] **QI-10 生产 0-RTT 关闭**：中继生产配置将 `max_early_data_size` 设为 0，客户端不默认启用 early data；不得让 Register/Packet 进入 0-RTT。
- [x] **QI-11 0-RTT 重放回归**：更新自检与集成测试，证明恢复会话不能提交早期 Register/Packet；若未来启用，必须另行具备幂等处理、有界重放缓存、时间窗和降级规则。
- [ ] **QI-12 Relay 端到端集成**：正确绑定的客户端注册和转发成功，错误绑定的客户端不产生 relay 状态或流量计数副作用。
- [ ] **QI-13 文档与状态同步**：同步 `SECURITY_PERFORMANCE_TODO.md`、`SECURITY_TRACEABILITY.md`、`PROTOCOL_V2.md`、`CHANGELOG.md` 和 README 的准确限制。
- [ ] **QI-14 全量门禁与完成记录**：执行格式、文档检查、workspace check/clippy/test 和相关 QUIC 测试；每项独立提交后更新本节状态。

## 验收口径

1. 应用身份错绑必须在服务端写入 `RelayState` 前失败。
2. 客户端证书必须完成 TLS 握手中的私钥持有证明，不能只解析自签名证书内容。
3. 绑定材料必须同时覆盖服务端身份与本次 TLS 会话，不能是客户端可伪造的静态常量。
4. 生产中继不得接受 0-RTT 的注册或数据流；只关闭应用层副作用不视为完成 TLS 层限制。
5. 所有负向测试必须断言连接或注册失败，不得仅检查日志。
