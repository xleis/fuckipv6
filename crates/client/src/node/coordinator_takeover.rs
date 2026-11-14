use std::time::Duration;

use fuckipv6_common::domain::{
    CoordinatorCandidateConfigState, CoordinatorCandidateStatus, CoordinatorElectionProposal,
    CoordinatorElectionStage, CoordinatorElectionVote, CoordinatorEmergencyLeaderStatus,
    CoordinatorEmergencyState, CoordinatorEmergencyWriteCredential, DomainError, DomainResult,
    NodeId,
};
use fuckipv6_common::Identity;

/// Candidate status is polled independently of data-plane signaling events so
/// takeover still converges if a notification is delayed, dropped or replayed.
pub(super) const COORDINATOR_TAKEOVER_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Failed authority reads back off exponentially up to this bound. The cap
/// keeps a recovered Coordinator observable even when the previous outage was
/// long-lived, while still avoiding a fixed-rate retry storm.
pub(super) const COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL: Duration = Duration::from_secs(60);

const COORDINATOR_TAKEOVER_MAX_BACKOFF_EXPONENT: u32 = 4;
const COORDINATOR_TAKEOVER_JITTER_DIVISOR: u128 = 10;

/// Deterministic, bounded exponential backoff for authoritative Coordinator
/// status polls. Jitter is derived from the local node identity, network and
/// failure count so clients spread out without making retries non-reproducible
/// in tests or incident analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CoordinatorTakeoverPollBackoff {
    failures: u32,
    seed: u64,
}

impl CoordinatorTakeoverPollBackoff {
    pub(super) const fn new(seed: u64) -> Self {
        Self { failures: 0, seed }
    }

    #[cfg(test)]
    pub(super) const fn failures(&self) -> u32 {
        self.failures
    }

