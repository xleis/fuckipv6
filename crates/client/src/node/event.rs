use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

use super::{NodeLogLevel, NodeNetworkStatus};

/// Ordered event delivered to every attached UI and control adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeEvent {
    pub sequence: u64,
    pub kind: NodeEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum NodeEventKind {
    StateChanged {
        revision: u64,
    },
    NetworkChanged {
        network_name: String,
        status: NodeNetworkStatus,
    },
    LogAppended {
        level: NodeLogLevel,
        message: String,
    },
    CommandCompleted {
        request_id: u64,
        success: bool,
    },
}

/// Fan-out stream for ephemeral node events. Durable current state remains in
/// `NodeStateStore`; late subscribers recover by reading its latest snapshot.
#[derive(Clone)]
pub struct NodeEventBus {
    sender: broadcast::Sender<NodeEvent>,
    sequence: Arc<AtomicU64>,
}

impl NodeEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            sender,
            sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, kind: NodeEventKind) -> NodeEvent {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
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
