//! Durable event envelopes and replay/cursor semantics.

use super::foundation::{ContractIssue, UnixMillis};
use super::ids::{AuditId, EventId, NetworkId, NodeId, OperationId};
use super::operation::OperationState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventStreamId(String);

impl EventStreamId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractIssue> {
        let value = value.into();
        if value.is_empty() || value.len() > 256 {
            return Err(ContractIssue::new(
                "invalid_event_stream",
                "stream_id",
                "event stream id must contain 1-256 characters",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventCursor {
    pub stream_id: EventStreamId,
    pub sequence: u64,
    pub snapshot_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEventKind {
    SnapshotChanged,
    NetworkChanged {
        network_id: NetworkId,
    },
    MembershipChanged {
        network_id: NetworkId,
        node_id: NodeId,
    },
    TrafficPolicyChanged {
        revision: u64,
    },
    SecurityPolicyChanged {
        revision: u64,
    },
    CapabilityChanged {
        capability_id: String,
    },
    OperationChanged {
        operation_id: OperationId,
        state: OperationState,
    },
    AuditAppended {
        audit_id: AuditId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainEvent {
    pub id: EventId,
    pub stream_id: EventStreamId,
    pub sequence: u64,
    pub snapshot_revision: u64,
    pub occurred_at: UnixMillis,
    pub kind: DomainEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventBatch {
    pub cursor: EventCursor,
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    UnknownStream,
    CursorAhead,
    HistoryGap,
    SnapshotAhead,
    SnapshotRolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ReplayOutcome {
    Events(EventBatch),
    Resync {
        cursor: EventCursor,
        reason: ResyncReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventAppendError {
    Contract(Vec<ContractIssue>),
    WrongStream,
    SequenceGap { expected: u64, actual: u64 },
    DuplicateIdConflict,
}

impl fmt::Display for EventAppendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid event: {issues:?}"),
            Self::WrongStream => f.write_str("event belongs to another stream"),
            Self::SequenceGap { expected, actual } => {
                write!(f, "event sequence gap: expected {expected}, got {actual}")
            }
            Self::DuplicateIdConflict => {
                f.write_str("an event id was reused with a different event body")
            }
        }
    }
}

impl std::error::Error for EventAppendError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventAppendOutcome {
    Appended,
    DuplicateIgnored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLog {
    pub stream_id: EventStreamId,
    pub snapshot_revision: u64,
    events: BTreeMap<u64, DomainEvent>,
    ids: BTreeMap<EventId, u64>,
}

impl EventLog {
    pub fn new(stream_id: EventStreamId, snapshot_revision: u64) -> Self {
        Self {
            stream_id,
            snapshot_revision,
            events: BTreeMap::new(),
            ids: BTreeMap::new(),
        }
    }

    pub fn latest_sequence(&self) -> u64 {
        self.events.keys().next_back().copied().unwrap_or(0)
    }

    pub fn oldest_sequence(&self) -> Option<u64> {
        self.events.keys().next().copied()
    }

    pub fn latest_cursor(&self) -> EventCursor {
        EventCursor {
            stream_id: self.stream_id.clone(),
            sequence: self.latest_sequence(),
            snapshot_revision: self.snapshot_revision,
        }
    }

    pub fn append(&mut self, event: DomainEvent) -> Result<EventAppendOutcome, EventAppendError> {
        if event.stream_id != self.stream_id {
            return Err(EventAppendError::WrongStream);
        }
        if let Some(existing_sequence) = self.ids.get(&event.id) {
            let existing = &self.events[existing_sequence];
            return if existing == &event {
                Ok(EventAppendOutcome::DuplicateIgnored)
            } else {
                Err(EventAppendError::DuplicateIdConflict)
            };
        }
        let expected = self.latest_sequence().saturating_add(1);
        if event.sequence != expected {
            return Err(EventAppendError::SequenceGap {
                expected,
                actual: event.sequence,
            });
        }
        if event.snapshot_revision < self.snapshot_revision {
            return Err(EventAppendError::Contract(vec![ContractIssue::new(
                "revision_regression",
                "snapshot_revision",
                "event snapshot revision must not regress",
            )]));
        }
        self.snapshot_revision = self.snapshot_revision.max(event.snapshot_revision);
        self.ids.insert(event.id, event.sequence);
        self.events.insert(event.sequence, event);
        Ok(EventAppendOutcome::Appended)
    }

    /// SSE reconnect must either resume strictly after the cursor or request a
    /// full snapshot. It never silently skips a retention gap.
    pub fn replay_after(&self, cursor: &EventCursor) -> ReplayOutcome {
        if cursor.stream_id != self.stream_id {
            return ReplayOutcome::Resync {
                cursor: cursor.clone(),
                reason: ResyncReason::UnknownStream,
            };
        }
        let latest = self.latest_sequence();
        if cursor.sequence > latest {
            return ReplayOutcome::Resync {
                cursor: cursor.clone(),
                reason: ResyncReason::CursorAhead,
            };
        }
        if cursor.snapshot_revision > self.snapshot_revision {
            return ReplayOutcome::Resync {
                cursor: cursor.clone(),
                reason: ResyncReason::SnapshotRolledBack,
            };
        }
        if let Some(oldest) = self.oldest_sequence() {
            if cursor.sequence.saturating_add(1) < oldest {
                return ReplayOutcome::Resync {
                    cursor: cursor.clone(),
                    reason: ResyncReason::HistoryGap,
                };
            }
        }
        let events = self
            .events
            .range(cursor.sequence.saturating_add(1)..)
            .map(|(_, event)| event.clone())
            .collect::<Vec<_>>();
        // A snapshot revision is not an event sequence. If the client missed a
        // snapshot boundary, retained incremental events cannot reconstruct the
        // new baseline unless the replay includes the SnapshotChanged marker.
        let crosses_snapshot = events.iter().any(|event| {
            event.snapshot_revision > cursor.snapshot_revision
                && event.kind == DomainEventKind::SnapshotChanged
        });
        if cursor.snapshot_revision < self.snapshot_revision && !crosses_snapshot {
            return ReplayOutcome::Resync {
                cursor: cursor.clone(),
                reason: ResyncReason::SnapshotAhead,
            };
        }
        let next_sequence = events
            .last()
            .map(|event| event.sequence)
            .unwrap_or(cursor.sequence);
        ReplayOutcome::Events(EventBatch {
            cursor: EventCursor {
                stream_id: self.stream_id.clone(),
                sequence: next_sequence,
                snapshot_revision: self.snapshot_revision,
            },
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> EventStreamId {
        EventStreamId::new("node-1").expect("stream")
    }

    fn event(sequence: u64, id: u8, revision: u64) -> DomainEvent {
        DomainEvent {
            id: EventId::from_bytes([id; 16]),
            stream_id: stream(),
            sequence,
            snapshot_revision: revision,
            occurred_at: UnixMillis(sequence),
            kind: DomainEventKind::SnapshotChanged,
        }
    }

    #[test]
    fn reconnect_replays_strictly_after_cursor() {
        let mut log = EventLog::new(stream(), 1);
        log.append(event(1, 1, 1)).expect("append");
        log.append(event(2, 2, 2)).expect("append");
        let outcome = log.replay_after(&EventCursor {
            stream_id: stream(),
            sequence: 1,
            snapshot_revision: 1,
        });
        match outcome {
            ReplayOutcome::Events(batch) => {
                assert_eq!(batch.events.len(), 1);
                assert_eq!(batch.events[0].sequence, 2);
                assert_eq!(batch.cursor.sequence, 2);
            }
            other => panic!("unexpected replay: {other:?}"),
        }
    }

    #[test]
    fn duplicate_event_is_idempotent_but_reused_id_is_rejected() {
        let mut log = EventLog::new(stream(), 1);
        let first = event(1, 1, 1);
        assert_eq!(log.append(first.clone()), Ok(EventAppendOutcome::Appended));
        assert_eq!(
            log.append(first.clone()),
            Ok(EventAppendOutcome::DuplicateIgnored)
        );
        let mut conflict = first;
        conflict.kind = DomainEventKind::NetworkChanged {
            network_id: NetworkId::from_bytes([2; 32]),
        };
        assert_eq!(
            log.append(conflict),
            Err(EventAppendError::DuplicateIdConflict)
        );
    }

    #[test]
    fn sequence_gap_triggers_error_and_replay_gap_triggers_resync() {
        let mut log = EventLog::new(stream(), 1);
        assert_eq!(
            log.append(event(2, 2, 2)),
            Err(EventAppendError::SequenceGap {
                expected: 1,
                actual: 2,
            })
        );
        log.append(event(1, 1, 1)).expect("append");
        log.append(event(2, 2, 2)).expect("append");
        log.append(event(3, 3, 3)).expect("append");

        let mut truncated = log.clone();
        truncated.events.remove(&1);
        truncated.ids.remove(&EventId::from_bytes([1; 16]));
        let outcome = truncated.replay_after(&EventCursor {
            stream_id: stream(),
            sequence: 0,
            snapshot_revision: 1,
        });
        assert!(matches!(
            outcome,
            ReplayOutcome::Resync {
                reason: ResyncReason::HistoryGap,
                ..
            }
        ));
    }

    #[test]
    fn cursor_from_future_or_unknown_stream_requires_snapshot() {
        let log = EventLog::new(stream(), 4);
        assert!(matches!(
            log.replay_after(&EventCursor {
                stream_id: stream(),
                sequence: 1,
                snapshot_revision: 4,
            }),
            ReplayOutcome::Resync {
                reason: ResyncReason::CursorAhead,
                ..
            }
        ));
        assert!(matches!(
            log.replay_after(&EventCursor {
                stream_id: EventStreamId::new("other").expect("other"),
                sequence: 0,
                snapshot_revision: 1,
            }),
            ReplayOutcome::Resync {
                reason: ResyncReason::UnknownStream,
                ..
            }
        ));
    }
}
