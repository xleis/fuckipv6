use anyhow::{bail, Result};
use fuckipv6_common::domain::{
    authorize_member_lifecycle_action, member_lifecycle_target, CapabilityKind,
    InvitationRedemption, JoinRequest, JoinRequestState, MemberLifecycleAction,
    MemberLifecycleActorRole, MemberLifecycleRecord, MemberLifecycleStatus, MembershipRole,
    NetworkId, NetworkMember, NetworkMemberDirectory, NetworkMemberGroup, NetworkMemberPresence,
    NetworkRecord, NodeId,
};
use std::collections::{BTreeMap, BTreeSet};

use super::exit_availability::ExitAvailabilityObservationRegistry;
use super::invitations::InvitationStore;
use super::join_requests::JoinRequestStore;
use super::member_lifecycle::MemberLifecycleStore;
use super::session::SessionManager;

/// Assemble the only member projection sent to clients. The Coordinator reads
/// authoritative roster/request/redemption data and presence observations; no
/// adapter is allowed to infer status or merge an unfiltered request payload.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_member_directory(
    network_id: &NetworkId,
    actor: &NodeId,
    now_unix_ms: u64,
    join_requests: &JoinRequestStore,
    invitations: Option<&InvitationStore>,
    lifecycle: Option<&MemberLifecycleStore>,
    sessions: &SessionManager,
    exit_availability: &ExitAvailabilityObservationRegistry,
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
    let lifecycle_records = match lifecycle {
        Some(store) => store.records(network_id)?,
        None => Vec::new(),
    };
    let presence = sessions.member_presence(now_unix_ms).await;
    let fingerprints = sessions.member_identity_fingerprints().await;
    let viewer_role = role_for_viewer(&record, actor);

    Ok(NetworkMemberDirectory {
        network_id: network_id.clone(),
        generated_at_unix_ms: now_unix_ms,
        viewer_role,
        can_manage_members: viewer_role != MembershipRole::Member,
        members: project_members(
            &record,
            actor,
            &requests,
            &redemptions,
            &lifecycle_records,
            &presence,
            &fingerprints,
            now_unix_ms,
            exit_availability,
        ),
    })
}

