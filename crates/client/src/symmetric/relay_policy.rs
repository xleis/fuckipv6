//! Production Relay policy assembly for an embedded Coordinator.
//!
//! The Relay server deliberately knows only a trusted policy authority. This
//! adapter re-reads the durable network roster and policy transaction snapshot
//! for every decision so revocation and expiry take effect without restarting
//! the listener.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fuckipv6_common::domain::{
    NetworkAcl, NetworkRecord, NetworkRole, NodeId, NodeLocalAccessPolicy, RelayAccessPolicy,
    RelayAuthorizationDecision, RelayLimitPolicy, RelayMemberGrant, RelayMemberPolicyContext,
};
use fuckipv6_coordinator::relay::server::RelayPolicyAuthority;

use crate::config_manager::ConfigManager;
use crate::policy_transaction::{PolicySnapshot, PolicyTransactionStore};

/// Conservative limits used until an owner or network policy authors a
/// stricter/looser explicit value. They are always combined with the limits in
/// the network snapshot; the runtime additionally enforces hard task caps.
fn default_provider_limits() -> RelayLimitPolicy {
    RelayLimitPolicy {
        max_connections: Some(128),
        max_concurrent_streams: Some(1_024),
        max_bandwidth_kbps: Some(100_000),
        max_bytes: None,
        max_cpu_percent: None,
        max_memory_bytes: None,
        max_session_duration_secs: Some(24 * 60 * 60),
        quota_reset_at_unix_ms: None,
    }
}

/// Relay authorization backed by this node's durable configuration.
#[derive(Clone)]
pub(crate) struct DurableRelayPolicyAuthority {
    configs: ConfigManager,
    network_name: String,
    fallback_snapshot: PolicySnapshot,
}

impl DurableRelayPolicyAuthority {
    pub(crate) fn new(
        configs: ConfigManager,
        network_name: impl Into<String>,
        fallback_snapshot: PolicySnapshot,
    ) -> Self {
        Self {
            configs,
            network_name: network_name.into(),
            fallback_snapshot,
        }
    }

    fn load_record(&self) -> Result<NetworkRecord> {
        self.configs
            .load_network_record(&self.network_name)
            .map_err(anyhow::Error::new)
            .context("无法加载 Relay 网络名册")
    }

    fn load_snapshot(&self) -> Result<PolicySnapshot> {
        let store = PolicyTransactionStore::new(self.configs.config_dir().to_path_buf());
        store
            .current_snapshot(&self.fallback_snapshot)
            .map_err(anyhow::Error::new)
            .context("无法加载 Relay 策略事务快照")
    }

    fn load_policy_at(&self, now: u64) -> Result<(NetworkRecord, RelayAccessPolicy)> {
        let record = self.load_record()?;
        let snapshot = self.load_snapshot()?;
        policy_from_snapshot(&self.configs, &record, &snapshot, now)
    }
}

impl RelayPolicyAuthority for DurableRelayPolicyAuthority {
    fn current_policy(&self) -> Result<RelayAccessPolicy> {
        let now = now_unix_ms()?;
        self.load_policy_at(now).map(|(_, policy)| policy)
    }

    fn authorize_grant(
        &self,
        grant: &RelayMemberGrant,
        now: u64,
    ) -> Result<(RelayAccessPolicy, RelayAuthorizationDecision)> {
        let (record, mut policy) = self.load_policy_at(now)?;
        let membership_active = record.members.contains(&grant.consumer_node_id);
        let role = network_role(&record, &grant.consumer_node_id);
        let decision = policy
            .authorize_signed_grant(grant, role, BTreeSet::new(), membership_active, false, now)
            .context("Relay 使用方签名授权无效")?;
        Ok((policy, decision))
    }
}

