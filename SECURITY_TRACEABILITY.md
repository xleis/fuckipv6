# 安全威胁—控制—测试追踪矩阵

> 状态：工程追踪文档，不是独立安全审计结论。  
> 基线：`3f2f127` 之后的 Secure Session v2 本地原语。  
> 判定口径：源码存在不等于生产路径已接入；单元测试通过不等于协议已获独立评审。

## 1. 结论先行

| 声明 | 当前是否成立 | 依据 |
|---|---|---|
| UDP/TUN 生产路径使用 AEAD 加密 | 是，静态 X25519 派生密钥 | 客户端现有数据面路径 |
| 拥有可测试的临时握手、重放窗口和重协商 v2 原语 | 是 | `crates/secure-session` |
| 客户端生产路径已使用 v2 临时握手和计数 Nonce | 否 | `fuckipv6-client` 未依赖 `fuckipv6-secure-session` |
| 已具备端到端后量子机密性和身份认证 | 否 | 仅完成 ML-KEM/ML-DSA 原语适配，未接入 X-Wing、复合身份或握手 |
| QUIC 双向身份已统一 | 否 | 服务端尚未要求客户端证书，应用层身份未与证书绑定 |
| 已达到 WireGuard 安全等级 | 否 | 生产数据面、独立审计和 QUIC 身份等缺口仍在 |
| 安全等级和性能超过 WireGuard | 否 | 完整安全与性能门槛均未满足 |

## 2. 追踪矩阵

