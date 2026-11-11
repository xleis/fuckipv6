//! Owner-local File setup state for the native shell.
//!
//! This workflow deliberately lives outside the manifest action system. A
//! selected root is retained only in this desktop process and is sent to the
//! node over same-user local IPC for preview and configuration.

use std::path::PathBuf;

use fuckipv6_common::domain::NetworkId;
#[cfg(test)]
use fuckipv6_common::domain::{FileQuota, FileShareMode, SymlinkPolicy};

use crate::file_share::{FileShareConfiguration, FileSharePreview};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FileShareAccessScope {
    #[default]
    NoMembers,
    ActiveMembersReadOnly,
}

#[derive(Debug, Default)]
pub(crate) struct GpuiFileShareDraft {
    pub network_id: Option<NetworkId>,
    pub network_name: String,
    pub root: Option<PathBuf>,
    pub access_scope: FileShareAccessScope,
    pub preview: Option<FileSharePreview>,
    pub picking_root: bool,
    pub preview_pending: bool,
    pub configure_pending: bool,
    pub disable_pending: bool,
    pub error: Option<String>,
    generation: u64,
}

impl GpuiFileShareDraft {
    pub fn begin(&mut self, network_id: NetworkId, network_name: String) {
        if self.network_id.as_ref() == Some(&network_id) {
            self.network_name = network_name;
            return;
        }
        self.reset();
        self.network_id = Some(network_id);
        self.network_name = network_name;
    }

    pub fn select_root(&mut self, root: PathBuf) {
        if self.operation_pending() {
            return;
        }
        self.root = Some(root);
        self.error = None;
        self.invalidate_preview();
    }

    pub fn set_access_scope(&mut self, access_scope: FileShareAccessScope) {
        if self.operation_pending() {
            return;
        }
        if self.access_scope != access_scope {
            self.access_scope = access_scope;
            self.error = None;
            self.invalidate_preview();
        }
    }

    pub fn begin_pick_root(&mut self) -> bool {
        if self.network_id.is_none()
            || self.picking_root
            || self.preview_pending
            || self.configure_pending
            || self.disable_pending
        {
            return false;
        }
        self.picking_root = true;
        self.error = None;
        true
    }

    pub fn finish_pick_root(&mut self) {
        self.picking_root = false;
    }

    pub fn can_preview(&self) -> bool {
        self.network_id.is_some()
            && self.root.is_some()
            && !self.picking_root
            && !self.preview_pending
            && !self.configure_pending
            && !self.disable_pending
    }

    pub fn start_preview(&mut self) -> Option<(u64, PathBuf, FileShareConfiguration)> {
        if !self.can_preview() {
            return None;
        }
        let root = self.root.clone()?;
        let configuration = self.configuration()?;
        self.preview = None;
        self.preview_pending = true;
        self.error = None;
        Some((self.generation, root, configuration))
    }

    /// Returns false when the response belongs to an older root/scope/network.
    pub fn finish_preview(
        &mut self,
        generation: u64,
        result: Result<FileSharePreview, String>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.preview_pending = false;
        match result {
            Ok(preview) => {
                self.preview = Some(preview);
                self.error = None;
            }
            Err(error) => {
                self.preview = None;
                self.error = Some(error);
            }
        }
        true
    }

    pub fn can_configure(&self) -> bool {
        self.network_id.is_some()
            && self.root.is_some()
            && self.preview.is_some()
            && !self.picking_root
            && !self.preview_pending
            && !self.configure_pending
            && !self.disable_pending
    }

    pub fn start_configure(&mut self) -> Option<(u64, PathBuf, FileShareConfiguration)> {
        if !self.can_configure() {
            return None;
        }
        let root = self.root.clone()?;
        let configuration = self.configuration()?;
        self.configure_pending = true;
        self.error = None;
        Some((self.generation, root, configuration))
    }

    pub fn finish_configure(&mut self, generation: u64, result: Result<(), String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.configure_pending = false;
        if let Err(error) = result {
            self.error = Some(error);
        }
        true
    }

