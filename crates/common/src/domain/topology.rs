//! Single topology model shared by GPUI, WebUI and Android.
//!
//! The topology is derived data: it never owns membership identity and it can
//! always be rebuilt from the authoritative roster plus live session
//! observations. Four invariants are enforced here so every surface renders the
//! same graph:
//!
//! * one node object per `NodeId`, even when the node belongs to many networks;
//! * one connection object per unordered node pair, carrying every network that
//!   observed the link;
//! * only nodes reachable through a network the viewer may see are projected,
//!   and oversized networks are aggregated instead of dumped;
//! * a connection whose observations have gone stale is never reported as an
//!   active link.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::capability::CapabilityKind;
use super::ids::{ContentHash, NetworkId, NodeId};
use super::membership::{
    MembershipRole, NetworkMemberOnlineState, NetworkMemberPresence, NetworkMemberStatus,
};
use super::{DomainError, DomainResult};

/// Above this many members a network is aggregated until the viewer expands it.
pub const MAX_VISIBLE_MEMBERS_PER_NETWORK: usize = 64;
/// A connection observation older than this is stale, never "active".
pub const CONNECTION_STALE_AFTER_MS: u64 = 60_000;
/// Fixed angular slots keep anchors and node positions stable across refreshes.
pub const ANCHOR_SLOTS: u16 = 12;
pub const NODE_SLOTS_PER_RING: u16 = 16;

/// Aggregated node health. `Unknown` is deliberately distinct from `Healthy`
/// so a surface can never claim a healthy node it has not observed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TopologyNodeHealth {
    Healthy,
    Unknown,
    Degraded,
    Unreachable,
}

impl TopologyNodeHealth {
    /// Larger is worse. Aggregation uses the maximum across networks.
    pub fn severity(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Unknown => 1,
            Self::Degraded => 2,
            Self::Unreachable => 3,
        }
    }

    pub fn from_member(status: NetworkMemberStatus, presence: NetworkMemberPresence) -> Self {
        match status {
            NetworkMemberStatus::Degraded => Self::Degraded,
            NetworkMemberStatus::Suspended => Self::Unreachable,
            NetworkMemberStatus::Active => match presence.state {
                NetworkMemberOnlineState::Online => Self::Healthy,
                NetworkMemberOnlineState::Offline => Self::Unreachable,
                NetworkMemberOnlineState::Unknown => Self::Unknown,
            },
            NetworkMemberStatus::Approved
            | NetworkMemberStatus::Pending
            | NetworkMemberStatus::Exited
            | NetworkMemberStatus::Removed
            | NetworkMemberStatus::Expired => Self::Unknown,
        }
    }
}

/// Link state as observed, not as assumed. `Stale` is terminal for rendering
/// purposes: it never counts as an active link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyConnectionState {
    Active,
    Idle,
    Stale,
}

/// Who is asking. Only networks in `visible_networks` contribute node
/// identities to the projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyViewer {
    pub node_id: NodeId,
    pub visible_networks: BTreeSet<NetworkId>,
}

impl TopologyViewer {
    pub fn new(node_id: NodeId, visible_networks: impl IntoIterator<Item = NetworkId>) -> Self {
        Self {
            node_id,
            visible_networks: visible_networks.into_iter().collect(),
        }
    }
}

/// One roster member as the server is allowed to expose it to this viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyMemberInput {
    pub node_id: NodeId,
    pub display_name: Option<String>,
    pub role: MembershipRole,
    pub status: NetworkMemberStatus,
    pub presence: NetworkMemberPresence,
    pub capabilities: Vec<CapabilityKind>,
}

impl TopologyMemberInput {
    pub fn new(node_id: NodeId, role: MembershipRole) -> Self {
        Self {
            node_id,
            display_name: None,
            role,
            status: NetworkMemberStatus::Approved,
            presence: NetworkMemberPresence::unknown(),
            capabilities: Vec::new(),
        }
    }

    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    pub fn with_status(mut self, status: NetworkMemberStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_presence(mut self, presence: NetworkMemberPresence) -> Self {
        self.presence = presence;
        self
    }

    pub fn with_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = CapabilityKind>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().collect();
        self
    }