    pub(super) fn delay(&self) -> Duration {
        let exponent = self.failures.min(COORDINATOR_TAKEOVER_MAX_BACKOFF_EXPONENT);
        let nominal = COORDINATOR_TAKEOVER_POLL_INTERVAL
            .saturating_mul(1_u32 << exponent)
            .min(COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL);
        let jitter_span = nominal.as_millis() / COORDINATOR_TAKEOVER_JITTER_DIVISOR;
        if jitter_span == 0 {
            return nominal;
        }
        let span = jitter_span.min(u64::MAX as u128) as u64;
        let mixed = splitmix64(
            self.seed
                .wrapping_add(u64::from(self.failures).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        );
        let offset = (mixed % (span.saturating_mul(2).saturating_add(1))) as i128 - span as i128;
        let nominal_ms = i128::try_from(nominal.as_millis()).unwrap_or(i128::MAX);
        let jittered_ms = nominal_ms.saturating_add(offset).max(0);
        let jittered_ms = u64::try_from(jittered_ms).unwrap_or(u64::MAX);
        Duration::from_millis(jittered_ms).min(COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL)
    }

    pub(super) fn record_success(&mut self) {
        self.failures = 0;
    }

    pub(super) fn record_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// A proposal is intentionally shorter than the protocol maximum. This bounds
/// how long a stalled election can delay the next deterministic candidate.
pub(super) const COORDINATOR_TAKEOVER_PROPOSAL_VALIDITY_MS: u64 = 2 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CoordinatorTakeoverDecision {
    Wait,
    Propose(CoordinatorElectionProposal),
    Vote {
        proposal_id: fuckipv6_common::domain::ContentHash,
        vote: CoordinatorElectionVote,
    },
    Timeout {
        network_id: fuckipv6_common::domain::NetworkId,
        proposal_id: fuckipv6_common::domain::ContentHash,
    },
    IssueEmergency {
        network_id: fuckipv6_common::domain::NetworkId,
    },
    RenewEmergency {
        network_id: fuckipv6_common::domain::NetworkId,
        credential: CoordinatorEmergencyWriteCredential,
    },
}

/// Decide one safe control-plane action from an authoritative candidate status.
///
/// The returned vote or proposal is independently signed only after the status
/// projection proves the active candidate group, expired lease, next term and
/// deterministic preferred candidate. A terminal or stale status always yields
/// `Wait`; malformed active proposals fail closed with a domain error.
pub(super) fn decide_coordinator_takeover(
    status: &CoordinatorCandidateStatus,
    identity: &Identity,
    now_unix_ms: u64,
) -> DomainResult<CoordinatorTakeoverDecision> {
    if status.config_state != CoordinatorCandidateConfigState::Current {
        return Ok(CoordinatorTakeoverDecision::Wait);
    }
    let local_node = NodeId::from_public_key(&identity.public_key());
    if let Some(local_candidate) = status
        .candidates
        .iter()
        .find(|candidate| candidate.node_id == local_node)
    {
        if !local_candidate.eligible_for_election {
            return Ok(CoordinatorTakeoverDecision::Wait);
        }
        return decide_candidate_takeover(status, identity, &local_node, now_unix_ms);
    }

    decide_emergency_takeover(status, &local_node, now_unix_ms)
}

fn decide_candidate_takeover(
    status: &CoordinatorCandidateStatus,
    identity: &Identity,
    local_node: &NodeId,
    now_unix_ms: u64,
) -> DomainResult<CoordinatorTakeoverDecision> {
    if let Some(election) = status.latest_election.as_ref() {
        if matches!(
            election.stage,
            CoordinatorElectionStage::Started | CoordinatorElectionStage::Voting
        ) {
            if now_unix_ms >= election.deadline_unix_ms {
                return Ok(CoordinatorTakeoverDecision::Timeout {
                    network_id: election.proposal.network_id.clone(),
                    proposal_id: election.proposal.proposal_id.clone(),
                });
            }
            if election.stage != CoordinatorElectionStage::Voting {
                return Ok(CoordinatorTakeoverDecision::Wait);
            }

            election
                .proposal
                .validate_for_candidate_status(status, now_unix_ms)?;
            if election.votes.iter().any(|vote| vote.voter == *local_node) {
                return Ok(CoordinatorTakeoverDecision::Wait);
            }
            let vote = CoordinatorElectionVote::signed(
                election.proposal.network_id.clone(),
                election.proposal.candidate_config_revision,
                election.proposal.term,
                election.proposal.leader.clone(),
                election.proposal.candidate_group.clone(),
                now_unix_ms,
                election.proposal.expires_at_unix_ms,
                identity,
            )?;
            vote.validate_for_proposal(&election.proposal)?;
            return Ok(CoordinatorTakeoverDecision::Vote {
                proposal_id: election.proposal.proposal_id.clone(),
                vote,
            });
        }
    }

    if status.lease.is_writable() {
        return Ok(CoordinatorTakeoverDecision::Wait);
    }
    let preferred = status.preferred_election_candidate().ok_or_else(|| {
        DomainError::new(
            "domain.coordinator_election_no_eligible_candidate",
            "coordinator election has no healthy willing candidate",
        )
    })?;
    if preferred.node_id != *local_node {
        return Ok(CoordinatorTakeoverDecision::Wait);
    }
    let config = status
        .config
        .as_ref()
        .ok_or_else(|| DomainError::invariant("current candidate status has no config"))?;
    let term = match status.latest_term.as_ref() {
        Some(term) => term
            .term
            .checked_add(1)
            .ok_or_else(|| DomainError::invariant("coordinator term overflow"))?,
        None => 1,
    };
    let proposed_expiry = now_unix_ms
        .checked_add(COORDINATOR_TAKEOVER_PROPOSAL_VALIDITY_MS)
        .ok_or_else(|| DomainError::invariant("coordinator election proposal expiry overflow"))?
        .min(config.expires_at_unix_ms);
    if proposed_expiry <= now_unix_ms {
        return Ok(CoordinatorTakeoverDecision::Wait);
    }
    let proposal = CoordinatorElectionProposal::signed(
        status.network_id.clone(),
        config.revision,
        term,
        config
            .candidates
            .iter()
            .map(|candidate| candidate.node_id.clone())
            .collect(),
        now_unix_ms,
        proposed_expiry,
        identity,
    )?;
    proposal.validate_for_candidate_status(status, now_unix_ms)?;
    Ok(CoordinatorTakeoverDecision::Propose(proposal))
}

pub(super) fn validated_emergency_lease(
    status: &CoordinatorCandidateStatus,
    now_unix_ms: u64,
) -> DomainResult<Option<CoordinatorEmergencyLeaderStatus>> {
    let Some(lease) = status.emergency_leader.clone() else {
        return Ok(None);
    };
    let mut validated = status.clone();
    validated.attach_emergency_leader(Some(lease), now_unix_ms)?;
    Ok(validated.emergency_leader)
}

pub(super) fn active_local_emergency(
    status: &CoordinatorCandidateStatus,
    local_node: &NodeId,
    now_unix_ms: u64,
) -> DomainResult<Option<CoordinatorEmergencyLeaderStatus>> {
    Ok(validated_emergency_lease(status, now_unix_ms)?
        .filter(|lease| lease.is_active() && lease.leader == *local_node))
}

fn decide_emergency_takeover(
    status: &CoordinatorCandidateStatus,
    local_node: &NodeId,
    now_unix_ms: u64,
) -> DomainResult<CoordinatorTakeoverDecision> {
    if status.lease.is_writable() {
        return Ok(CoordinatorTakeoverDecision::Wait);
    }
    if status.latest_election.as_ref().is_some_and(|election| {
        matches!(
            election.stage,
            CoordinatorElectionStage::Started | CoordinatorElectionStage::Voting
        ) && now_unix_ms < election.deadline_unix_ms
    }) {
        return Ok(CoordinatorTakeoverDecision::Wait);
    }

    if let Some(lease) = validated_emergency_lease(status, now_unix_ms)? {
        if lease.is_active() {
            if lease.leader != *local_node {
                return Ok(CoordinatorTakeoverDecision::Wait);
            }
            return Ok(CoordinatorTakeoverDecision::RenewEmergency {
                network_id: lease.network_id.clone(),
                credential: CoordinatorEmergencyWriteCredential {
                    network_id: lease.network_id.clone(),
                    leader: lease.leader.clone(),
                    emergency_epoch: lease.emergency_epoch,
                    candidate_config_revision: lease.candidate_config_revision,
                },
            });
        }
    }

    let all_candidates_unavailable = status.emergency
        == CoordinatorEmergencyState::AllCandidatesUnavailable
        && !status.candidates.is_empty()
        && status
            .candidates
            .iter()
            .all(|candidate| !candidate.eligible_for_election);
    if all_candidates_unavailable {
        return Ok(CoordinatorTakeoverDecision::IssueEmergency {
            network_id: status.network_id.clone(),
        });
    }
    Ok(CoordinatorTakeoverDecision::Wait)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{
        CapabilityKind, CoordinatorCandidate, CoordinatorCandidateConfig,
        CoordinatorCandidateDeclaration, CoordinatorCandidateResources,
        CoordinatorCandidateWillingness, CoordinatorElectionSession,
        CoordinatorEmergencyLeaderStatus, CoordinatorPublicReachability, ExitContributionMode,
        ExitContributionObligation, ExitContributionScope, ExitSelectionPolicy,
        NetworkCreateProfile, NetworkJoinMode, NetworkRecord, NetworkVisibility, PublicKey,
    };

    const NOW: u64 = 2_000_000;
    const VALIDITY: u64 = 120_000;

    #[test]
    fn takeover_poll_backoff_is_deterministic_bounded_and_resettable() {
        let mut first = CoordinatorTakeoverPollBackoff::new(0xA11CE);
        let mut replay = CoordinatorTakeoverPollBackoff::new(0xA11CE);
        let initial = first.delay();
        assert_eq!(first.delay(), replay.delay());
        assert!(initial >= Duration::from_millis(4_500));
        assert!(initial <= COORDINATOR_TAKEOVER_POLL_INTERVAL);

        let bounds = [
            (Duration::from_millis(9_000), Duration::from_secs(11)),
            (Duration::from_millis(18_000), Duration::from_secs(22)),
            (Duration::from_millis(36_000), Duration::from_secs(44)),
            (
                Duration::from_secs(54),
                COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL,
            ),
            (
                Duration::from_secs(54),
                COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL,
            ),
            (
                Duration::from_secs(54),
                COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL,
            ),
            (
                Duration::from_secs(54),
                COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL,
            ),
            (
                Duration::from_secs(54),
                COORDINATOR_TAKEOVER_MAX_POLL_INTERVAL,
            ),
        ];
        for (expected_failures, (minimum, maximum)) in (1..=8).zip(bounds) {
            first.record_failure();
            replay.record_failure();
            let delay = first.delay();
            assert_eq!(first.failures(), expected_failures);
            assert_eq!(delay, replay.delay());
            assert!((minimum..=maximum).contains(&delay), "{delay:?}");
        }

        first.record_success();
        replay.record_success();
        assert_eq!(first.failures(), 0);
        assert_eq!(first.delay(), initial);
        assert_eq!(first.delay(), replay.delay());
    }

    #[test]
    fn takeover_poll_jitter_differs_by_stable_seed() {
        let first = CoordinatorTakeoverPollBackoff::new(1).delay();
        let second = CoordinatorTakeoverPollBackoff::new(2).delay();
        assert_ne!(first, second);
        for delay in [first, second] {
            assert!(delay >= Duration::from_millis(4_500));
            assert!(delay <= COORDINATOR_TAKEOVER_POLL_INTERVAL);
        }
    }

    struct Fixture {
        network: NetworkRecord,
        owner: Identity,
        ordinary: Identity,
        candidate_one: Identity,
        candidate_two: Identity,
        config: CoordinatorCandidateConfig,
        declarations: Vec<CoordinatorCandidateDeclaration>,
    }

    impl Fixture {
        fn new() -> Self {
            let owner = Identity::generate().unwrap();
            let ordinary = Identity::generate().unwrap();
            let candidate_one = Identity::generate().unwrap();
            let candidate_two = Identity::generate().unwrap();
            let owner_id = NodeId::from_public_key(&owner.public_key());
            let ordinary_id = NodeId::from_public_key(&ordinary.public_key());
            let candidate_one_id = NodeId::from_public_key(&candidate_one.public_key());
            let candidate_two_id = NodeId::from_public_key(&candidate_two.public_key());
            let mut network = NetworkRecord::create_owned(
                NetworkCreateProfile {
                    name: "takeover-fixture".into(),
                    purpose: "Coordinator takeover decision tests".into(),
                    icon: "network".into(),
                    language: "zh-CN".into(),
                    region: "ZZ".into(),
                    tags: vec![],
                    capability_declarations: vec![CapabilityKind::Coordinator],
                    visibility: NetworkVisibility::Private,
                    join_mode: NetworkJoinMode::InviteOnly,
                    exit_contribution: ExitContributionObligation::NotRequired,
                    exit_contribution_scope: ExitContributionScope::NotApplicable,
                    exit_contribution_mode: ExitContributionMode::Disabled,
                    exit_selection_policy: ExitSelectionPolicy::default(),
                },
                PublicKey::from_bytes([61; 32]),
                owner_id,
                NOW - 10_000,
            )
            .unwrap();
            network
                .members
                .extend([candidate_one_id, candidate_two_id, ordinary_id]);
            network.validate().unwrap();
            let config = CoordinatorCandidateConfig::signed(
                network.network_id.clone(),
                1,
                vec![
                    CoordinatorCandidate {
                        node_id: NodeId::from_public_key(&candidate_one.public_key()),
                        priority: 10,
                    },
                    CoordinatorCandidate {
                        node_id: NodeId::from_public_key(&candidate_two.public_key()),
                        priority: 20,
                    },
                ],
                NOW,
                NOW + VALIDITY,
                &owner,
            )
            .unwrap();
            let declarations = [&candidate_one, &candidate_two]
                .into_iter()
                .map(|identity| {
                    CoordinatorCandidateDeclaration::signed(
                        network.network_id.clone(),
                        1,
                        CoordinatorPublicReachability {
                            public_endpoint: Some("203.0.113.20:443".parse().unwrap()),
                            reachable_via_relay: true,
                        },
                        CoordinatorCandidateResources {
                            cpu_cores: 4,
                            memory_bytes: 512 * 1024 * 1024,
                            max_connections: 1_000,
                        },
                        CoordinatorCandidateWillingness::Willing,
                        NOW,
                        NOW + 60_000,
                        identity,
                    )
                    .unwrap()
                })
                .collect();
            Self {
                network,
                owner,
                ordinary,
                candidate_one,
                candidate_two,
                config,
                declarations,
            }
        }

        fn status(&self) -> CoordinatorCandidateStatus {
            CoordinatorCandidateStatus::assess(
                &self.network,
                Some(&self.config),
                &self.declarations,
                None,
                NOW,
            )
            .unwrap()
        }

        fn unavailable_status(&self) -> CoordinatorCandidateStatus {
            CoordinatorCandidateStatus::assess(&self.network, Some(&self.config), &[], None, NOW)
                .unwrap()
        }

        fn emergency_status(&self, leader: &Identity) -> CoordinatorCandidateStatus {
            let leader = NodeId::from_public_key(&leader.public_key());
            let lease = CoordinatorEmergencyLeaderStatus::issue(
                &self.network,
                &self.config,
                std::slice::from_ref(&leader),
                1,
                NOW,
            )
            .unwrap();
            let mut status = self.unavailable_status();
            status.attach_emergency_leader(Some(lease), NOW).unwrap();
            status
        }

        fn proposal(&self, proposer: &Identity) -> CoordinatorElectionProposal {
            CoordinatorElectionProposal::signed(
                self.network.network_id.clone(),
                1,
                1,
                vec![
                    NodeId::from_public_key(&self.candidate_one.public_key()),
                    NodeId::from_public_key(&self.candidate_two.public_key()),
                ],
                NOW,
                NOW + 30_000,
                proposer,
            )
            .unwrap()
        }

        fn voting_status(
            &self,
            proposal: CoordinatorElectionProposal,
        ) -> CoordinatorCandidateStatus {
            let mut session = CoordinatorElectionSession::started(proposal, NOW).unwrap();
            session.begin_voting(NOW).unwrap();
            let mut status = self.status();
            status.latest_election = Some(session);
            status
        }
    }

    #[test]
    fn active_local_emergency_leader_renews_the_exact_lease() {
        let fixture = Fixture::new();
        let status = fixture.emergency_status(&fixture.owner);
        let lease = status
            .emergency_leader
            .as_ref()
            .expect("fixture issues an emergency lease");
        assert!(lease.is_active());

        let decision = decide_coordinator_takeover(&status, &fixture.owner, NOW).unwrap();
        assert_eq!(
            decision,
            CoordinatorTakeoverDecision::RenewEmergency {
                network_id: fixture.network.network_id.clone(),
                credential: CoordinatorEmergencyWriteCredential {
                    network_id: lease.network_id.clone(),
                    leader: lease.leader.clone(),
                    emergency_epoch: lease.emergency_epoch,
                    candidate_config_revision: lease.candidate_config_revision,
                },
            }
        );
    }

    #[test]
    fn active_remote_emergency_leader_prevents_local_issue_or_renewal() {
        let fixture = Fixture::new();
        let status = fixture.emergency_status(&fixture.ordinary);
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.owner, NOW).unwrap(),
            CoordinatorTakeoverDecision::Wait
        );
    }

