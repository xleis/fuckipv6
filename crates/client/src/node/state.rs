use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

use super::{NodeEventBus, NodeEventKind, NodeLifecycleSnapshot};
use crate::ui::UiModeResolution;

/// Stable, transport-neutral state rendered by every node UI adapter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub revision: u64,
    pub lifecycle: NodeLifecycleSnapshot,
    pub identity: Option<NodeIdentitySnapshot>,
    #[serde(default)]
    pub configuration: NodeConfigurationSnapshot,
    #[serde(default)]
    pub permissions: NodePermissionSnapshot,
    #[serde(default)]
    pub embedded_services: Vec<NodeEmbeddedServiceSnapshot>,
    #[serde(default)]
    pub ui_mode: Option<UiModeResolution>,
    pub networks: Vec<NodeNetworkSnapshot>,
    pub peers: Vec<NodePeerSnapshot>,
    pub logs: Vec<NodeLogEntry>,
    pub last_error: Option<NodeProblem>,
}

impl NodeSnapshot {
    /// Returns machine-readable consistency violations for diagnostics and
    /// tests. UI adapters may surface these values, but must never repair state
    /// locally or derive a parallel state machine from them.
    pub fn invariant_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();

        collect_duplicate_names(
            self.networks.iter().map(|network| network.name.as_str()),
            "duplicate_network",
            &mut violations,
        );
        collect_duplicate_names(
            self.configuration
                .networks
                .iter()
                .map(|network| network.network_name.as_str()),
            "duplicate_configuration",
            &mut violations,
        );
        collect_duplicate_names(
            self.embedded_services
                .iter()
                .map(|service| service.network_name.as_str()),
            "duplicate_embedded_service",
            &mut violations,
        );

        let configured = self
            .configuration
            .networks
            .iter()
            .map(|network| network.network_name.as_str())
            .collect::<HashSet<_>>();
        let networks = self
            .networks
            .iter()
            .map(|network| (network.name.as_str(), network))
            .collect::<HashMap<_, _>>();

        for network in &self.networks {
            if !configured.contains(network.name.as_str()) {
                violations.push(format!("missing_network_configuration:{}", network.name));
            }

            let expected_peers = self
                .peers
                .iter()
                .filter(|peer| peer.network_name == network.name)
                .count();
            if network.peer_count != expected_peers {
                violations.push(format!(
                    "peer_count_mismatch:{}:expected={expected_peers}:actual={}",
                    network.name, network.peer_count
                ));
            }
            if network.status == NodeNetworkStatus::Offline && expected_peers > 0 {
                violations.push(format!("offline_network_has_peers:{}", network.name));
            }
        }

        for peer in &self.peers {
            match networks.get(peer.network_name.as_str()) {
                None => violations.push(format!("peer_for_unknown_network:{}", peer.network_name)),
                Some(network) if network.status == NodeNetworkStatus::Offline => {
                    violations.push(format!("peer_for_offline_network:{}", peer.network_name));
                }
                Some(_) => {}
            }
        }

        violations
    }
}

fn collect_duplicate_names<'a>(
    names: impl Iterator<Item = &'a str>,
    code: &str,
    violations: &mut Vec<String>,
) {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            violations.push(format!("{code}:{name}"));
        }
    }
}

/// Effective, non-secret network configuration exposed to every UI adapter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeConfigurationSnapshot {
    pub networks: Vec<NodeNetworkConfigSnapshot>,
}