#[allow(clippy::too_many_arguments)]
fn project_members(
    record: &NetworkRecord,
    actor: &NodeId,
    requests: &[JoinRequest],
    redemptions: &[InvitationRedemption],
    lifecycle_records: &[MemberLifecycleRecord],
    presence: &BTreeMap<NodeId, NetworkMemberPresence>,
    fingerprints: &BTreeMap<NodeId, String>,
    now_unix_ms: u64,
    exit_availability: &ExitAvailabilityObservationRegistry,
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
    let lifecycle_by_node =
        lifecycle_records
            .iter()
            .cloned()
            .fold(BTreeMap::new(), |mut entries, record| {
                entries.insert(record.node_id.clone(), record);
                entries
            });

    let mut node_ids = BTreeSet::new();
    node_ids.extend(record.members.iter().cloned());
    node_ids.extend(approved.keys().cloned());
    node_ids.extend(pending.keys().cloned());
    node_ids.extend(expired.keys().cloned());
    node_ids.extend(redemption_by_node.keys().cloned());
    // A lifecycle ledger may contain tombstones and invitations for nodes that
    // are not in this viewer's roster/request projection. Managers need those
    // tombstones to perform a safe undo, but extending an ordinary member's
    // directory from the ledger would leak unrelated Node IDs.
    if role_for_viewer(record, actor) != MembershipRole::Member {
        node_ids.extend(lifecycle_by_node.keys().cloned());
    }

    let actor_role = role_for_viewer(record, actor).into();
    let retained_administrators = record.administrators.len();
    let mut members = Vec::with_capacity(node_ids.len());
    for node_id in node_ids {
        let approved_request = approved.get(&node_id);
        let pending_request = pending.get(&node_id);
        let expired_request = expired.get(&node_id);
        let redemption = redemption_by_node.get(&node_id);
        let lifecycle = lifecycle_by_node.get(&node_id);
        let role = role_for_member(
            record,
            &node_id,
            lifecycle,
            approved_request,
            pending_request.or(expired_request),
            redemption,
        );
        let lifecycle_state = lifecycle
            .map(|record| record.status)
            .or_else(|| pending_request.map(|_| MemberLifecycleStatus::PendingApproval))
            .or_else(|| expired_request.map(|_| MemberLifecycleStatus::Expired))
            .unwrap_or(MemberLifecycleStatus::Approved);
        let member_presence = if lifecycle_state.grants_network_access() {
            presence
                .get(&node_id)
                .cloned()
                .unwrap_or_else(NetworkMemberPresence::unknown)
        } else {
            NetworkMemberPresence::unknown()
        };
        let capabilities = if lifecycle_state.grants_network_access() {
            approved_request
                .map(|request| request.grant_preview.capabilities.clone())
                .or_else(|| redemption.map(|redemption| redemption.capabilities.clone()))
        } else if lifecycle_state == MemberLifecycleStatus::PendingApproval {
            pending_request.map(|request| request.grant_preview.capabilities.clone())
        } else {
            None
        };
        let revision = lifecycle.map(|record| record.revision).unwrap_or(1);
        let access_epoch = lifecycle.map(|record| record.access_epoch).unwrap_or(1);
        let profile = approved_request.or(pending_request).or(expired_request);
        let declared_exit_available = capabilities
            .as_ref()
            .map(|capabilities| capabilities.contains(&CapabilityKind::Exit));
        let member_exit_availability = if lifecycle_state.grants_network_access() {
            exit_availability.project(
                &record.network_id,
                &node_id,
                declared_exit_available,
                now_unix_ms,
            )
        } else {
            Default::default()
        };
        members.push(NetworkMember {
            node_id: node_id.clone(),
            display_name: profile.and_then(|request| request.applicant.display_name.clone()),
            identity_fingerprint: profile
                .map(|request| request.applicant.public_key_fingerprint.clone()),
            role,
            groups: if record.members.contains(&node_id) {
                vec![group_for_role(role)]
            } else {
                Vec::new()
            },
            capabilities,
            status: lifecycle_state.to_member_status(member_presence.clone()),
            lifecycle_state,
            revision,
            access_epoch,
            available_actions: available_lifecycle_actions(
                actor,
                actor_role,
                &node_id,
                role,
                lifecycle_state,
                retained_administrators,
            ),
            presence: member_presence,
            exit_availability: member_exit_availability,
            joined_at_unix_ms: approved_request
                .and_then(|request| request.approved_at_unix_ms)
                .or_else(|| redemption.map(|redemption| redemption.joined_at_unix_ms))
                .or_else(|| (role == MembershipRole::Owner).then_some(record.created_at_unix_ms)),
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

fn role_for_viewer(record: &NetworkRecord, actor: &NodeId) -> MembershipRole {
    if record.owner == *actor {
        MembershipRole::Owner
    } else if record.administrators.contains(actor) {
        MembershipRole::Administrator
    } else {
        MembershipRole::Member
    }
}

fn role_for_member(
    record: &NetworkRecord,
    node_id: &NodeId,
    lifecycle: Option<&MemberLifecycleRecord>,
    approved: Option<&JoinRequest>,
    pending_or_expired: Option<&JoinRequest>,
    redemption: Option<&InvitationRedemption>,
) -> MembershipRole {
    if *node_id == record.owner {
        MembershipRole::Owner
    } else if record.administrators.contains(node_id) {
        MembershipRole::Administrator
    } else if let Some(record) = lifecycle {
        record.role
    } else if let Some(request) = approved.or(pending_or_expired) {
        request.grant_preview.role
    } else if let Some(redemption) = redemption {
        redemption.role
    } else {
        MembershipRole::Member
    }
}

fn available_lifecycle_actions(
    actor: &NodeId,
    actor_role: MemberLifecycleActorRole,
    target: &NodeId,
    target_role: MembershipRole,
    current: MemberLifecycleStatus,
    administrators: usize,
) -> Vec<MemberLifecycleAction> {
    let retained_administrators = administrators.saturating_sub(usize::from(
        target_role == MembershipRole::Administrator && target != actor,
    ));
    [
        MemberLifecycleAction::Invite,
        MemberLifecycleAction::Approve,
        MemberLifecycleAction::Reject,
        MemberLifecycleAction::Suspend,
        MemberLifecycleAction::Resume,
        MemberLifecycleAction::Exit,
        MemberLifecycleAction::Remove,
        MemberLifecycleAction::UndoRemove,
    ]
    .into_iter()
    .filter(|action| member_lifecycle_target(Some(current), *action).is_ok())
    .filter(|action| {
        authorize_member_lifecycle_action(
            actor,
            actor_role,
            target,
            target_role,
            *action,
            retained_administrators,
        )
        .is_ok()
    })
    .collect()
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
        CapabilityKind, ExitContributionObligation, ExitContributionScope, IdempotencyKey,
        InvitationId, JoinApplicantProfile, JoinRequestGrantPreview, JoinRequestReviewAction,
        MemberLifecycleAction, MemberLifecycleChangeRequest, MemberLifecycleStatus,
        MembershipTerms, NetworkCreateProfile, NetworkJoinMode, NetworkMemberOnlineState,
        NetworkMemberStatus, NetworkVisibility, PublicKey,
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
                exit_contribution_mode: Default::default(),
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
                exit_contribution_mode: Default::default(),
                member_limit: Some(20),
            },
            JoinRequestGrantPreview {
                role: MembershipRole::Member,
                capabilities: vec![CapabilityKind::File],
                exit_contribution: ExitContributionObligation::NotRequired,
                exit_contribution_scope: ExitContributionScope::NotApplicable,
                exit_contribution_mode: Default::default(),
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
            invited_by: NodeId::from_public_key(b"owner"),
            network_revision: 2,
            invitation_revision: 1,
            joined_at_unix_ms: 4_000,
            already_redeemed: false,
            grants_permanent_membership: true,
        };
        let presence = BTreeMap::from([(node("member"), NetworkMemberPresence::live(5_000))]);
        let exit_availability = ExitAvailabilityObservationRegistry::default();
        exit_availability
            .observe(record.network_id.clone(), node("member"), true, None, 5_000)
            .unwrap();

        let projected = project_members(
            &record,
            &record.owner,
            &[approved, pending],
            &[redemption],
            &[],
            &presence,
            &BTreeMap::new(),
            5_000,
            &exit_availability,
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
        assert_eq!(
            member.exit_availability.state,
            fuckipv6_common::domain::ExitAvailabilityState::Degraded
        );
        assert_eq!(member.exit_availability.declared_available, Some(false));
        assert_eq!(
            member.exit_availability.reason_code.as_deref(),
            Some("exit.availability.available_but_not_declared")
        );

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
        let now_unix_ms = expires_at + 1;

        let projected = project_members(
            &record,
            &record.owner,
            &[expired, pending],
            &[],
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            now_unix_ms,
            &ExitAvailabilityObservationRegistry::default(),
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
            None,
            &sessions,
            &ExitAvailabilityObservationRegistry::default(),
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
            None,
            &sessions,
            &ExitAvailabilityObservationRegistry::default(),
        )
        .await
        .expect_err("a missing authoritative roster must fail closed");
        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn directory_projects_authoritative_lifecycle_revision_actions_and_tombstones() {
        let dir = test_dir("lifecycle-projection");
        let record_path = dir.join("network.json");
        let record = record();
        write_record(&record_path, &record);
        let lifecycle =
            MemberLifecycleStore::new(dir.join("member_lifecycle.json"), record_path.clone())
                .unwrap();
        let target = node("member");
        lifecycle
            .change(
                &record.owner,
                MemberLifecycleChangeRequest {
                    network_id: record.network_id.clone(),
                    target_node_id: target.clone(),
                    action: MemberLifecycleAction::Remove,
                    role: MembershipRole::Member,
                    expected_revision: 1,
                    idempotency_key: IdempotencyKey::from_public_key(b"directory-remove"),
                    reason: None,
                },
                10_000,
            )
            .unwrap();
        let requests = JoinRequestStore::new(
            dir.join("requests.json"),
            Some(record_path),
            None,
            Vec::new(),
        )
        .unwrap();
        let sessions = SessionManager::new("fd00::1000/120").unwrap();
        let directory = build_member_directory(
            &record.network_id,
            &record.owner,
            11_000,
            &requests,
            None,
            Some(&lifecycle),
            &sessions,
            &ExitAvailabilityObservationRegistry::default(),
        )
        .await
        .unwrap();

        assert_eq!(directory.viewer_role, MembershipRole::Owner);
        assert!(directory.can_manage_members);
        let removed = directory
            .members
            .iter()
            .find(|member| member.node_id == target)
            .expect("removed lifecycle tombstone must remain visible for safe undo");
        assert_eq!(removed.lifecycle_state, MemberLifecycleStatus::Removed);
        assert_eq!(removed.status, NetworkMemberStatus::Removed);
        assert_eq!(removed.revision, 2);
        assert_eq!(removed.access_epoch, 2);
        assert!(removed
            .available_actions
            .contains(&MemberLifecycleAction::UndoRemove));
        assert!(!removed
            .available_actions
            .contains(&MemberLifecycleAction::Remove));

        let member_view = build_member_directory(
            &record.network_id,
            &node("unverified-member"),
            12_000,
            &requests,
            None,
            Some(&lifecycle),
            &sessions,
            &ExitAvailabilityObservationRegistry::default(),
        )
        .await
        .unwrap();
        assert_eq!(member_view.viewer_role, MembershipRole::Member);
        assert!(!member_view.can_manage_members);
        assert!(member_view.members.iter().all(|member| {
            member.available_actions.is_empty()
                || member.available_actions == vec![MemberLifecycleAction::Exit]
        }));
        assert!(
            member_view
                .members
                .iter()
                .all(|member| member.node_id != target),
            "ordinary members must not discover lifecycle-only tombstones"
        );
    }

    #[test]
    fn owner_identity_is_not_inferred_from_network_wide_declarations() {
        let record = record();
        let projected = project_members(
            &record,
            &record.owner,
            &[],
            &[],
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            9_000,
            &ExitAvailabilityObservationRegistry::default(),
        );
        let owner = projected
            .iter()
            .find(|member| member.node_id == record.owner)
            .unwrap();
        assert!(owner.display_name.is_none());
        assert!(owner.identity_fingerprint.is_none());
        assert!(owner.capabilities.is_none());
    }
}
