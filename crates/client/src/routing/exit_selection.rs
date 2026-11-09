//! Runtime enforcement of administrator-owned Exit selection.
//!
//! The shared domain resolution owns deny-first rule semantics. This module is
//! the trusted boundary between an authenticated member directory and the data
//! plane: it validates directory freshness and policy revision monotonicity,
//! turns roster observations into health evidence, and fails closed whenever a
//! decision cannot be proven from fresh authority.

use std::collections::BTreeMap;

use fuckipv6_common::domain::{
    DomainError, ExitAvailabilitySource, ExitAvailabilityState, ExitContributionAvailability,
    ExitSelectionResolution, ExitSessionTransition, NetworkId, NetworkMember,
    NetworkMemberDirectory, NetworkMemberOnlineState, NodeId,
};

pub(crate) const EXIT_SELECTION_DIRECTORY_MAX_AGE_MS: u64 = 90_000;
pub(crate) const EXIT_SELECTION_AVAILABILITY_MAX_AGE_MS: u64 = 90_000;

/// Result of merging a directory into the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitSelectionDirectoryUpdate {
    Applied,
    /// The existing directory is newer by policy revision or generation and was
    /// deliberately retained. This is not permission to clear the gate.
    IgnoredStale,
}

/// A directory rejected before it can influence selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitSelectionRuntimeDenial {
    MissingNetworkContext,
    DirectoryNetworkMismatch,
    InvalidPolicyRevision,
    InvalidPolicy(DomainError),
    DirectoryUnavailable,
    RequesterNotInRoster,
    RequesterNotActive,
    Selection(DomainError),
}

impl ExitSelectionRuntimeDenial {
    pub(crate) fn reason_code(&self) -> &str {
        match self {
            Self::MissingNetworkContext => "exit.selection.missing_network_context",
            Self::DirectoryNetworkMismatch => "exit.selection.directory_network_mismatch",
            Self::InvalidPolicyRevision => "exit.selection.invalid_policy_revision",
            Self::InvalidPolicy(error) => error.code(),
            Self::DirectoryUnavailable => "exit.selection.directory_unavailable",
            Self::RequesterNotInRoster => "exit.selection.requester_not_in_roster",
            Self::RequesterNotActive => "exit.selection.requester_not_active",
            Self::Selection(error) => error.code(),
        }
    }
}

/// Stateful per-network selection gate. Policy and roster updates are pushed by
/// the runtime control plane; packet handling only reads this cached authority.
#[derive(Debug)]
pub(crate) struct ExitSelectionGate {
    local_node_id: NodeId,
    network_id: Option<NetworkId>,
    member_directory: Option<NetworkMemberDirectory>,
    member_directory_received_at_unix_ms: Option<u64>,
    current_exit: Option<NodeId>,
    pending_exit: Option<NodeId>,
}

impl ExitSelectionGate {
    pub(crate) fn new(local_node_id: NodeId, network_id: Option<NetworkId>) -> Self {
        Self {
            local_node_id,
            network_id,
            member_directory: None,
            member_directory_received_at_unix_ms: None,
            current_exit: None,
            pending_exit: None,
        }
    }

    /// Merge an authenticated directory projection. A lower policy revision or
    /// an older generation at the same revision never replaces fresher state.
    pub(crate) fn replace_member_directory(
        &mut self,
        directory: NetworkMemberDirectory,
        received_at_unix_ms: u64,
    ) -> Result<ExitSelectionDirectoryUpdate, ExitSelectionRuntimeDenial> {
        let network_id = self
            .network_id
            .as_ref()
            .ok_or(ExitSelectionRuntimeDenial::MissingNetworkContext)?;
        if directory.network_id != *network_id {
            return Err(ExitSelectionRuntimeDenial::DirectoryNetworkMismatch);
        }
        if directory.exit_selection_policy_revision == 0 {
            return Err(ExitSelectionRuntimeDenial::InvalidPolicyRevision);
        }
        directory
            .exit_selection_policy
            .validate()
            .map_err(ExitSelectionRuntimeDenial::InvalidPolicy)?;

        if let Some(current) = self.member_directory.as_ref() {
            let incoming_is_older = directory.exit_selection_policy_revision
                < current.exit_selection_policy_revision
                || (directory.exit_selection_policy_revision
                    == current.exit_selection_policy_revision
                    && directory.generated_at_unix_ms < current.generated_at_unix_ms);
            if incoming_is_older {
                return Ok(ExitSelectionDirectoryUpdate::IgnoredStale);
            }
        }

        self.member_directory = Some(directory);
        self.member_directory_received_at_unix_ms = Some(received_at_unix_ms);
        Ok(ExitSelectionDirectoryUpdate::Applied)
    }