    pub fn start_disable(&mut self) -> Option<(u64, NetworkId)> {
        if self.operation_pending() {
            return None;
        }
        let network_id = self.network_id.clone()?;
        self.disable_pending = true;
        self.error = None;
        Some((self.generation, network_id))
    }

    pub fn finish_disable(&mut self, generation: u64, result: Result<(), String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.disable_pending = false;
        if let Err(error) = result {
            self.error = Some(error);
        }
        true
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.picking_root = false;
        self.error = Some(error.into());
    }

    pub fn clear(&mut self) {
        self.reset();
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn configuration(&self) -> Option<FileShareConfiguration> {
        let network_id = self.network_id.clone()?;
        let display_name = format!("{} File", self.network_name.trim());
        let configuration = FileShareConfiguration::readonly(network_id, display_name);
        Some(match self.access_scope {
            FileShareAccessScope::NoMembers => configuration,
            FileShareAccessScope::ActiveMembersReadOnly => {
                configuration.allow_active_members_readonly()
            }
        })
    }

    fn invalidate_preview(&mut self) {
        self.preview = None;
        self.preview_pending = false;
        self.generation = self.generation.saturating_add(1).max(1);
    }

    fn operation_pending(&self) -> bool {
        self.picking_root || self.preview_pending || self.configure_pending || self.disable_pending
    }

    fn reset(&mut self) {
        self.network_id = None;
        self.network_name.clear();
        self.root = None;
        self.access_scope = FileShareAccessScope::NoMembers;
        self.preview = None;
        self.picking_root = false;
        self.preview_pending = false;
        self.configure_pending = false;
        self.disable_pending = false;
        self.error = None;
        self.generation = self.generation.saturating_add(1).max(1);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use fuckipv6_common::domain::{FileAclEffect, FileAction, SharedPath, SubjectSelector};

    use super::*;

    fn network() -> NetworkId {
        NetworkId::from_public_key(b"file-ui-network")
    }

    fn preview() -> FileSharePreview {
        FileSharePreview {
            network_id: network(),
            display_name: "Studio File".into(),
            mode: FileShareMode::ReadOnly,
            quota: FileQuota::default(),
            network_acl_rule_count: 1,
            local_acl_rule_count: 1,
            symlink_policy: SymlinkPolicy::Reject,
            audit_enabled: true,
            available_bytes: 1_000,
            directory_bytes: 500,
            directory_entries: 2,
            symlinks_skipped: 0,
            special_files_skipped: 0,
            deny_by_default: true,
        }
    }

    #[test]
    fn changing_root_or_scope_invalidates_preview_generation() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        let initial = draft.generation();
        draft.select_root(PathBuf::from("/tmp/share"));
        let after_root = draft.generation();
        assert!(after_root > initial);
        draft.set_access_scope(FileShareAccessScope::ActiveMembersReadOnly);
        assert!(draft.generation() > after_root);
    }

    #[test]
    fn preview_must_finish_before_configure_is_available() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (generation, _, _) = draft.start_preview().expect("preview request");
        assert!(!draft.can_configure());
        assert!(draft.finish_preview(generation, Ok(preview())));
        assert!(draft.can_configure());
        assert_eq!(draft.preview, Some(preview()));
    }