| ID | 威胁/失败模式 | 预期安全不变量 | 当前控制与代码 | 主要测试 | 当前状态与剩余缺口 |
|---|---|---|---|---|---|
| T-01 | 被动窃听、长期私钥日后泄漏 | 每次会话使用独立临时密钥；长期私钥泄漏不能解密历史流量 | `handshake.rs` 的临时 X25519/Ed25519 握手；`key_schedule.rs` 从 transcript 和共享秘密派生会话密钥；秘密使用 `Zeroizing`/`zeroize` | `both_endpoints_establish_the_same_transcript_and_x25519_secret`、`both_handshake_endpoints_derive_identical_keys`、`explicit_erase_zeroizes_all_output_keys` | 本地原语已验证；生产客户端仍走静态 X25519，未获得前向保密 |
| T-02 | 恶意 Coordinator/Relay 或主动攻击者替换公钥、临时密钥或会话字段 | 双方直接验证长期身份签名；所有握手字段绑定到同一 transcript；临时密钥替换必须失败 | `handshake.rs` 校验 HELLO/ACCEPT 身份、签名、transcript、时间和 endpoint context；`transcript.rs` 使用长度前缀与域分离 | `mitm_ephemeral_key_substitution_fails_in_both_directions`、`hello_signature_tampering_fails_closed`、`accept_signature_tampering_fails_closed`、`wrong_identity_and_wrong_endpoint_are_rejected` | 经典握手原语覆盖较强；尚未接入客户端身份目录和真实控制通道 |
| T-03 | 算法套件降级、算法混淆或协商结果被替换 | 数据套件只由已认证 transcript 决定；未知或降级套件失败关闭；双方套件必须一致 | `handshake.rs`、`transcript.rs` 固定 suite；`handshake_epoch.rs` 从认证握手选择数据套件；`lib.rs` 拒绝跨套件状态 | `protocol_downgrade_attempts_fail_even_with_a_valid_responder_signature`、`unsupported_suite_and_zero_ephemeral_key_are_rejected`、`aes_suite_is_selected_from_the_authenticated_handshake`、`mixed_suite_epoch_pairs_are_rejected` | classic profile 已有本地验证；混合后量子能力协商和迁移策略未实现 |
| T-04 | Transcript 边界歧义、跨会话拼接或角色/epoch/SID 错绑 | 每个字段无歧义编码；角色、双方身份、临时公钥、suite、SID、epoch、endpoint 和 PSK 状态均被绑定 | `transcript.rs` canonical builder；`handshake.rs` 校验 ACCEPT 与 HELLO 一致；`handshake_epoch.rs` 再校验确认值 | `canonical_transcript_matches_the_fixed_vector`、`every_bound_field_changes_the_hash`、`length_prefixes_prevent_field_boundary_ambiguity`、`a_different_transcript_cannot_authorize_installation` | 固定向量和字段敏感性已覆盖；独立协议评审、模糊测试和跨语言向量未完成 |
| T-05 | 数据帧重放、重排、跨方向反射、跨会话拼接或计数器复用 | `(session_id, direction, counter)` 每密钥最多使用一次；未认证包不推进窗口；旧包只在窗口/宽限内接受 | `lib.rs` 固定头、方向 Nonce、长度绑定 AAD、4096 包窗口和 32-bit 包上限；接收状态仅在认证成功后提交 | `data_aad_binds_header_session_and_payload_length`、`suite_mismatch_tampering_and_reflection_fail_without_consuming_counters`、`authenticated_packet_cannot_be_replayed`、`packets_can_be_opened_out_of_order`、`packet_counter_never_wraps` | 本地快路径覆盖充分；生产 UDP/TUN 仍使用随机 Nonce 且无重放窗口 |
| T-06 | 重协商回滚、半开事务耗尽、旧 epoch 密钥泄漏或错误确认激活新密钥 | Responder 在确认前不安装新密钥；epoch/SID 单调；超时失败关闭；旧接收密钥仅短期宽限；重复消息有界幂等 | `rekey.rs` 状态机；`rekey_codec.rs` 固定帧；`rekey_session.rs` 暂存并原子安装；`EpochSession::rotate` 管理宽限 | `rekey_frames_round_trip_with_canonical_lengths`、`all_frame_types_reject_truncated_and_trailing_bytes`、`rekey_installs_both_endpoints_only_after_confirmation_and_graces_old_receive`、`mismatched_material_and_bad_confirm_preserve_the_old_epoch`、`timeout_discards_staged_responder_keys_without_rotation` | 本地状态机与 epoch 桥接已实现；未接入真实控制通道、密钥提供方和数据面轮换 |
| T-07 | 握手反射放大、CPU 耗尽、内存耗尽或重放缓存污染 | 未验证请求不创建昂贵会话；cookie 绑定源地址/身份/临时密钥/时间；缓存和半开事务有上限；无效输入在分配前拒绝 | `cookie.rs` 41 字节 HMAC cookie 和有界策略；`handshake.rs` 有界 replay cache；`rekey.rs` 半开事务上限；各 decoder 边界检查 | `challenge_never_amplifies_and_has_a_canonical_frame`、`cookie_validates_only_the_original_source_identity_and_ephemeral_key`、`cookie_rejects_expired_future_and_wrong_key_values`、`replay_cache_fails_closed_when_full`、`decoder_rejects_oversized_unknown_and_mismatched_frames` | cookie 和缓存原语存在；尚无生产握手限速、每源预算、全局背压和持续模糊测试 |
| T-08 | 密钥、Nonce、PSK 或签名秘密进入内存转储、日志、panic 或 Debug | 秘密及时清零；认证比较恒定时间；日志和 Debug 只暴露公开标识；错误不包含秘密 | `zeroize` 包装和显式擦除；`pq.rs` 脱敏 Debug；各校验路径在状态变更前完成认证 | `explicit_erase_zeroizes_all_output_keys`、`secret_debug_output_is_redacted`、`fixed_tag`/KAT 测试、各 `fail_closed` 负向测试 | 本地原语有清理和脱敏测试；生产运行时日志、panic hook、core dump 和长期身份生命周期未系统验证 |
| T-09 | 截断、尾随字节、保留字段非零、超长帧或随机输入触发 panic/越界/歧义 | 所有解码器做精确长度和值域检查；失败不改变状态；随机输入不 panic | `handshake.rs`、`transcript.rs`、`rekey_codec.rs`、`cookie.rs`、`lib.rs`、`pq.rs` 的严格解码 | `all_frame_types_reject_truncated_and_trailing_bytes`、`decoder_rejects_versions_reserved_bytes_and_body_length_changes`、`cookie_parser_rejects_version_truncation_trailing_bytes_and_tampering`、`fixed_size_decoders_reject_truncation_and_trailing_bytes`、`data_header_rejects_invalid_fields` | 定向负向测试和短时 `cargo fuzz` CI 已存在；长期 fuzz、panic=abort 和真实生产数据面 fuzz 尚未完成 |
| T-10 | 量子攻击者先存储后解密，或后量子身份迁移被静默降级 | X25519 与 ML-KEM 结果必须组合；Ed25519 与 ML-DSA 必须共同验证；降级必须显式策略；混合组件不能单独授权 | `pq.rs` 锁定 ML-KEM-768/ML-DSA-65 严格编解码、KAT、清零与脱敏 Debug；`PROTOCOL_V2.md` 描述 X-Wing 门槛 | `ml_kem_768_keygen_matches_nist_acvp_vector`、`ml_kem_768_encapsulation_matches_nist_acvp_vector`、`ml_dsa_65_keygen_matches_nist_acvp_vector`、`ml_dsa_65_siggen_matches_nist_acvp_vector` | 仅原语适配；X-Wing、复合签名、握手接入、容量测试和独立审计均未完成 |
| T-11 | Relay/Coordinator 看到或篡改跨端身份，QUIC 连接与应用身份错绑 | QUIC 双向认证；证书/公钥绑定应用层身份；0-RTT 重放受限；错误证书和身份错绑失败关闭 | `common/src/quic.rs` 有服务端证书 pin 和客户端校验逻辑 | 现有测试主要覆盖证书 pin/编码；缺少客户端证书和身份绑定负向测试 | 生产缺口：服务端未启用客户端证书认证，应用层身份与证书未统一绑定 |
| T-12 | 性能优化引入降级、错误日志泄露或未经测量的“超越 WireGuard”声明 | 安全路径不得以关闭认证/重放检查换性能；发布材料只能引用可复现同条件数据 | `benches/dataplane.rs`、端到端基准、发布门禁和 `SECURITY_PERFORMANCE_TODO.md` | Criterion seal/open、批量路径、epoch 开销和端到端基线 | 当前 TCP 与小包性能显著落后 WireGuard；无回归阈值、空闲机复测和独立审计 |

