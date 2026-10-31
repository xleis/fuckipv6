//! Durable event envelopes, replay cursors and gap recovery.
//!
//! The event log is deliberately independent from a transport. WebSocket,
//! SSE, IPC and local UI adapters can share the same cursor and must resync
//! from an authoritative snapshot whenever retained history cannot prove an
//! incremental transition.

use std::collections::BTreeMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{AuditId, EventId, NetworkId, NodeId, OperationId};
use super::membership::MembershipState;
use super::operation::OperationState;
use super::{DomainError, DomainResult};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(try_from = "String")]
pub struct EventStreamId(
    #[schemars(regex(pattern = "^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$"))] String,
);

impl EventStreamId {
    pub fn parse(value: impl Into<String>) -> DomainResult<Self> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid_first = bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric());
        let valid_rest = bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
        if value.len() > 128 || !valid_first || !valid_rest {
            return Err(DomainError::invalid_value("event stream id is invalid"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EventStreamId {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl fmt::Display for EventStreamId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A client-positioned point in one durable event stream. `sequence` and
/// `snapshot_revision` are independent and must both be acknowledged.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct EventCursor {
    pub stream_id: EventStreamId,
    pub sequence: u64,
    pub snapshot_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DomainEventKind {
    SnapshotChanged {
        revision: u64,
    },
    OperationChanged {
        operation_id: OperationId,
        state: OperationState,
    },
    NetworkChanged {
        network_id: NetworkId,
        revision: u64,
    },
    MembershipChanged {
        network_id: NetworkId,
        node_id: NodeId,
        state: MembershipState,
    },
    SecurityPolicyChanged {
        network_id: NetworkId,
        revision: u64,
    },
    TrafficPolicyChanged {
        revision: u64,
    },
    CapabilityChanged {
        capability_id: String,
        enabled: bool,
    },
    AuditAppended {
        audit_id: AuditId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct DomainEvent {
    pub event_id: EventId,
    pub stream_id: EventStreamId,
    pub sequence: u64,
    pub snapshot_revision: u64,
    pub occurred_at_unix_ms: u64,
    pub kind: DomainEventKind,
}

impl DomainEvent {
    pub fn validate(&self) -> DomainResult<()> {
        if self.sequence == 0 || self.snapshot_revision == 0 {
            return Err(DomainError::invalid_value(
                "event sequence and snapshot revision must be positive",
            ));
        }
        if self.occurred_at_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "event occurrence time must be positive",
            ));
        }
        match &self.kind {
            DomainEventKind::SnapshotChanged { revision }
            | DomainEventKind::NetworkChanged { revision, .. }
            | DomainEventKind::SecurityPolicyChanged { revision, .. }
            | DomainEventKind::TrafficPolicyChanged { revision } => {
                if *revision == 0 || *revision > self.snapshot_revision {
                    return Err(DomainError::invalid_value(
                        "event revision must be positive and not exceed its snapshot revision",
                    ));
                }
            }
            DomainEventKind::CapabilityChanged { capability_id, .. } => {
                if capability_id.is_empty() || capability_id.len() > 128 {
                    return Err(DomainError::invalid_value("capability id is invalid"));
                }
            }
            DomainEventKind::OperationChanged { .. }
            | DomainEventKind::MembershipChanged { .. }
            | DomainEventKind::AuditAppended { .. } => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventAppendError {
    Invalid(DomainError),
    WrongStream,
    SequenceGap { expected: u64, actual: u64 },
    SnapshotRollback { current: u64, actual: u64 },
    DuplicateIdConflict,
    DuplicateSequenceConflict,
}

impl fmt::Display for EventAppendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => write!(formatter, "{error}"),
            Self::WrongStream => formatter.write_str("event belongs to another stream"),
            Self::SequenceGap { expected, actual } => {
                write!(
                    formatter,
                    "event sequence gap: expected {expected}, got {actual}"
                )
            }
            Self::SnapshotRollback { current, actual } => write!(
                formatter,
                "event snapshot revision moved backwards: current {current}, got {actual}"
            ),
            Self::DuplicateIdConflict => {
                formatter.write_str("an event id was reused with a different event")
            }
            Self::DuplicateSequenceConflict => {
                formatter.write_str("an event sequence was reused with a different event id")
            }
        }
    }
}

impl std::error::Error for EventAppendError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventAppendOutcome {
    Appended,
    DuplicateIgnored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    UnknownStream,
    CursorAhead,
    HistoryGap,
    SnapshotAhead,
    SnapshotRolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplayOutcome {
    Events {
        cursor: EventCursor,
        events: Vec<DomainEvent>,
    },
    ResyncRequired {
        cursor: EventCursor,
        reason: ResyncReason,
    },
}

/// Bounded append-only history for one stream. Sequence numbers are never
/// reused after pruning; an expired cursor therefore transitions to an
/// explicit `HistoryGap` resync rather than silently skipping events.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct EventLog {
    pub stream_id: EventStreamId,
    pub snapshot_revision: u64,
    pub retention_capacity: usize,
    events: BTreeMap<u64, DomainEvent>,
    ids: BTreeMap<EventId, u64>,
}

impl EventLog {
    pub fn new(
        stream_id: EventStreamId,
        snapshot_revision: u64,
        retention_capacity: usize,
    ) -> Self {
        Self {
            stream_id,
            snapshot_revision,
            retention_capacity: retention_capacity.max(1),
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

    pub fn events(&self) -> impl Iterator<Item = &DomainEvent> + '_ {
        self.events.values()
    }

    /// Advance the authoritative snapshot baseline. Snapshot revisions never
    /// move backwards; a replay cursor on an older baseline is resynced.
    pub fn advance_snapshot(&mut self, revision: u64) -> DomainResult<()> {
        if revision < self.snapshot_revision {
            return Err(DomainError::new(
                "domain.snapshot_revision_rollback",
                "snapshot revision moved backwards",
            ));
        }
        self.snapshot_revision = revision;
        Ok(())
    }

    pub fn append(&mut self, event: DomainEvent) -> Result<EventAppendOutcome, EventAppendError> {
        event.validate().map_err(EventAppendError::Invalid)?;
        if event.stream_id != self.stream_id {
            return Err(EventAppendError::WrongStream);
        }
        if let Some(existing_sequence) = self.ids.get(&event.event_id) {
            let existing = &self.events[existing_sequence];
            return if existing == &event {
                Ok(EventAppendOutcome::DuplicateIgnored)
            } else {
                Err(EventAppendError::DuplicateIdConflict)
            };
        }
        if event.snapshot_revision < self.snapshot_revision {
            return Err(EventAppendError::SnapshotRollback {
                current: self.snapshot_revision,
                actual: event.snapshot_revision,
            });
        }
        let expected = self.latest_sequence().checked_add(1).ok_or_else(|| {
            EventAppendError::Invalid(DomainError::invariant("event sequence overflow"))
        })?;
        if event.sequence != expected {
            return Err(EventAppendError::SequenceGap {
                expected,
                actual: event.sequence,
            });
        }
        if self.events.contains_key(&event.sequence) {
            return Err(EventAppendError::DuplicateSequenceConflict);
        }

        self.snapshot_revision = self.snapshot_revision.max(event.snapshot_revision);
        self.ids.insert(event.event_id.clone(), event.sequence);
        self.events.insert(event.sequence, event);
        self.prune();
        Ok(EventAppendOutcome::Appended)
    }

    pub fn replay_after(&self, cursor: &EventCursor) -> ReplayOutcome {
        let resync = |reason| ReplayOutcome::ResyncRequired {
            cursor: self.latest_cursor(),
            reason,
        };
        if cursor.stream_id != self.stream_id {
            return resync(ResyncReason::UnknownStream);
        }
        if cursor.snapshot_revision > self.snapshot_revision {
            return resync(ResyncReason::SnapshotRolledBack);
        }
        if cursor.sequence > self.latest_sequence() {
            return resync(ResyncReason::CursorAhead);
        }
        if cursor.snapshot_revision < self.snapshot_revision {
            return resync(ResyncReason::SnapshotAhead);
        }
        if let Some(oldest) = self.oldest_sequence() {
            if cursor.sequence.saturating_add(1) < oldest {
                return resync(ResyncReason::HistoryGap);
            }
        }

        let first = cursor.sequence.saturating_add(1);
        let mut expected = first;
        let mut events = Vec::new();
        for (sequence, event) in self.events.range(first..) {
            if *sequence != expected {
                return resync(ResyncReason::HistoryGap);
            }
            expected = expected.saturating_add(1);
            events.push(event.clone());
        }
        ReplayOutcome::Events {
            cursor: self.latest_cursor(),
            events,
        }
    }

    fn prune(&mut self) {
        while self.events.len() > self.retention_capacity.max(1) {
            let Some((sequence, event)) = self.events.pop_first() else {
                break;
            };
            self.ids.remove(&event.event_id);
            debug_assert_eq!(sequence, event.sequence);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> EventStreamId {
        EventStreamId::parse("node:test").expect("stream id")
    }

    fn event(sequence: u64, snapshot_revision: u64) -> DomainEvent {
        DomainEvent {
            event_id: EventId::from_public_key(format!("event-{sequence}").as_bytes()),
            stream_id: stream(),
            sequence,
            snapshot_revision,
            occurred_at_unix_ms: sequence,
            kind: DomainEventKind::SnapshotChanged {
                revision: snapshot_revision,
            },
        }
    }

    fn cursor(sequence: u64, snapshot_revision: u64) -> EventCursor {
        EventCursor {
            stream_id: stream(),
            sequence,
            snapshot_revision,
        }
    }

    #[test]
    fn reconnect_replays_strictly_after_the_cursor() {
        let mut log = EventLog::new(stream(), 1, 16);
        log.append(event(1, 1)).expect("first event");
        log.advance_snapshot(2).expect("advance snapshot");
        log.append(event(2, 2)).expect("second event");
        let ReplayOutcome::Events { cursor, events } = log.replay_after(&cursor(1, 2)) else {
            panic!("cursor should replay");
        };
        assert_eq!(cursor.sequence, 2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 2);
    }

    #[test]
    fn duplicate_events_are_idempotent_and_identity_reuse_is_rejected() {
        let mut log = EventLog::new(stream(), 1, 16);
        let first = event(1, 1);
        assert_eq!(log.append(first.clone()), Ok(EventAppendOutcome::Appended));
        assert_eq!(
            log.append(first.clone()),
            Ok(EventAppendOutcome::DuplicateIgnored)
        );
        let mut changed = first;
        changed.occurred_at_unix_ms = 99;
        assert_eq!(
            log.append(changed),
            Err(EventAppendError::DuplicateIdConflict)
        );
    }

    #[test]
    fn append_rejects_sequence_gaps_and_snapshot_rollback() {
        let mut log = EventLog::new(stream(), 2, 16);
        assert_eq!(
            log.append(event(2, 2)),
            Err(EventAppendError::SequenceGap {
                expected: 1,
                actual: 2
            })
        );
        log.append(event(1, 2)).expect("first event");
        assert_eq!(
            log.append(event(2, 1)),
            Err(EventAppendError::SnapshotRollback {
                current: 2,
                actual: 1
            })
        );
    }

    #[test]
    fn retained_prefix_loss_and_forward_cursor_require_resync() {
        let mut log = EventLog::new(stream(), 3, 2);
        for sequence in 1..=3 {
            log.append(event(sequence, 3)).expect("append");
        }
        assert_eq!(log.oldest_sequence(), Some(2));
        assert!(matches!(
            log.replay_after(&cursor(0, 3)),
            ReplayOutcome::ResyncRequired {
                reason: ResyncReason::HistoryGap,
                ..
            }
        ));
        assert!(matches!(
            log.replay_after(&cursor(9, 3)),
            ReplayOutcome::ResyncRequired {
                reason: ResyncReason::CursorAhead,
                ..
            }
        ));
    }

    #[test]
    fn unknown_stream_and_snapshot_mismatch_return_explicit_resync() {
        let mut log = EventLog::new(stream(), 2, 16);
        log.append(event(1, 2)).expect("append");
        assert!(matches!(
            log.replay_after(&EventCursor {
                stream_id: EventStreamId::parse("node:other").unwrap(),
                sequence: 0,
                snapshot_revision: 2,
            }),
            ReplayOutcome::ResyncRequired {
                reason: ResyncReason::UnknownStream,
                ..
            }
        ));
        assert!(matches!(
            log.replay_after(&cursor(1, 1)),
            ReplayOutcome::ResyncRequired {
                reason: ResyncReason::SnapshotAhead,
                ..
            }
        ));
        assert!(matches!(
            log.replay_after(&cursor(1, 3)),
            ReplayOutcome::ResyncRequired {
                reason: ResyncReason::SnapshotRolledBack,
                ..
            }
        ));
    }

    #[test]
    fn snapshot_revision_never_rolls_back() {
        let mut log = EventLog::new(stream(), 2, 16);
        assert_eq!(
            log.advance_snapshot(1).unwrap_err().code(),
            "domain.snapshot_revision_rollback"
        );
        log.advance_snapshot(3).unwrap();
        assert_eq!(log.latest_cursor().snapshot_revision, 3);
    }
}