    /// Pending, exited, removed and expired identities are not part of the
    /// network graph and are therefore never projected.
    fn is_graph_member(&self) -> bool {
        matches!(
            self.status,
            NetworkMemberStatus::Approved
                | NetworkMemberStatus::Active
                | NetworkMemberStatus::Degraded
                | NetworkMemberStatus::Suspended
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyNetworkInput {
    pub network_id: NetworkId,
    pub name: String,
    pub members: Vec<TopologyMemberInput>,
}

impl TopologyNetworkInput {
    pub fn new(network_id: NetworkId, name: impl Into<String>) -> Self {
        Self {
            network_id,
            name: name.into(),
            members: Vec::new(),
        }
    }

    pub fn with_member(mut self, member: TopologyMemberInput) -> Self {
        self.members.push(member);
        self
    }

    pub fn with_members(mut self, members: impl IntoIterator<Item = TopologyMemberInput>) -> Self {
        self.members.extend(members);
        self
    }
}

/// A single observation of a data-plane link. Observations are deduplicated by
/// unordered node pair at build time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyLinkObservation {
    pub from: NodeId,
    pub to: NodeId,
    pub network_id: NetworkId,
    pub observed_at_unix_ms: u64,
    pub active: bool,
}

impl TopologyLinkObservation {
    pub fn new(
        from: NodeId,
        to: NodeId,
        network_id: NetworkId,
        observed_at_unix_ms: u64,
        active: bool,
    ) -> Self {
        Self {
            from,
            to,
            network_id,
            observed_at_unix_ms,
            active,
        }
    }
}

/// Accumulates authoritative roster data plus live observations and projects
/// them into a privacy-filtered, deterministic snapshot.
#[derive(Debug, Clone)]
pub struct TopologyBuilder {
    viewer: TopologyViewer,
    revision: u64,
    generated_at_unix_ms: u64,
    max_visible_members_per_network: usize,
    expanded_networks: BTreeSet<NetworkId>,
    networks: BTreeMap<NetworkId, TopologyNetworkInput>,
    links: Vec<TopologyLinkObservation>,
}

impl TopologyBuilder {
    pub fn new(viewer: TopologyViewer, revision: u64, generated_at_unix_ms: u64) -> Self {
        Self {
            viewer,
            revision,
            generated_at_unix_ms,
            max_visible_members_per_network: MAX_VISIBLE_MEMBERS_PER_NETWORK,
            expanded_networks: BTreeSet::new(),
            networks: BTreeMap::new(),
            links: Vec::new(),
        }
    }

    pub fn with_visible_member_limit(mut self, limit: usize) -> Self {
        self.max_visible_members_per_network = limit.max(1);
        self
    }

    /// On-demand expansion: a network listed here is never aggregated.
    pub fn expand_network(mut self, network_id: NetworkId) -> Self {
        self.expanded_networks.insert(network_id);
        self
    }

    pub fn add_network(mut self, network: TopologyNetworkInput) -> Self {
        self.networks.insert(network.network_id.clone(), network);
        self
    }

    pub fn observe_link(mut self, observation: TopologyLinkObservation) -> Self {
        self.links.push(observation);
        self
    }

    pub fn viewer(&self) -> &TopologyViewer {
        &self.viewer
    }

    pub fn build(self) -> DomainResult<TopologySnapshot> {
        let mut boundaries = Vec::new();
        let mut clusters = Vec::new();
        // `node_id -> (display name, per-network entries)`
        let mut nodes: BTreeMap<NodeId, (Option<String>, Vec<TopologyNodeNetwork>)> =
            BTreeMap::new();
        let mut visible_networks = BTreeSet::new();

        for network in self.networks.values() {
            if !self.viewer.visible_networks.contains(&network.network_id) {
                // Fail closed: an invisible network contributes neither
                // identities nor counts.
                continue;
            }
            visible_networks.insert(network.network_id.clone());
            let mut members: Vec<&TopologyMemberInput> = network
                .members
                .iter()
                .filter(|member| member.is_graph_member())
                .collect();
            members.sort_by(|left, right| left.node_id.cmp(&right.node_id));
            if members
                .windows(2)
                .any(|pair| pair[0].node_id == pair[1].node_id)
            {
                return Err(DomainError::invariant(
                    "topology network contains duplicate members",
                ));
            }
            let total = members.len();
            let show_all = total <= self.max_visible_members_per_network
                || self.expanded_networks.contains(&network.network_id);
            let visible: Vec<&TopologyMemberInput> = if show_all {
                members
            } else {
                let (shown, hidden) = members.split_at(self.max_visible_members_per_network);
                clusters.push(TopologyCluster {
                    network_id: network.network_id.clone(),
                    hidden_member_count: hidden.len() as u32,
                    sample: hidden
                        .iter()
                        .take(3)
                        .map(|member| member.node_id.clone())
                        .collect(),
                    expandable: true,
                });
                shown.to_vec()
            };
            let mut capabilities = BTreeSet::new();
            let mut viewer_role = None;
            for member in &visible {
                capabilities.extend(member.capabilities.iter().copied());
                if member.node_id == self.viewer.node_id {
                    viewer_role = Some(member.role);
                }
            }
            boundaries.push(TopologyBoundary {
                network_id: network.network_id.clone(),
                name: network.name.clone(),
                member_count: total as u32,
                visible_member_count: visible.len() as u32,
                viewer_role,
                capabilities: capabilities.into_iter().collect(),
            });
            for member in visible {
                let entry = nodes
                    .entry(member.node_id.clone())
                    .or_insert_with(|| (None, Vec::new()));
                if entry.0.is_none() {
                    entry.0 = member
                        .display_name
                        .clone()
                        .filter(|name| !name.trim().is_empty());
                }
                entry.1.push(TopologyNodeNetwork {
                    network_id: network.network_id.clone(),
                    role: member.role,
                    status: member.status,
                    presence: member.presence.clone(),
                    health: TopologyNodeHealth::from_member(member.status, member.presence.clone()),
                    capabilities: sorted_capabilities(&member.capabilities),
                });
            }
        }

        let mut projected = Vec::with_capacity(nodes.len());
        for (node_id, (display_name, mut entries)) in nodes {
            entries.sort_by(|left, right| left.network_id.cmp(&right.network_id));
            let mut capabilities = BTreeSet::new();
            for entry in &entries {
                capabilities.extend(entry.capabilities.iter().copied());
            }
            let health = entries
                .iter()
                .map(|entry| entry.health)
                .max_by_key(|health| health.severity())
                .unwrap_or(TopologyNodeHealth::Unknown);
            projected.push(TopologyNode {
                is_self: node_id == self.viewer.node_id,
                node_id,
                display_name,
                capabilities: capabilities.into_iter().collect(),
                health,
                networks: entries,
            });
        }

        let mut connections = BTreeMap::<ContentHash, TopologyConnection>::new();
        for observation in &self.links {
            if !visible_networks.contains(&observation.network_id) {
                continue;
            }
            let (from, to) = canonical_pair(&observation.from, &observation.to);
            if !projected.iter().any(|node| node.node_id == *from)
                || !projected.iter().any(|node| node.node_id == *to)
            {
                // Privacy: a link to an unprojected node is not rendered.
                continue;
            }
            let connection_id = connection_id(from, to);
            let fresh = self
                .generated_at_unix_ms
                .saturating_sub(observation.observed_at_unix_ms)
                <= CONNECTION_STALE_AFTER_MS;
            let entry =
                connections
                    .entry(connection_id.clone())
                    .or_insert_with(|| TopologyConnection {
                        connection_id,
                        from: from.clone(),
                        to: to.clone(),
                        networks: Vec::new(),
                        state: TopologyConnectionState::Stale,
                        observed_at_unix_ms: None,
                    });
            if !entry.networks.contains(&observation.network_id) {
                entry.networks.push(observation.network_id.clone());
                entry.networks.sort();
            }
            entry.observed_at_unix_ms = Some(
                entry
                    .observed_at_unix_ms
                    .map_or(observation.observed_at_unix_ms, |current| {
                        current.max(observation.observed_at_unix_ms)
                    }),
            );
            let next = if fresh && observation.active {
                TopologyConnectionState::Active
            } else if fresh {
                TopologyConnectionState::Idle
            } else {
                TopologyConnectionState::Stale
            };
            entry.state = match (entry.state, next) {
                (TopologyConnectionState::Active, _) | (_, TopologyConnectionState::Active) => {
                    TopologyConnectionState::Active
                }
                (TopologyConnectionState::Idle, _) | (_, TopologyConnectionState::Idle) => {
                    TopologyConnectionState::Idle
                }
                _ => TopologyConnectionState::Stale,
            };
        }

        Ok(TopologySnapshot {
            revision: self.revision,
            generated_at_unix_ms: self.generated_at_unix_ms,
            viewer: self.viewer.node_id,
            truncated: !clusters.is_empty(),
            boundaries,
            nodes: projected,
            connections: connections.into_values().collect(),
            clusters,
        })
    }
}

fn canonical_pair<'a>(left: &'a NodeId, right: &'a NodeId) -> (&'a NodeId, &'a NodeId) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn connection_id(from: &NodeId, to: &NodeId) -> ContentHash {
    ContentHash::from_bytes(format!("{from}|{to}").as_bytes())
}

fn sorted_capabilities(capabilities: &[CapabilityKind]) -> Vec<CapabilityKind> {
    capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// One network boundary. `member_count` is the authoritative count, while
/// `visible_member_count` reflects aggregation so a surface can explain it.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyBoundary {
    pub network_id: NetworkId,
    pub name: String,
    pub member_count: u32,
    pub visible_member_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_role: Option<MembershipRole>,
    #[serde(default)]
    pub capabilities: Vec<CapabilityKind>,
}

/// The node's relationship with one visible network.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyNodeNetwork {
    pub network_id: NetworkId,
    pub role: MembershipRole,
    pub status: NetworkMemberStatus,
    pub presence: NetworkMemberPresence,
    pub health: TopologyNodeHealth,
    #[serde(default)]
    pub capabilities: Vec<CapabilityKind>,
}

