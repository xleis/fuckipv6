//! Local-only discovery browsing projection.
//!
//! Search text is never sent to the node or persisted. The projection consumes
//! already verified index records and re-checks their signature and validity at
//! render time so a stale snapshot cannot make an expired or revoked record
//! look joinable.

use fuckipv6_common::domain::{CapabilityKind, NetworkId, NetworkJoinMode, NetworkVisibility};

use crate::discovery_index::{NodeDiscoveryIndexErrorCategory, NodeDiscoveryIndexState};
use crate::node::NodeSnapshot;

/// Cached records older than this are still shown when signature-valid, but
/// every card and the result list carry an explicit stale marker.
pub const DISCOVERY_RESULT_STALE_AFTER_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryVisibilityFilter {
    Public,
    Private,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryFilters {
    pub visibility: Option<DiscoveryVisibilityFilter>,
    pub join_mode: Option<NetworkJoinMode>,
    pub language: Option<String>,
    pub region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryResult {
    pub network_id: NetworkId,
    pub title: String,
    pub purpose: String,
    pub language: String,
    pub region: String,
    pub tags: Vec<String>,
    pub capabilities: Vec<CapabilityKind>,
    pub visibility: NetworkVisibility,
    pub join_mode: NetworkJoinMode,
    pub verified_at_unix_ms: u64,
    pub stale: bool,
}

impl DiscoveryResult {
    pub fn join_action_id(&self) -> &'static str {
        "join_network"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryProjectionState {
    Idle,
    Pending,
    Ready,
    Empty,
    NoMatches,
    Degraded,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryProjection {
    pub state: DiscoveryProjectionState,
    pub results: Vec<DiscoveryResult>,
    /// Number of signature-valid, unexpired, non-revoked records before local
    /// search and filters were applied.
    pub valid_record_count: usize,
    /// Number of index records rejected during the last refresh.
    pub rejected_record_count: usize,
    /// True when an error or rejected record reduced the available result set.
    pub partial: bool,
    /// The snapshot contains the complete index response, so a non-empty
    /// result list with this flag set must say that no more results exist.
    pub no_more_results: bool,
    pub last_success_unix_ms: Option<u64>,
    pub error_category: Option<NodeDiscoveryIndexErrorCategory>,
}

impl DiscoveryProjection {
    pub fn has_stale_results(&self) -> bool {
        self.results.iter().any(|result| result.stale)
    }
}

/// Project verified discovery records into a deterministic, local result set.
///
/// Empty index data, a query with no matches, an index failure, partial data
/// and stale cached data are intentionally separate states so the UI does not
/// misreport one condition as another.
pub fn project_discovery(
    snapshot: &NodeSnapshot,
    query: &str,
    filters: &DiscoveryFilters,
    now_unix_ms: u64,
) -> DiscoveryProjection {
    let index = &snapshot.discovery_index;
    let normalized_query = query.trim().to_lowercase();
    let mut verified = Vec::new();

    for record in &index.records {
        if !record.discoverable
            || record.record.document.revoked_at_unix_ms.is_some()
            || record.record.validate_at(now_unix_ms).is_err()
        {
            continue;
        }
        let document = &record.record.document;
        if !matches_filters(document, filters) {
            continue;
        }
        if let Some(rank) = match_rank(document, &normalized_query) {
            verified.push((
                rank,
                document.name.clone(),
                document.network_id.as_str().to_string(),
                record,
            ));
        }
    }

    verified.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });

    let valid_record_count = index
        .records
        .iter()
        .filter(|record| {
            record.discoverable
                && record.record.document.revoked_at_unix_ms.is_none()
                && record.record.validate_at(now_unix_ms).is_ok()
        })
        .count();
    let degraded = index.state == NodeDiscoveryIndexState::Degraded;
    let results = verified
        .into_iter()
        .map(|(_, _, _, record)| DiscoveryResult {
            network_id: record.record.document.network_id.clone(),
            title: record.record.document.name.clone(),
            purpose: record.record.document.purpose.clone(),
            language: record.record.document.language.clone(),
            region: record.record.document.region.clone(),
            tags: record.record.document.tags.clone(),
            capabilities: record.record.document.capability_summary.clone(),
            visibility: record.record.document.visibility,
            join_mode: record.record.document.join_mode,
            verified_at_unix_ms: record.accepted_at_unix_ms,
            stale: degraded
                || now_unix_ms.saturating_sub(record.accepted_at_unix_ms)
                    > DISCOVERY_RESULT_STALE_AFTER_MS,
        })
        .collect::<Vec<_>>();

    let error_category = index.last_error.as_ref().map(|error| error.category);
    let partial = degraded || index.rejected_record_count > 0;
    let state = match index.state {
        NodeDiscoveryIndexState::Idle => DiscoveryProjectionState::Idle,
        NodeDiscoveryIndexState::Pending => DiscoveryProjectionState::Pending,
        NodeDiscoveryIndexState::Failed => DiscoveryProjectionState::Failed,
        NodeDiscoveryIndexState::Unsupported => DiscoveryProjectionState::Unsupported,
        // A partial response remains degraded even when the current query has
        // no matches. Reporting `empty` or `no_matches` would incorrectly claim
        // that the complete index was searched.
        NodeDiscoveryIndexState::Degraded => DiscoveryProjectionState::Degraded,
        NodeDiscoveryIndexState::Ready if partial => DiscoveryProjectionState::Degraded,
        NodeDiscoveryIndexState::Ready if valid_record_count == 0 => {
            DiscoveryProjectionState::Empty
        }
        NodeDiscoveryIndexState::Ready if results.is_empty() => DiscoveryProjectionState::NoMatches,
        NodeDiscoveryIndexState::Ready => DiscoveryProjectionState::Ready,
    };

    DiscoveryProjection {
        state,
        // Only a complete, partial-free response can truthfully claim that all
        // results are shown.
        no_more_results: state == DiscoveryProjectionState::Ready && !results.is_empty(),
        results,
        valid_record_count,
        rejected_record_count: index.rejected_record_count,
        partial,
        last_success_unix_ms: index.last_success_unix_ms,
        error_category,
    }
}

fn matches_filters(
    document: &fuckipv6_common::domain::NetworkDiscoveryDocument,
    filters: &DiscoveryFilters,
) -> bool {
    if let Some(visibility) = filters.visibility {
        let matches = match visibility {
            DiscoveryVisibilityFilter::Public => document.visibility == NetworkVisibility::Public,
            DiscoveryVisibilityFilter::Private => document.visibility != NetworkVisibility::Public,
        };
        if !matches {
            return false;
        }
    }
    if filters
        .join_mode
        .is_some_and(|join_mode| document.join_mode != join_mode)
    {
        return false;
    }
    if filters
        .language
        .as_deref()
        .is_some_and(|language| !document.language.eq_ignore_ascii_case(language.trim()))
    {
        return false;
    }
    if filters
        .region
        .as_deref()
        .is_some_and(|region| !document.region.eq_ignore_ascii_case(region.trim()))
    {
        return false;
    }
    true
}

fn match_rank(
    document: &fuckipv6_common::domain::NetworkDiscoveryDocument,
    query: &str,
) -> Option<u8> {
    if query.is_empty() {
        return Some(3);
    }
    let title = document.name.to_lowercase();
    if title == query {
        return Some(0);
    }
    if title.starts_with(query) {
        return Some(1);
    }
    if title
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        return Some(2);
    }
    let capabilities = document
        .capability_summary
        .iter()
        .map(|capability| capability.wire_name())
        .collect::<Vec<_>>()
        .join(" ");
    let haystack = format!(
        "{} {} {} {} {} {} {}",
        document.name,
        document.purpose,
        document.tags.join(" "),
        capabilities,
        join_mode_search_terms(document.join_mode),
        document.language,
        document.region,
    )
    .to_lowercase();
    haystack.contains(query).then_some(3)
}

fn join_mode_search_terms(join_mode: NetworkJoinMode) -> &'static str {
    match join_mode {
        NetworkJoinMode::Open => "open",
        NetworkJoinMode::ApprovalRequired => "approval_required approval request",
        NetworkJoinMode::InviteOnly => "invite_only invite invitation",
    }
}