fn policy_from_snapshot(
    configs: &ConfigManager,
    record: &NetworkRecord,
    snapshot: &PolicySnapshot,
    now: u64,
) -> Result<(NetworkRecord, RelayAccessPolicy)> {
    snapshot
        .validate_at(now)
        .context("Relay 策略事务快照无效")?;

    let identity = configs
        .load_node_identity()
        .context("无法加载 Relay 提供者身份")?;
    let provider_node_id = NodeId::from_public_key(&identity.public_key());
    if !record.members.contains(&provider_node_id) {
        anyhow::bail!("本节点不是该网络的成员，拒绝提供 Relay");
    }

    let provider_role = network_role(record, &provider_node_id);
    let provider_local = snapshot
        .local_policy
        .clone()
        .context("缺少节点 owner 编写的 Relay 本地授权策略")?;
    if provider_local.node_id != provider_node_id {
        anyhow::bail!("Relay 本地授权策略属于其他节点");
    }
    if !provider_local.network_scope.allows(&record.network_id) {
        anyhow::bail!("Relay 本地授权策略不允许此网络");
    }
    provider_local
        .validate_at(now)
        .context("Relay 本地授权策略无效或已过期")?;

    let network_acl = snapshot
        .network_acls
        .iter()
        .find(|acl| acl.network_id == record.network_id)
        .cloned()
        .context("缺少网络管理员编写的 Relay ACL")?;
    network_acl
        .validate_at(now)
        .context("Relay 网络 ACL 无效或已过期")?;

    let local_limits = provider_limits(&provider_local, &network_acl)?;
    let provider = RelayMemberPolicyContext {
        role: provider_role,
        groups: Default::default(),
        membership_active: true,
        recent_reauthentication: false,
        local_rules: Vec::new(),
        local_access_policy: Some(provider_local.clone()),
        limits: local_limits.clone(),
    };

    // Member entries are intentionally empty until a consumer presents a
    // verifiable local grant. Membership or visibility alone is never an
    // authorization to consume Relay.
    let members = BTreeMap::new();

    let policy = RelayAccessPolicy {
        network_id: record.network_id.clone(),
        provider_node_id,
        provider,
        existing_session_policy: Default::default(),
        revision: policy_revision(record, snapshot, &network_acl, &provider_local),
        updated_at_unix_ms: now,
        network_rules: Vec::new(),
        network_acl: Some(network_acl),
        members,
        limits: local_limits,
    };
    policy.validate_at(now).context("组装后的 Relay 策略无效")?;
    Ok((record.clone(), policy))
}

fn provider_limits(local: &NodeLocalAccessPolicy, acl: &NetworkAcl) -> Result<RelayLimitPolicy> {
    // The ACL and local policy currently carry governance, not resource quota
    // values. Keep the conservative provider defaults so an old snapshot never
    // silently removes every resource bound.
    let limits = default_provider_limits();
    limits.validate().map_err(anyhow::Error::new)?;
    local.validate_definition_for_relay(acl.network_id())?;
    Ok(limits)
}

fn policy_revision(
    record: &NetworkRecord,
    snapshot: &PolicySnapshot,
    acl: &NetworkAcl,
    local: &NodeLocalAccessPolicy,
) -> u64 {
    record
        .revision
        .max(record.policy_revision)
        .max(snapshot_revision(snapshot))
        .max(acl.revision)
        .max(local.revision)
        .max(1)
}

fn snapshot_revision(snapshot: &PolicySnapshot) -> u64 {
    // PolicyCurrent's transaction revision is not part of PolicySnapshot. The
    // document revisions still advance on mutation and are enough to prevent a
    // stale assembled policy from appearing newer. Monotonic rollback rejection
    // remains the Relay server's responsibility.
    snapshot
        .local_policy
        .as_ref()
        .map_or(0, |policy| policy.revision)
        .max(
            snapshot
                .network_acls
                .iter()
                .map(|acl| acl.revision)
                .max()
                .unwrap_or(0),
        )
}

fn network_role(record: &NetworkRecord, node_id: &NodeId) -> NetworkRole {
    if &record.owner == node_id {
        NetworkRole::Owner
    } else if record.administrators.contains(node_id) {
        NetworkRole::Administrator
    } else {
        NetworkRole::Member
    }
}

fn now_unix_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("系统时间超出 Relay 策略范围")
}

trait RelayNetworkId {
    fn network_id(&self) -> &fuckipv6_common::domain::NetworkId;
}

impl RelayNetworkId for NetworkAcl {
    fn network_id(&self) -> &fuckipv6_common::domain::NetworkId {
        &self.network_id
    }
}

trait RelayLocalPolicyValidation {
    fn validate_definition_for_relay(
        &self,
        network_id: &fuckipv6_common::domain::NetworkId,
    ) -> Result<()>;
}

impl RelayLocalPolicyValidation for NodeLocalAccessPolicy {
    fn validate_definition_for_relay(
        &self,
        network_id: &fuckipv6_common::domain::NetworkId,
    ) -> Result<()> {
        if !self.network_scope.allows(network_id) {
            anyhow::bail!("Relay 本地授权策略未覆盖当前网络");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_authority_files_denies_relay_policy() {
        let root = std::env::temp_dir().join(format!(
            "fuckipv6-relay-policy-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let configs = ConfigManager::new_for_dir(root.clone()).expect("config manager");
        let authority =
            DurableRelayPolicyAuthority::new(configs, "missing", PolicySnapshot::default());
        assert!(authority.current_policy().is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_limits_are_bounded_and_valid() {
        let limits = default_provider_limits();
        limits.validate().expect("default limits must validate");
        assert_eq!(limits.max_connections, Some(128));
        assert_eq!(limits.max_concurrent_streams, Some(1024));
        assert_eq!(limits.max_bandwidth_kbps, Some(100_000));
    }
}