    pub(crate) fn clear_member_directory(&mut self) {
        self.member_directory = None;
        self.member_directory_received_at_unix_ms = None;
        self.current_exit = None;
        self.pending_exit = None;
    }

    /// Record the Exit currently carrying this node's established session.
    pub(crate) fn set_current_exit(&mut self, exit: Option<NodeId>) {
        self.current_exit = exit;
    }

    pub(crate) fn current_exit(&self) -> Option<&NodeId> {
        self.current_exit.as_ref()
    }

    /// Stage an administrator-authorized replacement without changing the
    /// established Exit. Packet admission must stay closed until recovery
    /// evidence commits this pending target.
    pub(crate) fn begin_pending_exit(&mut self, exit: NodeId) -> bool {
        if self.pending_exit.is_some() || self.current_exit.as_ref() == Some(&exit) {
            return false;
        }
        self.pending_exit = Some(exit);
        true
    }

    pub(crate) fn pending_exit(&self) -> Option<&NodeId> {
        self.pending_exit.as_ref()
    }

    /// Commit exactly the staged target. Callers must only invoke this after a
    /// successful recovery pass with complete traffic-session validation.
    pub(crate) fn commit_pending_exit(&mut self) -> Option<NodeId> {
        let pending = self.pending_exit.take()?;
        self.current_exit = Some(pending.clone());
        Some(pending)
    }

    pub(crate) fn discard_pending_exit(&mut self) {
        self.pending_exit = None;
    }

    /// Resolve the final administrator-authorized set for the local member.
    /// Missing, stale or invalid authority always fails closed.
    pub(crate) fn resolution(
        &self,
        now_unix_ms: u64,
    ) -> Result<ExitSelectionResolution, ExitSelectionRuntimeDenial> {
        let directory = trusted_directory(
            self.network_id.as_ref(),
            self.member_directory.as_ref(),
            self.member_directory_received_at_unix_ms,
            now_unix_ms,
        )?;
        resolve_directory_exit_selection(&self.local_node_id, directory, now_unix_ms)
    }

    /// Select the Exit for a new or reopened session. `requested` is the local
    /// preference and may not escape the administrator-authorized set.
    pub(crate) fn select_for_session(
        &self,
        now_unix_ms: u64,
        requested: Option<&NodeId>,
    ) -> Result<NodeId, ExitSelectionRuntimeDenial> {
        self.resolution(now_unix_ms)?
            .select_for_session(requested)
            .map_err(ExitSelectionRuntimeDenial::Selection)
    }

    /// Plan the transition for the established session using the current
    /// directory policy. The caller applies the returned action before routing
    /// another packet through the old Exit.
    pub(crate) fn plan_current_session_transition(
        &self,
        now_unix_ms: u64,
    ) -> Result<ExitSessionTransition, ExitSelectionRuntimeDenial> {
        let directory = self.fresh_directory(now_unix_ms)?;
        let resolution = self.resolution(now_unix_ms)?;
        Ok(resolution
            .plan_session_transition(self.current_exit.as_ref(), &directory.exit_selection_policy))
    }

    fn fresh_directory(
        &self,
        now_unix_ms: u64,
    ) -> Result<&NetworkMemberDirectory, ExitSelectionRuntimeDenial> {
        trusted_directory(
            self.network_id.as_ref(),
            self.member_directory.as_ref(),
            self.member_directory_received_at_unix_ms,
            now_unix_ms,
        )
    }
}