/// Exactly one entry per `NodeId`, regardless of how many networks it joins.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyNode {
    pub node_id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub is_self: bool,
    pub health: TopologyNodeHealth,
    #[serde(default)]
    pub capabilities: Vec<CapabilityKind>,
    /// Sorted by network id so both UIs render the same order.
    pub networks: Vec<TopologyNodeNetwork>,
}

impl TopologyNode {
    pub fn belongs_to(&self, network_id: &NetworkId) -> bool {
        self.networks
            .iter()
            .any(|network| &network.network_id == network_id)
    }

    pub fn role_in(&self, network_id: &NetworkId) -> Option<MembershipRole> {
        self.networks
            .iter()
            .find(|network| &network.network_id == network_id)
            .map(|network| network.role)
    }
}

/// One unordered node pair, with every network that observed it.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyConnection {
    pub connection_id: ContentHash,
    pub from: NodeId,
    pub to: NodeId,
    #[serde(default)]
    pub networks: Vec<NetworkId>,
    pub state: TopologyConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_unix_ms: Option<u64>,
}

/// Aggregated remainder of an oversized network.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyCluster {
    pub network_id: NetworkId,
    pub hidden_member_count: u32,
    #[serde(default)]
    pub sample: Vec<NodeId>,
    pub expandable: bool,
}

/// Server-owned, privacy-filtered topology projection.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologySnapshot {
    pub revision: u64,
    pub generated_at_unix_ms: u64,
    pub viewer: NodeId,
    pub truncated: bool,
    pub boundaries: Vec<TopologyBoundary>,
    pub nodes: Vec<TopologyNode>,
    pub connections: Vec<TopologyConnection>,
    #[serde(default)]
    pub clusters: Vec<TopologyCluster>,
}