## 3. 生产接入门槛

在以下条件满足前，T-01 至 T-09、T-12 只能标记为“本地原语已测试”，不能标记为生产安全属性：

1. `fuckipv6-client` 显式依赖并调用 `fuckipv6-secure-session`，不再让 UDP/TUN 路径回退到静态长期密钥。
2. 握手身份来源、Coordinator 公钥目录、endpoint context、时间源和能力协商完成端到端绑定。
3. Cookie、有界重放缓存、每源/每身份限速和全局背压接入真实握手入口。
4. 重协商控制帧进入真实控制通道，旧 epoch 宽限、超时和失败关闭有网络级集成测试。
5. 生产路径具备运行时计数：握手成功/失败、降级拒绝、重放拒绝、cookie 拒绝、重协商延迟和密钥销毁。
6. cargo-fuzz/AFL、依赖审计、SBOM、许可证扫描和性能回归阈值进入 CI。
7. Linux、Windows、macOS 实机或 CI 运行验证通过，并由独立厂商完成协议与密码学审计。

## 4. 文档与测试维护规则

- 新增或修改安全控制时，必须在本矩阵补一行或更新现有行。
- 测试名称变更时必须同步本文件，避免引用不存在的证据。
- “本地原语通过”只能更新“当前状态”，不能跳过“生产接入门槛”。
- 独立审计结论必须记录审计范围、版本、发现、修复状态和残余风险；本文不得替代审计报告。
