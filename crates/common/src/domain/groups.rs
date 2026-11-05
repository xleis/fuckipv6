//! Static, dynamic and nested network groups.
//!
//! Groups are a pure authorization input: evaluating them never mutates
//! membership identity, never executes user-supplied expressions and never
//! grants access on its own. The catalog resolves every group to a
//! deterministic, explainable `NodeId` set and leaves the final decision to
//! the permission engine, where a matching deny always wins.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityKind;
use super::ids::{ContentHash, GroupId, NetworkId, NodeId, SecurityRuleId};
use super::permissions::{
    evaluate_network_permissions, AccessContext, GroupDefinition, GroupKind, NetworkGroup,
    NetworkRole, PermissionDecision, PermissionRequest, PermissionRule, SubjectSelector,
};
use super::{DomainError, DomainResult};

/// Maximum nesting depth for nested groups. The root group counts as depth 1.
pub const MAX_GROUP_NESTING_DEPTH: usize = 8;
/// Hard bound so a network cannot build an unbounded group graph.
pub const MAX_GROUPS_PER_NETWORK: usize = 256;

/// Durable system roles that can never be redefined, renamed or deleted.
///
/// The runtime-only emergency Coordinator role is deliberately absent because
/// it is not a durable network membership.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SystemGroupRole {
    Owner,
    Administrator,
    Member,
}

impl SystemGroupRole {
    pub const ALL: [Self; 3] = [Self::Owner, Self::Administrator, Self::Member];

    pub fn network_role(self) -> NetworkRole {
        match self {
            Self::Owner => NetworkRole::Owner,
            Self::Administrator => NetworkRole::Administrator,
            Self::Member => NetworkRole::Member,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Administrator => "administrator",
            Self::Member => "member",
        }
    }
}

/// Reserved system roles are accepted only as role selectors; a custom group
/// must never shadow their names.
pub const RESERVED_SYSTEM_ROLES: [SystemGroupRole; 3] = SystemGroupRole::ALL;

fn name_is_reserved(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    RESERVED_SYSTEM_ROLES
        .iter()
        .any(|role| role.name() == normalized)
}

/// One roster member as seen by group evaluation. `membership_active` is
/// carried into the access context rather than folded into group membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterMember {
    pub node_id: NodeId,
    pub role: NetworkRole,
    pub capabilities: BTreeSet<CapabilityKind>,
    pub membership_active: bool,
}

impl RosterMember {
    pub fn new(node_id: NodeId, role: NetworkRole) -> Self {
        Self {
            node_id,
            role,
            capabilities: BTreeSet::new(),
            membership_active: true,
        }
    }

    pub fn with_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = CapabilityKind>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().collect();
        self
    }

    pub fn with_active(mut self, active: bool) -> Self {
        self.membership_active = active;
        self
    }
}

/// Sanitized roster used for deterministic group evaluation. Members are
/// keyed by NodeId, so a group can never hold stale or duplicate identities.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemberRoster {
    members: BTreeMap<NodeId, RosterMember>,
}

impl MemberRoster {
    pub fn new(members: impl IntoIterator<Item = RosterMember>) -> DomainResult<Self> {
        let mut roster = BTreeMap::new();
        for member in members {
            if roster.insert(member.node_id.clone(), member).is_some() {
                return Err(DomainError::invariant(
                    "member roster contains duplicate nodes",
                ));
            }
        }
        Ok(Self { members: roster })
    }

    pub fn get(&self, node_id: &NodeId) -> Option<&RosterMember> {
        self.members.get(node_id)
    }

    pub fn contains(&self, node_id: &NodeId) -> bool {
        self.members.contains_key(node_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RosterMember> {
        self.members.values()
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Deterministic, explainable resolution of a single group.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct GroupEvaluation {
    pub group_id: GroupId,
    pub kind: GroupKind,
    /// Members matched directly by the group's own definition.
    pub direct_members: Vec<NodeId>,
    /// Sorted union of direct and nested members that exist in the roster.
    pub resolved_members: Vec<NodeId>,
    /// For nested members, the child groups that contributed the node.
    pub via: BTreeMap<NodeId, Vec<GroupId>>,
}

/// Per-group member diff produced by a preview.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct GroupMemberChange {
    pub group_id: GroupId,
    pub added_nodes: Vec<NodeId>,
    pub removed_nodes: Vec<NodeId>,
}

/// Serializable projection of a [`PermissionDecision`]. The decision code is
/// kept so a preview can explain why access changed, while the internal layer
/// tag stays out of the wire format.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AccessDecisionView {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl AccessDecisionView {
    pub fn from_decision(decision: &PermissionDecision) -> Self {
        match decision {
            PermissionDecision::Allow => Self {
                allowed: true,
                code: None,
            },
            PermissionDecision::Deny { code, .. } => Self {
                allowed: false,
                code: Some(code.clone()),
            },
        }
    }
}

/// Access change for one roster member between two catalog revisions.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct MemberAccessChange {
    pub node_id: NodeId,
    pub before: AccessDecisionView,
    pub after: AccessDecisionView,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupReferenceSource {
    NestedGroup { group_id: GroupId },
    PermissionRule { rule_id: SecurityRuleId },
}

/// A reference that blocks deletion of a group.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct GroupReference {
    pub group_id: GroupId,
    pub source: GroupReferenceSource,
}

/// A single group deletion. A replacement is only honoured for nested group
/// references; permission rules must be updated first so no policy can keep a
/// dangling grant.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct GroupDelete {
    pub group_id: GroupId,
    pub replacement: Option<GroupId>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct GroupChange {
    pub expected_revision: u64,
    #[serde(default)]
    pub upserts: Vec<NetworkGroup>,
    #[serde(default)]
    pub deletes: Vec<GroupDelete>,
}

/// Preview of a prospective group change. Nothing is applied here.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct GroupChangePreview {
    pub base_revision: u64,
    pub candidate_revision: u64,
    pub added_group_ids: Vec<GroupId>,
    pub removed_group_ids: Vec<GroupId>,
    pub updated_group_ids: Vec<GroupId>,
    pub member_changes: Vec<GroupMemberChange>,
    pub permission_impact: Vec<MemberAccessChange>,
    /// Groups change authorization inputs only. Membership identity is never
    /// rewritten, so this is always zero.
    pub identity_changes: u32,
    /// References that must be resolved before the change can be applied.
    pub blocking_references: Vec<GroupReference>,
    pub applicable: bool,
}

