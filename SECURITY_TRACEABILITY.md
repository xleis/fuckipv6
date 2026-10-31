# 安全威胁—控制—测试追踪矩阵

> 状态：工程追踪文档，不是独立安全审计结论。  
> 基线：`55b53c5` 之后的生产 Secure Session v2 classic 数据面、QUIC 统一身份控制与实验性 hybrid 原语。
> 判定口径：源码存在不等于生产路径已接入；单元测试通过不等于协议已获独立评审。

## 1. 结论先行

| 声明 | 当前是否成立 | 依据 |
|---|---|---|
| UDP/TUN 生产路径使用 AEAD 加密 | 是，Secure Session v2 classic 临时会话密钥 | `P2pEngine` v2 握手/数据路由与双引擎真实 UDP 测试 |
| 拥有可测试的临时握手、重放窗口、重协商和运输无关会话驱动 v2 原语 | 是 | `crates/secure-session`；`driver.rs` 与集成测试 |
| 客户端生产路径已使用 v2 临时握手和计数 Nonce | 是 | `P2pEngine` 使用共享 UDP socket 完成 HELLO/ACCEPT/CONFIRM 并将 v2 密文交给 TUN/ACL 路径 |
| 已具备端到端后量子机密性和身份认证 | 否 | X-Wing、复合身份和实验性 `HELLO/ACCEPT` 已实现并有固定向量，但尚未接入生产控制通道或数据面 |
| QUIC 中继双向身份与通道绑定已实现 | 是，工程控制与回归测试已落地 | mandatory client auth、动态客户端证书 allowlist、TLS exporter、双 pin、Register 应用身份绑定和运行期黑名单；尚无 CA/CRL/OCSP、持久撤销列表、网络级最终授权或独立安全审计 |
| 已达到 WireGuard 安全等级 | 否 | 生产数据面、证书授权策略、跨平台验证和独立审计等缺口仍在 |
| 安全等级和性能超过 WireGuard | 否 | 完整安全与性能门槛均未满足 |

## 2. 追踪矩阵

