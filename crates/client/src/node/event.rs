use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use fuckipv6_common::domain::OperationState;

use super::{NodeEmbeddedServiceStatus, NodeLogLevel, NodeNetworkStatus, NodeRoutingMode};

/// Ordered event delivered to every attached UI and control adapter.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeEvent {
    pub sequence: u64,
    pub kind: NodeEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum NodeEventKind {
    StateChanged {
        revision: u64,
    },
    NetworkChanged {
        network_name: String,
        status: NodeNetworkStatus,
    },
    EmbeddedServiceChanged {
        network_name: String,
        status: NodeEmbeddedServiceStatus,
    },
    LogAppended {
        level: NodeLogLevel,
        message: String,
    },
    RoutingChanged {
        network_name: String,
        mode: NodeRoutingMode,
        exit_node: Option<String>,
        is_exit_node: bool,
    },
    RoleChanged {
        network_name: String,
        role: super::NodeNetworkRole,
        enabled: bool,
        roles: Vec<super::NodeNetworkRole>,
    },
    CommandCompleted {
        request_id: u64,
        success: bool,
    },
    OperationChanged {
        operation_id: String,
        state: OperationState,
        stage: String,
    },
}

/// Fan-out stream for ephemeral node events. Durable current state remains in
/// `NodeStateStore`; late subscribers recover by reading its latest snapshot.
#[derive(Clone)]
pub struct NodeEventBus {
    sender: broadcast::Sender<NodeEvent>,
    sequence: Arc<AtomicU64>,
    subscription_lock: Arc<Mutex<()>>,
}

impl NodeEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            sender,
            sequence: Arc::new(AtomicU64::new(0)),
            subscription_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.sender.subscribe()
    }

    /// Subscribe and capture an exact event boundary atomically with respect to
    /// `publish`. Adapters use the returned sequence as the point from which an
    /// authoritative snapshot must be followed. Events after that boundary are
    /// guaranteed to be queued for the returned receiver.
    pub fn subscribe_with_sequence(&self) -> (broadcast::Receiver<NodeEvent>, u64) {
        let _guard = self
            .subscription_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let receiver = self.sender.subscribe();
        let sequence = self.sequence.load(Ordering::Acquire);
        (receiver, sequence)
    }

    /// Return the latest published sequence without creating a subscriber.
    pub fn latest_sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    pub fn publish(&self, kind: NodeEventKind) -> NodeEvent {
        let _guard = self
            .subscription_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        let event = NodeEvent { sequence, kind };
        let _ = self.sender.send(event.clone());
        event
    }
}

impl Default for NodeEventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishes_monotonic_events_to_all_subscribers() {
        let events = NodeEventBus::new(8);
        let mut first = events.subscribe();
        let mut second = events.subscribe();

        let published = events.publish(NodeEventKind::NetworkChanged {
            network_name: "home".into(),
            status: NodeNetworkStatus::Online,
        });
        assert_eq!(published.sequence, 1);
        assert_eq!(first.recv().await.expect("first event").sequence, 1);
        assert_eq!(second.recv().await.expect("second event").sequence, 1);

        let published = events.publish(NodeEventKind::StateChanged { revision: 7 });
        assert_eq!(published.sequence, 2);
        assert_eq!(first.recv().await.expect("third event").sequence, 2);
    }
}
