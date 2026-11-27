# Android 与桌面能力矩阵

> 由 `scripts/android-capability-matrix.py` 从共享 manifest 与 Android 主源码生成并校验。
> `mounted` 表示 Android 的 bridge/ui 源码里**真的出现**该命令字面量；只有注释或测试提到不算。

- 可见动作：**77**（唯一命令 76）
- Android 已挂载：**18**；桌面专属：**59**
- manifest 之外的 Android 能力：**8** 项（黑名单/File/事件流/VpnService）
- 生成状态：**passed**

| 页面 | 动作 | 命令 | 桌面 | Android | Android 证据 |
|---|---|---|---|---|---|
| `networks` | `create_network` | `create_network` | `available` | `desktop-only` | — |
| `networks` | `preview_network_profile` | `preview_network_profile` | `available` | `desktop-only` | — |
| `networks` | `update_network_profile` | `update_network_profile` | `available` | `desktop-only` | — |
| `networks` | `publish_network_discovery` | `publish_network_discovery` | `available` | `desktop-only` | — |
| `networks` | `update_network_discovery` | `update_network_discovery` | `available` | `desktop-only` | — |
| `networks` | `unpublish_network_discovery` | `unpublish_network_discovery` | `available` | `desktop-only` | — |
| `networks` | `refresh_discovery_index` | `refresh_discovery_index` | `available` | `desktop-only` | — |
| `networks` | `retry_discovery_index` | `retry_discovery_index` | `available` | `desktop-only` | — |
| `networks` | `preview_join_network` | `preview_join_network` | `available` | `desktop-only` | — |
| `networks` | `join_network` | `join_network` | `available` | `desktop-only` | — |
| `networks` | `submit_join_request` | `submit_join_request` | `available` | `desktop-only` | — |
| `networks` | `list_join_requests` | `list_join_requests` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/JoinRequestSurface.kt` |
| `networks` | `list_network_members` | `list_network_members` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/MemberSurface.kt` |
| `networks` | `preview_member_removal` | `preview_member_removal` | `available` | `desktop-only` | — |
| `networks` | `change_member_lifecycle` | `change_member_lifecycle` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/MemberLifecycle.kt` |
| `networks` | `review_join_request` | `review_join_request` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/JoinRequestSurface.kt` |
| `networks` | `batch_review_join_requests` | `batch_review_join_requests` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/JoinRequestSurface.kt` |
| `networks` | `withdraw_join_request` | `withdraw_join_request` | `available` | `desktop-only` | — |
| `networks` | `resubmit_join_request` | `resubmit_join_request` | `available` | `desktop-only` | — |
| `networks` | `create_invitation` | `create_invitation` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/InvitationSurface.kt` |
| `networks` | `list_invitations` | `list_invitations` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/InvitationSurface.kt` |
| `networks` | `get_invitation_status` | `get_invitation_status` | `available` | `desktop-only` | — |
| `networks` | `revoke_invitation` | `revoke_invitation` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/InvitationSurface.kt` |
| `networks` | `preview_invitation` | `preview_invitation` | `available` | `desktop-only` | — |
| `networks` | `redeem_invitation` | `redeem_invitation` | `available` | `desktop-only` | — |
| `networks` | `leave_network` | `leave_network` | `available` | `desktop-only` | — |
| `networks` | `acknowledge_membership_policy` | `acknowledge_membership_policy` | `available` | `desktop-only` | — |
| `networks` | `get_coordinator_candidates` | `get_coordinator_candidates` | `available` | `desktop-only` | — |
| `networks` | `update_coordinator_candidates` | `update_coordinator_candidates` | `available` | `desktop-only` | — |
| `networks` | `publish_coordinator_candidate_declaration` | `publish_coordinator_candidate_declaration` | `available` | `desktop-only` | — |
| `networks` | `start_networks` | `start_networks` | `available` | `desktop-only` | — |
| `networks` | `stop_networks` | `stop_networks` | `available` | `desktop-only` | — |
| `networks` | `restart_networks` | `restart_networks` | `available` | `desktop-only` | — |
| `networks` | `rebuild_tun` | `rebuild_tun` | `available` | `desktop-only` | — |
| `peers` | `probe_peer` | `probe_peer` | `available` | `desktop-only` | — |
| `peers` | `get_blacklist` | `get_blacklist` | `available` | `desktop-only` | — |
| `peers` | `ban_client` | `ban_client` | `available` | `desktop-only` | — |
| `peers` | `unban_client` | `unban_client` | `available` | `desktop-only` | — |
| `node_runtime` | `refresh_snapshot` | `get_snapshot` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/NodeApiConfig.kt` |
| `node_runtime` | `get_node_diagnostics` | `get_node_diagnostics` | `available` | `desktop-only` | — |
| `node_runtime` | `start_node` | `start_node` | `available` | `desktop-only` | — |
| `node_runtime` | `stop_node` | `stop_node` | `available` | `desktop-only` | — |
| `node_runtime` | `restart_node` | `restart_node` | `available` | `desktop-only` | — |
| `node_runtime` | `cleanup_node_residuals` | `cleanup_node_residuals` | `available` | `desktop-only` | — |
| `node_runtime` | `submit_admin_recovery` | `submit_admin_recovery` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/AdminRecoverySurface.kt` |
| `node_runtime` | `get_admin_recovery_status` | `get_admin_recovery_status` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/AdminRecoverySurface.kt` |
| `node_runtime` | `list_admin_recoveries` | `list_admin_recoveries` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/AdminRecoverySurface.kt` |
| `node_runtime` | `revoke_admin_recovery` | `revoke_admin_recovery` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/AdminRecoverySurface.kt` |
| `node_runtime` | `execute_admin_recovery` | `execute_admin_recovery` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/AdminRecoverySurface.kt` |
| `routing` | `update_routing` | `update_routing` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/RoutingSurface.kt` |
| `routing` | `get_traffic_policy` | `get_traffic_policy` | `available` | `desktop-only` | — |
| `routing` | `preview_traffic_policy` | `preview_traffic_policy` | `available` | `desktop-only` | — |
| `routing` | `preview_traffic_policy_rollback` | `preview_traffic_policy_rollback` | `available` | `desktop-only` | — |
| `routing` | `apply_traffic_policy` | `apply_traffic_policy` | `available` | `desktop-only` | — |
| `routing` | `rollback_traffic_policy` | `rollback_traffic_policy` | `available` | `desktop-only` | — |
| `routing` | `get_traffic_policy_history` | `get_traffic_policy_history` | `available` | `desktop-only` | — |
| `routing` | `test_traffic_policy` | `test_traffic_policy` | `available` | `desktop-only` | — |
| `exit_node` | `set_local_exit_capability` | `set_local_exit_capability` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/ExitSurface.kt` |
| `exit_node` | `set_exit_node_role` | `set_network_role` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/CommandJson.kt` |
| `relay` | `set_relay_role` | `set_network_role` | `available` | `mounted` | `cc/xlei/fuckipv6/android/bridge/CommandJson.kt` |
| `identity` | `set_node_display_name` | `set_node_display_name` | `available` | `desktop-only` | — |
| `identity` | `rotate_node_identity` | `rotate_node_identity` | `available` | `desktop-only` | — |
| `settings` | `get_network_allocation` | `get_network_allocation` | `available` | `desktop-only` | — |
| `settings` | `update_network_allocation` | `update_network_allocation` | `available` | `desktop-only` | — |
| `settings` | `get_configuration` | `get_configuration` | `available` | `desktop-only` | — |
| `settings` | `preview_configuration` | `preview_configuration` | `available` | `desktop-only` | — |
| `settings` | `apply_configuration` | `apply_configuration` | `available` | `desktop-only` | — |
| `settings` | `rollback_configuration` | `rollback_configuration` | `available` | `desktop-only` | — |
| `settings` | `get_configuration_history` | `get_configuration_history` | `available` | `desktop-only` | — |
| `settings` | `test_policy` | `test_policy` | `available` | `desktop-only` | — |
| `settings` | `get_policy` | `get_policy` | `available` | `desktop-only` | — |
| `settings` | `preview_policy` | `preview_policy` | `available` | `desktop-only` | — |
| `settings` | `preview_policy_rollback` | `preview_policy_rollback` | `available` | `desktop-only` | — |
| `settings` | `apply_policy` | `apply_policy` | `available` | `desktop-only` | — |
| `settings` | `rollback_policy` | `rollback_policy` | `available` | `desktop-only` | — |
| `settings` | `get_policy_history` | `get_policy_history` | `available` | `desktop-only` | — |
| `settings` | `shutdown_node` | `shutdown` | `available` | `desktop-only` | — |

## manifest 之外的 Android 侧能力

| 能力 | 通道 | 标识 | Android |
|---|---|---|---|
| 客户端黑名单查询 | `command` | `get_blacklist` | `mounted` |
| 客户端封禁 | `command` | `ban_client` | `mounted` |
| 客户端解除封禁 | `command` | `unban_client` | `mounted` |
| File 上传 | `rest` | `/api/v1/file/uploads` | `mounted` |
| File 下载 | `rest` | `/api/v1/file/downloads` | `mounted` |
| File 传输列表/控制 | `rest` | `/api/v1/file/transfers` | `mounted` |
| SSE 事件订阅 | `rest` | `/api/v1/events` | `mounted` |
| VpnService 启停 | `platform` | `TUNNEL_START` | `mounted` |