    #[test]
    fn scope_change_is_ignored_while_preview_is_pending() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (generation, _, _) = draft.start_preview().expect("preview request");
        draft.set_access_scope(FileShareAccessScope::ActiveMembersReadOnly);
        assert_eq!(draft.access_scope, FileShareAccessScope::NoMembers);
        assert!(draft.finish_preview(generation, Ok(preview())));
        assert_eq!(draft.preview, Some(preview()));
    }

    #[test]
    fn switching_network_discards_the_previous_owner_selection() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        draft.set_access_scope(FileShareAccessScope::ActiveMembersReadOnly);
        let old_generation = draft.generation();
        draft.begin(
            NetworkId::from_public_key(b"other-file-ui-network"),
            "Other".into(),
        );
        assert_eq!(draft.root, None);
        assert_eq!(draft.access_scope, FileShareAccessScope::NoMembers);
        assert!(draft.generation() > old_generation);
    }

    #[test]
    fn active_member_scope_intersects_the_same_minimal_rule_in_both_layers() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.set_access_scope(FileShareAccessScope::ActiveMembersReadOnly);
        let configuration = draft.configuration().expect("configuration");
        assert_eq!(
            configuration.network_acl.rules,
            configuration.local_acl.rules
        );
        let rule = &configuration.network_acl.rules[0];
        assert_eq!(rule.effect, FileAclEffect::Allow);
        assert_eq!(rule.path_prefix, SharedPath::root());
        assert_eq!(rule.subjects, BTreeSet::from([SubjectSelector::AnyMember]));
        assert_eq!(
            rule.actions,
            BTreeSet::from([
                FileAction::List,
                FileAction::ReadMetadata,
                FileAction::Download,
            ])
        );
    }

    #[test]
    fn default_scope_grants_no_member_access() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        let configuration = draft.configuration().expect("configuration");
        assert!(configuration.network_acl.rules.is_empty());
        assert!(configuration.local_acl.rules.is_empty());
    }

    #[test]
    fn stale_preview_errors_do_not_overwrite_current_state() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (stale_generation, _, _) = draft.start_preview().expect("preview request");
        draft.clear();
        assert!(!draft.finish_preview(stale_generation, Err("stale error".into())));
        assert_eq!(draft.network_id, None);
        assert_eq!(draft.error, None);
        assert!(!draft.preview_pending);
    }

    #[test]
    fn clearing_the_draft_invalidates_in_flight_preview() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (generation, _, _) = draft.start_preview().expect("preview request");
        draft.clear();
        assert!(!draft.finish_preview(generation, Ok(preview())));
        assert!(draft.preview.is_none());
        assert!(draft.root.is_none());
        assert!(!draft.preview_pending);
    }

    #[test]
    fn pending_configure_cannot_restart_or_change_inputs() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (preview_generation, _, _) = draft.start_preview().expect("preview request");
        assert!(draft.finish_preview(preview_generation, Ok(preview())));
        let (configure_generation, _, _) = draft.start_configure().expect("configure request");
        assert!(draft.start_configure().is_none());
        draft.select_root(PathBuf::from("/tmp/other"));
        draft.set_access_scope(FileShareAccessScope::ActiveMembersReadOnly);
        assert_eq!(draft.root, Some(PathBuf::from("/tmp/share")));
        assert_eq!(draft.access_scope, FileShareAccessScope::NoMembers);
        assert!(draft.finish_configure(configure_generation, Ok(())));
        assert!(!draft.configure_pending);
    }

    #[test]
    fn disable_is_serialized_and_blocks_preview_and_configure() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (preview_generation, _, _) = draft.start_preview().expect("preview request");
        assert!(draft.finish_preview(preview_generation, Ok(preview())));
        let (disable_generation, _) = draft.start_disable().expect("disable request");
        assert!(draft.start_disable().is_none());
        assert!(!draft.can_preview());
        assert!(!draft.can_configure());
        assert!(draft.finish_disable(disable_generation, Ok(())));
        assert!(!draft.disable_pending);
    }

    #[test]
    fn disable_cannot_overlap_an_in_flight_preview() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (preview_generation, _, _) = draft.start_preview().expect("preview request");
        assert!(draft.start_disable().is_none());
        assert!(draft.finish_preview(preview_generation, Ok(preview())));
    }

    #[test]
    fn successful_configure_clears_pending_state() {
        let mut draft = GpuiFileShareDraft::default();
        draft.begin(network(), "Studio".into());
        draft.select_root(PathBuf::from("/tmp/share"));
        let (preview_generation, _, _) = draft.start_preview().expect("preview request");
        assert!(draft.finish_preview(preview_generation, Ok(preview())));
        let (configure_generation, _, _) = draft.start_configure().expect("configure request");
        assert!(draft.configure_pending);
        assert!(draft.finish_configure(configure_generation, Ok(())));
        assert!(!draft.configure_pending);
        assert_eq!(draft.error, None);
    }
}
