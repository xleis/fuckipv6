use std::collections::BTreeMap;

use fuckipv6_common::domain::{TransferDirection, TransferState};
use serde::{Deserialize, Serialize};

use super::network_origin::OriginTransferEvent;

pub(crate) const MAX_FILE_TRANSFER_STATUS_ENTRIES: usize = 256;
pub(crate) const FILE_TRANSFER_TERMINAL_STATUS_TTL_MS: u64 = 5 * 60 * 1_000;

/// Stable, path-free progress returned by requester-side File adapters.
///
/// This type intentionally mirrors only fields that are safe for every UI:
/// authenticated operation identifiers, transfer direction/state, byte and
/// chunk counters, and a bounded failure classification. Local filesystem
/// paths, peer addresses and wire frames never enter this projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileTransferStatus {
    pub transfer_id: String,
    pub operation_id: String,
    pub direction: TransferDirection,
    pub state: TransferState,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub received_chunks: u32,
    pub total_chunks: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    pub updated_at_unix_ms: u64,
}

impl FileTransferStatus {
    fn from_event(event: &OriginTransferEvent, now_unix_ms: u64) -> Self {
        Self {
            transfer_id: event.transfer_id.as_str().to_owned(),
            operation_id: event.operation_id.as_str().to_owned(),
            direction: event.direction,
            state: event.state,
            transferred_bytes: event.transferred_bytes,
            total_bytes: event.total_bytes,
            received_chunks: event.received_chunks,
            total_chunks: event.total_chunks,
            error_code: event.error_code.clone(),
            failure_kind: event.failure_kind().map(|kind| kind.as_str().to_owned()),
            updated_at_unix_ms: now_unix_ms,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }
}

/// Bounded latest-value projection of transfer events owned by the runtime
/// event loop. Active operations remain visible; terminal operations expire
/// after a short window and the oldest terminal entries are evicted first.
pub(crate) struct FileTransferStatusCache {
    statuses: BTreeMap<(String, String), FileTransferStatus>,
}

impl Default for FileTransferStatusCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTransferStatusCache {
    pub(crate) fn new() -> Self {
        Self {
            statuses: BTreeMap::new(),
        }
    }

    pub(crate) fn ingest(&mut self, event: &OriginTransferEvent, now_unix_ms: u64) {
        let key = (
            event.transfer_id.as_str().to_owned(),
            event.operation_id.as_str().to_owned(),
        );
        let next = FileTransferStatus::from_event(event, now_unix_ms);
        if let Some(current) = self.statuses.get(&key) {
            // A late non-terminal frame must never roll a terminal operation
            // back into an actionable state. Authenticated response and retry
            // ordering normally prevents this, but the cache fails closed too.
            if current.is_terminal() && !next.is_terminal() {
                return;
            }
        }
        self.statuses.insert(key, next);
        self.prune(now_unix_ms);
    }

    pub(crate) fn list(&mut self, now_unix_ms: u64) -> Vec<FileTransferStatus> {
        self.prune(now_unix_ms);
        let mut statuses = self.statuses.values().cloned().collect::<Vec<_>>();
        statuses.sort_by(|left, right| {
            right
                .updated_at_unix_ms
                .cmp(&left.updated_at_unix_ms)
                .then_with(|| right.transfer_id.cmp(&left.transfer_id))
                .then_with(|| right.operation_id.cmp(&left.operation_id))
        });
        statuses
    }

    #[cfg(test)]
    pub(crate) fn get(
        &mut self,
        transfer_id: &str,
        operation_id: &str,
        now_unix_ms: u64,
    ) -> Option<FileTransferStatus> {
        self.prune(now_unix_ms);
        self.statuses
            .get(&(transfer_id.to_owned(), operation_id.to_owned()))
            .cloned()
    }

