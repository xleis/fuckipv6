use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

use super::{NodeEventBus, NodeEventKind, NodeLifecycleSnapshot};

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
    pub networks: Vec<NodeNetworkSnapshot>,
    pub peers: Vec<NodePeerSnapshot>,
    pub logs: Vec<NodeLogEntry>,
    pub last_error: Option<NodeProblem>,
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
        let store = Self::new();
        Self {
            events: Some(events),
            ..store
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
        snapshot.revision = snapshot.revision.wrapping_add(1);
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