| ID | 威胁/失败模式 | 预期安全不变量 | 当前控制与代码 | 主要测试 | 当前状态与剩余缺口 |
|---|---|---|---|---|---|
| T-01 | 被动窃听、长期私钥日后泄漏 | 每次会话使用独立临时密钥；长期私钥泄漏不能解密历史流量 | `secure_runtime.rs` 在生产连接中驱动临时 X25519 + Ed25519 握手；`key_schedule.rs` 从 transcript 和共享秘密派生会话密钥；秘密使用 `Zeroizing`/`zeroize` | `test_p2p_engine_data_transfer`、`both_endpoints_establish_the_same_transcript_and_x25519_secret`、`explicit_erase_zeroizes_all_output_keys` | 生产路径已接入临时握手；仍需独立密码学/协议审计和生产运行时密钥生命周期验证 |
| T-02 | 恶意 Coordinator/Relay 或主动攻击者替换公钥、临时密钥或会话字段 | 双方直接验证长期身份签名；所有握手字段绑定到同一 transcript；临时密钥替换必须失败 | `handshake.rs` 校验经典 HELLO/ACCEPT 身份、签名、transcript、时间和 endpoint context；`hybrid/handshake.rs` 校验完整 X-Wing transcript 和 Ed25519 + ML-DSA-65 双签名；`transcript.rs` 使用长度前缀与域分离 | 经典：`mitm_ephemeral_key_substitution_fails_in_both_directions`、`hello_signature_tampering_fails_closed`、`accept_signature_tampering_fails_closed`、`wrong_identity_and_wrong_endpoint_are_rejected`；hybrid：`canonical_hybrid_transcript_matches_fixed_vectors`、`hello_tampering_and_downgrade_fail_closed`、`accept_tampering_and_validly_signed_downgrade_fail_closed`、`classic_only_material_and_public_key_substitution_fail_closed` | classic 已接入生产 UDP；hybrid 仅在 crate 内，尚未接入真实控制通道、客户端身份目录或生产数据面 |
| T-03 | 算法套件降级、算法混淆或协商结果被替换 | 数据套件只由已认证 transcript 决定；未知或降级套件失败关闭；双方套件必须一致 | `handshake.rs`、`hybrid/handshake.rs`、`transcript.rs` 固定 suite/profile；`handshake_epoch.rs` 从认证握手选择数据套件；`lib.rs` 拒绝跨套件状态 | `protocol_downgrade_attempts_fail_even_with_a_valid_responder_signature`、`unsupported_suite_and_zero_ephemeral_key_are_rejected`、`aes_suite_is_selected_from_the_authenticated_handshake`、`mixed_suite_epoch_pairs_are_rejected`、`hello_tampering_and_downgrade_fail_closed`、`accept_tampering_and_validly_signed_downgrade_fail_closed`、`psk_mode_is_required_to_match_on_both_sides` | classic 已接入生产；hybrid 已拒绝本地有效签名降级，但完整后量子能力协商、迁移策略和生产接入未实现 |
| T-04 | Transcript 边界歧义、跨会话拼接或角色/epoch/SID 错绑 | 每个字段无歧义编码；角色、双方身份、临时公钥、suite、SID、epoch、endpoint 和 PSK 状态均被绑定 | `transcript.rs` canonical builder；`handshake.rs` 校验 ACCEPT 与 HELLO 一致；`handshake_epoch.rs` 再校验确认值 | `canonical_transcript_matches_the_fixed_vector`、`every_bound_field_changes_the_hash`、`length_prefixes_prevent_field_boundary_ambiguity`、`a_different_transcript_cannot_authorize_installation` | 固定向量和字段敏感性已覆盖；独立协议评审、模糊测试和跨语言向量未完成 |
| T-05 | 数据帧重放、重排、跨方向反射、跨会话拼接或计数器复用 | `(session_id, direction, counter)` 每密钥最多使用一次；未认证包不推进窗口；旧包只在窗口/宽限内接受 | `lib.rs` 固定头、方向 Nonce、长度绑定 AAD、4096 包窗口和 32-bit 包上限；生产 `P2pEngine` 在 TUN/ACL 前调用认证 open，接收状态仅在认证成功后提交 | `test_p2p_engine_data_transfer`、`data_aad_binds_header_session_and_payload_length`、`suite_mismatch_tampering_and_reflection_fail_without_consuming_counters`、`authenticated_packet_cannot_be_replayed` | 生产 UDP/TUN 和 relay 已覆盖篡改/重放失败关闭；仍需真实 WAN、跨平台与长期 fuzz 证据 |
| T-06 | 重协商回滚、半开事务耗尽、旧 epoch 密钥泄漏或错误确认激活新密钥 | Responder 在确认前不安装新密钥；epoch/SID 单调；超时失败关闭；旧接收密钥仅短期宽限；重复消息有界幂等 | `rekey.rs` 状态机；`rekey_codec.rs` 固定帧；`rekey_session.rs` 暂存并原子安装；`EpochSession::rotate` 管理宽限 | `rekey_frames_round_trip_with_canonical_lengths`、`all_frame_types_reject_truncated_and_trailing_bytes`、`rekey_installs_both_endpoints_only_after_confirmation_and_graces_old_receive`、`mismatched_material_and_bad_confirm_preserve_the_old_epoch`、`timeout_discards_staged_responder_keys_without_rotation` | 本地状态机与 epoch 桥接已实现；未接入真实控制通道、密钥提供方和数据面轮换 |
| T-07 | 握手反射放大、CPU 耗尽、内存耗尽或重放缓存污染 | 未验证请求不创建昂贵会话；cookie 绑定源地址/身份/临时密钥/时间；缓存和半开事务有上限；direct 与 Cookie retry 均受全局签名/KEM 验证预算约束；无效输入在分配前拒绝 | `cookie.rs` 41 字节 HMAC cookie 和有界策略；`handshake.rs` 有界 replay cache；`rekey.rs` 半开事务上限；`p2p/admission.rs` 全局 HELLO 验证预算与 RAII permit；各 decoder 边界检查 | `challenge_never_amplifies_and_has_a_canonical_frame`、`cookie_validates_only_the_original_source_identity_and_ephemeral_key`、`cookie_rejects_expired_future_and_wrong_key_values`、`replay_cache_fails_closed_when_full`、`hello_validation_budget_is_global_and_released_by_raii`、`exhausted_hello_budget_falls_back_to_cookie_and_recovers`、`cookie_retry_checks_cookie_before_shared_validation_budget` | Cookie、准入预算和缓存原语已接入 classic UDP；仍需负载自适应策略、准入前 `O(N)` Peer 扫描消除、产品指标/UI、跨节点容量压测和持续模糊测试 |
| T-08 | 密钥、Nonce、PSK 或签名秘密进入内存转储、日志、panic 或 Debug | 秘密及时清零；认证比较恒定时间；日志和 Debug 只暴露公开标识；错误不包含秘密 | `zeroize` 包装和显式擦除；`pq.rs` 脱敏 Debug；各校验路径在状态变更前完成认证 | `explicit_erase_zeroizes_all_output_keys`、`secret_debug_output_is_redacted`、`fixed_tag`/KAT 测试、各 `fail_closed` 负向测试 | 本地原语有清理和脱敏测试；生产运行时日志、panic hook、core dump 和长期身份生命周期未系统验证 |
| T-09 | 截断、尾随字节、保留字段非零、超长帧或随机输入触发 panic/越界/歧义 | 所有解码器做精确长度和值域检查；失败不改变状态；随机输入不 panic | `handshake.rs`、`transcript.rs`、`rekey_codec.rs`、`cookie.rs`、`lib.rs`、`pq.rs` 的严格解码 | `all_frame_types_reject_truncated_and_trailing_bytes`、`decoder_rejects_versions_reserved_bytes_and_body_length_changes`、`cookie_parser_rejects_version_truncation_trailing_bytes_and_tampering`、`fixed_size_decoders_reject_truncation_and_trailing_bytes`、`data_header_rejects_invalid_fields` | 定向负向测试和短时 `cargo fuzz` CI 已存在；长期 fuzz、panic=abort 和真实生产数据面 fuzz 尚未完成 |
| T-10 | 量子攻击者先存储后解密，或后量子身份迁移被静默降级 | X25519 与 ML-KEM 结果必须组合；Ed25519 与 ML-DSA 必须共同验证；降级必须显式策略；混合组件不能单独授权 | `pq.rs` 锁定 ML-KEM-768/ML-DSA-65 严格编解码、KAT、清零与脱敏 Debug；`hybrid/xwing.rs` 实现 draft-11 组合 KEM；`hybrid/identity.rs` 强制双签名；`hybrid/handshake.rs` 固定 TH1/TH2、PSK 模式、时间窗与有界 replay cache；`PROTOCOL_V2.md` 描述 hybrid 边界 | `ml_kem_768_keygen_matches_nist_acvp_vector`、`ml_dsa_65_siggen_matches_nist_acvp_vector`、`matches_all_draft11_fixed_vectors`、`composite_identity_round_trips_and_verifies_both_signatures`、`canonical_hybrid_transcript_matches_fixed_vectors`、`psk_mode_is_required_to_match_on_both_sides`、`replayed_hello_is_rejected_without_poisoning_the_cache` | 原语、复合身份和两消息实验 profile 已本地验证；生产接入、Initiator CONFIRM、Cookie/限速、容量性能测试和独立审计仍未完成 |
| T-11 | Relay/Coordinator 看到或篡改跨端身份，QUIC 连接与应用身份错绑，或未准入/已撤销身份继续中继 | QUIC 双向认证；relay TLS 只接受当前会话 allowlist；证书/公钥绑定应用层身份；0-RTT 重放受限；错误证书、身份错绑和运行期撤销身份失败关闭 | `common/src/quic.rs` 强制客户端证书、同步 allowlist 授权并验证 TLS exporter；`signaling/session.rs` 随注册、换钥、注销和按节点 ID 封禁维护授权快照；`relay/server.rs` 在写状态前校验 Register 的全字段签名、双 pin、证书 SPKI 和应用公钥，并在注册前及每个转发包校验源、目的运行期黑名单 | `client_certificate_allowlist_accepts_authorized_identity`、`client_certificate_allowlist_rejects_unlisted_identity`、`empty_client_certificate_allowlist_rejects_all_identities`、`client_certificate_authorizer_tracks_registration_rekey_unregistration_and_ban`、`mandatory_client_auth_rejects_missing_client_certificate`、`mandatory_client_auth_rejects_non_client_auth_certificate`、`authenticated_endpoints_derive_same_relay_channel_binding`、`authenticated_client_rejects_wrong_server_pin`、`resumed_session_cannot_send_early_register_or_packet`、`quic_relay_rejects_unregistered_tls_certificate_before_registration`、`quic_relay_revokes_certificate_allowlist_after_session_unregister`、`quic_relay_rejects_valid_tls_certificate_with_another_application_identity`、`quic_relay_rejects_tampered_registration_fields_without_state_changes`、`quic_relay_rejects_missing_registration_binding_without_state_changes`、`quic_relay_rejects_blacklisted_registration_without_state_changes`、`quic_relay_blocks_revoked_clients_in_both_directions` | Relay TLS allowlist 与运行期撤销已覆盖握手、注册和双向转发；仍缺 CA/CRL/OCSP、持久撤销列表、P2P 同等身份目录、网络级最终授权、证书生命周期生产验证和独立安全审计 |
| T-12 | 性能优化引入降级、错误日志泄露或未经测量的“超越 WireGuard”声明 | 安全路径不得以关闭认证/重放检查换性能；发布材料只能引用可复现同条件数据 | `benches/dataplane.rs`、端到端基准、发布门禁和 `SECURITY_PERFORMANCE_TODO.md` | Criterion seal/open、批量路径、epoch 开销和端到端基线 | 当前 TCP 与小包性能显著落后 WireGuard；无回归阈值、空闲机复测和独立审计 |

