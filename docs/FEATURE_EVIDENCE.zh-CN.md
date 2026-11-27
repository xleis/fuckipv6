# 功能—证据映射表

> 由 `scripts/feature-evidence-matrix.py` 从共享 manifest 生成并校验：表中的每一行对应 manifest 里的一个可见动作，
> 证据列必须指向仓库内**真实提到该命令**的文件；缺失或对不上即视为“看起来可用但没有可验证路径”。

- 可见动作：**77**（implemented 77 / 未实现 0）
- 生成状态：**passed**

| 页面 | 动作 | 命令 | 权限 | 审计策略 | 后端 | 证据 |
|---|---|---|---|---|---|---|
| `networks` | `create_network` | `create_network` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `preview_network_profile` | `preview_network_profile` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `update_network_profile` | `update_network_profile` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `publish_network_discovery` | `publish_network_discovery` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `update_network_discovery` | `update_network_discovery` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `unpublish_network_discovery` | `unpublish_network_discovery` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `refresh_discovery_index` | `refresh_discovery_index` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `retry_discovery_index` | `retry_discovery_index` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `preview_join_network` | `preview_join_network` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `join_network` | `join_network` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `submit_join_request` | `submit_join_request` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `list_join_requests` | `list_join_requests` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `list_network_members` | `list_network_members` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `preview_member_removal` | `preview_member_removal` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `change_member_lifecycle` | `change_member_lifecycle` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `review_join_request` | `review_join_request` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `batch_review_join_requests` | `batch_review_join_requests` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `withdraw_join_request` | `withdraw_join_request` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `resubmit_join_request` | `resubmit_join_request` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `create_invitation` | `create_invitation` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `list_invitations` | `list_invitations` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `get_invitation_status` | `get_invitation_status` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `revoke_invitation` | `revoke_invitation` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `preview_invitation` | `preview_invitation` | `manage_networks` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `redeem_invitation` | `redeem_invitation` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `leave_network` | `leave_network` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `acknowledge_membership_policy` | `acknowledge_membership_policy` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `get_coordinator_candidates` | `get_coordinator_candidates` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `update_coordinator_candidates` | `update_coordinator_candidates` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `publish_coordinator_candidate_declaration` | `publish_coordinator_candidate_declaration` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `start_networks` | `start_networks` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `stop_networks` | `stop_networks` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `restart_networks` | `restart_networks` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `networks` | `rebuild_tun` | `rebuild_tun` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `peers` | `probe_peer` | `probe_peer` | `probe_peers` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `peers` | `get_blacklist` | `get_blacklist` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `peers` | `ban_client` | `ban_client` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `peers` | `unban_client` | `unban_client` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `refresh_snapshot` | `get_snapshot` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `get_node_diagnostics` | `get_node_diagnostics` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `start_node` | `start_node` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `stop_node` | `stop_node` | `stop_node` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `restart_node` | `restart_node` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `cleanup_node_residuals` | `cleanup_node_residuals` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `submit_admin_recovery` | `submit_admin_recovery` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `get_admin_recovery_status` | `get_admin_recovery_status` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `list_admin_recoveries` | `list_admin_recoveries` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `revoke_admin_recovery` | `revoke_admin_recovery` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `node_runtime` | `execute_admin_recovery` | `execute_admin_recovery` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `update_routing` | `update_routing` | `change_routing` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `get_traffic_policy` | `get_traffic_policy` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `preview_traffic_policy` | `preview_traffic_policy` | `change_routing` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `preview_traffic_policy_rollback` | `preview_traffic_policy_rollback` | `change_routing` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `apply_traffic_policy` | `apply_traffic_policy` | `change_routing` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `rollback_traffic_policy` | `rollback_traffic_policy` | `change_routing` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `get_traffic_policy_history` | `get_traffic_policy_history` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `routing` | `test_traffic_policy` | `test_traffic_policy` | `change_routing` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `exit_node` | `set_local_exit_capability` | `set_local_exit_capability` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `exit_node` | `set_exit_node_role` | `set_network_role` | `manage_roles` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `relay` | `set_relay_role` | `set_network_role` | `manage_roles` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `identity` | `set_node_display_name` | `set_node_display_name` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `identity` | `rotate_node_identity` | `rotate_node_identity` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `get_network_allocation` | `get_network_allocation` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `update_network_allocation` | `update_network_allocation` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `get_configuration` | `get_configuration` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `preview_configuration` | `preview_configuration` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `apply_configuration` | `apply_configuration` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `rollback_configuration` | `rollback_configuration` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `get_configuration_history` | `get_configuration_history` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `test_policy` | `test_policy` | `manage_networks` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `get_policy` | `get_policy` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `preview_policy` | `preview_policy` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `preview_policy_rollback` | `preview_policy_rollback` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `apply_policy` | `apply_policy` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `rollback_policy` | `rollback_policy` | `manage_networks` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `get_policy_history` | `get_policy_history` | `view_state` | `none` | `implemented` | `crates/client/src/node/command.rs` |
| `settings` | `shutdown_node` | `shutdown` | `stop_node` | `record_outcome` | `implemented` | `crates/client/src/node/command.rs` |