    #[test]
    fn non_candidate_requests_issue_only_after_every_candidate_is_unavailable() {
        let fixture = Fixture::new();
        let status = fixture.unavailable_status();
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.owner, NOW).unwrap(),
            CoordinatorTakeoverDecision::IssueEmergency {
                network_id: fixture.network.network_id.clone(),
            }
        );
    }

    #[test]
    fn configured_candidate_never_requests_emergency_authority() {
        let fixture = Fixture::new();
        let status = fixture.unavailable_status();
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.candidate_one, NOW).unwrap(),
            CoordinatorTakeoverDecision::Wait
        );
    }

    #[test]
    fn expired_emergency_lease_requests_a_new_epoch_while_candidates_remain_unavailable() {
        let fixture = Fixture::new();
        let mut status = fixture.emergency_status(&fixture.owner);
        let expires_at = status
            .emergency_leader
            .as_ref()
            .expect("fixture issues an emergency lease")
            .expires_at_unix_ms;
        status
            .attach_emergency_leader(status.emergency_leader.clone(), expires_at)
            .unwrap();
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.owner, expires_at).unwrap(),
            CoordinatorTakeoverDecision::IssueEmergency {
                network_id: fixture.network.network_id.clone(),
            }
        );
    }

    #[test]
    fn expired_emergency_lease_does_not_issue_after_candidates_recover() {
        let fixture = Fixture::new();
        let unavailable = fixture.emergency_status(&fixture.owner);
        let lease = unavailable
            .emergency_leader
            .expect("fixture issues an emergency lease");
        let mut status = fixture.status();
        status
            .attach_emergency_leader(Some(lease), NOW + 30_000)
            .unwrap();
        assert!(status
            .emergency_leader
            .as_ref()
            .is_some_and(|lease| !lease.is_active()));
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.owner, NOW + 30_000).unwrap(),
            CoordinatorTakeoverDecision::Wait
        );
    }

    #[test]
    fn non_candidate_waits_for_an_active_election_until_its_deadline() {
        let fixture = Fixture::new();
        let mut status = fixture.unavailable_status();
        let proposal = fixture.proposal(&fixture.candidate_one);
        let deadline = proposal.expires_at_unix_ms;
        status.latest_election = Some(CoordinatorElectionSession::started(proposal, NOW).unwrap());
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.owner, NOW + 1).unwrap(),
            CoordinatorTakeoverDecision::Wait
        );
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.owner, deadline).unwrap(),
            CoordinatorTakeoverDecision::IssueEmergency {
                network_id: fixture.network.network_id.clone(),
            }
        );
    }

    #[test]
    fn preferred_candidate_proposes_the_next_term_after_lease_expiry() {
        let fixture = Fixture::new();
        let decision =
            decide_coordinator_takeover(&fixture.status(), &fixture.candidate_one, NOW).unwrap();
        let CoordinatorTakeoverDecision::Propose(proposal) = decision else {
            panic!("preferred candidate must propose");
        };
        assert_eq!(proposal.proposer, proposal.leader);
        assert_eq!(
            proposal.proposer,
            NodeId::from_public_key(&fixture.candidate_one.public_key())
        );
        assert_eq!(proposal.term, 1);
        assert_eq!(proposal.expires_at_unix_ms, NOW + VALIDITY);
        assert_eq!(proposal.candidate_config_revision, 1);
    }

    #[test]
    fn non_preferred_candidate_waits_instead_of_signing_a_conflicting_proposal() {
        let fixture = Fixture::new();
        assert_eq!(
            decide_coordinator_takeover(&fixture.status(), &fixture.candidate_two, NOW).unwrap(),
            CoordinatorTakeoverDecision::Wait
        );
    }

    #[test]
    fn eligible_candidate_independently_signs_one_vote_for_an_active_proposal() {
        let fixture = Fixture::new();
        let proposal = fixture.proposal(&fixture.candidate_one);
        let status = fixture.voting_status(proposal.clone());
        let decision =
            decide_coordinator_takeover(&status, &fixture.candidate_two, NOW + 1).unwrap();
        let CoordinatorTakeoverDecision::Vote { proposal_id, vote } = decision else {
            panic!("eligible candidate must vote");
        };
        assert_eq!(proposal_id, proposal.proposal_id);
        vote.validate_for_proposal(&proposal).unwrap();

        let mut voted_status = status;
        voted_status
            .latest_election
            .as_mut()
            .unwrap()
            .votes
            .push(vote);
        assert_eq!(
            decide_coordinator_takeover(&voted_status, &fixture.candidate_two, NOW + 2).unwrap(),
            CoordinatorTakeoverDecision::Wait
        );
    }

    #[test]
    fn malformed_active_proposal_fails_closed_before_voting() {
        let fixture = Fixture::new();
        let mut status = fixture.voting_status(fixture.proposal(&fixture.candidate_one));
        status.latest_election.as_mut().unwrap().proposal.term = 2;
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.candidate_two, NOW + 1)
                .unwrap_err()
                .code(),
            "domain.invalid_signature"
        );
    }

    #[test]
    fn expired_active_proposal_is_driven_to_timeout() {
        let fixture = Fixture::new();
        let proposal = fixture.proposal(&fixture.candidate_one);
        let proposal_id = proposal.proposal_id.clone();
        let status = fixture.voting_status(proposal);
        assert_eq!(
            decide_coordinator_takeover(&status, &fixture.candidate_two, NOW + 30_000).unwrap(),
            CoordinatorTakeoverDecision::Timeout {
                network_id: fixture.network.network_id.clone(),
                proposal_id,
            }
        );
    }
}