    fn prune(&mut self, now_unix_ms: u64) {
        self.statuses.retain(|_, status| {
            !status.is_terminal()
                || now_unix_ms.saturating_sub(status.updated_at_unix_ms)
                    < FILE_TRANSFER_TERMINAL_STATUS_TTL_MS
        });

        while self.statuses.len() > MAX_FILE_TRANSFER_STATUS_ENTRIES {
            let oldest_terminal = self
                .statuses
                .iter()
                .filter(|(_, status)| status.is_terminal())
                .min_by(|(left_key, left), (right_key, right)| {
                    left.updated_at_unix_ms
                        .cmp(&right.updated_at_unix_ms)
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone());
            let Some(key) = oldest_terminal else {
                break;
            };
            self.statuses.remove(&key);
        }

        // The active-operation limit is 16, so this is unreachable unless an
        // internal invariant is broken. Still bound memory even then.
        while self.statuses.len() > MAX_FILE_TRANSFER_STATUS_ENTRIES {
            let Some(oldest_key) = self
                .statuses
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    left.updated_at_unix_ms
                        .cmp(&right.updated_at_unix_ms)
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.statuses.remove(&oldest_key);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.statuses.len()
    }
}

#[cfg(test)]
mod tests {
    use fuckipv6_common::domain::{OperationId, TransferId};

    use super::*;

    fn transfer(index: u16) -> TransferId {
        TransferId::from_public_key(&index.to_le_bytes())
    }

    fn operation(index: u16) -> OperationId {
        OperationId::from_public_key(&index.to_le_bytes())
    }

    fn event(index: u16, state: TransferState) -> OriginTransferEvent {
        OriginTransferEvent {
            transfer_id: transfer(index),
            operation_id: operation(index),
            direction: TransferDirection::Upload,
            state,
            transferred_bytes: u64::from(index),
            total_bytes: 100,
            received_chunks: u32::from(index),
            total_chunks: 10,
            error_code: (state == TransferState::Failed)
                .then(|| "file_transfer_send_failed".to_owned()),
        }
    }

    #[test]
    fn latest_event_replaces_prior_progress() {
        let mut cache = FileTransferStatusCache::new();
        cache.ingest(&event(1, TransferState::Queued), 1_000);
        cache.ingest(&event(1, TransferState::Active), 1_100);

        let status = cache.get(transfer(1).as_str(), operation(1).as_str(), 1_200);
        assert_eq!(status.expect("status").state, TransferState::Active);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn terminal_status_does_not_roll_back_and_expires() {
        let mut cache = FileTransferStatusCache::new();
        cache.ingest(&event(1, TransferState::Failed), 1_000);
        cache.ingest(&event(1, TransferState::Active), 1_100);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache
                .get(transfer(1).as_str(), operation(1).as_str(), 1_200)
                .expect("failed status")
                .failure_kind
                .as_deref(),
            Some("network")
        );
        assert!(cache
            .get(
                transfer(1).as_str(),
                operation(1).as_str(),
                1_000 + FILE_TRANSFER_TERMINAL_STATUS_TTL_MS,
            )
            .is_none());
    }

    #[test]
    fn cache_evicts_oldest_terminal_entries_but_keeps_active_statuses() {
        let mut cache = FileTransferStatusCache::new();
        for index in 0..=MAX_FILE_TRANSFER_STATUS_ENTRIES {
            cache.ingest(
                &event(index as u16, TransferState::Completed),
                10_000 + index as u64,
            );
        }
        assert_eq!(cache.len(), MAX_FILE_TRANSFER_STATUS_ENTRIES);

        cache.ingest(&event(999, TransferState::Active), 20_000);
        let statuses = cache.list(20_000);
        assert_eq!(statuses.len(), MAX_FILE_TRANSFER_STATUS_ENTRIES);
        assert!(statuses.iter().any(|status| {
            status.transfer_id == transfer(999).as_str() && status.state == TransferState::Active
        }));
    }

    #[test]
    fn public_status_json_is_path_free_and_strict() {
        let mut cache = FileTransferStatusCache::new();
        cache.ingest(&event(7, TransferState::Failed), 10_000);
        let status = cache
            .get(transfer(7).as_str(), operation(7).as_str(), 10_001)
            .expect("status");
        let value = serde_json::to_value(&status).expect("serialize status");
        let json = value.to_string();

        for forbidden in [
            "local_path",
            "/home/",
            "peer",
            "address",
            "socket",
            "frame",
            "checksum",
        ] {
            assert!(
                !json.contains(forbidden),
                "status leaked {forbidden}: {json}"
            );
        }

        let mut unknown = value;
        unknown
            .as_object_mut()
            .expect("status object")
            .insert("local_path".to_owned(), serde_json::json!("/private/file"));
        let error = serde_json::from_value::<FileTransferStatus>(unknown)
            .expect_err("unknown field must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }
}