/// One network's effective configuration after loading startup overrides.
///
/// Filesystem paths and other host-local details are deliberately omitted. The
/// boolean fields only state whether optional host-local settings are present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeNetworkConfigSnapshot {
    pub network_name: String,
    pub coordinator_url: String,
    pub virtual_mode: Option<fuckipv6_common::NetworkMode>,
    pub preferred_ipv4: Option<Ipv4Addr>,
    pub preferred_ipv6: Option<Ipv6Addr>,
    pub virtual_network: Option<String>,
    pub tun_name: String,
    pub mtu: u16,
    pub p2p_listen_port: u16,
    pub p2p_connect_timeout_secs: u64,
    pub p2p_heartbeat_interval_secs: u64,
    pub routing_mode: NodeRoutingMode,
    pub exit_node: Option<String>,
    pub direct_cidrs: Vec<String>,
    pub proxy_cidrs: Vec<String>,
    pub exit_node_enabled: bool,
    pub exit_node_allowed_clients: Vec<String>,
    pub exit_node_allow_lan_access: bool,
    pub exit_node_region: Option<String>,
    pub acl_enabled: bool,
    pub acl_default_action: fuckipv6_common::AclAction,
    pub acl_rule_count: usize,
    pub log_level: String,
    pub log_file_configured: bool,
    pub identity_configured: bool,
}

/// Non-secret runtime state for one network's embedded signaling and relay
/// services. Invitation tickets, identity material, and host-local paths are
/// deliberately excluded from this shared UI contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeEmbeddedServiceSnapshot {
    pub network_name: String,
    pub status: NodeEmbeddedServiceStatus,
    pub signaling_addr: Option<SocketAddr>,
    pub relay_addr: Option<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeEmbeddedServiceStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// Control-plane authorization posture shared with UI adapters.
///
/// `TrustedControlPlane` means the runtime accepts every connected adapter as
/// fully privileged. It does not mean per-user or remote authentication exists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodePermissionSnapshot {
    pub access_model: NodeAccessModel,
    pub permissions: Vec<NodePermission>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeAccessModel {
    #[default]
    TrustedControlPlane,
}

/// Stable capability classes used by both the permission snapshot and UI actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodePermission {
    ViewState,
    ManageNetworks,
    ChangeRouting,
    ManageRoles,
    ProbePeers,
    StopNode,
    ViewLogs,
}

impl NodePermission {
    pub const ALL: [Self; 7] = [
        Self::ViewState,
        Self::ManageNetworks,
        Self::ChangeRouting,
        Self::ManageRoles,
        Self::ProbePeers,
        Self::StopNode,
        Self::ViewLogs,
    ];
}

/// Public node identity metadata. Secret key material must never enter this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentitySnapshot {
    pub node_id: String,
    pub display_name: Option<String>,
    pub public_key_fingerprint: String,
}

/// One local network and the roles this node currently performs in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeNetworkSnapshot {
    pub name: String,
    pub tun_name: String,
    pub status: NodeNetworkStatus,
    pub assigned_ipv6: Option<Ipv6Addr>,
    pub assigned_ipv4: Option<Ipv4Addr>,
    pub p2p_listen_addr: Option<SocketAddr>,
    pub uptime_secs: u64,
    pub roles: Vec<NodeNetworkRole>,
    pub peer_count: usize,
    pub traffic: NodeTrafficSnapshot,
}