/// Validate the one directory snapshot that both runtime admission and the UI
/// authority projection are allowed to trust. Keeping freshness and identity
/// checks here prevents the state store from growing a second, weaker gate.
pub(crate) fn trusted_directory<'a>(
    expected_network_id: Option<&NetworkId>,
    directory: Option<&'a NetworkMemberDirectory>,
    received_at_unix_ms: Option<u64>,
    now_unix_ms: u64,
) -> Result<&'a NetworkMemberDirectory, ExitSelectionRuntimeDenial> {
    let network_id =
        expected_network_id.ok_or(ExitSelectionRuntimeDenial::MissingNetworkContext)?;
    let directory = directory.ok_or(ExitSelectionRuntimeDenial::DirectoryUnavailable)?;
    if directory.network_id != *network_id || directory.exit_selection_policy_revision == 0 {
        return Err(ExitSelectionRuntimeDenial::DirectoryUnavailable);
    }
    let received_at =
        received_at_unix_ms.ok_or(ExitSelectionRuntimeDenial::DirectoryUnavailable)?;
    let received_is_fresh =
        now_unix_ms.saturating_sub(received_at) <= EXIT_SELECTION_DIRECTORY_MAX_AGE_MS;
    let generated_is_fresh = now_unix_ms.saturating_sub(directory.generated_at_unix_ms)
        <= EXIT_SELECTION_DIRECTORY_MAX_AGE_MS;
    if !received_is_fresh || !generated_is_fresh {
        return Err(ExitSelectionRuntimeDenial::DirectoryUnavailable);
    }
    directory
        .exit_selection_policy
        .validate()
        .map_err(ExitSelectionRuntimeDenial::InvalidPolicy)?;
    Ok(directory)
}

/// Resolve one subject against a directory already validated by
/// [`trusted_directory`]. This is the shared implementation consumed by both
/// data-plane admission and the node snapshot projection.
pub(crate) fn resolve_directory_exit_selection(
    subject_node_id: &NodeId,
    directory: &NetworkMemberDirectory,
    now_unix_ms: u64,
) -> Result<ExitSelectionResolution, ExitSelectionRuntimeDenial> {
    let subject = directory
        .members
        .iter()
        .find(|member| &member.node_id == subject_node_id)
        .ok_or(ExitSelectionRuntimeDenial::RequesterNotInRoster)?;
    if !subject.lifecycle_state.grants_network_access() {
        return Err(ExitSelectionRuntimeDenial::RequesterNotActive);
    }

    let availability = exit_selection_availability_map(directory, now_unix_ms);
    directory
        .exit_selection_policy
        .resolve(
            directory.exit_selection_policy_revision,
            subject_node_id,
            &availability,
        )
        .map_err(ExitSelectionRuntimeDenial::Selection)
}

/// Build the health evidence used by deny-first resolution. Missing, stale,
/// inactive and offline members are never treated as selectable.
pub(crate) fn exit_selection_availability_map(
    directory: &NetworkMemberDirectory,
    now_unix_ms: u64,
) -> BTreeMap<NodeId, ExitContributionAvailability> {
    directory
        .exit_selection_policy
        .available_exits
        .iter()
        .map(|exit| {
            let observed = directory
                .members
                .iter()
                .find(|member| &member.node_id == exit)
                .map(|member| exit_member_availability(member, now_unix_ms))
                .unwrap_or_else(|| ExitContributionAvailability {
                    state: ExitAvailabilityState::Unavailable,
                    source: ExitAvailabilitySource::NotObserved,
                    observed_at_unix_ms: None,
                    declared_available: None,
                    reason_code: Some("exit.selection.exit_not_in_roster".into()),
                });
            (exit.clone(), observed)
        })
        .collect()
}

fn exit_member_availability(
    member: &NetworkMember,
    now_unix_ms: u64,
) -> ExitContributionAvailability {
    if !member.lifecycle_state.grants_network_access() {
        return unavailable_from_member(member, "exit.selection.member_inactive");
    }
    if member.presence.state == NetworkMemberOnlineState::Offline {
        return unavailable_from_member(member, "exit.selection.member_offline");
    }

    let availability = &member.exit_availability;
    if !availability.is_fresh(now_unix_ms, EXIT_SELECTION_AVAILABILITY_MAX_AGE_MS) {
        return match availability.observed_at_unix_ms {
            Some(observed_at_unix_ms) => ExitContributionAvailability {
                state: ExitAvailabilityState::Unknown,
                source: availability.source,
                observed_at_unix_ms: Some(observed_at_unix_ms),
                declared_available: availability.declared_available,
                reason_code: Some("exit.availability.stale".into()),
            },
            None => ExitContributionAvailability {
                state: ExitAvailabilityState::Unknown,
                source: availability.source,
                observed_at_unix_ms: None,
                declared_available: availability.declared_available,
                reason_code: Some("exit.selection.availability_missing".into()),
            },
        };
    }
    availability.clone()
}