## 3. 生产接入门槛

在以下条件满足前，T-01 至 T-09、T-12 只能标记为“本地原语已测试”，不能标记为生产安全属性：

1. [x] 将已验证的 UDP 会话适配器接入生产 `P2pEngine` UDP/TUN 与 relay 密文路径；legacy/static 数据失败关闭，不提供静默降级。
2. [ ] 握手身份来源、Coordinator 公钥目录、endpoint context、时间源和完整能力协商完成端到端绑定；当前经典套件与 PeerInfo 公钥路径仍待审计。
3. [x] Cookie、有界重放缓存、每源/每身份配额和全局 HELLO 验证背压接入真实 classic UDP 握手入口；负载自适应 Cookie、准入前 Peer 路由优化和跨节点容量证据仍待补齐。
4. 重协商控制帧进入真实控制通道，旧 epoch 宽限、超时和失败关闭有网络级集成测试。
5. 生产路径具备运行时计数：握手成功/失败、降级拒绝、重放拒绝、cookie 拒绝、重协商延迟和密钥销毁。
6. cargo-fuzz/AFL、依赖审计、SBOM、许可证扫描和性能回归阈值进入 CI。
7. Linux、Windows、macOS 实机或 CI 运行验证通过，并由独立厂商完成协议与密码学审计。
8. [ ] 在 Relay 动态客户端证书 allowlist 基础上补齐 CA、CRL/OCSP、持久撤销列表、P2P 同等身份目录与网络级最终授权，并对证书生命周期、时钟和错误/panic 日志做生产验证；当前 allowlist 与运行期 Coordinator 黑名单均不等同于证书层撤销体系。

## 4. 文档与测试维护规则

- 新增或修改安全控制时，必须在本矩阵补一行或更新现有行。
- 测试名称变更时必须同步本文件，避免引用不存在的证据。
- “本地原语通过”只能更新“当前状态”，不能跳过“生产接入门槛”。
- 独立审计结论必须记录审计范围、版本、发现、修复状态和残余风险；本文不得替代审计报告。
