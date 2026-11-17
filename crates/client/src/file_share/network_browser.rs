//! Requester-side directory browsing.
//!
//! Browsing is deliberately non-persistent and separate from transfer state:
//! each request is a one-shot, bounded RPC bound to an authenticated peer,
//! logical share path and requester-local completion channel. A timeout,
//! authorization loss, malformed response or mismatched peer fails the request
//! closed and never creates resumable transfer state.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use fuckipv6_common::domain::{FileShareId, SharedPath};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{oneshot, Notify};

use super::network::{
    FileNetworkDirectoryEntry, FileNetworkFrame, FileNetworkRequest, FileNetworkResponse,
    MAX_FILE_NETWORK_DIRECTORY_CURSOR_LEN,
};
use super::root::MAX_DIRECTORY_LIST_LIMIT;
use super::OriginPeer;

pub const DEFAULT_FILE_DIRECTORY_LIST_LIMIT: u32 = 100;
pub(crate) const FILE_DIRECTORY_BROWSE_TTL_MS: u64 = 15_000;
pub(crate) const MAX_PENDING_FILE_DIRECTORY_BROWSERS: usize = 16;

fn default_directory_limit() -> u32 {
    DEFAULT_FILE_DIRECTORY_LIST_LIMIT
}

/// Local-only request for one requester-side directory page.
///
/// `path` is a logical path inside the remote share. It is never interpreted as
/// an owner-local filesystem path by the requester or any UI adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFileDirectoryRequest {
    pub peer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    pub share_id: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_directory_limit")]
    pub limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// User-safe directory entry returned by the authenticated owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDirectoryEntry {
    pub path: SharedPath,
    pub is_directory: bool,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at_unix_ms: Option<u64>,
}

/// One bounded directory page. `next_cursor` is opaque to callers and valid
/// only for the same directory generation and authorization context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDirectoryListing {
    pub entries: Vec<FileDirectoryEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum FileDirectoryBrowserError {
    #[error("File directory browser capacity is exhausted")]
    Capacity,
    #[error("File directory request is invalid")]
    InvalidRequest,
    #[error("File directory request is unknown")]
    UnknownRequest,
    #[error("File directory response came from the wrong peer")]
    PeerMismatch,
}

impl FileDirectoryBrowserError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Capacity => "file_browse_limit",
            Self::InvalidRequest => "file_browse_invalid_request",
            Self::UnknownRequest => "file_browse_request_unknown",
            Self::PeerMismatch => "file_browse_peer_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrowserRequestHandle {
    pub(crate) request_id: [u8; 16],
}

#[derive(Debug)]
pub(crate) struct BrowserOutbound {
    pub(crate) peer: OriginPeer,
    pub(crate) actor_access_epoch: u64,
    pub(crate) frame: FileNetworkFrame,
}

struct PendingDirectoryRequest {
    peer: OriginPeer,
    path: SharedPath,
    limit: u32,
    deadline_unix_ms: u64,
    reply: oneshot::Sender<Result<FileDirectoryListing, String>>,
}

/// One bounded table of non-persistent directory requests.
pub(crate) struct FileNetworkBrowser {
    pending: BTreeMap<[u8; 16], PendingDirectoryRequest>,
    outbound: VecDeque<BrowserOutbound>,
    notify: Arc<Notify>,
}