impl TopologySnapshot {
    pub fn node(&self, node_id: &NodeId) -> Option<&TopologyNode> {
        self.nodes.iter().find(|node| &node.node_id == node_id)
    }

    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|node| node.node_id.clone()).collect()
    }

    pub fn boundary(&self, network_id: &NetworkId) -> Option<&TopologyBoundary> {
        self.boundaries
            .iter()
            .find(|boundary| &boundary.network_id == network_id)
    }

    /// Only links that are currently active. Stale and idle links must never be
    /// drawn as live paths.
    pub fn active_connections(&self) -> Vec<&TopologyConnection> {
        self.connections
            .iter()
            .filter(|connection| connection.state == TopologyConnectionState::Active)
            .collect()
    }

    /// Deterministic layout for the current snapshot.
    pub fn layout(&self, viewport: TopologyViewport) -> TopologyLayout {
        let mut anchors = Vec::new();
        for (position, boundary) in self.boundaries.iter().enumerate() {
            let slot = position as u16;
            let (x, y) = anchor_position(viewport, slot);
            anchors.push(TopologyAnchor {
                network_id: boundary.network_id.clone(),
                slot,
                x,
                y,
            });
        }
        let mut next_slot: BTreeMap<NetworkId, u16> = BTreeMap::new();
        let mut placements = Vec::new();
        for node in &self.nodes {
            let Some(anchor) = primary_anchor(&anchors, node) else {
                continue;
            };
            let slot = next_slot.entry(anchor.network_id.clone()).or_insert(0);
            let placement_slot = *slot;
            *slot += 1;
            let (x, y) = node_position(Some(anchor), placement_slot);
            placements.push(TopologyPlacement {
                node_id: node.node_id.clone(),
                anchor_network_id: anchor.network_id.clone(),
                slot: placement_slot,
                x,
                y,
            });
        }
        TopologyLayout {
            viewport,
            anchors,
            placements,
        }
    }

    /// Incremental layout: retained nodes keep their anchor slot and node slot,
    /// so a refresh never moves or duplicates an existing object. Only new
    /// nodes take a free slot and only new boundaries take a free anchor slot.
    pub fn layout_preserving(
        &self,
        previous: &TopologyLayout,
        viewport: TopologyViewport,
    ) -> TopologyLayout {
        let mut anchors = Vec::new();
        let mut used_anchor_slots = BTreeSet::new();
        for previous_anchor in &previous.anchors {
            if self.boundary(&previous_anchor.network_id).is_some() {
                used_anchor_slots.insert(previous_anchor.slot);
                anchors.push(TopologyAnchor {
                    network_id: previous_anchor.network_id.clone(),
                    slot: previous_anchor.slot,
                    ..anchor_positioned(viewport, previous_anchor.slot, &previous_anchor.network_id)
                });
            }
        }
        for boundary in &self.boundaries {
            if anchors
                .iter()
                .any(|anchor| anchor.network_id == boundary.network_id)
            {
                continue;
            }
            let slot = (0..=u16::MAX)
                .find(|slot| !used_anchor_slots.contains(slot))
                .unwrap_or(0);
            used_anchor_slots.insert(slot);
            anchors.push(anchor_positioned(viewport, slot, &boundary.network_id));
        }
        anchors.sort_by(|left, right| left.network_id.cmp(&right.network_id));

        let mut used_slots: BTreeMap<NetworkId, BTreeSet<u16>> = BTreeMap::new();
        let mut placements = Vec::new();
        for node in &self.nodes {
            let previous_placement = previous
                .placements
                .iter()
                .find(|placement| placement.node_id == node.node_id);
            let anchor_network_id = match previous_placement {
                Some(placement) if self.boundary(&placement.anchor_network_id).is_some() => {
                    placement.anchor_network_id.clone()
                }
                _ => match primary_anchor(&anchors, node) {
                    Some(anchor) => anchor.network_id.clone(),
                    None => continue,
                },
            };
            let slot = match previous_placement {
                Some(placement)
                    if placement.anchor_network_id == anchor_network_id
                        && !used_slots
                            .get(&anchor_network_id)
                            .is_some_and(|slots| slots.contains(&placement.slot)) =>
                {
                    placement.slot
                }
                _ => {
                    let slots = used_slots.entry(anchor_network_id.clone()).or_default();
                    (0..=u16::MAX)
                        .find(|candidate| !slots.contains(candidate))
                        .unwrap_or(0)
                }
            };
            used_slots
                .entry(anchor_network_id.clone())
                .or_default()
                .insert(slot);
            let anchor = anchors
                .iter()
                .find(|anchor| anchor.network_id == anchor_network_id);
            let (x, y) = match anchor {
                Some(anchor) => node_position(Some(anchor), slot),
                None => (
                    (viewport.width / 2.0).round() as i32,
                    (viewport.height / 2.0).round() as i32,
                ),
            };
            placements.push(TopologyPlacement {
                node_id: node.node_id.clone(),
                anchor_network_id,
                slot,
                x,
                y,
            });
        }
        TopologyLayout {
            viewport,
            anchors,
            placements,
        }
    }

    /// Selection survives a refresh only while the node still exists.
    pub fn retain_selection(&self, selected: Option<&NodeId>) -> Option<NodeId> {
        selected
            .filter(|node_id| self.node(node_id).is_some())
            .cloned()
    }

    /// The same filter vocabulary as the member list, applied to the projected
    /// graph so a search result is identical in both surfaces.
    pub fn query(&self, query: &TopologyQuery) -> Vec<&TopologyNode> {
        let needle = query.search.trim().to_lowercase();
        self.nodes
            .iter()
            .filter(|node| {
                if !query.roles.is_empty()
                    && !node
                        .networks
                        .iter()
                        .any(|network| query.roles.contains(&network.role))
                {
                    return false;
                }
                if !query.capabilities.is_empty()
                    && !query
                        .capabilities
                        .iter()
                        .any(|capability| node.capabilities.contains(capability))
                {
                    return false;
                }
                if !query.health.is_empty() && !query.health.contains(&node.health) {
                    return false;
                }
                if let Some(network_id) = &query.network_id {
                    if !node.belongs_to(network_id) {
                        return false;
                    }
                }
                if needle.is_empty() {
                    return true;
                }
                node.display_name
                    .as_deref()
                    .map(|name| name.to_lowercase().contains(&needle))
                    .unwrap_or(false)
                    || node.node_id.as_str().to_lowercase().starts_with(&needle)
            })
            .collect()
    }

    /// Incremental diff between two revisions of the same projection.
    pub fn diff(&self, previous: &TopologySnapshot) -> TopologyDiff {
        let previous_nodes: BTreeMap<&NodeId, &TopologyNode> = previous
            .nodes
            .iter()
            .map(|node| (&node.node_id, node))
            .collect();
        let current_nodes: BTreeMap<&NodeId, &TopologyNode> = self
            .nodes
            .iter()
            .map(|node| (&node.node_id, node))
            .collect();
        let mut diff = TopologyDiff {
            base_revision: previous.revision,
            revision: self.revision,
            added_nodes: current_nodes
                .keys()
                .filter(|node| !previous_nodes.contains_key(**node))
                .map(|node| (*node).clone())
                .collect(),
            updated_nodes: current_nodes
                .iter()
                .filter(|(node_id, node)| {
                    previous_nodes
                        .get(*node_id)
                        .is_some_and(|previous| previous != *node)
                })
                .map(|(node_id, _)| (*node_id).clone())
                .collect(),
            removed_nodes: previous_nodes
                .keys()
                .filter(|node| !current_nodes.contains_key(**node))
                .map(|node| (*node).clone())
                .collect(),
            added_connections: Vec::new(),
            updated_connections: Vec::new(),
            removed_connections: Vec::new(),
            added_boundaries: Vec::new(),
            removed_boundaries: Vec::new(),
            unchanged_node_count: 0,
        };
        diff.unchanged_node_count = current_nodes
            .len()
            .saturating_sub(diff.added_nodes.len() + diff.updated_nodes.len())
            as u32;
        let previous_connections: BTreeMap<&ContentHash, &TopologyConnection> = previous
            .connections
            .iter()
            .map(|connection| (&connection.connection_id, connection))
            .collect();
        let current_connections: BTreeMap<&ContentHash, &TopologyConnection> = self
            .connections
            .iter()
            .map(|connection| (&connection.connection_id, connection))
            .collect();
        for (connection_id, connection) in &current_connections {
            match previous_connections.get(*connection_id) {
                None => diff.added_connections.push((*connection_id).clone()),
                Some(previous) if *previous != *connection => {
                    diff.updated_connections.push((*connection_id).clone())
                }
                Some(_) => {}
            }
        }
        for connection_id in previous_connections.keys() {
            if !current_connections.contains_key(*connection_id) {
                diff.removed_connections.push((*connection_id).clone());
            }
        }
        let previous_boundaries: BTreeSet<&NetworkId> = previous
            .boundaries
            .iter()
            .map(|boundary| &boundary.network_id)
            .collect();
        let current_boundaries: BTreeSet<&NetworkId> = self
            .boundaries
            .iter()
            .map(|boundary| &boundary.network_id)
            .collect();
        diff.added_boundaries = current_boundaries
            .difference(&previous_boundaries)
            .map(|network| (*network).clone())
            .collect();
        diff.removed_boundaries = previous_boundaries
            .difference(&current_boundaries)
            .map(|network| (*network).clone())
            .collect();
        diff
    }
}