/// Result of an atomic apply. The rollback token can restore the previous
/// revision only while the applied revision is still current.
#[derive(Debug, Clone)]
pub struct GroupApplyOutcome {
    pub catalog: GroupCatalog,
    pub preview: GroupChangePreview,
    pub rollback: GroupRollback,
}

#[derive(Debug, Clone)]
pub struct GroupRollback {
    network_id: NetworkId,
    applied_revision: u64,
    applied_digest: ContentHash,
    previous: GroupCatalog,
}

/// Versioned, immutable-per-revision group catalog.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct GroupCatalog {
    pub network_id: NetworkId,
    pub revision: u64,
    #[serde(default)]
    pub groups: Vec<NetworkGroup>,
}

impl GroupCatalog {
    pub fn new(
        network_id: NetworkId,
        revision: u64,
        groups: impl IntoIterator<Item = NetworkGroup>,
    ) -> DomainResult<Self> {
        let mut catalog = Self {
            network_id,
            revision,
            groups: groups.into_iter().collect(),
        };
        catalog
            .groups
            .sort_by(|left, right| left.group_id.cmp(&right.group_id));
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> DomainResult<()> {
        if self.revision == 0 {
            return Err(DomainError::invalid_value(
                "group catalog revision must be positive",
            ));
        }
        if self.groups.len() > MAX_GROUPS_PER_NETWORK {
            return Err(DomainError::invariant(
                "group catalog exceeds the per-network limit",
            ));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for group in &self.groups {
            if group.network_id != self.network_id {
                return Err(DomainError::invariant("group belongs to another network"));
            }
            group.validate()?;
            if name_is_reserved(&group.name) {
                return Err(DomainError::invalid_value(
                    "group name is reserved by a system role",
                ));
            }
            if !ids.insert(&group.group_id) {
                return Err(DomainError::invariant("duplicate group id"));
            }
            let normalized = group.name.trim().to_ascii_lowercase();
            if !names.insert(normalized) {
                return Err(DomainError::invariant("duplicate group name"));
            }
        }
        self.validate_references()
    }

    fn validate_references(&self) -> DomainResult<()> {
        let by_id: BTreeMap<&GroupId, &NetworkGroup> = self
            .groups
            .iter()
            .map(|group| (&group.group_id, group))
            .collect();
        for group in &self.groups {
            let mut stack = Vec::new();
            self.visit(&by_id, &group.group_id, 1, &mut stack)?;
        }
        Ok(())
    }

    fn visit<'a>(
        &self,
        by_id: &BTreeMap<&'a GroupId, &'a NetworkGroup>,
        group_id: &'a GroupId,
        depth: usize,
        stack: &mut Vec<&'a GroupId>,
    ) -> DomainResult<()> {
        if depth > MAX_GROUP_NESTING_DEPTH {
            return Err(DomainError::new(
                "domain.group_depth_exceeded",
                "nested group depth exceeds the limit",
            ));
        }
        if stack.contains(&group_id) {
            return Err(DomainError::new(
                "domain.group_cycle",
                "nested groups must not form a cycle",
            ));
        }
        let group = by_id.get(group_id).ok_or_else(|| {
            DomainError::new(
                "domain.group_unknown_reference",
                "nested group references an unknown group",
            )
        })?;
        if let GroupDefinition::Nested { members } = &group.definition {
            stack.push(group_id);
            for child in members {
                self.visit(by_id, child, depth + 1, stack)?;
            }
            stack.pop();
        }
        Ok(())
    }

    pub fn group(&self, group_id: &GroupId) -> Option<&NetworkGroup> {
        self.groups.iter().find(|group| &group.group_id == group_id)
    }

    pub fn content_hash(&self) -> DomainResult<ContentHash> {
        let bytes = serde_json::to_vec(self)
            .map_err(|_| DomainError::invalid_value("group catalog cannot be encoded"))?;
        Ok(ContentHash::from_bytes(&bytes))
    }

    /// Resolve one group against the current roster. Nested groups are
    /// resolved recursively with cycle and depth checks at evaluation time as
    /// well, so no code path can loop forever.
    pub fn evaluate(
        &self,
        group_id: &GroupId,
        roster: &MemberRoster,
    ) -> DomainResult<GroupEvaluation> {
        let mut memo = BTreeMap::new();
        let mut stack = Vec::new();
        let resolved = self.resolve(group_id, roster, &mut memo, &mut stack, 1)?;
        let group = self.group(group_id).ok_or_else(|| {
            DomainError::new(
                "domain.group_unknown_reference",
                "group does not exist in this catalog",
            )
        })?;
        let direct_members = self.direct_members(group, roster);
        let mut via: BTreeMap<NodeId, Vec<GroupId>> = BTreeMap::new();
        if let GroupDefinition::Nested { members } = &group.definition {
            for child in members {
                let child_group = self.group(child).ok_or_else(|| {
                    DomainError::new(
                        "domain.group_unknown_reference",
                        "nested group references an unknown group",
                    )
                })?;
                let child_members = self.resolve(child, roster, &mut memo, &mut stack, 2)?;
                for node in child_members {
                    let entry = via.entry(node).or_default();
                    if !entry.contains(&child_group.group_id) {
                        entry.push(child_group.group_id.clone());
                    }
                }
            }
        }
        Ok(GroupEvaluation {
            group_id: group_id.clone(),
            kind: group.kind(),
            direct_members: direct_members.into_iter().collect(),
            resolved_members: resolved.into_iter().collect(),
            via,
        })
    }

    fn direct_members(&self, group: &NetworkGroup, roster: &MemberRoster) -> BTreeSet<NodeId> {
        match &group.definition {
            GroupDefinition::Manual { members } => members
                .iter()
                .filter(|node| roster.contains(node))
                .cloned()
                .collect(),
            GroupDefinition::Dynamic { selector } => roster
                .iter()
                .filter(|member| {
                    selector.matches_member(member.role, &member.node_id, &member.capabilities)
                })
                .map(|member| member.node_id.clone())
                .collect(),
            GroupDefinition::Nested { .. } => BTreeSet::new(),
        }
    }

    fn resolve(
        &self,
        group_id: &GroupId,
        roster: &MemberRoster,
        memo: &mut BTreeMap<GroupId, BTreeSet<NodeId>>,
        stack: &mut Vec<GroupId>,
        depth: usize,
    ) -> DomainResult<BTreeSet<NodeId>> {
        if depth > MAX_GROUP_NESTING_DEPTH {
            return Err(DomainError::new(
                "domain.group_depth_exceeded",
                "nested group depth exceeds the limit",
            ));
        }
        if let Some(resolved) = memo.get(group_id) {
            return Ok(resolved.clone());
        }
        if stack.contains(group_id) {
            return Err(DomainError::new(
                "domain.group_cycle",
                "nested groups must not form a cycle",
            ));
        }
        let group = self.group(group_id).ok_or_else(|| {
            DomainError::new(
                "domain.group_unknown_reference",
                "nested group references an unknown group",
            )
        })?;
        let resolved = match &group.definition {
            GroupDefinition::Manual { .. } | GroupDefinition::Dynamic { .. } => {
                self.direct_members(group, roster)
            }
            GroupDefinition::Nested { members } => {
                stack.push(group_id.clone());
                let mut union = BTreeSet::new();
                for child in members {
                    union.extend(self.resolve(child, roster, memo, stack, depth + 1)?);
                }
                stack.pop();
                union
            }
        };
        memo.insert(group_id.clone(), resolved.clone());
        Ok(resolved)
    }

    /// Every group whose resolved membership contains `node_id`.
    pub fn access_groups(&self, roster: &MemberRoster, node_id: &NodeId) -> BTreeSet<GroupId> {
        let mut groups = BTreeSet::new();
        for group in &self.groups {
            if let Ok(evaluation) = self.evaluate(&group.group_id, roster) {
                if evaluation
                    .resolved_members
                    .iter()
                    .any(|member| member == node_id)
                {
                    groups.insert(group.group_id.clone());
                }
            }
        }
        groups
    }

    pub fn preview(
        &self,
        change: &GroupChange,
        rules: &[PermissionRule],
        roster: &MemberRoster,
        request: &PermissionRequest,
    ) -> DomainResult<GroupChangePreview> {
        // Deletion is refused before anything else: a group that is still
        // referenced must never leave a dangling nested or policy reference
        // behind, so the preview reports the blocking references instead of a
        // generic validation error.
        let blocking_references = self.blocking_references(change, rules);
        if !blocking_references.is_empty() {
            return Ok(GroupChangePreview {
                base_revision: self.revision,
                candidate_revision: self.revision + 1,
                added_group_ids: Vec::new(),
                removed_group_ids: Vec::new(),
                updated_group_ids: Vec::new(),
                member_changes: Vec::new(),
                permission_impact: Vec::new(),
                identity_changes: 0,
                blocking_references,
                applicable: false,
            });
        }
        let candidate = self.candidate(change)?;
        let old_ids: BTreeSet<&GroupId> = self.groups.iter().map(|group| &group.group_id).collect();
        let new_ids: BTreeSet<&GroupId> = candidate
            .groups
            .iter()
            .map(|group| &group.group_id)
            .collect();
        let added_group_ids: Vec<GroupId> = new_ids
            .difference(&old_ids)
            .map(|id| (*id).clone())
            .collect();
        let removed_group_ids: Vec<GroupId> = old_ids
            .difference(&new_ids)
            .map(|id| (*id).clone())
            .collect();
        let mut updated_group_ids = Vec::new();
        let mut member_changes = Vec::new();
        let union: BTreeSet<&GroupId> = old_ids.union(&new_ids).copied().collect();
        for group_id in union {
            let before = self.evaluate(group_id, roster).ok();
            let after = candidate.evaluate(group_id, roster).ok();
            let before_members: BTreeSet<NodeId> = before
                .as_ref()
                .map(|evaluation| evaluation.resolved_members.iter().cloned().collect())
                .unwrap_or_default();
            let after_members: BTreeSet<NodeId> = after
                .as_ref()
                .map(|evaluation| evaluation.resolved_members.iter().cloned().collect())
                .unwrap_or_default();
            if before_members != after_members {
                member_changes.push(GroupMemberChange {
                    group_id: group_id.clone(),
                    added_nodes: after_members.difference(&before_members).cloned().collect(),
                    removed_nodes: before_members.difference(&after_members).cloned().collect(),
                });
            }
            let definitions_differ = match (self.group(group_id), candidate.group(group_id)) {
                (Some(before), Some(after)) => before.definition != after.definition,
                (None, Some(_)) | (Some(_), None) => true,
                (None, None) => false,
            };
            if definitions_differ && !removed_group_ids.contains(group_id) {
                updated_group_ids.push(group_id.clone());
            }
        }

        let mut permission_impact = Vec::new();
        for member in roster.iter() {
            let before =
                evaluate_network_permissions(rules, request, &self.access_context(roster, member));
            let after = evaluate_network_permissions(
                rules,
                request,
                &candidate.access_context(roster, member),
            );
            if before != after {
                permission_impact.push(MemberAccessChange {
                    node_id: member.node_id.clone(),
                    before: AccessDecisionView::from_decision(&before),
                    after: AccessDecisionView::from_decision(&after),
                });
            }
        }

        Ok(GroupChangePreview {
            base_revision: self.revision,
            candidate_revision: candidate.revision,
            added_group_ids,
            removed_group_ids,
            updated_group_ids,
            member_changes,
            permission_impact,
            identity_changes: 0,
            blocking_references: Vec::new(),
            applicable: true,
        })
    }

    /// Apply a change atomically. The candidate catalog is fully validated
    /// before it replaces the current revision, so a failed change leaves the
    /// live catalog untouched.
    pub fn apply(
        self,
        change: &GroupChange,
        rules: &[PermissionRule],
        roster: &MemberRoster,
        request: &PermissionRequest,
    ) -> DomainResult<GroupApplyOutcome> {
        let preview = self.preview(change, rules, roster, request)?;
        if !preview.applicable {
            return Err(DomainError::new(
                "domain.group_change_blocked",
                "group deletion is referenced by nested groups or permission rules",
            ));
        }
        let candidate = self.candidate(change)?;
        let digest = candidate.content_hash()?;
        let rollback = GroupRollback {
            network_id: self.network_id.clone(),
            applied_revision: candidate.revision,
            applied_digest: digest,
            previous: self,
        };
        Ok(GroupApplyOutcome {
            catalog: candidate,
            preview,
            rollback,
        })
    }

    /// Restore the catalog captured by an apply. This fails closed if any
    /// later revision or a different catalog is currently live.
    pub fn rollback(self, token: GroupRollback) -> DomainResult<GroupCatalog> {
        if token.network_id != self.network_id {
            return Err(DomainError::invariant(
                "rollback token belongs to another network",
            ));
        }
        if self.revision != token.applied_revision || self.content_hash()? != token.applied_digest {
            return Err(DomainError::new(
                "domain.group_stale_revision",
                "group catalog changed after the rollback token was issued",
            ));
        }
        Ok(token.previous)
    }

    fn access_context(&self, roster: &MemberRoster, member: &RosterMember) -> AccessContext {
        AccessContext {
            network_id: self.network_id.clone(),
            actor: member.node_id.clone(),
            role: member.role,
            groups: self.access_groups(roster, &member.node_id),
            membership_active: member.membership_active,
            recent_reauthentication: true,
            emergency_coordinator: false,
        }
    }

    fn candidate(&self, change: &GroupChange) -> DomainResult<GroupCatalog> {
        if change.expected_revision != self.revision {
            return Err(DomainError::new(
                "domain.group_stale_revision",
                "group catalog revision changed before the request was applied",
            ));
        }
        let mut by_id: BTreeMap<GroupId, NetworkGroup> = self
            .groups
            .iter()
            .map(|group| (group.group_id.clone(), group.clone()))
            .collect();
        let delete_ids: BTreeSet<&GroupId> = change
            .deletes
            .iter()
            .map(|delete| &delete.group_id)
            .collect();
        if delete_ids.len() != change.deletes.len() {
            return Err(DomainError::invariant("duplicate group deletion"));
        }
        for delete in &change.deletes {
            if !by_id.contains_key(&delete.group_id) {
                return Err(DomainError::new(
                    "domain.group_unknown_reference",
                    "deleted group does not exist",
                ));
            }
        }
        let mut upsert_ids = BTreeSet::new();
        for group in &change.upserts {
            if group.network_id != self.network_id {
                return Err(DomainError::invariant("group belongs to another network"));
            }
            if delete_ids.contains(&group.group_id) {
                return Err(DomainError::invariant(
                    "group change both upserts and deletes the same group",
                ));
            }
            if !upsert_ids.insert(&group.group_id) {
                return Err(DomainError::invariant("duplicate group upsert"));
            }
        }
        // Deletions are applied after rewriting nested references so a
        // replacement never leaves a dangling group id behind.
        let replacements: BTreeMap<&GroupId, &GroupId> = change
            .deletes
            .iter()
            .filter_map(|delete| {
                delete
                    .replacement
                    .as_ref()
                    .map(|replacement| (&delete.group_id, replacement))
            })
            .collect();
        for delete in &change.deletes {
            by_id.remove(&delete.group_id);
        }
        for group in by_id.values_mut() {
            if let GroupDefinition::Nested { members } = &mut group.definition {
                for member in members.iter_mut() {
                    if let Some(replacement) = replacements.get(member) {
                        *member = (*replacement).clone();
                    }
                }
            }
        }
        for group in change.upserts.iter() {
            by_id.insert(group.group_id.clone(), group.clone());
        }
        let groups: Vec<NetworkGroup> = by_id.into_values().collect();
        GroupCatalog::new(self.network_id.clone(), self.revision + 1, groups)
    }

    /// Every reference that would dangle if the deletion were applied as-is.
    /// Nested references are only rewritten when the deletion supplies a
    /// replacement group; permission rules must be updated by their owner
    /// first so no policy can keep a stale grant.
    fn blocking_references(
        &self,
        change: &GroupChange,
        rules: &[PermissionRule],
    ) -> Vec<GroupReference> {
        let deleted: BTreeSet<&GroupId> = change
            .deletes
            .iter()
            .map(|delete| &delete.group_id)
            .collect();
        let mut references = Vec::new();
        for delete in &change.deletes {
            if delete.replacement.is_none() {
                let sources: Vec<&NetworkGroup> = self
                    .groups
                    .iter()
                    .chain(change.upserts.iter())
                    .filter(|group| !deleted.contains(&group.group_id))
                    .collect();
                for group in sources {
                    if let GroupDefinition::Nested { members } = &group.definition {
                        if members.contains(&delete.group_id) {
                            references.push(GroupReference {
                                group_id: delete.group_id.clone(),
                                source: GroupReferenceSource::NestedGroup {
                                    group_id: group.group_id.clone(),
                                },
                            });
                        }
                    }
                }
            }
            for rule in rules {
                if rule
                    .subjects
                    .iter()
                    .any(|subject| matches!(subject, SubjectSelector::Group(id) if id == &delete.group_id))
                {
                    references.push(GroupReference {
                        group_id: delete.group_id.clone(),
                        source: GroupReferenceSource::PermissionRule {
                            rule_id: rule.rule_id.clone(),
                        },
                    });
                }
            }
        }
        references
    }
}

#[cfg(test)]
mod tests {
    use super::super::permissions::{
        PermissionCondition, PermissionEffect, PolicyAction, ResourceKind, ResourceRef,
    };
    use super::*;

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"group-network")
    }

    fn node(seed: u8) -> NodeId {
        NodeId::from_public_key(&[seed; 32])
    }

    fn group_id(seed: u8) -> GroupId {
        GroupId::from_public_key(&[seed; 32])
    }

    fn group(
        network_id: &NetworkId,
        seed: u8,
        name: &str,
        definition: GroupDefinition,
    ) -> NetworkGroup {
        NetworkGroup {
            group_id: group_id(seed),
            network_id: network_id.clone(),
            name: name.to_owned(),
            definition,
        }
    }

    fn dynamic_selector(
        roles: Vec<NetworkRole>,
        nodes: Vec<NodeId>,
        capabilities: Vec<CapabilityKind>,
    ) -> GroupDefinition {
        GroupDefinition::Dynamic {
            selector: super::super::permissions::DynamicGroupSelector {
                roles,
                nodes,
                capabilities,
            },
        }
    }

    fn roster() -> MemberRoster {
        MemberRoster::new(vec![
            RosterMember::new(node(1), NetworkRole::Owner),
            RosterMember::new(node(2), NetworkRole::Administrator)
                .with_capabilities([CapabilityKind::Exit]),
            RosterMember::new(node(3), NetworkRole::Member)
                .with_capabilities([CapabilityKind::File]),
            RosterMember::new(node(4), NetworkRole::Member).with_active(false),
        ])
        .unwrap()
    }

    fn request() -> PermissionRequest {
        PermissionRequest {
            action: PolicyAction::Use,
            resource: ResourceRef {
                kind: ResourceKind::ExitGrant,
                network_id: network(),
                object_id: None,
            },
        }
    }

    fn rule(
        seed: u8,
        effect: PermissionEffect,
        subject: SubjectSelector,
        action: PolicyAction,
        resource: ResourceKind,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(&[seed; 32]),
            effect,
            subjects: vec![subject],
            resources: vec![resource],
            actions: vec![action],
            conditions: vec![PermissionCondition::MembershipActive],
        }
    }

    fn exit_rule(seed: u8, effect: PermissionEffect, subject: SubjectSelector) -> PermissionRule {
        rule(
            seed,
            effect,
            subject,
            PolicyAction::Use,
            ResourceKind::ExitGrant,
        )
    }

    fn chain(network_id: &NetworkId, length: u8) -> Vec<NetworkGroup> {
        (0..length)
            .map(|index| {
                let definition = if index + 1 == length {
                    GroupDefinition::Manual {
                        members: vec![node(1)],
                    }
                } else {
                    GroupDefinition::Nested {
                        members: vec![group_id(index + 2)],
                    }
                };
                group(network_id, index + 1, &format!("level-{index}"), definition)
            })
            .collect()
    }

    #[test]
    fn manual_dynamic_and_nested_groups_resolve_deterministically() {
        let network_id = network();
        let catalog = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![
                group(
                    &network_id,
                    1,
                    "manual",
                    GroupDefinition::Manual {
                        members: vec![node(1), node(2)],
                    },
                ),
                group(
                    &network_id,
                    2,
                    "administrators",
                    dynamic_selector(vec![NetworkRole::Administrator], Vec::new(), Vec::new()),
                ),
                group(
                    &network_id,
                    3,
                    "file-peers",
                    dynamic_selector(Vec::new(), Vec::new(), vec![CapabilityKind::File]),
                ),
                group(
                    &network_id,
                    4,
                    "pinned",
                    dynamic_selector(Vec::new(), vec![node(1)], Vec::new()),
                ),
                group(
                    &network_id,
                    5,
                    "everyone-important",
                    GroupDefinition::Nested {
                        members: vec![group_id(1), group_id(2), group_id(3), group_id(4)],
                    },
                ),
            ],
        )
        .unwrap();
        let roster = roster();

        let nested = catalog.evaluate(&group_id(5), &roster).unwrap();
        assert_eq!(nested.kind, GroupKind::Nested);
        assert!(nested.direct_members.is_empty());
        let resolved: BTreeSet<NodeId> = nested.resolved_members.iter().cloned().collect();
        assert_eq!(resolved, BTreeSet::from([node(1), node(2), node(3)]));
        let mut sorted = nested.resolved_members.clone();
        sorted.sort();
        assert_eq!(sorted, nested.resolved_members);
        assert_eq!(nested.via[&node(1)], vec![group_id(1), group_id(4)]);
        assert_eq!(nested.via[&node(2)], vec![group_id(1), group_id(2)]);
        assert_eq!(nested.via[&node(3)], vec![group_id(3)]);

        let file_peers = catalog.evaluate(&group_id(3), &roster).unwrap();
        assert_eq!(file_peers.kind, GroupKind::Dynamic);
        assert_eq!(file_peers.direct_members, vec![node(3)]);
        assert_eq!(file_peers.resolved_members, vec![node(3)]);

        // Resolving the same catalog twice is stable: no iteration, clock or
        // roster ordering can change the answer.
        assert_eq!(catalog.evaluate(&group_id(5), &roster).unwrap(), nested);

        // Inactive members stay visible as identities; the activation flag is
        // carried into the access context instead of silently dropping them.
        assert!(roster.contains(&node(4)));
        assert!(!roster.get(&node(4)).unwrap().membership_active);
    }

    #[test]
    fn reserved_system_role_names_cannot_be_shadowed_by_custom_groups() {
        assert_eq!(SystemGroupRole::ALL.len(), 3);
        assert_eq!(SystemGroupRole::Owner.network_role(), NetworkRole::Owner);
        assert_eq!(
            SystemGroupRole::Administrator.network_role(),
            NetworkRole::Administrator
        );
        assert_eq!(SystemGroupRole::Member.network_role(), NetworkRole::Member);

        let network_id = network();
        for role in SystemGroupRole::ALL {
            let err = GroupCatalog::new(
                network_id.clone(),
                1,
                vec![group(
                    &network_id,
                    9,
                    &format!("  {}  ", role.name().to_ascii_uppercase()),
                    GroupDefinition::Manual {
                        members: vec![node(1)],
                    },
                )],
            )
            .unwrap_err();
            assert_eq!(err.code(), "domain.invalid_value");
        }

        // Names that merely contain a role name are still available.
        let catalog = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![group(
                &network_id,
                8,
                "owners-circle",
                GroupDefinition::Manual {
                    members: vec![node(1)],
                },
            )],
        )
        .unwrap();
        assert_eq!(catalog.groups.len(), 1);

        // Names are unique per network regardless of case.
        let err = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![
                group(
                    &network_id,
                    6,
                    "ops",
                    GroupDefinition::Manual { members: vec![] },
                ),
                group(
                    &network_id,
                    7,
                    " OPS ",
                    GroupDefinition::Manual { members: vec![] },
                ),
            ],
        )
        .unwrap_err();
        assert_eq!(err.code(), "domain.invariant_violation");
    }

    #[test]
    fn group_expressions_are_structured_fields_and_never_scripts() {
        // A selector is a closed set of typed fields. Any expression payload is
        // rejected at the deserialization boundary.
        assert!(serde_json::from_str::<GroupDefinition>(
            r#"{"kind":"dynamic","data":{"selector":{"script":"role == 'owner'"}}}"#
        )
        .is_err());
        assert!(serde_json::from_str::<GroupDefinition>(
            r#"{"kind":"expression","data":{"source":"role == 'owner'"}}"#
        )
        .is_err());

        // Unknown selector types cannot be smuggled through the tagged enum.
        assert!(serde_json::from_str::<GroupDefinition>(
            r#"{"kind":"query","data":{"selector":{"roles":["owner"]}}}"#
        )
        .is_err());

        // An empty selector parses but fails domain validation instead of
        // silently matching everyone or no one.
        let empty: GroupDefinition =
            serde_json::from_str(r#"{"kind":"dynamic","data":{"selector":{}}}"#).unwrap();
        assert_eq!(empty.validate().unwrap_err().code(), "domain.invalid_value");

        let structured: GroupDefinition = serde_json::from_str(
            r#"{"kind":"dynamic","data":{"selector":{"roles":["administrator"],"capabilities":["file"]}}}"#,
        )
        .unwrap();
        structured.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&structured).unwrap()["kind"],
            serde_json::json!("dynamic")
        );
    }

    #[test]
    fn nested_group_cycles_depth_and_unknown_references_are_rejected() {
        let network_id = network();

        let cycle = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![
                group(
                    &network_id,
                    1,
                    "a",
                    GroupDefinition::Nested {
                        members: vec![group_id(2)],
                    },
                ),
                group(
                    &network_id,
                    2,
                    "b",
                    GroupDefinition::Nested {
                        members: vec![group_id(1)],
                    },
                ),
            ],
        )
        .unwrap_err();
        assert_eq!(cycle.code(), "domain.group_cycle");

        let self_reference = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![group(
                &network_id,
                3,
                "loop",
                GroupDefinition::Nested {
                    members: vec![group_id(3)],
                },
            )],
        )
        .unwrap_err();
        assert_eq!(self_reference.code(), "domain.group_cycle");

        let unknown = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![group(
                &network_id,
                4,
                "dangling",
                GroupDefinition::Nested {
                    members: vec![group_id(200)],
                },
            )],
        )
        .unwrap_err();
        assert_eq!(unknown.code(), "domain.group_unknown_reference");

        let at_limit = GroupCatalog::new(network_id.clone(), 1, chain(&network_id, 8));
        assert!(at_limit.is_ok());

        let too_deep = GroupCatalog::new(network_id.clone(), 1, chain(&network_id, 9)).unwrap_err();
        assert_eq!(too_deep.code(), "domain.group_depth_exceeded");

        let oversized: Vec<NetworkGroup> = (0..=MAX_GROUPS_PER_NETWORK as u32)
            .map(|index| NetworkGroup {
                group_id: GroupId::from_public_key(&index.to_le_bytes()),
                network_id: network_id.clone(),
                name: format!("group-{index}"),
                definition: GroupDefinition::Manual {
                    members: Vec::new(),
                },
            })
            .collect();
        let err = GroupCatalog::new(network_id.clone(), 1, oversized).unwrap_err();
        assert_eq!(err.code(), "domain.invariant_violation");
    }

    #[test]
    fn preview_reports_member_sets_diffs_and_permission_impact() {
        let network_id = network();
        let roster = roster();
        let request = request();
        let base_definition =
            dynamic_selector(vec![NetworkRole::Administrator], Vec::new(), Vec::new());
        let catalog = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![group(&network_id, 1, "exit-users", base_definition.clone())],
        )
        .unwrap();
        let rules = vec![exit_rule(
            1,
            PermissionEffect::Allow,
            SubjectSelector::Group(group_id(1)),
        )];

        let decision = |catalog: &GroupCatalog, node_id: NodeId| {
            evaluate_network_permissions(
                &rules,
                &request,
                &catalog.access_context(&roster, roster.get(&node_id).unwrap()),
            )
            .is_allowed()
        };
        assert!(decision(&catalog, node(2)));
        assert!(!decision(&catalog, node(3)));

        let change = GroupChange {
            expected_revision: 1,
            upserts: vec![group(
                &network_id,
                1,
                "exit-users",
                dynamic_selector(vec![NetworkRole::Member], Vec::new(), Vec::new()),
            )],
            deletes: Vec::new(),
        };
        let preview = catalog.preview(&change, &rules, &roster, &request).unwrap();
        assert_eq!(preview.base_revision, 1);
        assert_eq!(preview.candidate_revision, 2);
        assert!(preview.added_group_ids.is_empty());
        assert!(preview.removed_group_ids.is_empty());
        assert_eq!(preview.updated_group_ids, vec![group_id(1)]);
        assert_eq!(preview.identity_changes, 0);
        assert!(preview.applicable);
        assert!(preview.blocking_references.is_empty());
        assert_eq!(preview.member_changes.len(), 1);
        assert_eq!(preview.member_changes[0].group_id, group_id(1));
        assert_eq!(
            preview.member_changes[0]
                .added_nodes
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([node(3), node(4)])
        );
        assert_eq!(preview.member_changes[0].removed_nodes, vec![node(2)]);

        let mut sorted_added = preview.member_changes[0].added_nodes.clone();
        sorted_added.sort();
        assert_eq!(sorted_added, preview.member_changes[0].added_nodes);

        let impact: Vec<(NodeId, bool, bool)> = preview
            .permission_impact
            .iter()
            .map(|change| {
                (
                    change.node_id.clone(),
                    change.before.allowed,
                    change.after.allowed,
                )
            })
            .collect();
        assert_eq!(impact.len(), 2);
        assert!(impact.contains(&(node(2), true, false)));
        assert!(impact.contains(&(node(3), false, true)));

        // Preview is a pure projection: the live revision is untouched.
        assert_eq!(catalog.revision, 1);
        assert_eq!(
            catalog
                .evaluate(&group_id(1), &roster)
                .unwrap()
                .resolved_members,
            vec![node(2)]
        );
        assert_eq!(catalog.groups[0].definition, base_definition);
    }

    #[test]
    fn group_changes_apply_atomically_and_roll_back() {
        let network_id = network();
        let roster = roster();
        let request = request();
        let base = GroupCatalog::new(network_id.clone(), 1, Vec::new()).unwrap();

        let change = GroupChange {
            expected_revision: 1,
            upserts: vec![group(
                &network_id,
                1,
                "operators",
                GroupDefinition::Manual {
                    members: vec![node(1)],
                },
            )],
            deletes: Vec::new(),
        };
        let outcome = base.clone().apply(&change, &[], &roster, &request).unwrap();
        assert_eq!(outcome.catalog.revision, 2);
        assert_eq!(outcome.preview.added_group_ids, vec![group_id(1)]);
        assert_eq!(outcome.catalog.groups.len(), 1);

        // A rejected change leaves the live catalog untouched.
        let invalid = GroupChange {
            expected_revision: 1,
            upserts: Vec::new(),
            deletes: vec![GroupDelete {
                group_id: group_id(99),
                replacement: None,
            }],
        };
        let err = base
            .clone()
            .apply(&invalid, &[], &roster, &request)
            .unwrap_err();
        assert_eq!(err.code(), "domain.group_unknown_reference");
        assert_eq!(base.revision, 1);
        assert!(base.groups.is_empty());

        // Rollback restores the exact previous revision while it is current.
        let restored = outcome
            .catalog
            .clone()
            .rollback(outcome.rollback.clone())
            .unwrap();
        assert_eq!(restored, base);

        // ...and fails closed after any later revision becomes live.
        let noop = GroupChange {
            expected_revision: 2,
            upserts: Vec::new(),
            deletes: Vec::new(),
        };
        let advanced = outcome
            .catalog
            .clone()
            .apply(&noop, &[], &roster, &request)
            .unwrap();
        assert_eq!(advanced.catalog.revision, 3);
        let err = advanced
            .catalog
            .clone()
            .rollback(outcome.rollback.clone())
            .unwrap_err();
        assert_eq!(err.code(), "domain.group_stale_revision");

        // Concurrent editors are serialized by the revision CAS.
        let err = advanced
            .catalog
            .clone()
            .apply(&change, &[], &roster, &request)
            .unwrap_err();
        assert_eq!(err.code(), "domain.group_stale_revision");
    }

    #[test]
    fn dynamic_membership_changes_only_affect_authorization() {
        let network_id = network();
        let roster = roster();
        let original_roster = roster.clone();
        let request = request();
        let catalog = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![group(
                &network_id,
                1,
                "exits",
                dynamic_selector(vec![NetworkRole::Administrator], Vec::new(), Vec::new()),
            )],
        )
        .unwrap();
        let rules = vec![exit_rule(
            1,
            PermissionEffect::Allow,
            SubjectSelector::Group(group_id(1)),
        )];

        let change = GroupChange {
            expected_revision: 1,
            upserts: vec![group(
                &network_id,
                1,
                "exits",
                dynamic_selector(Vec::new(), Vec::new(), vec![CapabilityKind::File]),
            )],
            deletes: Vec::new(),
        };
        let outcome = catalog
            .clone()
            .apply(&change, &rules, &roster, &request)
            .unwrap();

        // Group membership moved from node 2 to node 3, but no identity,
        // role or activation flag was rewritten by the group engine.
        assert_eq!(outcome.preview.identity_changes, 0);
        assert_eq!(roster, original_roster);
        let groups = outcome.catalog.access_groups(&roster, &node(3));
        assert!(groups.contains(&group_id(1)));
        let groups = outcome.catalog.access_groups(&roster, &node(2));
        assert!(!groups.contains(&group_id(1)));
        assert_eq!(roster.get(&node(3)).unwrap().role, NetworkRole::Member);

        let after = outcome
            .catalog
            .access_context(&roster, roster.get(&node(3)).unwrap());
        assert!(evaluate_network_permissions(&rules, &request, &after).is_allowed());
        let before = catalog.access_context(&roster, roster.get(&node(3)).unwrap());
        assert!(!evaluate_network_permissions(&rules, &request, &before).is_allowed());
    }

    #[test]
    fn group_deletion_requires_reference_cleanup_and_handles_empty_groups() {
        let network_id = network();
        let roster = roster();
        let request = request();
        let leaf = group(
            &network_id,
            1,
            "leaf",
            GroupDefinition::Manual {
                members: Vec::new(),
            },
        );
        let spare = group(
            &network_id,
            2,
            "spare",
            GroupDefinition::Manual {
                members: Vec::new(),
            },
        );
        let parent = group(
            &network_id,
            3,
            "parent",
            GroupDefinition::Nested {
                members: vec![group_id(1)],
            },
        );
        let catalog = GroupCatalog::new(network_id.clone(), 1, vec![leaf, spare, parent]).unwrap();
        let rules = vec![exit_rule(
            1,
            PermissionEffect::Allow,
            SubjectSelector::Group(group_id(1)),
        )];

        let plain_delete = GroupChange {
            expected_revision: 1,
            upserts: Vec::new(),
            deletes: vec![GroupDelete {
                group_id: group_id(1),
                replacement: None,
            }],
        };
        let preview = catalog
            .preview(&plain_delete, &rules, &roster, &request)
            .unwrap();
        assert!(!preview.applicable);
        assert!(preview.blocking_references.iter().any(|reference| matches!(
            &reference.source,
            GroupReferenceSource::NestedGroup { group_id: nested } if nested == &group_id(3)
        )));
        assert!(preview.blocking_references.iter().any(|reference| matches!(
            &reference.source,
            GroupReferenceSource::PermissionRule { .. }
        )));
        let err = catalog
            .clone()
            .apply(&plain_delete, &rules, &roster, &request)
            .unwrap_err();
        assert_eq!(err.code(), "domain.group_change_blocked");

        // A replacement rewrites nested references, but never a policy rule.
        let replaced_delete = GroupChange {
            expected_revision: 1,
            upserts: Vec::new(),
            deletes: vec![GroupDelete {
                group_id: group_id(1),
                replacement: Some(group_id(2)),
            }],
        };
        let preview = catalog
            .preview(&replaced_delete, &rules, &roster, &request)
            .unwrap();
        assert!(!preview.applicable);
        assert_eq!(preview.blocking_references.len(), 1);
        assert!(matches!(
            preview.blocking_references[0].source,
            GroupReferenceSource::PermissionRule { .. }
        ));

        // Once the policy rule no longer references the group the change is
        // applicable and removes the empty group in one atomic revision.
        let outcome = catalog
            .clone()
            .apply(&replaced_delete, &[], &roster, &request)
            .unwrap();
        assert!(outcome.preview.applicable);
        assert_eq!(outcome.preview.removed_group_ids, vec![group_id(1)]);
        assert!(outcome.catalog.group(&group_id(1)).is_none());
        assert_eq!(
            outcome.catalog.group(&group_id(3)).unwrap().definition,
            GroupDefinition::Nested {
                members: vec![group_id(2)]
            }
        );

        // Deleting an unreferenced empty group needs no replacement.
        let delete_spare = GroupChange {
            expected_revision: 2,
            upserts: Vec::new(),
            deletes: vec![GroupDelete {
                group_id: group_id(2),
                replacement: None,
            }],
        };
        let err = outcome
            .catalog
            .clone()
            .apply(&delete_spare, &[], &roster, &request)
            .unwrap_err();
        // `parent` still nests it, so this is blocked rather than dangling.
        assert_eq!(err.code(), "domain.group_change_blocked");
    }

    #[test]
    fn deny_rules_outrank_any_group_allow_and_the_role_ceiling() {
        let network_id = network();
        let roster = roster();
        let request = request();
        let catalog = GroupCatalog::new(
            network_id.clone(),
            1,
            vec![group(
                &network_id,
                1,
                "file-peers",
                GroupDefinition::Manual {
                    members: vec![node(3)],
                },
            )],
        )
        .unwrap();
        let context = catalog.access_context(&roster, roster.get(&node(3)).unwrap());
        assert!(context.groups.contains(&group_id(1)));

        let group_allow = exit_rule(
            1,
            PermissionEffect::Allow,
            SubjectSelector::Group(group_id(1)),
        );
        assert_eq!(
            evaluate_network_permissions(std::slice::from_ref(&group_allow), &request, &context),
            PermissionDecision::Allow
        );

        let any_member_deny = exit_rule(2, PermissionEffect::Deny, SubjectSelector::AnyMember);
        let decision = evaluate_network_permissions(
            &[group_allow.clone(), any_member_deny],
            &request,
            &context,
        );
        assert!(matches!(
            decision,
            PermissionDecision::Deny { ref code, .. } if code == "permission.denied"
        ));

        // Group membership cannot lift a member above the role ceiling.
        let escalation = rule(
            3,
            PermissionEffect::Allow,
            SubjectSelector::Group(group_id(1)),
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
        );
        let request = PermissionRequest {
            action: PolicyAction::Update,
            resource: ResourceRef {
                kind: ResourceKind::SecurityPolicy,
                network_id: network(),
                object_id: None,
            },
        };
        let decision = evaluate_network_permissions(&[escalation], &request, &context);
        assert!(matches!(
            decision,
            PermissionDecision::Deny { ref code, .. } if code == "permission.role_ceiling_exceeded"
        ));
    }

    #[test]
    fn roster_and_selector_inputs_reject_duplicates() {
        let err = MemberRoster::new(vec![
            RosterMember::new(node(1), NetworkRole::Member),
            RosterMember::new(node(1), NetworkRole::Member),
        ])
        .unwrap_err();
        assert_eq!(err.code(), "domain.invariant_violation");

        let err = dynamic_selector(
            vec![NetworkRole::Member, NetworkRole::Member],
            Vec::new(),
            Vec::new(),
        )
        .validate()
        .unwrap_err();
        assert_eq!(err.code(), "domain.invariant_violation");

        let err = GroupDefinition::Nested {
            members: vec![group_id(1), group_id(1)],
        }
        .validate()
        .unwrap_err();
        assert_eq!(err.code(), "domain.invariant_violation");

        let err = GroupDefinition::Nested {
            members: Vec::new(),
        }
        .validate()
        .unwrap_err();
        assert_eq!(err.code(), "domain.invalid_value");
    }

    #[test]
    fn catalog_rejects_groups_from_another_network() {
        let network_id = network();
        let foreign = NetworkGroup {
            group_id: group_id(1),
            network_id: NetworkId::from_public_key(b"other-network"),
            name: "foreign".into(),
            definition: GroupDefinition::Manual {
                members: Vec::new(),
            },
        };
        let err = GroupCatalog::new(network_id, 1, vec![foreign]).unwrap_err();
        assert_eq!(err.code(), "domain.invariant_violation");

        let err = GroupCatalog::new(network(), 0, Vec::new()).unwrap_err();
        assert_eq!(err.code(), "domain.invalid_value");
    }
}
