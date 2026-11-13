//! Native File requester workspace state.
//!
//! This module contains no GPUI entities and performs no I/O. It derives the
//! bounded target list from the authoritative node snapshot, serializes
//! directory requests, validates logical paths and protects every asynchronous
//! completion with a generation token. Requester-local filesystem paths remain
//! in this desktop-only state and are submitted only through the same-user
//! local IPC adapter.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fuckipv6_common::domain::{FileShareId, SharedPath, TransferDirection, TransferState};

use crate::file_share::{
    FileDirectoryEntry, FileDirectoryListing, FileTransferStatus, ListFileDirectoryRequest,
    StartFileTransferRequest, DEFAULT_FILE_DIRECTORY_LIST_LIMIT, DEFAULT_FILE_TRANSFER_CHUNK_SIZE,
};
use crate::node::NodeSnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuiFileTarget {
    pub peer: String,
    pub network_id: String,
    pub network_name: String,
    pub share_id: String,
}

impl GpuiFileTarget {
    pub fn key(&self) -> String {
        format!("{}\0{}", self.network_id, self.peer)
    }
}

/// Derive just the local selector values needed by the requester. The remote
/// owner still authorizes every directory page and transfer using its current
/// capability, signed share revision, ACL and member epoch.
pub(crate) fn file_targets_from_snapshot(snapshot: &NodeSnapshot) -> Vec<GpuiFileTarget> {
    let memberships = snapshot
        .memberships
        .iter()
        .filter(|membership| {
            !membership.network_name.trim().is_empty() && !membership.network_id.trim().is_empty()
        })
        .map(|membership| {
            (
                membership.network_name.as_str(),
                membership.network_id.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut targets = BTreeMap::new();
    for peer in &snapshot.peers {
        if !peer.online || peer.peer_id.trim().is_empty() {
            continue;
        }
        let Some(network_id) = memberships.get(peer.network_name.as_str()).copied() else {
            continue;
        };
        let share_id = FileShareId::from_public_key(
            format!("file-share:{}:{}", peer.peer_id, network_id).as_bytes(),
        );
        let target = GpuiFileTarget {
            peer: peer.peer_id.clone(),
            network_id: network_id.to_owned(),
            network_name: peer.network_name.clone(),
            share_id: share_id.as_str().to_owned(),
        };
        targets.insert(
            (
                peer.network_name.clone(),
                peer.peer_id.clone(),
                network_id.to_owned(),
            ),
            target,
        );
    }
    targets.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuiDirectoryRequest {
    pub generation: u64,
    pub target_key: String,
    pub path: String,
    pub append: bool,
    pub request: ListFileDirectoryRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GpuiFileSelectionError {
    TargetUnavailable,
    MissingFileName,
    NonUtf8LocalPath,
    InvalidSharedPath,
    OperationPending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuiPendingUpload {
    pub local_path: PathBuf,
    pub remote_path: SharedPath,
}

impl GpuiPendingUpload {
    pub fn name(&self) -> &str {
        self.remote_path
            .as_str()
            .rsplit_once('/')
            .map_or(self.remote_path.as_str(), |(_, name)| name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuiUploadRequest {
    pub generation: u64,
    pub target_key: String,
    pub name: String,
    pub request: StartFileTransferRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuiDownloadRequest {
    pub generation: u64,
    pub target_key: String,
    pub name: String,
    pub request: StartFileTransferRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuiFileControl {
    Pause,
    Resume,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GpuiPendingFileControl {
    pub generation: u64,
    pub transfer_id: String,
    pub operation_id: String,
    pub action: GpuiFileControl,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GpuiFileWorkspace {
    pub targets: Vec<GpuiFileTarget>,
    pub selected_target: Option<GpuiFileTarget>,
    pub directory_path: String,
    pub directory_entries: Vec<FileDirectoryEntry>,
    pub directory_loaded: bool,
    pub directory_loading: bool,
    pub directory_loading_more: bool,
    pub directory_next_cursor: Option<String>,
    pub directory_truncated: bool,
    pub directory_error: Option<String>,
    pub upload: Option<GpuiPendingUpload>,
    pub picking_upload: bool,
    pub uploading: bool,
    upload_transfer_id: Option<String>,
    upload_operation_id: Option<String>,
    pub upload_error: Option<String>,
    pub downloading_path: Option<String>,
    download_transfer_id: Option<String>,
    download_operation_id: Option<String>,
    pub picking_download: bool,
    pub download_error: Option<String>,
    pub transfers: Vec<FileTransferStatus>,
    pub transfers_loaded: bool,
    pub transfers_loading: bool,
    pub transfers_error: Option<String>,
    pub control: Option<GpuiPendingFileControl>,
    pub notice: Option<String>,
    directory_generation: u64,
    upload_generation: u64,
    download_generation: u64,
    transfer_generation: u64,
    control_generation: u64,
}

impl GpuiFileWorkspace {
    /// Reconcile the selector with the latest snapshot. Keeping the same
    /// target preserves the current directory and upload draft; disappearing
    /// targets invalidate every request tied to the old selector.
    pub fn sync_targets(
        &mut self,
        targets: Vec<GpuiFileTarget>,
        preferred_network: Option<&str>,
    ) -> bool {
        let previous_key = self.selected_target.as_ref().map(GpuiFileTarget::key);
        let next_selected = previous_key
            .as_ref()
            .and_then(|key| targets.iter().find(|target| &target.key() == key))
            .cloned()
            .or_else(|| {
                targets
                    .iter()
                    .find(|target| {
                        preferred_network.is_some_and(|network| {
                            target.network_name == network || target.network_id == network
                        })
                    })
                    .or_else(|| targets.first())
                    .cloned()
            });
        let next_key = next_selected.as_ref().map(GpuiFileTarget::key);
        self.targets = targets;
        if previous_key == next_key {
            self.selected_target = next_selected;
            return false;
        }
        self.selected_target = next_selected;
        self.reset_target_bound_state();
        true
    }

    pub fn select_target(&mut self, key: &str) -> bool {
        let Some(target) = self.targets.iter().find(|target| target.key() == key) else {
            return false;
        };
        if self
            .selected_target
            .as_ref()
            .is_some_and(|selected| selected.key() == key)
        {
            return false;
        }
        self.selected_target = Some(target.clone());
        self.reset_target_bound_state();
        true
    }

    pub fn navigate_directory(&mut self, path: impl Into<String>) -> bool {
        let Ok(path) = SharedPath::parse(path.into()) else {
            return false;
        };
        if self.selected_target.is_none() {
            return false;
        }
        self.directory_path = path.as_str().to_owned();
        self.clear_directory();
        self.directory_error = None;
        self.upload_error = None;
        self.download_error = None;
        self.notice = None;
        true
    }

    pub fn needs_directory_load(&self) -> bool {
        self.selected_target.is_some()
            && !self.directory_loaded
            && !self.directory_loading
            && self.directory_error.is_none()
    }

    pub fn begin_directory(&mut self, append: bool) -> Option<GpuiDirectoryRequest> {
        let target = self.selected_target.clone()?;
        if self.directory_loading || self.directory_loading_more {
            return None;
        }
        let cursor = if append {
            Some(self.directory_next_cursor.clone()?)
        } else {
            None
        };
        if !append {
            self.clear_directory();
        }
        self.directory_error = None;
        self.directory_loading = !append;
        self.directory_loading_more = append;
        self.directory_generation = self.directory_generation.wrapping_add(1).max(1);
        let generation = self.directory_generation;
        let path = self.directory_path.clone();
        Some(GpuiDirectoryRequest {
            generation,
            target_key: target.key(),
            path: path.clone(),
            append,
            request: ListFileDirectoryRequest {
                peer: target.peer,
                network_id: Some(target.network_id),
                share_id: target.share_id,
                path,
                limit: DEFAULT_FILE_DIRECTORY_LIST_LIMIT,
                cursor,
            },
        })
    }

    pub fn finish_directory(
        &mut self,
        request: &GpuiDirectoryRequest,
        result: Result<FileDirectoryListing, String>,
    ) -> bool {
        if !self.directory_request_is_current(request) {
            return false;
        }
        if request.append {
            self.directory_loading_more = false;
        } else {
            self.directory_loading = false;
        }
        match result {
            Ok(listing) => {
                if request.append {
                    for entry in listing.entries {
                        if !self
                            .directory_entries
                            .iter()
                            .any(|current| current.path == entry.path)
                        {
                            self.directory_entries.push(entry);
                        }
                    }
                } else {
                    self.directory_entries = listing.entries;
                }
                self.directory_next_cursor = listing.next_cursor;
                self.directory_truncated = listing.truncated;
                self.directory_loaded = true;
                self.directory_error = None;
            }
            Err(error) => {
                self.directory_error = Some(error);
                self.directory_loaded = false;
            }
        }
        true
    }

    pub fn can_load_more(&self) -> bool {
        self.directory_loaded
            && self.directory_next_cursor.is_some()
            && !self.directory_loading
            && !self.directory_loading_more
    }

    pub fn select_upload_file(
        &mut self,
        local_path: PathBuf,
    ) -> Result<(), GpuiFileSelectionError> {
        if self.uploading || self.picking_upload {
            return Err(GpuiFileSelectionError::OperationPending);
        }
        if self.selected_target.is_none() {
            return Err(GpuiFileSelectionError::TargetUnavailable);
        }
        let name = local_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or(GpuiFileSelectionError::MissingFileName)?;
        if local_path.to_str().is_none() {
            return Err(GpuiFileSelectionError::NonUtf8LocalPath);
        }
        let remote_path = child_shared_path(&self.directory_path, name)
            .ok_or(GpuiFileSelectionError::InvalidSharedPath)?;
        self.upload_generation = self.upload_generation.wrapping_add(1).max(1);
        self.upload = Some(GpuiPendingUpload {
            local_path,
            remote_path,
        });
        self.upload_error = None;
        self.notice = None;
        Ok(())
    }

    pub fn begin_upload_pick(&mut self) -> bool {
        if self.selected_target.is_none() || self.uploading || self.picking_upload {
            return false;
        }
        self.picking_upload = true;
        self.upload_error = None;
        self.notice = None;
        true
    }

    pub fn finish_upload_pick(&mut self) {
        self.picking_upload = false;
    }

    pub fn begin_upload(
        &mut self,
        transfer_id: String,
        operation_id: String,
    ) -> Option<GpuiUploadRequest> {
        if self.uploading {
            return None;
        }
        let target = self.selected_target.clone()?;
        let upload = self.upload.clone()?;
        let local_path = upload.local_path.to_str()?.to_owned();
        self.upload_generation = self.upload_generation.wrapping_add(1).max(1);
        self.upload_transfer_id = Some(transfer_id.clone());
        self.upload_operation_id = Some(operation_id.clone());
        self.uploading = true;
        self.upload_error = None;
        self.notice = None;
        Some(GpuiUploadRequest {
            generation: self.upload_generation,
            target_key: target.key(),
            name: upload.name().to_owned(),
            request: StartFileTransferRequest {
                peer: target.peer,
                network_id: Some(target.network_id),
                direction: crate::file_share::FileTransferDirection::Upload,
                share_id: target.share_id,
                transfer_id,
                operation_id,
                path: upload.remote_path.as_str().to_owned(),
                local_path,
                chunk_size: DEFAULT_FILE_TRANSFER_CHUNK_SIZE,
            },
        })
    }

    pub fn finish_upload_start(
        &mut self,
        generation: u64,
        target_key: &str,
        transfer_id: &str,
        operation_id: &str,
        result: Result<(), String>,
    ) -> Option<bool> {
        if generation != self.upload_generation
            || self
                .selected_target
                .as_ref()
                .is_none_or(|target| target.key() != target_key)
        {
            return None;
        }
        if !self.upload_request_identity_is_current(transfer_id, operation_id) {
            return None;
        }
        self.upload_transfer_id = None;
        self.upload_operation_id = None;
        self.uploading = false;
        match result {
            Ok(()) => {
                self.upload = None;
                self.upload_error = None;
                Some(true)
            }
            Err(error) => {
                self.upload_error = Some(error);
                Some(false)
            }
        }
    }

    pub fn begin_download(
        &mut self,
        entry: &FileDirectoryEntry,
        destination_directory: &Path,
        transfer_id: String,
        operation_id: String,
    ) -> Result<GpuiDownloadRequest, GpuiFileSelectionError> {
        let target = self
            .selected_target
            .clone()
            .ok_or(GpuiFileSelectionError::TargetUnavailable)?;
        if entry.is_directory || self.downloading_path.is_some() {
            return Err(GpuiFileSelectionError::OperationPending);
        }
        if !destination_directory.is_absolute() {
            return Err(GpuiFileSelectionError::InvalidSharedPath);
        }
        let name = file_entry_name(&entry.path);
        if name.is_empty() || name == "." || name == ".." {
            return Err(GpuiFileSelectionError::MissingFileName);
        }
        let local_path = destination_directory.join(name);
        let local_path_string = local_path
            .to_str()
            .ok_or(GpuiFileSelectionError::NonUtf8LocalPath)?
            .to_owned();
        self.download_generation = self.download_generation.wrapping_add(1).max(1);
        self.downloading_path = Some(entry.path.as_str().to_owned());
        self.download_transfer_id = Some(transfer_id.clone());
        self.download_operation_id = Some(operation_id.clone());
        self.picking_download = false;
        self.download_error = None;
        self.notice = None;
        Ok(GpuiDownloadRequest {
            generation: self.download_generation,
            target_key: target.key(),
            name: name.to_owned(),
            request: StartFileTransferRequest {
                peer: target.peer,
                network_id: Some(target.network_id),
                direction: crate::file_share::FileTransferDirection::Download,
                share_id: target.share_id,
                transfer_id,
                operation_id,
                path: entry.path.as_str().to_owned(),
                local_path: local_path_string,
                chunk_size: DEFAULT_FILE_TRANSFER_CHUNK_SIZE,
            },
        })
    }

    pub fn cancel_download_picker(&mut self) {
        self.download_generation = self.download_generation.wrapping_add(1).max(1);
        self.downloading_path = None;
        self.download_transfer_id = None;
        self.download_operation_id = None;
        self.picking_download = false;
    }

    pub fn begin_download_pick(&mut self, entry: &FileDirectoryEntry) -> bool {
        if self.selected_target.is_none()
            || entry.is_directory
            || self.downloading_path.is_some()
            || self.picking_download
        {
            return false;
        }
        self.picking_download = true;
        self.download_error = None;
        self.notice = None;
        true
    }

    pub fn finish_download_start(
        &mut self,
        generation: u64,
        target_key: &str,
        transfer_id: &str,
        operation_id: &str,
        result: Result<(), String>,
    ) -> Option<bool> {
        if generation != self.download_generation
            || self
                .selected_target
                .as_ref()
                .is_none_or(|target| target.key() != target_key)
            || !self.download_request_identity_is_current(
                transfer_id,
                operation_id,
                self.downloading_path.as_deref(),
            )
        {
            return None;
        }
        self.download_transfer_id = None;
        self.download_operation_id = None;
        self.downloading_path = None;
        self.picking_download = false;
        match result {
            Ok(()) => {
                self.download_error = None;
                Some(true)
            }
            Err(error) => {
                self.download_error = Some(error);
                Some(false)
            }
        }
    }

    pub fn begin_transfer_refresh(&mut self) -> Option<u64> {
        if self.transfers_loading {
            return None;
        }
        self.transfers_loading = true;
        self.transfer_generation = self.transfer_generation.wrapping_add(1).max(1);
        Some(self.transfer_generation)
    }

    pub fn finish_transfer_refresh(
        &mut self,
        generation: u64,
        result: Result<Vec<FileTransferStatus>, String>,
    ) -> bool {
        if generation != self.transfer_generation {
            return false;
        }
        self.transfers_loading = false;
        self.transfers_loaded = true;
        match result {
            Ok(transfers) => {
                self.transfers = transfers;
                self.transfers_error = None;
            }
            Err(error) => self.transfers_error = Some(error),
        }
        true
    }

    pub fn begin_control(
        &mut self,
        status: &FileTransferStatus,
        action: GpuiFileControl,
    ) -> Option<GpuiPendingFileControl> {
        if self.control.is_some() || !control_allowed(status.state, action) {
            return None;
        }
        self.control_generation = self.control_generation.wrapping_add(1).max(1);
        let pending = GpuiPendingFileControl {
            generation: self.control_generation,
            transfer_id: status.transfer_id.clone(),
            operation_id: status.operation_id.clone(),
            action,
        };
        self.control = Some(pending.clone());
        self.transfers_error = None;
        Some(pending)
    }

    pub fn finish_control(
        &mut self,
        generation: u64,
        transfer_id: &str,
        operation_id: &str,
        result: Result<(), String>,
    ) -> bool {
        if generation != self.control_generation
            || self.control.as_ref().is_none_or(|control| {
                control.generation != generation
                    || control.transfer_id != transfer_id
                    || control.operation_id != operation_id
            })
        {
            return false;
        }
        self.control = None;
        if let Err(error) = result {
            self.transfers_error = Some(error);
        }
        true
    }

    pub fn active_transfer_count(&self) -> usize {
        self.transfers
            .iter()
            .filter(|status| !status.is_terminal())
            .count()
    }

    fn reset_target_bound_state(&mut self) {
        self.directory_generation = self.directory_generation.wrapping_add(1).max(1);
        self.directory_path.clear();
        self.clear_directory();
        self.directory_error = None;
        self.upload_generation = self.upload_generation.wrapping_add(1).max(1);
        self.upload = None;
        self.picking_upload = false;
        self.uploading = false;
        self.upload_transfer_id = None;
        self.upload_operation_id = None;
        self.upload_error = None;
        self.download_generation = self.download_generation.wrapping_add(1).max(1);
        self.downloading_path = None;
        self.download_transfer_id = None;
        self.download_operation_id = None;
        self.picking_download = false;
        self.download_error = None;
        self.notice = None;
    }

    fn clear_directory(&mut self) {
        self.directory_generation = self.directory_generation.wrapping_add(1).max(1);
        self.directory_entries.clear();
        self.directory_loaded = false;
        self.directory_loading = false;
        self.directory_loading_more = false;
        self.directory_next_cursor = None;
        self.directory_truncated = false;
    }

    fn directory_request_is_current(&self, request: &GpuiDirectoryRequest) -> bool {
        request.generation == self.directory_generation
            && request.path == self.directory_path
            && self
                .selected_target
                .as_ref()
                .is_some_and(|target| target.key() == request.target_key)
    }

    fn upload_request_identity_is_current(&self, transfer_id: &str, operation_id: &str) -> bool {
        self.uploading
            && self.upload_transfer_id.as_deref() == Some(transfer_id)
            && self.upload_operation_id.as_deref() == Some(operation_id)
    }

    fn download_request_identity_is_current(
        &self,
        transfer_id: &str,
        operation_id: &str,
        expected_path: Option<&str>,
    ) -> bool {
        expected_path.is_some()
            && expected_path == self.downloading_path.as_deref()
            && self.download_transfer_id.as_deref() == Some(transfer_id)
            && self.download_operation_id.as_deref() == Some(operation_id)
    }
}

pub(crate) fn child_shared_path(directory: &str, name: &str) -> Option<SharedPath> {
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let joined = if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    };
    SharedPath::parse(joined).ok()
}

pub(crate) fn file_entry_name(path: &SharedPath) -> &str {
    path.as_str()
        .rsplit_once('/')
        .map_or(path.as_str(), |(_, name)| name)
}

pub(crate) fn transfer_direction_key(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Upload => "file.page.transfer.direction.upload",
        TransferDirection::Download => "file.page.transfer.direction.download",
    }
}

pub(crate) fn transfer_state_key(state: TransferState) -> &'static str {
    match state {
        TransferState::Queued => "file.page.transfer.state.queued",
        TransferState::Active => "file.page.transfer.state.active",
        TransferState::Paused => "file.page.transfer.state.paused",
        TransferState::Verifying => "file.page.transfer.state.verifying",
        TransferState::Completed => "file.page.transfer.state.completed",
        TransferState::Failed => "file.page.transfer.state.failed",
        TransferState::Cancelled => "file.page.transfer.state.cancelled",
        TransferState::Expired => "file.page.transfer.state.expired",
    }
}

pub(crate) fn control_allowed(state: TransferState, action: GpuiFileControl) -> bool {
    match action {
        GpuiFileControl::Pause => matches!(state, TransferState::Queued | TransferState::Active),
        GpuiFileControl::Resume => state == TransferState::Paused,
        GpuiFileControl::Cancel => !state.is_terminal(),
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use fuckipv6_common::domain::{
        ExitContributionMode, ExitContributionObligation, ExitContributionScope, MembershipRole,
        MembershipState,
    };

    use super::*;
    use crate::node::{NodeNetworkMembershipSnapshot, NodePeerSnapshot, NodeTrafficSnapshot};

    fn membership(network_name: &str, network_id: &str) -> NodeNetworkMembershipSnapshot {
        NodeNetworkMembershipSnapshot {
            network_name: network_name.into(),
            network_id: network_id.into(),
            node_id: "local-node".into(),
            state: MembershipState::Active,
            role: MembershipRole::Member,
            local_policy_revision: 1,
            network_policy_revision: Some(1),
            privacy_policy_revision: Some(1),
            member_limit: None,
            exit_contribution: Some(ExitContributionObligation::NotRequired),
            exit_contribution_scope: Some(ExitContributionScope::NotApplicable),
            exit_contribution_mode: Some(ExitContributionMode::Disabled),
            terms_hash: None,
            current_terms: None,
            current_terms_hash: None,
            policy_confirmation_required: false,
            acknowledged_at_unix_ms: None,
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            suspension_reason: None,
        }
    }

    fn peer(network_name: &str, peer_id: &str, online: bool) -> NodePeerSnapshot {
        NodePeerSnapshot {
            peer_id: peer_id.into(),
            network_name: network_name.into(),
            virtual_ipv6: Ipv6Addr::LOCALHOST,
            virtual_ipv4: None,
            endpoints: Vec::new(),
            online,
            p2p_active: online,
            p2p_healthy: online,
            rtt_us: None,
            loss_rate: 0.0,
            traffic: NodeTrafficSnapshot::default(),
        }
    }

    fn target(network_name: &str, network_id: &str, peer_id: &str) -> GpuiFileTarget {
        GpuiFileTarget {
            peer: peer_id.into(),
            network_id: network_id.into(),
            network_name: network_name.into(),
            share_id: FileShareId::from_public_key(
                format!("file-share:{peer_id}:{network_id}").as_bytes(),
            )
            .as_str()
            .to_owned(),
        }
    }

    fn entry(path: &str, directory: bool) -> FileDirectoryEntry {
        FileDirectoryEntry {
            path: SharedPath::parse(path).expect("shared path"),
            is_directory: directory,
            size: if directory { 0 } else { 42 },
            modified_at_unix_ms: None,
        }
    }

    fn status(state: TransferState) -> FileTransferStatus {
        FileTransferStatus {
            transfer_id: "transfer".into(),
            operation_id: "operation".into(),
            direction: TransferDirection::Upload,
            state,
            transferred_bytes: 1,
            total_bytes: 2,
            received_chunks: 1,
            total_chunks: 2,
            error_code: None,
            failure_kind: None,
            updated_at_unix_ms: 1,
        }
    }

    #[test]
    fn target_derivation_requires_online_peer_and_membership() {
        let mut snapshot = NodeSnapshot::default();
        snapshot
            .memberships
            .push(membership("studio", "network-studio"));
        snapshot.peers.push(peer("studio", "peer-online", true));
        snapshot.peers.push(peer("studio", "peer-offline", false));
        snapshot.peers.push(peer("unknown", "peer-unknown", true));

        let targets = file_targets_from_snapshot(&snapshot);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].peer, "peer-online");
        assert_eq!(targets[0].network_id, "network-studio");
        assert_eq!(
            targets[0].share_id,
            FileShareId::from_public_key(b"file-share:peer-online:network-studio").as_str()
        );
    }

    #[test]
    fn target_sync_preserves_selection_and_resets_disappearing_target() {
        let mut workspace = GpuiFileWorkspace::default();
        let first = target("studio", "network-a", "peer-a");
        let second = target("home", "network-b", "peer-b");
        assert!(workspace.sync_targets(vec![first.clone(), second.clone()], Some("studio")));
        assert_eq!(workspace.selected_target.as_ref(), Some(&first));
        workspace.directory_path = "docs".into();
        workspace.directory_loaded = true;
        assert!(!workspace.sync_targets(vec![first.clone(), second.clone()], None));
        assert_eq!(workspace.directory_path, "docs");
        assert!(workspace.sync_targets(vec![second.clone()], None));
        assert_eq!(workspace.selected_target.as_ref(), Some(&second));
        assert!(workspace.directory_path.is_empty());
        assert!(!workspace.directory_loaded);
    }

    #[test]
    fn directory_generation_rejects_stale_and_merges_bounded_pages() {
        let mut workspace = GpuiFileWorkspace::default();
        let selected = target("studio", "network-a", "peer-a");
        workspace.sync_targets(vec![selected], None);
        let first = workspace.begin_directory(false).expect("first page");
        assert!(workspace.finish_directory(
            &first,
            Ok(FileDirectoryListing {
                entries: vec![entry("docs", true), entry("README", false)],
                next_cursor: Some("next".into()),
                truncated: true,
            })
        ));
        assert_eq!(workspace.directory_entries.len(), 2);
        assert!(workspace.can_load_more());

        let stale = workspace.begin_directory(true).expect("next page");
        assert!(workspace.navigate_directory("docs"));
        assert!(!workspace.finish_directory(
            &stale,
            Ok(FileDirectoryListing {
                entries: vec![entry("docs/stale", false)],
                next_cursor: None,
                truncated: false,
            })
        ));
        assert!(workspace.directory_entries.is_empty());

        let fresh = workspace.begin_directory(false).expect("navigate page");
        assert!(workspace.finish_directory(
            &fresh,
            Ok(FileDirectoryListing {
                entries: vec![entry("docs/guide", false)],
                next_cursor: None,
                truncated: false,
            })
        ));
        assert_eq!(workspace.directory_entries[0].path.as_str(), "docs/guide");
        assert!(!workspace.can_load_more());
    }

    #[test]
    fn upload_request_stays_inside_the_logical_share_and_invalidates_on_target_change() {
        let mut workspace = GpuiFileWorkspace::default();
        let selected = target("studio", "network-a", "peer-a");
        workspace.sync_targets(vec![selected], None);
        assert!(workspace.navigate_directory("docs"));
        workspace
            .select_upload_file(PathBuf::from("/home/member/report.pdf"))
            .expect("upload selection");
        let request = workspace
            .begin_upload("transfer".into(), "operation".into())
            .expect("upload request");
        assert_eq!(
            request.request.direction,
            crate::file_share::FileTransferDirection::Upload
        );
        assert_eq!(request.request.path, "docs/report.pdf");
        assert_eq!(request.request.local_path, "/home/member/report.pdf");

        let next_target = target("home", "network-b", "peer-b");
        assert!(workspace.sync_targets(vec![next_target], None));
        assert!(workspace
            .finish_upload_start(
                request.generation,
                &request.target_key,
                &request.request.transfer_id,
                &request.request.operation_id,
                Ok(())
            )
            .is_none());
        assert!(workspace.upload.is_none());
    }

    #[test]
    fn unsupported_upload_names_and_relative_download_destinations_are_rejected() {
        let mut workspace = GpuiFileWorkspace::default();
        let selected = target("studio", "network-a", "peer-a");
        workspace.sync_targets(vec![selected], None);
        assert_eq!(
            workspace.select_upload_file(PathBuf::from("/home/member/..")),
            Err(GpuiFileSelectionError::MissingFileName)
        );
        assert_eq!(
            workspace.select_upload_file(PathBuf::from("/home/member/bad\\name")),
            Err(GpuiFileSelectionError::InvalidSharedPath)
        );
        assert_eq!(
            workspace.begin_download(
                &entry("docs/report.pdf", false),
                Path::new("relative"),
                "transfer".into(),
                "operation".into(),
            ),
            Err(GpuiFileSelectionError::InvalidSharedPath)
        );
    }

    #[test]
    fn download_request_uses_selected_directory_and_completion_generation() {
        let mut workspace = GpuiFileWorkspace::default();
        let selected = target("studio", "network-a", "peer-a");
        workspace.sync_targets(vec![selected], None);
        let request = workspace
            .begin_download(
                &entry("docs/report.pdf", false),
                Path::new("/home/member/Downloads"),
                "transfer".into(),
                "operation".into(),
            )
            .expect("download request");
        assert_eq!(request.request.path, "docs/report.pdf");
        assert_eq!(
            request.request.local_path,
            "/home/member/Downloads/report.pdf"
        );
        assert_eq!(
            workspace.downloading_path.as_deref(),
            Some("docs/report.pdf")
        );
        assert_eq!(
            workspace.finish_download_start(
                request.generation,
                &request.target_key,
                &request.request.transfer_id,
                &request.request.operation_id,
                Ok(())
            ),
            Some(true)
        );
        assert!(workspace.downloading_path.is_none());
    }

    #[test]
    fn transfer_controls_are_state_gated_and_serialized() {
        let mut workspace = GpuiFileWorkspace::default();
        let active = status(TransferState::Active);
        let control = workspace
            .begin_control(&active, GpuiFileControl::Pause)
            .expect("pause");
        assert!(workspace
            .begin_control(&active, GpuiFileControl::Cancel)
            .is_none());
        assert!(workspace.finish_control(
            control.generation,
            &control.transfer_id,
            &control.operation_id,
            Ok(())
        ));
        assert!(workspace.control.is_none());
        assert!(workspace
            .begin_control(&status(TransferState::Completed), GpuiFileControl::Cancel)
            .is_none());
        assert!(workspace
            .begin_control(&status(TransferState::Paused), GpuiFileControl::Resume)
            .is_some());
    }

    #[test]
    fn transfer_control_matrix_matches_transfer_state() {
        let cases = [
            (TransferState::Queued, true, false, true),
            (TransferState::Active, true, false, true),
            (TransferState::Paused, false, true, true),
            (TransferState::Verifying, false, false, true),
            (TransferState::Completed, false, false, false),
            (TransferState::Failed, false, false, false),
            (TransferState::Cancelled, false, false, false),
            (TransferState::Expired, false, false, false),
        ];

        for (state, pause, resume, cancel) in cases {
            assert_eq!(
                control_allowed(state, GpuiFileControl::Pause),
                pause,
                "pause gating for {state:?}"
            );
            assert_eq!(
                control_allowed(state, GpuiFileControl::Resume),
                resume,
                "resume gating for {state:?}"
            );
            assert_eq!(
                control_allowed(state, GpuiFileControl::Cancel),
                cancel,
                "cancel gating for {state:?}"
            );
        }
    }
}