#[derive(Debug, Clone, Copy, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyViewport {
    pub width: f32,
    pub height: f32,
}

impl TopologyViewport {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyAnchor {
    pub network_id: NetworkId,
    pub slot: u16,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyPlacement {
    pub node_id: NodeId,
    pub anchor_network_id: NetworkId,
    pub slot: u16,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyLayout {
    pub viewport: TopologyViewport,
    pub anchors: Vec<TopologyAnchor>,
    pub placements: Vec<TopologyPlacement>,
}

impl TopologyLayout {
    pub fn placement(&self, node_id: &NodeId) -> Option<&TopologyPlacement> {
        self.placements
            .iter()
            .find(|placement| &placement.node_id == node_id)
    }
}

fn anchor_position(viewport: TopologyViewport, slot: u16) -> (i32, i32) {
    let ring = (slot / ANCHOR_SLOTS) as f32;
    let index = (slot % ANCHOR_SLOTS) as f32;
    let radius_x = viewport.width * (0.34 + ring * 0.16);
    let radius_y = viewport.height * (0.32 + ring * 0.16);
    let angle = std::f32::consts::TAU * index / ANCHOR_SLOTS as f32 - std::f32::consts::FRAC_PI_2;
    (
        (viewport.width / 2.0 + radius_x * angle.cos()).round() as i32,
        (viewport.height / 2.0 + radius_y * angle.sin()).round() as i32,
    )
}

fn anchor_positioned(
    viewport: TopologyViewport,
    slot: u16,
    network_id: &NetworkId,
) -> TopologyAnchor {
    let (x, y) = anchor_position(viewport, slot);
    TopologyAnchor {
        network_id: network_id.clone(),
        slot,
        x,
        y,
    }
}

fn anchors_slot<'a>(
    anchors: &'a [TopologyAnchor],
    network_id: &NetworkId,
) -> Option<&'a TopologyAnchor> {
    anchors
        .iter()
        .find(|anchor| &anchor.network_id == network_id)
}

/// Nodes are grouped by their first (sorted) network so a multi-network node is
/// still drawn exactly once.
fn primary_anchor<'a>(
    anchors: &'a [TopologyAnchor],
    node: &TopologyNode,
) -> Option<&'a TopologyAnchor> {
    for network in &node.networks {
        if let Some(anchor) = anchors_slot(anchors, &network.network_id) {
            return Some(anchor);
        }
    }
    None
}

fn node_position(anchor: Option<&TopologyAnchor>, slot: u16) -> (i32, i32) {
    let (base_x, base_y) = anchor.map_or((0, 0), |anchor| (anchor.x, anchor.y));
    let ring = (slot / NODE_SLOTS_PER_RING) as f32;
    let index = (slot % NODE_SLOTS_PER_RING) as f32;
    let radius = 56.0 + ring * 30.0;
    let angle =
        std::f32::consts::TAU * index / NODE_SLOTS_PER_RING as f32 - std::f32::consts::FRAC_PI_2;
    (
        base_x + (radius * angle.cos()).round() as i32,
        base_y + (radius * angle.sin()).round() as i32,
    )
}

/// Shared query vocabulary for the topology and the member list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopologyQuery {
    pub search: String,
    pub roles: Vec<MembershipRole>,
    pub capabilities: Vec<CapabilityKind>,
    pub health: Vec<TopologyNodeHealth>,
    pub network_id: Option<NetworkId>,
}

impl TopologyQuery {
    pub fn search(mut self, search: impl Into<String>) -> Self {
        self.search = search.into();
        self
    }

    pub fn with_network(mut self, network_id: NetworkId) -> Self {
        self.network_id = Some(network_id);
        self
    }

    pub fn with_roles(mut self, roles: impl IntoIterator<Item = MembershipRole>) -> Self {
        self.roles = roles.into_iter().collect();
        self
    }

    pub fn with_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = CapabilityKind>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().collect();
        self
    }

    pub fn with_health(mut self, health: impl IntoIterator<Item = TopologyNodeHealth>) -> Self {
        self.health = health.into_iter().collect();
        self
    }
}