impl Default for FileNetworkBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl FileNetworkBrowser {
    pub(crate) fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            outbound: VecDeque::new(),
            notify: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn outbound_notify(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn owns_request(&self, request_id: &[u8; 16]) -> bool {
        self.pending.contains_key(request_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_list(
        &mut self,
        peer: OriginPeer,
        actor_access_epoch: u64,
        share_id: FileShareId,
        path: SharedPath,
        limit: u32,
        cursor: Option<String>,
        now_unix_ms: u64,
        reply: &mut Option<oneshot::Sender<Result<FileDirectoryListing, String>>>,
    ) -> Result<BrowserRequestHandle, FileDirectoryBrowserError> {
        if actor_access_epoch == 0
            || limit == 0
            || limit > MAX_DIRECTORY_LIST_LIMIT
            || self.pending.len() >= MAX_PENDING_FILE_DIRECTORY_BROWSERS
        {
            return Err(
                if self.pending.len() >= MAX_PENDING_FILE_DIRECTORY_BROWSERS {
                    FileDirectoryBrowserError::Capacity
                } else {
                    FileDirectoryBrowserError::InvalidRequest
                },
            );
        }

        let request_id = self.random_request_id()?;
        let request = FileNetworkRequest::ListDirectory {
            share_id,
            path: path.clone(),
            limit,
            cursor,
        };
        let frame = FileNetworkFrame::request(request_id, request)
            .map_err(|_| FileDirectoryBrowserError::InvalidRequest)?;
        let deadline_unix_ms = now_unix_ms.saturating_add(FILE_DIRECTORY_BROWSE_TTL_MS);
        let Some(reply) = reply.take() else {
            return Err(FileDirectoryBrowserError::InvalidRequest);
        };
        self.pending.insert(
            request_id,
            PendingDirectoryRequest {
                peer: peer.clone(),
                path,
                limit,
                deadline_unix_ms,
                reply,
            },
        );
        self.outbound.push_back(BrowserOutbound {
            peer,
            actor_access_epoch,
            frame,
        });
        self.notify.notify_one();
        Ok(BrowserRequestHandle { request_id })
    }

    pub(crate) fn pop_outbound(&mut self) -> Option<BrowserOutbound> {
        self.outbound.pop_front()
    }

    pub(crate) fn fail_request(&mut self, request_id: &[u8; 16], code: &str) {
        if let Some(pending) = self.pending.remove(request_id) {
            let _ = pending.reply.send(Err(code.to_owned()));
            self.remove_queued_outbound(request_id);
        }
    }

    pub(crate) fn handle_response(
        &mut self,
        peer: &OriginPeer,
        request_id: [u8; 16],
        response: FileNetworkResponse,
        now_unix_ms: u64,
    ) -> Result<(), FileDirectoryBrowserError> {
        // Do not rely on the expiry tick to race the response path: a response
        // observed at or after the absolute deadline fails closed immediately.
        self.expire(now_unix_ms);
        let Some(pending) = self.pending.get(&request_id) else {
            return Err(FileDirectoryBrowserError::UnknownRequest);
        };
        if pending.peer != *peer {
            return Err(FileDirectoryBrowserError::PeerMismatch);
        }
        let Some(pending) = self.pending.remove(&request_id) else {
            return Err(FileDirectoryBrowserError::UnknownRequest);
        };
        self.remove_queued_outbound(&request_id);
        let result = directory_response(&pending, response);
        let _ = pending.reply.send(result);
        Ok(())
    }

    /// Expire one request at its absolute deadline. Late responses are rejected
    /// as unknown and cannot complete a newer request because IDs are random.
    pub(crate) fn expire(&mut self, now_unix_ms: u64) -> usize {
        let expired = self
            .pending
            .iter()
            .filter_map(|(request_id, pending)| {
                (now_unix_ms >= pending.deadline_unix_ms).then_some(*request_id)
            })
            .collect::<Vec<_>>();
        for request_id in &expired {
            self.fail_request(request_id, "file_browse_timeout");
        }
        expired.len()
    }

    fn remove_queued_outbound(&mut self, request_id: &[u8; 16]) {
        self.outbound.retain(|outbound| {
            !matches!(
                &outbound.frame,
                FileNetworkFrame::Request {
                    request_id: queued_id,
                    ..
                } if queued_id == request_id
            )
        });
    }

    fn random_request_id(&self) -> Result<[u8; 16], FileDirectoryBrowserError> {
        for _ in 0..8 {
            let mut request_id = [0u8; 16];
            getrandom::fill(&mut request_id)
                .map_err(|_| FileDirectoryBrowserError::InvalidRequest)?;
            if request_id != [0; 16] && !self.pending.contains_key(&request_id) {
                return Ok(request_id);
            }
        }
        Err(FileDirectoryBrowserError::InvalidRequest)
    }
}

fn directory_response(
    pending: &PendingDirectoryRequest,
    response: FileNetworkResponse,
) -> Result<FileDirectoryListing, String> {
    match response {
        FileNetworkResponse::Error { code, .. } => Err(code),
        FileNetworkResponse::DirectoryEntries {
            entries,
            next_cursor,
            truncated,
        } => {
            if entries.len() > pending.limit as usize
                || truncated != next_cursor.is_some()
                || next_cursor.as_deref().is_some_and(|cursor| {
                    cursor.is_empty()
                        || cursor.len() > MAX_FILE_NETWORK_DIRECTORY_CURSOR_LEN
                        || !cursor.bytes().all(|byte| byte.is_ascii_digit())
                })
            {
                return Err("file_browse_invalid_response".to_owned());
            }
            let mut previous: Option<&SharedPath> = None;
            for entry in &entries {
                if entry.path.parent().as_ref() != Some(&pending.path)
                    || entry.path == pending.path
                    || entry.is_directory && entry.size != 0
                    || previous.is_some_and(|previous| previous >= &entry.path)
                {
                    return Err("file_browse_invalid_response".to_owned());
                }
                previous = Some(&entry.path);
            }
            Ok(FileDirectoryListing {
                entries: entries.into_iter().map(directory_entry).collect(),
                next_cursor,
                truncated,
            })
        }
        _ => Err("file_browse_invalid_response".to_owned()),
    }
}

fn directory_entry(entry: FileNetworkDirectoryEntry) -> FileDirectoryEntry {
    FileDirectoryEntry {
        path: entry.path,
        is_directory: entry.is_directory,
        size: entry.size,
        modified_at_unix_ms: entry.modified_at_unix_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::NodeId;

    fn peer(byte: u8) -> OriginPeer {
        OriginPeer {
            node_id: NodeId::from_public_key(&[byte]),
            virtual_ip: std::net::Ipv6Addr::LOCALHOST,
        }
    }

    fn share() -> FileShareId {
        FileShareId::from_public_key(b"share")
    }

    fn start(
        browser: &mut FileNetworkBrowser,
        reply: oneshot::Sender<Result<FileDirectoryListing, String>>,
    ) -> BrowserRequestHandle {
        browser
            .start_list(
                peer(1),
                7,
                share(),
                SharedPath::parse("docs").expect("path"),
                2,
                None,
                1_000,
                &mut Some(reply),
            )
            .expect("request")
    }

    fn response() -> FileNetworkResponse {
        FileNetworkResponse::DirectoryEntries {
            entries: vec![
                FileNetworkDirectoryEntry {
                    path: SharedPath::parse("docs/a.txt").expect("path"),
                    is_directory: false,
                    size: 1,
                    modified_at_unix_ms: None,
                },
                FileNetworkDirectoryEntry {
                    path: SharedPath::parse("docs/b").expect("path"),
                    is_directory: true,
                    size: 0,
                    modified_at_unix_ms: None,
                },
            ],
            next_cursor: None,
            truncated: false,
        }
    }

    #[test]
    fn directory_browser_is_bounded_and_returns_only_matching_peer_responses() {
        let mut browser = FileNetworkBrowser::new();
        let mut replies = Vec::new();
        let mut handles = Vec::new();
        for _ in 0..MAX_PENDING_FILE_DIRECTORY_BROWSERS {
            let (reply, response) = oneshot::channel();
            replies.push(response);
            handles.push(start(&mut browser, reply));
        }
        let (reply, _response) = oneshot::channel();
        assert_eq!(
            browser.start_list(
                peer(1),
                7,
                share(),
                SharedPath::root(),
                1,
                None,
                1_000,
                &mut Some(reply),
            ),
            Err(FileDirectoryBrowserError::Capacity)
        );
        assert_eq!(browser.pending_count(), MAX_PENDING_FILE_DIRECTORY_BROWSERS);

        let request_id = handles[0].request_id;
        assert_eq!(
            browser.handle_response(&peer(2), request_id, response(), 1_001),
            Err(FileDirectoryBrowserError::PeerMismatch)
        );
        assert!(browser.owns_request(&request_id));
        browser
            .handle_response(&peer(1), request_id, response(), 1_001)
            .expect("matching response");
        assert_eq!(
            replies[0]
                .try_recv()
                .expect("completed")
                .expect("listing")
                .entries[0]
                .path
                .as_str(),
            "docs/a.txt"
        );
    }

    #[test]
    fn directory_browser_rejects_wrong_shape_and_expires_cleanly() {
        let mut browser = FileNetworkBrowser::new();
        let (reply, mut response_rx) = oneshot::channel();
        let handle = start(&mut browser, reply);
        let invalid = FileNetworkResponse::DirectoryEntries {
            entries: vec![FileNetworkDirectoryEntry {
                path: SharedPath::parse("docs/nested/outside").expect("path"),
                is_directory: false,
                size: 1,
                modified_at_unix_ms: None,
            }],
            next_cursor: None,
            truncated: false,
        };
        browser
            .handle_response(&peer(1), handle.request_id, invalid, 1_001)
            .expect("associated response");
        assert_eq!(
            response_rx.try_recv().expect("completed"),
            Err("file_browse_invalid_response".to_owned())
        );

        let (reply, mut response_rx) = oneshot::channel();
        let handle = start(&mut browser, reply);
        assert_eq!(browser.expire(1_000 + FILE_DIRECTORY_BROWSE_TTL_MS), 1);
        assert_eq!(
            response_rx.try_recv().expect("completed"),
            Err("file_browse_timeout".to_owned())
        );
        assert!(!browser.owns_request(&handle.request_id));
    }

    #[test]
    fn directory_browser_deadline_rejects_late_response_and_clears_queue() {
        let mut browser = FileNetworkBrowser::new();
        let (reply, mut response_rx) = oneshot::channel();
        let handle = start(&mut browser, reply);
        let deadline = 1_000 + FILE_DIRECTORY_BROWSE_TTL_MS;

        assert_eq!(
            browser.handle_response(&peer(1), handle.request_id, response(), deadline),
            Err(FileDirectoryBrowserError::UnknownRequest)
        );
        assert_eq!(
            response_rx.try_recv().expect("completed"),
            Err("file_browse_timeout".to_owned())
        );
        assert!(!browser.owns_request(&handle.request_id));
        assert!(browser.pop_outbound().is_none());
    }

    #[test]
    fn directory_browser_expiry_skips_stale_outbound_request() {
        let mut browser = FileNetworkBrowser::new();
        let (stale_reply, mut stale_response) = oneshot::channel();
        let stale = start(&mut browser, stale_reply);
        let (fresh_reply, _fresh_response) = oneshot::channel();
        let fresh = browser
            .start_list(
                peer(1),
                7,
                share(),
                SharedPath::root(),
                1,
                None,
                20_000,
                &mut Some(fresh_reply),
            )
            .expect("fresh request");

        assert_eq!(browser.expire(1_000 + FILE_DIRECTORY_BROWSE_TTL_MS), 1);
        assert_eq!(
            stale_response.try_recv().expect("completed"),
            Err("file_browse_timeout".to_owned())
        );
        assert!(!browser.owns_request(&stale.request_id));

        let outbound = browser.pop_outbound().expect("fresh outbound");
        assert!(matches!(
            outbound.frame,
            FileNetworkFrame::Request { request_id, .. } if request_id == fresh.request_id
        ));
        assert!(browser.pop_outbound().is_none());
    }

    #[test]
    fn directory_browser_accepts_only_directory_or_error_responses() {
        let mut browser = FileNetworkBrowser::new();
        let (reply, mut response_rx) = oneshot::channel();
        let handle = start(&mut browser, reply);
        browser
            .handle_response(
                &peer(1),
                handle.request_id,
                FileNetworkResponse::Completed {
                    progress: super::super::network::FileNetworkProgress {
                        state: fuckipv6_common::domain::TransferState::Completed,
                        transferred_bytes: 1,
                        total_bytes: 1,
                        received_chunks: 1,
                        total_chunks: 1,
                    },
                },
                1_001,
            )
            .expect("associated response");
        assert_eq!(
            response_rx.try_recv().expect("completed"),
            Err("file_browse_invalid_response".to_owned())
        );
    }

    #[test]
    fn directory_browser_request_debug_contains_only_logical_paths() {
        let request = ListFileDirectoryRequest {
            peer: peer(1).node_id.as_str().to_owned(),
            network_id: Some("network".into()),
            share_id: share().as_str().to_owned(),
            path: "docs".into(),
            limit: DEFAULT_FILE_DIRECTORY_LIST_LIMIT,
            cursor: None,
        };
        let debug = format!("{request:?}");
        assert!(debug.contains("docs"));
        assert!(!debug.contains("/home/"));
    }
}
