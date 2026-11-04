use anyhow::{bail, Result};
use fuckipv6_common::domain::{
    InvitationRedemption, JoinRequest, JoinRequestState, MembershipRole, NetworkId, NetworkMember,
    NetworkMemberDirectory, NetworkMemberGroup, NetworkMemberPresence, NetworkMemberStatus,
    NetworkRecord, NodeId,
};
use std::collections::BTreeMap;

use super::invitations::InvitationStore;
use super::join_requests::JoinRequestStore;
use super::session::SessionManager;

/// Assemble the only member projection sent to clients. The Coordinator reads
/// authoritative roster/request/redemption data and presence observations; no
/// adapter is allowed to infer status or merge an unfiltered request payload.
pub(crate) async fn build_member_directory(
    network_id: &NetworkId,
    actor: &NodeId,
    now_unix_ms: u64,
    join_requests: &JoinRequestStore,
    invitations: Option<&InvitationStore>,
    sessions: &SessionManager,
) -> Result<NetworkMemberDirectory> {
    let record = join_requests.member_directory_network(network_id)?;
    let is_member = record.members.contains(actor);
    let is_reviewer = record.owner == *actor || record.administrators.contains(actor);
    if !is_member && !is_reviewer {
        bail!("member_directory.error.permission_denied");
    }
    let requests = join_requests.member_directory_requests(network_id, actor, now_unix_ms)?;
    let redemptions = match invitations {
        Some(store) => store.member_directory_redemptions(network_id, actor)?,
        None => Vec::new(),
    };
    let presence = sessions.member_presence(now_unix_ms).await;
    let fingerprints = sessions.member_identity_fingerprints().await;

    Ok(NetworkMemberDirectory {
        network_id: network_id.clone(),
        generated_at_unix_ms: now_unix_ms,
        members: project_members(&record, &requests, &redemptions, &presence, &fingerprints),
    })
}

fn project_members(
    record: &NetworkRecord,
    requests: &[JoinRequest],
    redemptions: &[InvitationRedemption],
    presence: &BTreeMap<NodeId, NetworkMemberPresence>,
    fingerprints: &BTreeMap<NodeId, String>,
) -> Vec<NetworkMember> {
    let approved = requests
        .iter()
        .filter(|request| request.state == JoinRequestState::Approved)
        .fold(BTreeMap::new(), |mut entries, request| {
            insert_latest_request(&mut entries, request);
            entries
        });
    let pending = requests
        .iter()
        .filter(|request| {
            matches!(
                request.state,
                JoinRequestState::Pending | JoinRequestState::NeedsInformation
            )
        })
        .fold(BTreeMap::new(), |mut entries, request| {
            insert_latest_request(&mut entries, request);
            entries
        });
    let expired = requests
        .iter()
        .filter(|request| request.state == JoinRequestState::Expired)
        .fold(BTreeMap::new(), |mut entries, request| {
            insert_latest_request(&mut entries, request);
            entries
        });
    let redemption_by_node = redemptions
        .iter()
        .fold(BTreeMap::new(), |mut entries, redemption| {
            entries
                .entry(redemption.node_id.clone())
                .or_insert_with(|| redemption.clone());
            entries
        });

    let mut members = Vec::new();
    for node_id in &record.members {
        let role = if *node_id == record.owner {
            MembershipRole::Owner
        } else if record.administrators.contains(node_id) {
            MembershipRole::Administrator
        } else {
            MembershipRole::Member
        };
        let approved_request = approved.get(node_id);
        let redemption = redemption_by_node.get(node_id);
        let member_presence = presence
            .get(node_id)
            .cloned()
            .unwrap_or_else(NetworkMemberPresence::unknown);
        let capabilities = approved_request
            .map(|request| request.grant_preview.capabilities.clone())
            .or_else(|| redemption.map(|redemption| redemption.capabilities.clone()));
        members.push(NetworkMember {
            node_id: node_id.clone(),
            display_name: approved_request
                .and_then(|request| request.applicant.display_name.clone()),
            identity_fingerprint: approved_request
                .map(|request| request.applicant.public_key_fingerprint.clone()),
            role,
            groups: vec![group_for_role(role)],
            capabilities,
            status: if member_presence.state
                == fuckipv6_common::domain::NetworkMemberOnlineState::Online
            {
                NetworkMemberStatus::Active
            } else {
                NetworkMemberStatus::Approved
            },
            presence: member_presence,
            joined_at_unix_ms: approved_request
                .and_then(|request| request.approved_at_unix_ms)
                .or_else(|| redemption.map(|redemption| redemption.joined_at_unix_ms))
                .or_else(|| (role == MembershipRole::Owner).then_some(record.created_at_unix_ms)),
        });
    }

    for (node_id, request) in &pending {
        if record.members.contains(node_id) {
            continue;
        }
        members.push(NetworkMember {
            node_id: node_id.clone(),
            display_name: request.applicant.display_name.clone(),
            identity_fingerprint: Some(request.applicant.public_key_fingerprint.clone()),
            role: request.grant_preview.role,
            groups: Vec::new(),
            capabilities: Some(request.grant_preview.capabilities.clone()),
            status: NetworkMemberStatus::Pending,
            presence: NetworkMemberPresence::unknown(),
            joined_at_unix_ms: None,
        });
    }

    for (node_id, request) in expired {
        // A newer pending request for the same node is the current lifecycle
        // state. Never emit both the old expiry and the retry.
        if record.members.contains(&node_id)
            || approved.contains_key(&node_id)
            || pending.contains_key(&node_id)
        {
            continue;
        }
        members.push(NetworkMember {
            node_id,
            display_name: request.applicant.display_name.clone(),
            identity_fingerprint: Some(request.applicant.public_key_fingerprint.clone()),
            role: request.grant_preview.role,
            groups: Vec::new(),
            capabilities: Some(request.grant_preview.capabilities.clone()),
            status: NetworkMemberStatus::Expired,
            presence: NetworkMemberPresence::unknown(),
            joined_at_unix_ms: None,
        });
    }

    // A live session is the fallback identity source when no approval/redemption
    // profile exists. SessionManager exposes only the derived fingerprint.
    for member in &mut members {
        if member.identity_fingerprint.is_none() {
            member.identity_fingerprint = fingerprints.get(&member.node_id).cloned();
        }
    }

    members.sort_by(|left, right| {
        left.node_id
            .cmp(&right.node_id)
            .then_with(|| format!("{:?}", left.status).cmp(&format!("{:?}", right.status)))
    });
    members
}