/// Network roles are capabilities within a network, never product identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeNetworkRole {
    Member,
    Discovery,
    Relay,
    ExitNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRoutingMode {
    MeshOnly,
    BypassMainland,
    GlobalProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeNetworkStatus {
    Offline,
    Connecting,
    Online,
    Degraded,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeTrafficSnapshot {
    pub tun_rx_packets: u64,
    pub tun_rx_bytes: u64,
    pub tun_tx_packets: u64,
    pub tun_tx_bytes: u64,
    pub p2p_rx_packets: u64,
    pub p2p_rx_bytes: u64,
    pub p2p_tx_packets: u64,
    pub p2p_tx_bytes: u64,
    pub error_count: u64,
}

/// Peer state scoped to a network. The same node may appear in multiple networks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodePeerSnapshot {
    pub peer_id: String,
    pub network_name: String,
    pub virtual_ipv6: Ipv6Addr,
    pub virtual_ipv4: Option<Ipv4Addr>,
    pub endpoints: Vec<SocketAddr>,
    pub online: bool,
    pub p2p_active: bool,
    pub p2p_healthy: bool,
    pub rtt_us: Option<u64>,
    pub loss_rate: f32,
    pub traffic: NodeTrafficSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLogEntry {
    pub unix_ms: u64,
    pub level: NodeLogLevel,
    pub target: String,
    pub message: String,
}

/// UI- and transport-neutral error returned by every node command adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeProblem {
    pub code: NodeProblemCode,
    pub message: String,
    pub network_name: Option<String>,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeProblemCode {
    InvalidRequest,
    NotFound,
    Conflict,
    Unavailable,
    PermissionDenied,
    Cancelled,
    Internal,
}

impl NodeProblem {
    pub fn new(code: NodeProblemCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            network_name: None,
            retryable: false,
        }
    }

    pub fn for_network(mut self, network_name: impl Into<String>) -> Self {
        self.network_name = Some(network_name.into());
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

/// Single shared state store. Every UI adapter subscribes to this watch channel
/// instead of maintaining a private node/network state machine.
#[derive(Clone)]
pub struct NodeStateStore {
    snapshot: Arc<RwLock<NodeSnapshot>>,
    updates: watch::Sender<NodeSnapshot>,
    events: Option<NodeEventBus>,
}

impl NodeStateStore {
    pub fn new() -> Self {
        let snapshot = NodeSnapshot::default();
        let (updates, _) = watch::channel(snapshot.clone());
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            updates,
            events: None,
        }
    }

    /// Attach the shared event bus so every state revision is also emitted as a
    /// `StateChanged` event for streaming adapters.
    pub fn with_events(events: NodeEventBus) -> Self {
        Self::with_events_identity_and_ui(events, None, None)
    }

    /// Construct the canonical store with node identity and the resolved UI
    /// mode that is actually active for this process.
    pub(crate) fn with_events_identity_and_ui(
        events: NodeEventBus,
        identity: Option<NodeIdentitySnapshot>,
        ui_mode: Option<UiModeResolution>,
    ) -> Self {
        let snapshot = NodeSnapshot {
            identity,
            ui_mode,
            ..NodeSnapshot::default()
        };
        let (updates, _) = watch::channel(snapshot.clone());
        Self {
            snapshot: Arc::new(RwLock::new(snapshot)),
            updates,
            events: Some(events),
        }
    }

    pub async fn snapshot(&self) -> NodeSnapshot {
        self.snapshot.read().await.clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<NodeSnapshot> {
        self.updates.subscribe()
    }

    /// Apply one atomic state transition and publish the next revision.
    pub async fn update<F>(&self, update: F) -> NodeSnapshot
    where
        F: FnOnce(&mut NodeSnapshot),
    {
        let mut snapshot = self.snapshot.write().await;
        update(&mut snapshot);
        snapshot.revision = snapshot.revision.saturating_add(1);
        let published = snapshot.clone();
        self.updates.send_replace(published.clone());
        if let Some(events) = &self.events {
            events.publish(NodeEventKind::StateChanged {
                revision: published.revision,
            });
        }
        published
    }

    pub async fn set_lifecycle(&self, lifecycle: NodeLifecycleSnapshot) -> NodeSnapshot {
        self.update(|snapshot| snapshot.lifecycle = lifecycle).await
    }
}

impl Default for NodeStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(name: &str, status: NodeNetworkStatus) -> NodeNetworkSnapshot {
        NodeNetworkSnapshot {
            name: name.into(),
            tun_name: format!("fk-{name}"),
            status,
            assigned_ipv6: None,
            assigned_ipv4: None,
            p2p_listen_addr: None,
            uptime_secs: 0,
            roles: vec![NodeNetworkRole::Member],
            peer_count: 0,
            traffic: NodeTrafficSnapshot::default(),
        }
    }

    fn configuration(name: &str) -> NodeNetworkConfigSnapshot {
        NodeNetworkConfigSnapshot {
            network_name: name.into(),
            coordinator_url: "ws://127.0.0.1:6666".into(),
            virtual_mode: None,
            preferred_ipv4: None,
            preferred_ipv6: None,
            virtual_network: None,
            tun_name: format!("fk-{name}"),
            mtu: 1420,
            p2p_listen_port: 0,
            p2p_connect_timeout_secs: 30,
            p2p_heartbeat_interval_secs: 5,
            routing_mode: NodeRoutingMode::MeshOnly,
            exit_node: None,
            direct_cidrs: Vec::new(),
            proxy_cidrs: Vec::new(),
            exit_node_enabled: false,
            exit_node_allowed_clients: Vec::new(),
            exit_node_allow_lan_access: false,
            exit_node_region: None,
            acl_enabled: false,
            acl_default_action: fuckipv6_common::AclAction::Allow,
            acl_rule_count: 0,
            log_level: "info".into(),
            log_file_configured: false,
            identity_configured: true,
        }
    }

    fn embedded_service(name: &str) -> NodeEmbeddedServiceSnapshot {
        NodeEmbeddedServiceSnapshot {
            network_name: name.into(),
            status: NodeEmbeddedServiceStatus::Stopped,
            signaling_addr: None,
            relay_addr: None,
        }
    }

    fn peer(network_name: &str) -> NodePeerSnapshot {
        NodePeerSnapshot {
            peer_id: format!("peer-{network_name}"),
            network_name: network_name.into(),
            virtual_ipv6: Ipv6Addr::LOCALHOST,
            virtual_ipv4: None,
            endpoints: Vec::new(),
            online: true,
            p2p_active: true,
            p2p_healthy: true,
            rtt_us: Some(1_000),
            loss_rate: 0.0,
            traffic: NodeTrafficSnapshot::default(),
        }
    }

    #[test]
    fn default_snapshot_has_no_invariant_violations() {
        assert!(NodeSnapshot::default().invariant_violations().is_empty());
    }

    #[test]
    fn duplicate_names_are_reported_for_every_snapshot_collection() {
        let snapshot = NodeSnapshot {
            configuration: NodeConfigurationSnapshot {
                networks: vec![configuration("home"), configuration("home")],
            },
            embedded_services: vec![embedded_service("home"), embedded_service("home")],
            networks: vec![network("home", NodeNetworkStatus::Online); 2],
            ..NodeSnapshot::default()
        };

        let violations = snapshot.invariant_violations();
        assert!(violations.contains(&"duplicate_network:home".into()));
        assert!(violations.contains(&"duplicate_configuration:home".into()));
        assert!(violations.contains(&"duplicate_embedded_service:home".into()));
    }

    #[test]
    fn network_configuration_and_peer_relationships_are_checked() {
        let mut offline_without_config = network("offline", NodeNetworkStatus::Offline);
        offline_without_config.peer_count = 1;
        let mut count_mismatch = network("mismatch", NodeNetworkStatus::Online);
        count_mismatch.peer_count = 3;
        let snapshot = NodeSnapshot {
            configuration: NodeConfigurationSnapshot {
                networks: vec![configuration("mismatch")],
            },
            networks: vec![offline_without_config, count_mismatch],
            peers: vec![peer("offline"), peer("unknown")],
            ..NodeSnapshot::default()
        };

        let violations = snapshot.invariant_violations();
        assert!(violations.contains(&"missing_network_configuration:offline".into()));
        assert!(violations.contains(&"peer_count_mismatch:mismatch:expected=0:actual=3".into()));
        assert!(violations.contains(&"offline_network_has_peers:offline".into()));
        assert!(violations.contains(&"peer_for_offline_network:offline".into()));
        assert!(violations.contains(&"peer_for_unknown_network:unknown".into()));
    }

    #[tokio::test]
    async fn state_store_exposes_the_effective_ui_mode() {
        let resolution = crate::ui::resolve_ui_mode(
            crate::ui::UiModeRequest::WebUi,
            crate::ui::UiCapabilities {
                display: crate::ui::DisplayEnvironment::LinuxWayland,
                gpui_available: false,
            },
        )
        .expect("desktop WebUI should resolve");
        let state = NodeStateStore::with_events_identity_and_ui(
            NodeEventBus::default(),
            None,
            Some(resolution),
        );

        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.ui_mode, Some(resolution));
        let encoded = serde_json::to_value(snapshot).expect("serialize snapshot");
        assert_eq!(encoded["ui_mode"]["effective"], "web_ui");
        assert_eq!(encoded["ui_mode"]["requested"], "web_ui");
    }

    #[tokio::test]
    async fn state_store_publishes_atomic_revisions() {
        let state = NodeStateStore::new();
        let updates = state.subscribe();

        state
            .update(|snapshot| {
                snapshot.networks.push(NodeNetworkSnapshot {
                    name: "home".into(),
                    tun_name: "fk-home".into(),
                    status: NodeNetworkStatus::Online,
                    assigned_ipv6: Some("fd00::2".parse().expect("valid IPv6")),
                    assigned_ipv4: None,
                    p2p_listen_addr: Some("127.0.0.1:9000".parse().expect("valid socket")),
                    uptime_secs: 10,
                    roles: vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode],
                    peer_count: 1,
                    traffic: NodeTrafficSnapshot::default(),
                });
            })
            .await;

        assert_eq!(updates.borrow().revision, 1);
        assert_eq!(updates.borrow().networks[0].name, "home");
        assert_eq!(
            updates.borrow().networks[0].roles,
            vec![NodeNetworkRole::Member, NodeNetworkRole::ExitNode]
        );
    }

    #[tokio::test]
    async fn configured_event_bus_observes_every_state_revision() {
        let events = NodeEventBus::new(8);
        let mut receiver = events.subscribe();
        let state = NodeStateStore::with_events(events);

        state.update(|snapshot| snapshot.last_error = None).await;

        assert_eq!(
            receiver.recv().await.expect("state event").kind,
            NodeEventKind::StateChanged { revision: 1 }
        );
    }

    #[tokio::test]
    async fn revision_saturates_instead_of_wrapping() {
        let state = NodeStateStore::new();
        let updates = state.subscribe();

        state
            .update(|snapshot| snapshot.revision = u64::MAX - 1)
            .await;
        assert_eq!(updates.borrow().revision, u64::MAX);

        state.update(|_| {}).await;
        assert_eq!(updates.borrow().revision, u64::MAX);
    }

    #[test]
    fn node_problem_round_trips_with_stable_codes_and_flags() {
        let cases = [
            (NodeProblemCode::InvalidRequest, "invalid_request"),
            (NodeProblemCode::NotFound, "not_found"),
            (NodeProblemCode::Conflict, "conflict"),
            (NodeProblemCode::Unavailable, "unavailable"),
            (NodeProblemCode::PermissionDenied, "permission_denied"),
            (NodeProblemCode::Cancelled, "cancelled"),
            (NodeProblemCode::Internal, "internal"),
        ];

        for (code, wire_name) in cases {
            let problem = NodeProblem::new(code, "测试错误")
                .for_network("home")
                .retryable(true);
            let encoded = serde_json::to_value(&problem).expect("serialize node problem");
            assert_eq!(
                encoded,
                serde_json::json!({
                    "code": wire_name,
                    "message": "测试错误",
                    "network_name": "home",
                    "retryable": true,
                })
            );
            let decoded: NodeProblem =
                serde_json::from_value(encoded).expect("deserialize node problem");
            assert_eq!(decoded, problem);
        }
    }

    #[test]
    fn snapshot_round_trips_without_secret_identity_material() {
        let snapshot = NodeSnapshot {
            identity: Some(NodeIdentitySnapshot {
                node_id: "node-1".into(),
                display_name: Some("home-gateway".into()),
                public_key_fingerprint: "b3:fixture".into(),
            }),
            ..NodeSnapshot::default()
        };

        let encoded = serde_json::to_string(&snapshot).expect("serialize node snapshot");
        let decoded: NodeSnapshot = serde_json::from_str(&encoded).expect("deserialize snapshot");
        assert_eq!(decoded, snapshot);
        assert!(!encoded.contains("private_key"));
        assert!(!encoded.contains("secret_key"));
    }
}