#[cfg(test)]
mod tests {
    use fuckipv6_common::crypto::Identity;
    use fuckipv6_common::domain::{
        DiscoveryTransport, NetworkDiscoveryDocument, NetworkDiscoveryRecord, NetworkEndpoint,
        Signature,
    };

    use super::*;
    use crate::discovery_index::{
        NodeDiscoveryIndexError, NodeDiscoveryIndexRecordSnapshot, NodeDiscoveryIndexSnapshot,
    };

    struct RecordFixture<'a> {
        name: &'a str,
        visibility: NetworkVisibility,
        join_mode: NetworkJoinMode,
        language: &'a str,
        region: &'a str,
        tags: &'a [&'a str],
        capabilities: Vec<CapabilityKind>,
        accepted_at_unix_ms: u64,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    }

    impl<'a> RecordFixture<'a> {
        fn visibility(mut self, visibility: NetworkVisibility) -> Self {
            self.visibility = visibility;
            self
        }

        fn join_mode(mut self, join_mode: NetworkJoinMode) -> Self {
            self.join_mode = join_mode;
            self
        }

        fn place(mut self, language: &'a str, region: &'a str) -> Self {
            self.language = language;
            self.region = region;
            self
        }

        fn tags(mut self, tags: &'a [&'a str]) -> Self {
            self.tags = tags;
            self
        }

        fn capabilities(mut self, capabilities: Vec<CapabilityKind>) -> Self {
            self.capabilities = capabilities;
            self
        }

        fn build(self) -> NodeDiscoveryIndexRecordSnapshot {
            let identity = Identity::generate().unwrap();
            let public_key =
                fuckipv6_common::domain::PublicKey::from_bytes(identity.public_key_bytes());
            let owner = fuckipv6_common::domain::NodeId::from_public_key(self.name.as_bytes());
            let document = NetworkDiscoveryDocument {
                schema_version: fuckipv6_common::domain::NETWORK_DISCOVERY_SCHEMA_VERSION,
                network_id: NetworkId::from_public_key(public_key.as_str().as_bytes()),
                public_key,
                publisher: owner.clone(),
                name: self.name.into(),
                purpose: format!("{} purpose", self.name),
                language: self.language.into(),
                region: self.region.into(),
                tags: self.tags.iter().map(|tag| tag.to_string()).collect(),
                visibility: self.visibility,
                join_mode: self.join_mode,
                owner,
                administrators: Vec::new(),
                revision: 1,
                policy_revision: 1,
                privacy_policy_revision: 1,
                member_limit: None,
                member_count: 1,
                exit_contribution: fuckipv6_common::domain::ExitContributionObligation::NotRequired,
                privacy_changes: Vec::new(),
                issued_at_unix_ms: self.issued_at_unix_ms,
                expires_at_unix_ms: self.expires_at_unix_ms,
                capability_summary: self.capabilities,
                endpoints: vec![NetworkEndpoint {
                    transport: DiscoveryTransport::Quic,
                    host: "relay.example".into(),
                    port: 443,
                }],
                revoked_at_unix_ms: None,
            };
            let signature = identity.sign(&document.signing_payload().unwrap());
            let record = NetworkDiscoveryRecord {
                document,
                signature: Signature::from_bytes(signature.to_bytes()),
            };
            NodeDiscoveryIndexRecordSnapshot {
                digest: record.digest().unwrap(),
                record,
                accepted_at_unix_ms: self.accepted_at_unix_ms,
                discoverable: true,
                invalid_reason: None,
            }
        }
    }

    fn record(
        name: &str,
        accepted_at_unix_ms: u64,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> RecordFixture<'_> {
        RecordFixture {
            name,
            visibility: NetworkVisibility::Public,
            join_mode: NetworkJoinMode::Open,
            language: "en",
            region: "US",
            tags: &[],
            capabilities: Vec::new(),
            accepted_at_unix_ms,
            issued_at_unix_ms,
            expires_at_unix_ms,
        }
    }

    fn snapshot(
        state: NodeDiscoveryIndexState,
        records: Vec<NodeDiscoveryIndexRecordSnapshot>,
    ) -> NodeSnapshot {
        NodeSnapshot {
            discovery_index: NodeDiscoveryIndexSnapshot {
                state,
                records,
                ..NodeDiscoveryIndexSnapshot::default()
            },
            ..NodeSnapshot::default()
        }
    }

    #[test]
    fn searches_name_tags_capabilities_and_join_mode() {
        let now = 1_000;
        let relay = record("studio", now, 100, 100_000)
            .join_mode(NetworkJoinMode::ApprovalRequired)
            .tags(&["creative", "relay"])
            .capabilities(vec![CapabilityKind::Relay])
            .build();
        let home = record("home", now, 100, 100_000)
            .visibility(NetworkVisibility::Private)
            .place("zh-CN", "CN")
            .tags(&["family"])
            .capabilities(vec![CapabilityKind::Exit])
            .build();
        let snapshot = snapshot(NodeDiscoveryIndexState::Ready, vec![relay, home]);

        let by_name = project_discovery(&snapshot, "stu", &DiscoveryFilters::default(), now);
        assert_eq!(by_name.results[0].title, "studio");
        let by_tag = project_discovery(&snapshot, "family", &DiscoveryFilters::default(), now);
        assert_eq!(by_tag.results[0].title, "home");
        let by_capability =
            project_discovery(&snapshot, "relay", &DiscoveryFilters::default(), now);
        assert_eq!(by_capability.results[0].title, "studio");
        let by_join = project_discovery(&snapshot, "approval", &DiscoveryFilters::default(), now);
        assert_eq!(by_join.results[0].title, "studio");
    }

    #[test]
    fn filters_visibility_join_mode_language_and_region() {
        let now = 1_000;
        let public = record("alpha", now, 100, 100_000).build();
        let private = record("beta", now, 100, 100_000)
            .visibility(NetworkVisibility::Private)
            .join_mode(NetworkJoinMode::InviteOnly)
            .place("zh-CN", "CN")
            .build();
        let snapshot = snapshot(NodeDiscoveryIndexState::Ready, vec![public, private]);

        let mut filters = DiscoveryFilters {
            visibility: Some(DiscoveryVisibilityFilter::Public),
            join_mode: Some(NetworkJoinMode::Open),
            language: Some("EN".into()),
            region: Some("us".into()),
        };
        let result = project_discovery(&snapshot, "", &filters, now);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].title, "alpha");

        filters.visibility = Some(DiscoveryVisibilityFilter::Private);
        filters.join_mode = Some(NetworkJoinMode::InviteOnly);
        filters.language = Some("zh-cn".into());
        filters.region = Some("CN".into());
        let result = project_discovery(&snapshot, "", &filters, now);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].title, "beta");
    }

    #[test]
    fn only_currently_valid_records_are_visible() {
        let record = record("expired", 100, 100, 1_000).build();
        let snapshot = snapshot(NodeDiscoveryIndexState::Ready, vec![record]);
        let projection = project_discovery(&snapshot, "", &DiscoveryFilters::default(), 1_000);
        assert_eq!(projection.state, DiscoveryProjectionState::Empty);
        assert!(projection.results.is_empty());
    }

    #[test]
    fn empty_no_matches_failure_partial_and_stale_are_distinct() {
        let empty = snapshot(NodeDiscoveryIndexState::Ready, vec![]);
        assert_eq!(
            project_discovery(&empty, "", &DiscoveryFilters::default(), 1_000).state,
            DiscoveryProjectionState::Empty
        );

        let current = record("home", 1_000, 100, 100_000).build();
        let ready = snapshot(NodeDiscoveryIndexState::Ready, vec![current.clone()]);
        assert_eq!(
            project_discovery(&ready, "missing", &DiscoveryFilters::default(), 1_000).state,
            DiscoveryProjectionState::NoMatches
        );

        let failed = snapshot(NodeDiscoveryIndexState::Failed, vec![]);
        assert_eq!(
            project_discovery(&failed, "", &DiscoveryFilters::default(), 1_000).state,
            DiscoveryProjectionState::Failed
        );

        let partial = snapshot(NodeDiscoveryIndexState::Ready, vec![current.clone()]);
        let mut partial = partial;
        partial.discovery_index.rejected_record_count = 2;
        let projection = project_discovery(&partial, "", &DiscoveryFilters::default(), 1_000);
        assert!(projection.partial);
        assert_eq!(projection.rejected_record_count, 2);
        assert_eq!(projection.state, DiscoveryProjectionState::Degraded);
        assert!(!projection.no_more_results);

        let stale = snapshot(NodeDiscoveryIndexState::Degraded, vec![current]);
        let projection = project_discovery(&stale, "", &DiscoveryFilters::default(), 1_000);
        assert!(projection.results[0].stale);
        assert!(projection.has_stale_results());
        assert_eq!(projection.error_category, None);
    }

    #[test]
    fn rejects_invalid_revoked_expired_and_non_discoverable_records() {
        let now = 1_000;
        let valid = record("valid", now, 100, 100_000).build();
        let mut invalid_signature = valid.clone();
        invalid_signature.record.signature = Signature::from_bytes([7; 64]);

        let mut revoked = valid.clone();
        revoked.record.document.revoked_at_unix_ms = Some(900);

        let mut undiscoverable = valid.clone();
        undiscoverable.discoverable = false;

        let expired = record("expired", now, 100, now).build();
        let snapshot = snapshot(
            NodeDiscoveryIndexState::Ready,
            vec![valid, invalid_signature, revoked, undiscoverable, expired],
        );
        let projection = project_discovery(&snapshot, "", &DiscoveryFilters::default(), now);
        assert_eq!(projection.results.len(), 1);
        assert_eq!(projection.results[0].title, "valid");
        assert_eq!(projection.valid_record_count, 1);
        assert_eq!(projection.state, DiscoveryProjectionState::Ready);
        assert!(projection.no_more_results);
    }

    #[test]
    fn degraded_index_with_no_valid_records_is_not_reported_as_empty() {
        let mut snapshot = snapshot(
            NodeDiscoveryIndexState::Degraded,
            vec![record("valid", 100, 100, 100_000).build()],
        );
        snapshot.discovery_index.last_error = Some(NodeDiscoveryIndexError::new(
            NodeDiscoveryIndexErrorCategory::Timeout,
        ));
        let query_miss =
            project_discovery(&snapshot, "missing", &DiscoveryFilters::default(), 5_000);
        assert_eq!(query_miss.state, DiscoveryProjectionState::Degraded);
        assert_eq!(query_miss.valid_record_count, 1);
        assert!(!query_miss.no_more_results);
        assert_eq!(query_miss.results.len(), 0);

        let mut no_records = snapshot;
        no_records.discovery_index.records.clear();
        let empty = project_discovery(&no_records, "", &DiscoveryFilters::default(), 5_000);
        assert_eq!(empty.state, DiscoveryProjectionState::Degraded);
        assert_eq!(empty.valid_record_count, 0);
        assert!(!empty.no_more_results);
    }

    #[test]
    fn join_mode_search_accepts_stable_wire_and_human_terms() {
        let now = 1_000;
        let snapshot = snapshot(
            NodeDiscoveryIndexState::Ready,
            vec![
                record("open", now, 100, 100_000).build(),
                record("approval", now, 100, 100_000)
                    .join_mode(NetworkJoinMode::ApprovalRequired)
                    .build(),
                record("invite", now, 100, 100_000)
                    .visibility(NetworkVisibility::Private)
                    .join_mode(NetworkJoinMode::InviteOnly)
                    .build(),
            ],
        );
        for (query, expected) in [
            ("approval_required", "approval"),
            ("request", "approval"),
            ("invite", "invite"),
            ("invitation", "invite"),
            ("open", "open"),
        ] {
            let result = project_discovery(&snapshot, query, &DiscoveryFilters::default(), now);
            assert_eq!(result.results[0].title, expected, "query={query}");
        }
    }

    #[test]
    fn index_error_category_and_partial_status_are_preserved() {
        let mut value = snapshot(NodeDiscoveryIndexState::Degraded, vec![]);
        value.discovery_index.last_error = Some(NodeDiscoveryIndexError::new(
            NodeDiscoveryIndexErrorCategory::Timeout,
        ));
        let projection = project_discovery(&value, "", &DiscoveryFilters::default(), 1_000);
        assert_eq!(
            projection.error_category,
            Some(NodeDiscoveryIndexErrorCategory::Timeout)
        );
        assert!(projection.partial);
    }
}