fn insert_latest_request(entries: &mut BTreeMap<NodeId, JoinRequest>, request: &JoinRequest) {
    let node_id = request.applicant.node_id.clone();
    let replace = entries
        .get(&node_id)
        .is_none_or(|current| request.updated_at_unix_ms > current.updated_at_unix_ms);
    if replace {
        entries.insert(node_id, request.clone());
    }
}

fn group_for_role(role: MembershipRole) -> NetworkMemberGroup {
    match role {
        MembershipRole::Owner => NetworkMemberGroup::Owner,
        MembershipRole::Administrator => NetworkMemberGroup::Administrator,
        MembershipRole::Member => NetworkMemberGroup::Member,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        CapabilityKind, ExitContributionObligation, ExitContributionScope, InvitationId,
        JoinApplicantProfile, JoinRequestGrantPreview, JoinRequestReviewAction, MembershipTerms,
        NetworkCreateProfile, NetworkJoinMode, NetworkMemberOnlineState, NetworkVisibility,
        PublicKey,
    };
    use std::path::{Path, PathBuf};

    fn test_dir(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("fuckipv6-members-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn node(seed: &str) -> NodeId {
        NodeId::from_public_key(seed.as_bytes())
    }

    fn record() -> NetworkRecord {
        let owner = node("owner");
        let mut record = NetworkRecord::create_owned(
            NetworkCreateProfile {
                name: "MemberDirectory".into(),
                purpose: "Authoritative member projection".into(),
                icon: "network".into(),
                language: "zh-CN".into(),
                region: "ZZ".into(),
                tags: Vec::new(),
                capability_declarations: vec![CapabilityKind::File],
                visibility: NetworkVisibility::Private,
                join_mode: NetworkJoinMode::ApprovalRequired,
                exit_contribution: Default::default(),
                exit_contribution_scope: Default::default(),
            },
            PublicKey::from_bytes([42; 32]),
            owner,
            9_000,
        )
        .unwrap();
        record.members.push(node("member"));
        record.members.push(node("unverified-member"));
        record.validate().unwrap();
        record
    }

    fn request(seed: &str, nonce: &str, now: u64) -> JoinRequest {
        let applicant = node(seed);
        JoinRequest::new(
            JoinRequest::derived_id(&record().network_id, &applicant, nonce),
            record().network_id,
            JoinApplicantProfile {
                node_id: applicant,
                display_name: Some(format!("{seed}-display")),
                public_key_fingerprint: format!("{seed}-fingerprint"),
            },
            "SECRET-APPROVAL-REASON",
            MembershipTerms {
                network_policy_revision: 1,
                privacy_policy_revision: 1,
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
                member_limit: Some(20),
            },
            JoinRequestGrantPreview {
                role: MembershipRole::Member,
                capabilities: vec![CapabilityKind::File],
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
                file_access: true,
                grants_data_plane_access: true,
            },
            now,
            now + 60_000,
        )
        .unwrap()
    }

    fn write_record(path: &Path, record: &NetworkRecord) {
        std::fs::write(path, serde_json::to_vec_pretty(record).unwrap()).unwrap();
    }

    #[test]
    fn projection_omits_sensitive_material_and_preserves_unknown_presence_and_times() {
        let record = record();
        let mut approved = request("member", "approved", 1_000);
        approved
            .review(
                approved.revision,
                JoinRequestReviewAction::Approve,
                record.owner.clone(),
                None,
                2_000,
            )
            .unwrap();
        let pending = request("pending", "pending", 3_000);
        let redemption = InvitationRedemption {
            invitation_id: InvitationId::from_public_key(b"redemption-invitation"),
            network_id: record.network_id.clone(),
            node_id: node("unverified-member"),
            role: MembershipRole::Member,
            capabilities: vec![CapabilityKind::Exit],
            network_revision: 2,
            invitation_revision: 1,
            joined_at_unix_ms: 4_000,
            already_redeemed: false,
            grants_permanent_membership: true,
        };
        let presence = BTreeMap::from([(node("member"), NetworkMemberPresence::live(5_000))]);

        let projected = project_members(
            &record,
            &[approved, pending],
            &[redemption],
            &presence,
            &BTreeMap::new(),
        );
        let owner = projected
            .iter()
            .find(|member| member.node_id == node("owner"))
            .unwrap();
        assert_eq!(owner.presence.state, NetworkMemberOnlineState::Unknown);
        assert_eq!(owner.joined_at_unix_ms, Some(9_000));

        let member = projected
            .iter()
            .find(|member| member.node_id == node("member"))
            .unwrap();
        assert_eq!(member.status, NetworkMemberStatus::Active);
        assert_eq!(member.joined_at_unix_ms, Some(2_000));
        assert_eq!(member.presence.observed_at_unix_ms, Some(5_000));

        let unverified = projected
            .iter()
            .find(|member| member.node_id == node("unverified-member"))
            .unwrap();
        assert_eq!(unverified.status, NetworkMemberStatus::Approved);
        assert_eq!(unverified.joined_at_unix_ms, Some(4_000));
        assert_ne!(unverified.status, NetworkMemberStatus::Removed);

        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("SECRET-APPROVAL-REASON"));
        assert!(!encoded.contains("SECRET-INVITATION-CODE"));
        assert!(!encoded.contains("SECRET-INVITATION-DIGEST"));
        assert!(!encoded.contains("endpoint"));
        assert!(!encoded.contains("transfer"));
    }

    #[test]
    fn pending_retry_suppresses_an_older_expiry_for_the_same_node() {
        let record = record();
        let mut expired = request("retry", "first", 1_000);
        let expires_at = expired.expires_at_unix_ms;
        expired.expire_if_due(expires_at).unwrap();
        let pending = request("retry", "second", expires_at + 1);

        let projected = project_members(
            &record,
            &[expired, pending],
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let rows = projected
            .iter()
            .filter(|member| member.node_id == node("retry"))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, NetworkMemberStatus::Pending);
    }

    #[test]
    fn every_advertised_member_status_has_a_stable_wire_name() {
        for (status, wire_name) in [
            (NetworkMemberStatus::Pending, "pending"),
            (NetworkMemberStatus::Approved, "approved"),
            (NetworkMemberStatus::Active, "active"),
            (NetworkMemberStatus::Degraded, "degraded"),
            (NetworkMemberStatus::Suspended, "suspended"),
            (NetworkMemberStatus::Exited, "exited"),
            (NetworkMemberStatus::Removed, "removed"),
            (NetworkMemberStatus::Expired, "expired"),
        ] {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{wire_name}\"")
            );
        }
    }

    #[tokio::test]
    async fn non_member_and_missing_roster_fail_closed() {
        let dir = test_dir("permission");
        let record_path = dir.join("network.json");
        let record = record();
        write_record(&record_path, &record);
        let store = JoinRequestStore::new(
            dir.join("requests.json"),
            Some(record_path),
            None,
            Vec::new(),
        )
        .unwrap();
        let sessions = SessionManager::new("fd00::1000/120").unwrap();
        let error = build_member_directory(
            &record.network_id,
            &node("outsider"),
            10_000,
            &store,
            None,
            &sessions,
        )
        .await
        .expect_err("non-members must not receive a directory");
        assert_eq!(
            error.to_string(),
            "member_directory.error.permission_denied"
        );

        let unconfigured = JoinRequestStore::new(
            dir.join("unconfigured-requests.json"),
            None,
            None,
            Vec::new(),
        )
        .unwrap();
        let error = build_member_directory(
            &record.network_id,
            &record.owner,
            10_000,
            &unconfigured,
            None,
            &sessions,
        )
        .await
        .expect_err("a missing authoritative roster must fail closed");
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn owner_identity_is_not_inferred_from_network_wide_declarations() {
        let record = record();
        let projected = project_members(&record, &[], &[], &BTreeMap::new(), &BTreeMap::new());
        let owner = projected
            .iter()
            .find(|member| member.node_id == record.owner)
            .unwrap();
        assert!(owner.display_name.is_none());
        assert!(owner.identity_fingerprint.is_none());
        assert!(owner.capabilities.is_none());
    }
}