fn unavailable_from_member(
    member: &NetworkMember,
    reason_code: &str,
) -> ExitContributionAvailability {
    ExitContributionAvailability {
        state: ExitAvailabilityState::Unavailable,
        source: member.exit_availability.source,
        observed_at_unix_ms: member
            .presence
            .observed_at_unix_ms
            .or(member.exit_availability.observed_at_unix_ms),
        declared_available: member.exit_availability.declared_available,
        reason_code: Some(reason_code.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        ExitAvailabilitySource, ExitSelectionGroupRule, ExitSelectionMemberRule, ExitSelectionMode,
        ExitSelectionPolicy, ExitSelectionSource, ExitSessionAction, ExitSessionTransitionPolicy,
        ExitTransitionReason, MemberLifecycleStatus, MembershipRole, NetworkMemberGroup,
        NetworkMemberPresence, NetworkMemberStatus,
    };

    const NOW: u64 = 1_700_006_400_000;

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"selection-runtime-network")
    }

    fn subject() -> NodeId {
        NodeId::from_public_key(b"selection-runtime-subject")
    }

    fn exit(name: &str) -> NodeId {
        NodeId::from_public_key(name.as_bytes())
    }

    fn group(name: &str) -> fuckipv6_common::domain::GroupId {
        fuckipv6_common::domain::GroupId::from_public_key(name.as_bytes())
    }

    fn member(
        node_id: NodeId,
        lifecycle_state: MemberLifecycleStatus,
        presence: NetworkMemberOnlineState,
        availability: ExitContributionAvailability,
    ) -> NetworkMember {
        NetworkMember {
            node_id,
            display_name: None,
            identity_fingerprint: None,
            role: MembershipRole::Member,
            groups: vec![NetworkMemberGroup::Member],
            capabilities: None,
            status: NetworkMemberStatus::Active,
            lifecycle_state,
            revision: 1,
            access_epoch: 1,
            available_actions: Vec::new(),
            presence: NetworkMemberPresence {
                state: presence,
                source: fuckipv6_common::domain::NetworkMemberObservationSource::LiveSession,
                observed_at_unix_ms: Some(NOW),
            },
            exit_availability: availability,
            joined_at_unix_ms: Some(NOW.saturating_sub(60_000)),
        }
    }

    fn healthy() -> ExitContributionAvailability {
        ExitContributionAvailability::observed(
            true,
            true,
            ExitAvailabilitySource::CoordinatorHealth,
            NOW,
        )
    }

    fn policy() -> ExitSelectionPolicy {
        ExitSelectionPolicy {
            mode: ExitSelectionMode::MemberChoice,
            default_exit: Some(exit("exit-a")),
            available_exits: vec![exit("exit-a"), exit("exit-b")],
            ..ExitSelectionPolicy::default()
        }
    }

    fn directory(revision: u64, generated_at: u64) -> NetworkMemberDirectory {
        NetworkMemberDirectory {
            network_id: network(),
            generated_at_unix_ms: generated_at,
            viewer_role: MembershipRole::Member,
            can_manage_members: false,
            exit_selection_policy: policy(),
            exit_selection_policy_revision: revision,
            members: vec![
                member(
                    subject(),
                    MemberLifecycleStatus::Active,
                    NetworkMemberOnlineState::Online,
                    ExitContributionAvailability::default(),
                ),
                member(
                    exit("exit-a"),
                    MemberLifecycleStatus::Active,
                    NetworkMemberOnlineState::Online,
                    healthy(),
                ),
                member(
                    exit("exit-b"),
                    MemberLifecycleStatus::Active,
                    NetworkMemberOnlineState::Online,
                    healthy(),
                ),
            ],
        }
    }

    #[test]
    fn deny_wins_across_network_group_and_member_scopes() {
        let mut directory = directory(7, NOW);
        directory.exit_selection_policy.groups = vec![ExitSelectionGroupRule {
            group_id: group("engineering"),
            members: vec![subject()],
            allow: vec![exit("exit-a"), exit("exit-b")],
            deny: vec![exit("exit-b")],
        }];
        directory.exit_selection_policy.members = vec![ExitSelectionMemberRule {
            node_id: subject(),
            allow: vec![exit("exit-a"), exit("exit-b")],
            deny: vec![exit("exit-a")],
        }];
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory, NOW).unwrap();

        let resolution = gate.resolution(NOW).unwrap();
        let a = resolution.candidate(&exit("exit-a")).unwrap();
        let b = resolution.candidate(&exit("exit-b")).unwrap();
        assert!(!a.selectable);
        assert_eq!(a.reason_code, "exit.selection.member_denied");
        assert_eq!(a.source, ExitSelectionSource::Member);
        assert!(!b.selectable);
        assert_eq!(b.reason_code, "exit.selection.group_denied");
        assert!(matches!(b.source, ExitSelectionSource::Group { .. }));
        assert!(resolution.selectable_exits.is_empty());
    }

    #[test]
    fn revoked_member_exit_disappears_from_new_session_candidates() {
        let mut directory = directory(7, NOW);
        directory.members[1].lifecycle_state = MemberLifecycleStatus::Suspended;
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory, NOW).unwrap();

        let resolution = gate.resolution(NOW).unwrap();
        assert_eq!(resolution.selectable_exits, vec![exit("exit-b")]);
        let revoked = resolution.candidate(&exit("exit-a")).unwrap();
        assert!(!revoked.selectable);
        assert_eq!(
            revoked.availability.reason_code.as_deref(),
            Some("exit.selection.member_inactive")
        );
        assert!(gate.select_for_session(NOW, Some(&exit("exit-a"))).is_err());
    }

    #[test]
    fn offline_exit_is_never_selectable_even_with_fresh_health() {
        let mut directory = directory(7, NOW);
        directory.members[1].presence.state = NetworkMemberOnlineState::Offline;
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory, NOW).unwrap();

        let resolution = gate.resolution(NOW).unwrap();
        let offline = resolution.candidate(&exit("exit-a")).unwrap();
        assert!(!offline.selectable);
        assert_eq!(offline.reason_code, "exit.selection.offline");
        assert_eq!(
            offline.availability.reason_code.as_deref(),
            Some("exit.selection.member_offline")
        );
        assert_eq!(gate.select_for_session(NOW, None).unwrap(), exit("exit-b"));
    }

    #[test]
    fn missing_and_stale_health_is_unknown_not_available() {
        let mut directory = directory(7, NOW);
        directory.members[1].exit_availability =
            ExitContributionAvailability::declared_unknown(Some(true));
        directory.members[2].exit_availability = ExitContributionAvailability::observed(
            true,
            true,
            ExitAvailabilitySource::CoordinatorHealth,
            NOW.saturating_sub(EXIT_SELECTION_AVAILABILITY_MAX_AGE_MS + 1),
        );
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory, NOW).unwrap();

        let resolution = gate.resolution(NOW).unwrap();
        assert!(resolution.selectable_exits.is_empty());
        assert!(matches!(
            gate.select_for_session(NOW, None),
            Err(ExitSelectionRuntimeDenial::Selection(error))
                if error.code() == "domain.exit_selection_no_candidate"
        ));
    }

    #[test]
    fn fixed_exit_rejects_local_choice_and_fails_closed_when_unavailable() {
        let mut directory = directory(7, NOW);
        directory.exit_selection_policy.mode = ExitSelectionMode::Fixed {
            exit: exit("exit-a"),
        };
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory, NOW).unwrap();
        assert!(matches!(
            gate.select_for_session(NOW, Some(&exit("exit-b"))),
            Err(ExitSelectionRuntimeDenial::Selection(error))
                if error.code() == "domain.exit_selection_fixed_exit"
        ));

        let mut unavailable = super::tests::directory(7, NOW);
        unavailable.exit_selection_policy.mode = ExitSelectionMode::Fixed {
            exit: exit("exit-a"),
        };
        unavailable.members[1].presence.state = NetworkMemberOnlineState::Offline;
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(unavailable, NOW).unwrap();
        assert!(gate.select_for_session(NOW, None).is_err());
    }

    #[test]
    fn established_session_obeys_migrate_terminate_and_fail_closed_policy() {
        let mut migrate = directory(7, NOW);
        migrate.exit_selection_policy.available_exits = vec![exit("exit-b")];
        migrate.exit_selection_policy.default_exit = Some(exit("exit-b"));
        migrate.exit_selection_policy.session_transition = ExitSessionTransitionPolicy::Migrate;
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(migrate, NOW).unwrap();
        gate.set_current_exit(Some(exit("exit-a")));
        let transition = gate.plan_current_session_transition(NOW).unwrap();
        assert_eq!(
            transition.action,
            ExitSessionAction::Migrate { to: exit("exit-b") }
        );
        assert_eq!(transition.reason, ExitTransitionReason::ExitNotAuthorized);

        let mut terminate = directory(7, NOW);
        terminate.exit_selection_policy.available_exits = vec![exit("exit-b")];
        terminate.exit_selection_policy.default_exit = Some(exit("exit-b"));
        terminate.exit_selection_policy.session_transition = ExitSessionTransitionPolicy::Terminate;
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(terminate, NOW).unwrap();
        gate.set_current_exit(Some(exit("exit-a")));
        assert_eq!(
            gate.plan_current_session_transition(NOW).unwrap().action,
            ExitSessionAction::Terminate
        );

        let mut closed = directory(7, NOW);
        closed.exit_selection_policy.available_exits = vec![exit("exit-b")];
        closed.exit_selection_policy.default_exit = Some(exit("exit-b"));
        closed.exit_selection_policy.session_transition = ExitSessionTransitionPolicy::FailClosed;
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(closed, NOW).unwrap();
        gate.set_current_exit(Some(exit("exit-a")));
        let transition = gate.plan_current_session_transition(NOW).unwrap();
        assert_eq!(transition.action, ExitSessionAction::FailClosed);
    }

    #[test]
    fn pending_migration_does_not_change_current_exit_until_commit() {
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory(7, NOW), NOW)
            .unwrap();
        gate.set_current_exit(Some(exit("exit-a")));

        assert!(gate.begin_pending_exit(exit("exit-b")));
        assert_eq!(gate.current_exit(), Some(&exit("exit-a")));
        assert_eq!(gate.pending_exit(), Some(&exit("exit-b")));
        assert!(!gate.begin_pending_exit(exit("exit-b")));

        assert_eq!(gate.commit_pending_exit(), Some(exit("exit-b")));
        assert_eq!(gate.current_exit(), Some(&exit("exit-b")));
        assert_eq!(gate.pending_exit(), None);
    }

    #[test]
    fn discarding_pending_migration_never_promotes_the_candidate() {
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory(7, NOW), NOW)
            .unwrap();
        gate.set_current_exit(Some(exit("exit-a")));
        assert!(gate.begin_pending_exit(exit("exit-b")));

        gate.discard_pending_exit();
        assert_eq!(gate.current_exit(), Some(&exit("exit-a")));
        assert_eq!(gate.pending_exit(), None);
    }

    #[test]
    fn older_policy_revision_and_generation_never_replace_fresh_authority() {
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        gate.replace_member_directory(directory(8, NOW), NOW)
            .unwrap();

        let mut older_revision = directory(7, NOW + 1);
        older_revision.exit_selection_policy.available_exits = vec![exit("exit-b")];
        older_revision.exit_selection_policy.default_exit = Some(exit("exit-b"));
        assert_eq!(
            gate.replace_member_directory(older_revision, NOW + 1)
                .unwrap(),
            ExitSelectionDirectoryUpdate::IgnoredStale
        );

        let mut older_generation = directory(8, NOW.saturating_sub(1));
        older_generation.exit_selection_policy.available_exits = vec![exit("exit-b")];
        older_generation.exit_selection_policy.default_exit = Some(exit("exit-b"));
        assert_eq!(
            gate.replace_member_directory(older_generation, NOW + 1)
                .unwrap(),
            ExitSelectionDirectoryUpdate::IgnoredStale
        );
        let selectable = gate.resolution(NOW + 1).unwrap().selectable_exits;
        assert_eq!(selectable.len(), 2);
        assert!(selectable.contains(&exit("exit-a")));
        assert!(selectable.contains(&exit("exit-b")));
    }

    #[test]
    fn missing_stale_invalid_and_revision_zero_directories_fail_closed() {
        let mut gate = ExitSelectionGate::new(subject(), Some(network()));
        assert!(matches!(
            gate.resolution(NOW),
            Err(ExitSelectionRuntimeDenial::DirectoryUnavailable)
        ));

        let mut zero = directory(0, NOW);
        zero.exit_selection_policy.available_exits = vec![exit("exit-a")];
        assert!(matches!(
            gate.replace_member_directory(zero, NOW),
            Err(ExitSelectionRuntimeDenial::InvalidPolicyRevision)
        ));

        let mut invalid = directory(7, NOW);
        invalid.exit_selection_policy.default_exit = Some(exit("exit-outside"));
        assert!(matches!(
            gate.replace_member_directory(invalid, NOW),
            Err(ExitSelectionRuntimeDenial::InvalidPolicy(_))
        ));

        gate.replace_member_directory(directory(7, NOW), NOW)
            .unwrap();
        assert!(matches!(
            gate.resolution(NOW + EXIT_SELECTION_DIRECTORY_MAX_AGE_MS + 1),
            Err(ExitSelectionRuntimeDenial::DirectoryUnavailable)
        ));
    }
}
