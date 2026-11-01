use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use fuckipv6_common::domain::OperationState;

use super::{NodeEmbeddedServiceStatus, NodeLogLevel, NodeNetworkStatus, NodeRoutingMode};

/// Ordered event delivered to every attached UI and control adapter.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeEvent {
    pub sequence: u64,
    /// Canonical node-state revision in effect when this event was published.
    /// Consumers must acknowledge it together with `sequence`; an event cursor
    /// is not valid against a different snapshot epoch.
    pub snapshot_revision: u64,
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

/// Point in the node event stream. Sequence provides exactly-once delivery
/// within retained history; snapshot revision proves the cursor belongs to the
/// snapshot epoch the consumer is applying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct NodeEventCursor {
    pub sequence: u64,
    pub snapshot_revision: u64,
}

impl NodeEventCursor {
    pub const fn new(sequence: u64, snapshot_revision: u64) -> Self {
        Self {
            sequence,
            snapshot_revision,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeEventReplayReason {
    CursorAhead,
    HistoryGap,
    SnapshotRolledBack,
    Lagged,
    RevisionGap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEventReplay {
    Events(Vec<NodeEvent>),
    ResyncRequired {
        requested: NodeEventCursor,
        latest: NodeEventCursor,
        reason: NodeEventReplayReason,
    },
}

#[derive(Debug)]
struct NodeEventBusState {
    latest_sequence: u64,
    snapshot_revision: u64,
    history: VecDeque<NodeEvent>,
    retention_capacity: usize,
}

impl NodeEventBusState {
    fn cursor(&self) -> NodeEventCursor {
        NodeEventCursor::new(self.latest_sequence, self.snapshot_revision)
    }
}

/// Fan-out stream for ephemeral node events plus a bounded replay window used
/// to recover a reconnect without silently skipping changes. Durable current
/// state remains in `NodeStateStore`; expired cursors require a new snapshot.
#[derive(Clone)]
pub struct NodeEventBus {
    sender: broadcast::Sender<NodeEvent>,
    state: Arc<Mutex<NodeEventBusState>>,
}

impl NodeEventBus {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            state: Arc::new(Mutex::new(NodeEventBusState {
                latest_sequence: 0,
                snapshot_revision: 0,
                history: VecDeque::with_capacity(capacity),
                retention_capacity: capacity,
            })),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.sender.subscribe()
    }

    /// Subscribe and capture an exact event/snapshot boundary atomically with
    /// respect to `publish`. Events after the returned cursor are queued for
    /// the receiver.
    pub fn subscribe_with_cursor(&self) -> (broadcast::Receiver<NodeEvent>, NodeEventCursor) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let receiver = self.sender.subscribe();
        let cursor = state.cursor();
        (receiver, cursor)
    }

    /// Compatibility helper for adapters that only consume the sequence.
    pub fn subscribe_with_sequence(&self) -> (broadcast::Receiver<NodeEvent>, u64) {
        let (receiver, cursor) = self.subscribe_with_cursor();
        (receiver, cursor.sequence)
    }

    /// Return the latest published sequence without creating a subscriber.
    pub fn latest_sequence(&self) -> u64 {
        self.latest_cursor().sequence
    }

    /// Return the latest published cursor without creating a subscriber.
    pub fn latest_cursor(&self) -> NodeEventCursor {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cursor()
    }

    /// Advance the canonical snapshot revision independently of event fan-out.
    /// This closes the race where non-state events can be published while a
    /// state update is between its write and its `StateChanged` notification.
    pub fn advance_snapshot_revision(&self, revision: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.snapshot_revision = state.snapshot_revision.max(revision);
    }

    pub fn publish(&self, kind: NodeEventKind) -> NodeEvent {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let NodeEventKind::StateChanged { revision } = &kind {
            state.snapshot_revision = state.snapshot_revision.max(*revision);
        }
        state.latest_sequence = state.latest_sequence.saturating_add(1);
        let event = NodeEvent {
            sequence: state.latest_sequence,
            snapshot_revision: state.snapshot_revision,
            kind,
        };
        state.history.push_back(event.clone());
        while state.history.len() > state.retention_capacity {
            state.history.pop_front();
        }
        let _ = self.sender.send(event.clone());
        event
    }

    /// Replay retained events strictly after `requested`, capped at the
    /// subscription boundary captured before the authoritative snapshot read.
    pub fn replay_after(
        &self,
        requested: NodeEventCursor,
        boundary: NodeEventCursor,
    ) -> NodeEventReplay {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let latest = state.cursor();
        let resync = |reason| NodeEventReplay::ResyncRequired {
            requested,
            latest,
            reason,
        };

        if requested.snapshot_revision > latest.snapshot_revision {
            return resync(NodeEventReplayReason::SnapshotRolledBack);
        }
        if requested.sequence > boundary.sequence || requested.sequence > latest.sequence {
            return resync(NodeEventReplayReason::CursorAhead);
        }
        if requested.sequence == boundary.sequence {
            return NodeEventReplay::Events(Vec::new());
        }

        let first = requested.sequence.saturating_add(1);
        let mut expected = first;
        let mut events = Vec::new();
        for event in state
            .history
            .iter()
            .filter(|event| event.sequence <= boundary.sequence)
        {
            if event.sequence < first {
                continue;
            }
            if event.sequence != expected {
                return resync(NodeEventReplayReason::HistoryGap);
            }
            expected = expected.saturating_add(1);
            events.push(event.clone());
        }

        if events.is_empty() || expected != boundary.sequence.saturating_add(1) {
            return resync(NodeEventReplayReason::HistoryGap);
        }
        NodeEventReplay::Events(events)
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
        assert_eq!(published.snapshot_revision, 0);
        assert_eq!(first.recv().await.expect("first event").sequence, 1);
        assert_eq!(second.recv().await.expect("second event").sequence, 1);

        let published = events.publish(NodeEventKind::StateChanged { revision: 7 });
        assert_eq!(published.sequence, 2);
        assert_eq!(published.snapshot_revision, 7);
        assert_eq!(first.recv().await.expect("third event").sequence, 2);
    }

    #[test]
    fn replays_events_strictly_after_the_cursor() {
        let events = NodeEventBus::new(8);
        events.advance_snapshot_revision(1);
        events.publish(NodeEventKind::StateChanged { revision: 1 });
        events.advance_snapshot_revision(2);
        events.publish(NodeEventKind::StateChanged { revision: 2 });
        let boundary = events.latest_cursor();

        let NodeEventReplay::Events(replayed) =
            events.replay_after(NodeEventCursor::new(0, 2), boundary)
        else {
            panic!("retained history should replay");
        };
        assert_eq!(
            replayed
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(matches!(
            events.replay_after(boundary, boundary),
            NodeEventReplay::Events(events) if events.is_empty()
        ));
    }

    #[test]
    fn expired_cursor_and_future_revision_require_resync() {
        let events = NodeEventBus::new(2);
        for revision in 1..=3 {
            events.advance_snapshot_revision(revision);
            events.publish(NodeEventKind::StateChanged { revision });
        }
        let boundary = events.latest_cursor();
        assert!(matches!(
            events.replay_after(NodeEventCursor::new(0, 3), boundary),
            NodeEventReplay::ResyncRequired {
                reason: NodeEventReplayReason::HistoryGap,
                ..
            }
        ));
        assert!(matches!(
            events.replay_after(NodeEventCursor::new(3, 4), boundary),
            NodeEventReplay::ResyncRequired {
                reason: NodeEventReplayReason::SnapshotRolledBack,
                ..
            }
        ));
    }
}