/// Incremental difference between two projected revisions.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TopologyDiff {
    pub base_revision: u64,
    pub revision: u64,
    pub added_nodes: Vec<NodeId>,
    pub updated_nodes: Vec<NodeId>,
    pub removed_nodes: Vec<NodeId>,
    pub added_connections: Vec<ContentHash>,
    pub updated_connections: Vec<ContentHash>,
    pub removed_connections: Vec<ContentHash>,
    pub added_boundaries: Vec<NetworkId>,
    pub removed_boundaries: Vec<NetworkId>,
    pub unchanged_node_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;

    fn network(seed: u8) -> NetworkId {
        NetworkId::from_public_key(&[seed; 32])
    }

    fn node(seed: u8) -> NodeId {
        NodeId::from_public_key(&[seed; 32])
    }

    fn member(seed: u8, role: MembershipRole) -> TopologyMemberInput {
        TopologyMemberInput::new(node(seed), role)
    }

    fn active(seed: u8, role: MembershipRole, observed_at: u64) -> TopologyMemberInput {
        member(seed, role)
            .with_status(NetworkMemberStatus::Active)
            .with_presence(NetworkMemberPresence::live(observed_at))
    }

    fn builder(visible: &[NetworkId]) -> TopologyBuilder {
        TopologyBuilder::new(TopologyViewer::new(node(1), visible.to_vec()), 1, NOW)
    }

    #[test]
    fn nodes_are_aggregated_across_networks_exactly_once() {
        let (a, b) = (network(1), network(2));
        let snapshot = builder(&[a.clone(), b.clone()])
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(
                        active(1, MembershipRole::Owner, NOW)
                            .with_display_name("alice")
                            .with_capabilities([CapabilityKind::Coordinator]),
                    )
                    .with_member(active(2, MembershipRole::Member, NOW)),
            )
            .add_network(
                TopologyNetworkInput::new(b.clone(), "beta").with_member(
                    member(1, MembershipRole::Member)
                        .with_status(NetworkMemberStatus::Active)
                        .with_presence(NetworkMemberPresence::disconnected(NOW)),
                ),
            )
            .build()
            .unwrap();

        assert_eq!(snapshot.nodes.len(), 2);
        let alice = snapshot.node(&node(1)).unwrap();
        assert!(alice.is_self);
        assert_eq!(alice.display_name.as_deref(), Some("alice"));
        assert_eq!(alice.networks.len(), 2);
        let networks: BTreeSet<NetworkId> = alice
            .networks
            .iter()
            .map(|network| network.network_id.clone())
            .collect();
        assert_eq!(networks, BTreeSet::from([a.clone(), b.clone()]));
        assert_eq!(alice.role_in(&a), Some(MembershipRole::Owner));
        assert_eq!(alice.role_in(&b), Some(MembershipRole::Member));
        assert_eq!(alice.capabilities, vec![CapabilityKind::Coordinator]);
        // Worst health across networks wins, so a healthy link in one network
        // cannot hide an unreachable one in another.
        assert_eq!(alice.health, TopologyNodeHealth::Unreachable);
        assert!(alice.belongs_to(&b));
        assert_eq!(alice.role_in(&b), Some(MembershipRole::Member));

        let ids = snapshot.node_ids();
        let unique: BTreeSet<&NodeId> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
        assert_eq!(snapshot.boundaries.len(), 2);
        let alpha = snapshot.boundary(&a).unwrap();
        assert_eq!(alpha.member_count, 2);
        assert_eq!(alpha.visible_member_count, 2);
        assert_eq!(alpha.viewer_role, Some(MembershipRole::Owner));
        let beta = snapshot.boundary(&b).unwrap();
        assert_eq!(beta.member_count, 1);
        assert_eq!(beta.viewer_role, Some(MembershipRole::Member));
    }

    #[test]
    fn privacy_projection_omits_invisible_networks_and_their_nodes() {
        let (a, b) = (network(1), network(2));
        let snapshot = TopologyBuilder::new(TopologyViewer::new(node(2), [a.clone()]), 1, NOW)
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(active(1, MembershipRole::Owner, NOW))
                    .with_member(active(2, MembershipRole::Member, NOW)),
            )
            .add_network(
                TopologyNetworkInput::new(b.clone(), "beta").with_member(active(
                    3,
                    MembershipRole::Owner,
                    NOW,
                )),
            )
            .observe_link(TopologyLinkObservation::new(
                node(1),
                node(3),
                b.clone(),
                NOW,
                true,
            ))
            .build()
            .unwrap();

        assert_eq!(snapshot.boundaries.len(), 1);
        assert_eq!(snapshot.boundaries[0].network_id, a);
        assert_eq!(
            snapshot.boundaries[0].viewer_role,
            Some(MembershipRole::Member)
        );
        assert!(snapshot.node(&node(3)).is_none());
        assert!(snapshot.connections.is_empty());
        assert!(!snapshot
            .node_ids()
            .iter()
            .any(|node_id| node_id == &node(3)));

        // A viewer with no visible network sees nothing at all.
        let empty = TopologyBuilder::new(TopologyViewer::new(node(1), []), 1, NOW)
            .add_network(TopologyNetworkInput::new(a, "alpha").with_member(active(
                1,
                MembershipRole::Owner,
                NOW,
            )))
            .build()
            .unwrap();
        assert!(empty.nodes.is_empty());
        assert!(empty.boundaries.is_empty());
    }

    #[test]
    fn connections_are_deduplicated_and_keep_network_relations() {
        let (a, b) = (network(1), network(2));
        let snapshot = builder(&[a.clone(), b.clone()])
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(active(1, MembershipRole::Owner, NOW))
                    .with_member(active(2, MembershipRole::Member, NOW)),
            )
            .add_network(
                TopologyNetworkInput::new(b.clone(), "beta").with_member(active(
                    2,
                    MembershipRole::Member,
                    NOW,
                )),
            )
            .observe_link(TopologyLinkObservation::new(
                node(1),
                node(2),
                a.clone(),
                NOW,
                true,
            ))
            .observe_link(TopologyLinkObservation::new(
                node(2),
                node(1),
                a.clone(),
                NOW,
                true,
            ))
            .observe_link(TopologyLinkObservation::new(
                node(1),
                node(2),
                b.clone(),
                NOW,
                true,
            ))
            .build()
            .unwrap();

        assert_eq!(snapshot.connections.len(), 1);
        let connection = &snapshot.connections[0];
        assert!(pair_matches(connection, node(1), node(2)));
        assert_eq!(
            connection.networks.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([a.clone(), b])
        );
        assert_eq!(connection.state, TopologyConnectionState::Active);
        assert_eq!(connection.observed_at_unix_ms, Some(NOW));
        // The identifier is derived from the unordered pair, so observation
        // order and direction never create a second object.
        let first = node(1);
        let second = node(2);
        let (from, to) = canonical_pair(&first, &second);
        assert_eq!(connection.connection_id, connection_id(from, to));
        assert_eq!(snapshot.active_connections().len(), 1);
    }

    #[test]
    fn stale_observations_are_never_reported_as_active() {
        let a = network(1);
        let snapshot = builder(std::slice::from_ref(&a))
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(active(1, MembershipRole::Owner, NOW))
                    .with_member(active(2, MembershipRole::Member, NOW))
                    .with_member(active(3, MembershipRole::Member, NOW)),
            )
            .observe_link(TopologyLinkObservation::new(
                node(1),
                node(2),
                a.clone(),
                NOW - CONNECTION_STALE_AFTER_MS - 1,
                true,
            ))
            .observe_link(TopologyLinkObservation::new(
                node(1),
                node(3),
                a.clone(),
                NOW,
                false,
            ))
            .build()
            .unwrap();

        let stale = snapshot
            .connections
            .iter()
            .find(|connection| pair_matches(connection, node(1), node(2)))
            .unwrap();
        assert_eq!(stale.state, TopologyConnectionState::Stale);
        let idle = snapshot
            .connections
            .iter()
            .find(|connection| pair_matches(connection, node(1), node(3)))
            .unwrap();
        assert_eq!(idle.state, TopologyConnectionState::Idle);
        assert!(snapshot.active_connections().is_empty());
    }

    #[test]
    fn layout_is_deterministic_and_preserves_object_identity() {
        let a = network(1);
        let viewport = TopologyViewport::new(720.0, 420.0);
        let first = builder(std::slice::from_ref(&a))
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(active(1, MembershipRole::Owner, NOW))
                    .with_member(active(2, MembershipRole::Member, NOW)),
            )
            .build()
            .unwrap();
        let layout = first.layout(viewport);
        assert_eq!(layout, first.layout(viewport));
        assert_eq!(layout.placements.len(), 2);
        let mut slots = BTreeSet::new();
        for placement in &layout.placements {
            assert!(slots.insert((placement.anchor_network_id.clone(), placement.slot)));
            assert_eq!(placement.anchor_network_id, a);
        }

        let refresh =
            TopologyBuilder::new(TopologyViewer::new(node(1), [a.clone()]), 2, NOW + 1_000)
                .add_network(
                    TopologyNetworkInput::new(a.clone(), "alpha")
                        .with_member(
                            active(1, MembershipRole::Owner, NOW + 1_000)
                                .with_display_name("renamed"),
                        )
                        .with_member(active(4, MembershipRole::Member, NOW + 1_000)),
                )
                .build()
                .unwrap();
        let refreshed = refresh.layout_preserving(&layout, viewport);
        // Retained node keeps its exact anchor, slot and pixel position.
        assert_eq!(refreshed.placement(&node(1)), layout.placement(&node(1)));
        // Removed node disappears and the new node takes a free slot.
        assert!(refreshed.placement(&node(2)).is_none());
        let added = refreshed.placement(&node(4)).unwrap();
        assert_ne!(added.slot, layout.placement(&node(1)).unwrap().slot);
        assert!(refreshed.placements.len() == 2);

        // Selection survives only while the node still exists.
        assert_eq!(refreshed_retain(&refresh, &layout), Some(node(1)));
        assert_eq!(refresh.retain_selection(Some(&node(2))), None);
    }

    fn refreshed_retain(snapshot: &TopologySnapshot, _previous: &TopologyLayout) -> Option<NodeId> {
        snapshot.retain_selection(Some(&node(1)))
    }

    fn pair_matches(connection: &TopologyConnection, left: NodeId, right: NodeId) -> bool {
        (connection.from == left && connection.to == right)
            || (connection.from == right && connection.to == left)
    }

    #[test]
    fn large_networks_are_aggregated_and_expandable() {
        let a = network(1);
        let network_input = TopologyNetworkInput::new(a.clone(), "alpha")
            .with_members((1..=5).map(|seed| active(seed, MembershipRole::Member, NOW)));
        let aggregated = builder(std::slice::from_ref(&a))
            .with_visible_member_limit(2)
            .add_network(network_input.clone())
            .build()
            .unwrap();
        assert!(aggregated.truncated);
        assert_eq!(aggregated.boundaries[0].member_count, 5);
        assert_eq!(aggregated.boundaries[0].visible_member_count, 2);
        assert_eq!(aggregated.nodes.len(), 2);
        assert_eq!(aggregated.clusters.len(), 1);
        let cluster = &aggregated.clusters[0];
        assert_eq!(cluster.network_id, a);
        assert_eq!(cluster.hidden_member_count, 3);
        assert_eq!(cluster.sample.len(), 3);
        assert!(cluster.expandable);

        let expanded = builder(std::slice::from_ref(&a))
            .with_visible_member_limit(2)
            .expand_network(a)
            .add_network(network_input)
            .build()
            .unwrap();
        assert!(!expanded.truncated);
        assert_eq!(expanded.nodes.len(), 5);
        assert!(expanded.clusters.is_empty());
        assert_eq!(expanded.boundaries[0].visible_member_count, 5);
    }

    #[test]
    fn topology_and_list_share_search_filter_and_selection() {
        let (a, b) = (network(1), network(2));
        let snapshot = builder(&[a.clone(), b.clone()])
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(
                        active(1, MembershipRole::Owner, NOW)
                            .with_display_name("Alice")
                            .with_capabilities([CapabilityKind::Coordinator]),
                    )
                    .with_member(
                        active(2, MembershipRole::Member, NOW)
                            .with_display_name("Bob")
                            .with_capabilities([CapabilityKind::File]),
                    )
                    .with_member(member(3, MembershipRole::Member)),
            )
            .add_network(TopologyNetworkInput::new(b.clone(), "beta").with_member(
                member(4, MembershipRole::Member).with_status(NetworkMemberStatus::Suspended),
            ))
            .build()
            .unwrap();

        let all = snapshot.query(&TopologyQuery::default());
        assert_eq!(
            all.iter()
                .map(|node| node.node_id.clone())
                .collect::<Vec<_>>(),
            snapshot.node_ids()
        );
        let alice = snapshot.query(&TopologyQuery::default().search("ali"));
        assert_eq!(alice.len(), 1);
        assert_eq!(alice[0].node_id, node(1));
        assert_eq!(
            snapshot.query(&TopologyQuery::default().search(node(3).as_str()))[0].node_id,
            node(3)
        );
        assert!(snapshot
            .query(&TopologyQuery::default().search("nobody"))
            .is_empty());
        assert_eq!(
            snapshot
                .query(&TopologyQuery::default().with_roles([MembershipRole::Owner]))
                .len(),
            1
        );
        assert_eq!(
            snapshot.query(&TopologyQuery::default().with_capabilities([CapabilityKind::File]))[0]
                .node_id,
            node(2)
        );
        assert_eq!(
            snapshot
                .query(&TopologyQuery::default().with_health([TopologyNodeHealth::Unreachable]))[0]
                .node_id,
            node(4)
        );
        assert_eq!(
            snapshot
                .query(&TopologyQuery::default().with_network(b.clone()))
                .len(),
            1
        );
        assert!(snapshot
            .query(
                &TopologyQuery::default()
                    .with_network(b)
                    .with_roles([MembershipRole::Owner])
            )
            .is_empty());
    }

    #[test]
    fn diff_reports_incremental_changes_without_duplicating_nodes() {
        let a = network(1);
        let first = builder(std::slice::from_ref(&a))
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(active(1, MembershipRole::Owner, NOW))
                    .with_member(active(2, MembershipRole::Member, NOW)),
            )
            .observe_link(TopologyLinkObservation::new(
                node(1),
                node(2),
                a.clone(),
                NOW,
                true,
            ))
            .build()
            .unwrap();
        let second =
            TopologyBuilder::new(TopologyViewer::new(node(1), [a.clone()]), 2, NOW + 1_000)
                .add_network(
                    TopologyNetworkInput::new(a, "alpha")
                        .with_member(
                            active(1, MembershipRole::Owner, NOW + 1_000)
                                .with_display_name("alice"),
                        )
                        .with_member(active(3, MembershipRole::Member, NOW + 1_000)),
                )
                .build()
                .unwrap();

        let diff = second.diff(&first);
        assert_eq!(diff.base_revision, 1);
        assert_eq!(diff.revision, 2);
        assert_eq!(diff.added_nodes, vec![node(3)]);
        assert_eq!(diff.updated_nodes, vec![node(1)]);
        assert_eq!(diff.removed_nodes, vec![node(2)]);
        assert_eq!(diff.removed_connections.len(), 1);
        assert!(diff.added_connections.is_empty());
        assert!(diff.added_boundaries.is_empty());
        assert!(diff.removed_boundaries.is_empty());
        assert_eq!(diff.unchanged_node_count, 0);
        // Re-diffing the same revision is empty, so refreshes do not animate.
        let empty = second.diff(&second);
        assert!(empty.added_nodes.is_empty());
        assert!(empty.updated_nodes.is_empty());
        assert!(empty.removed_nodes.is_empty());
    }

    #[test]
    fn non_members_are_not_projected_and_duplicates_are_rejected() {
        let a = network(1);
        let snapshot = builder(std::slice::from_ref(&a))
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(member(1, MembershipRole::Owner))
                    .with_member(
                        member(2, MembershipRole::Member).with_status(NetworkMemberStatus::Pending),
                    )
                    .with_member(
                        member(3, MembershipRole::Member).with_status(NetworkMemberStatus::Removed),
                    )
                    .with_member(
                        member(4, MembershipRole::Member).with_status(NetworkMemberStatus::Expired),
                    )
                    .with_member(
                        member(5, MembershipRole::Member).with_status(NetworkMemberStatus::Exited),
                    ),
            )
            .build()
            .unwrap();
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.boundaries[0].member_count, 1);
        assert_eq!(snapshot.nodes[0].health, TopologyNodeHealth::Unknown);

        let err = builder(std::slice::from_ref(&a))
            .add_network(
                TopologyNetworkInput::new(a, "alpha")
                    .with_member(member(1, MembershipRole::Owner))
                    .with_member(member(1, MembershipRole::Member)),
            )
            .build()
            .unwrap_err();
        assert_eq!(err.code(), "domain.invariant_violation");
    }

    #[test]
    fn snapshot_round_trips_through_serde() {
        let a = network(1);
        let snapshot = builder(std::slice::from_ref(&a))
            .add_network(
                TopologyNetworkInput::new(a.clone(), "alpha")
                    .with_member(active(1, MembershipRole::Owner, NOW))
                    .with_member(active(2, MembershipRole::Member, NOW)),
            )
            .observe_link(TopologyLinkObservation::new(node(1), node(2), a, NOW, true))
            .build()
            .unwrap();
        let encoded = serde_json::to_string(&snapshot).unwrap();
        let decoded: TopologySnapshot = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, snapshot);
    }
}
