//! State machine for the native transactional configuration workspace.
//!
//! The view owns rendering and command dispatch, while this module owns every
//! transition that can affect whether a draft may be applied. Keeping the
//! decisions here makes the safety rules testable without a window or runtime.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config_transaction::{
    ConfigurationApplyResult, ConfigurationDocument, ConfigurationHistoryEntry,
    ConfigurationPreview,
};
use crate::node::{NodeProblem, NodeValidationIssue};

pub(crate) const CONFIGURATION_ACTION_IDS: [&str; 5] = [
    "get_configuration",
    "preview_configuration",
    "apply_configuration",
    "rollback_configuration",
    "get_configuration_history",
];

pub(crate) fn is_configuration_action(action_id: &str) -> bool {
    CONFIGURATION_ACTION_IDS.contains(&action_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigurationOperation {
    Load,
    Preview,
    Apply,
    Rollback,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigurationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub(crate) struct GpuiConfigurationWorkflow {
    pub document: Option<ConfigurationDocument>,
    pub draft: String,
    pub imported_from: Option<String>,
    pub preview: Option<ConfigurationPreview>,
    pub history: Vec<ConfigurationHistoryEntry>,
    pub applied: Option<ConfigurationApplyResult>,
    pub validation_issues: Vec<ConfigurationIssue>,
    pub active_issue_path: Option<String>,
    pub selected_audit: Option<String>,
    pub show_candidate: bool,
    pub confirm_apply: bool,
    pub rollback_target: Option<u64>,
    pub operation: Option<ConfigurationOperation>,
    pub error: Option<String>,
    pub notice: Option<String>,
    preview_draft: Option<String>,
}

impl GpuiConfigurationWorkflow {
    pub(crate) fn document_json(document: &ConfigurationDocument) -> String {
        serde_json::to_string_pretty(&document.networks).unwrap_or_else(|_| "{}".to_string())
    }

    pub(crate) fn parse_networks(source: &str) -> Result<BTreeMap<String, Value>, String> {
        let value: Value = serde_json::from_str(source).map_err(|error| {
            format!(
                "JSON 格式错误：第 {} 行，第 {} 列：{error}",
                error.line(),
                error.column()
            )
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| "配置根节点必须是 JSON 对象".to_string())?;
        let networks = object
            .get("networks")
            .and_then(Value::as_object)
            .unwrap_or(object);
        networks
            .iter()
            .map(|(name, config)| {
                if !config.is_object() {
                    return Err(format!("网络 {name} 的配置必须是 JSON 对象"));
                }
                Ok((name.clone(), config.clone()))
            })
            .collect()
    }

    pub(crate) fn draft_networks(&self) -> Result<BTreeMap<String, Value>, String> {
        Self::parse_networks(&self.draft)
    }

    pub(crate) fn receive_document(&mut self, document: ConfigurationDocument, reset_editor: bool) {
        let reset = self.document.is_none() || reset_editor;
        if reset {
            self.draft = Self::document_json(&document);
        }
        self.document = Some(document);
        self.preview = None;
        self.preview_draft = None;
        self.validation_issues.clear();
        self.active_issue_path = None;
    }

    pub(crate) fn receive_history(&mut self, history: Vec<ConfigurationHistoryEntry>) {
        self.history = history;
        if self
            .selected_audit
            .as_deref()
            .is_some_and(|id| !self.history.iter().any(|entry| entry.transaction_id == id))
        {
            self.selected_audit = None;
        }
    }

    pub(crate) fn receive_preview(&mut self, preview: ConfigurationPreview) {
        self.validation_issues = preview
            .validation_errors
            .iter()
            .map(|issue| ConfigurationIssue {
                path: issue.path.clone(),
                code: format!("{:?}", issue.code),
                message: issue.message.clone(),
            })
            .collect();
        self.active_issue_path = self
            .validation_issues
            .first()
            .map(|issue| issue.path.clone());
        self.preview_draft = Some(self.draft.clone());
        self.preview = Some(preview);
        self.confirm_apply = false;
        self.applied = None;
    }

    pub(crate) fn receive_applied(&mut self, result: ConfigurationApplyResult) {
        self.document = Some(result.document.clone());
        self.draft = Self::document_json(&result.document);
        self.preview = None;
        self.preview_draft = None;
        self.validation_issues.clear();
        self.active_issue_path = None;
        self.confirm_apply = false;
        self.rollback_target = None;
        self.applied = Some(result);
    }

    pub(crate) fn record_rejection(&mut self, problem: &NodeProblem) {
        if let Some(details) = problem.details.as_deref() {
            self.validation_issues = details
                .validation_issues
                .iter()
                .map(Self::issue_from_node)
                .collect();
            self.active_issue_path = self
                .validation_issues
                .first()
                .map(|issue| issue.path.clone());
        }
        self.error = Some(problem.message.clone());
        self.confirm_apply = false;
        self.rollback_target = None;
    }

    fn issue_from_node(issue: &NodeValidationIssue) -> ConfigurationIssue {
        ConfigurationIssue {
            path: issue.path.clone(),
            code: issue.code.clone(),
            message: issue.message.clone(),
        }
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.confirm_apply = false;
        self.rollback_target = None;
    }

    pub(crate) fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
        self.error = None;
    }

    pub(crate) fn begin(&mut self, operation: ConfigurationOperation) {
        self.operation = Some(operation);
        self.error = None;
        self.notice = None;
        self.confirm_apply = false;
        self.rollback_target = None;
    }

    pub(crate) fn finish(&mut self) {
        self.operation = None;
    }

    pub(crate) fn edit(&mut self, draft: impl Into<String>) {
        self.draft = draft.into();
        self.imported_from = None;
        self.applied = None;
        self.confirm_apply = false;
        self.active_issue_path = None;
        self.error = None;
        self.notice = None;
    }

    pub(crate) fn import(&mut self, draft: String, source: String) -> Result<(), String> {
        Self::parse_networks(&draft)?;
        self.edit(draft);
        self.imported_from = Some(source);
        Ok(())
    }

    pub(crate) fn is_dirty(&self) -> bool {
        let Some(document) = &self.document else {
            return false;
        };
        self.draft_networks()
            .is_ok_and(|draft| draft != document.networks)
    }

    pub(crate) fn preview_is_current(&self) -> bool {
        let (Some(preview), Some(document)) = (&self.preview, &self.document) else {
            return false;
        };
        self.preview_draft.as_deref() == Some(self.draft.as_str())
            && preview.current_revision == document.revision
            && preview.expected_revision == document.revision
    }

    pub(crate) fn has_revision_conflict(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|preview| preview.revision_conflict)
    }

    pub(crate) fn can_apply(&self) -> bool {
        let Some(preview) = &self.preview else {
            return false;
        };
        self.preview_is_current()
            && preview.can_apply
            && preview.has_changes
            && !preview.revision_conflict
            && preview.validation_errors.is_empty()
    }

    pub(crate) fn apply_block_key(&self) -> &'static str {
        if self.document.is_none() {
            return "configuration.loading";
        }
        if self.preview.is_none() {
            return "configuration.review.empty";
        }
        if !self.preview_is_current() {
            return "configuration.diff_count";
        }
        let Some(preview) = &self.preview else {
            return "configuration.review.empty";
        };
        if preview.revision_conflict {
            return "configuration.apply.conflict";
        }
        if !preview.validation_errors.is_empty() {
            return "configuration.apply.invalid";
        }
        if !preview.has_changes {
            return "configuration.apply.no_changes";
        }
        if !preview.can_apply {
            return "configuration.apply.blocked";
        }
        "configuration.apply.ready"
    }

    pub(crate) fn request_apply_confirmation(&mut self) -> bool {
        if !self.can_apply() {
            self.confirm_apply = false;
            return false;
        }
        self.confirm_apply = true;
        self.rollback_target = None;
        true
    }

    pub(crate) fn request_rollback_confirmation(&mut self, target: u64) -> bool {
        if self
            .document
            .as_ref()
            .is_none_or(|document| document.revision == target)
        {
            self.rollback_target = None;
            return false;
        }
        self.rollback_target = Some(target);
        self.confirm_apply = false;
        true
    }

    pub(crate) fn select_issue(&mut self, path: String) {
        self.active_issue_path = Some(path);
    }

    pub(crate) fn select_audit(&mut self, transaction_id: String) {
        self.selected_audit = if self.selected_audit.as_deref() == Some(transaction_id.as_str()) {
            None
        } else {
            Some(transaction_id)
        };
    }

    pub(crate) fn candidate_json(&self) -> Option<String> {
        self.preview
            .as_ref()
            .and_then(|preview| serde_json::to_string_pretty(&preview.candidate.networks).ok())
    }
}

/// Select the most specific visible JSON token for a JSON Pointer. Validation
/// pointers are rooted at the complete document (`/networks/...`), while the
/// native editor exposes the `networks` object directly, so missing segments
/// are skipped just like the WebUI implementation.
pub(crate) fn json_pointer_selection(source: &str, pointer: &str) -> std::ops::Range<usize> {
    if !pointer.starts_with('/') || pointer == "/" {
        return 0..0;
    }
    let segments = pointer
        .trim_start_matches('/')
        .split('/')
        .map(|segment| segment.replace("~1", "/").replace("~0", "~"));
    let mut cursor = 0;
    let mut found_range = 0..0;
    for segment in segments {
        let token = serde_json::to_string(&segment).unwrap_or_else(|_| format!("\"{segment}\""));
        if let Some(offset) = source[cursor..].find(&token) {
            let start = cursor + offset;
            cursor = start + token.len();
            found_range = start..cursor;
        }
    }
    found_range
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::config_transaction::{
        ConfigurationChangeAction, ConfigurationDiffEntry, ConfigurationImpact,
        ConfigurationValidationIssue,
    };
    use crate::node::{NodeProblem, NodeProblemCode};

    fn document(revision: u64, value: i64) -> ConfigurationDocument {
        ConfigurationDocument {
            schema_version: 1,
            revision,
            networks: BTreeMap::from([("home".into(), json!({ "mtu": value }))]),
        }
    }

    fn preview(revision: u64, candidate: i64, can_apply: bool) -> ConfigurationPreview {
        ConfigurationPreview {
            expected_revision: revision,
            current_revision: revision,
            candidate_revision: revision + 1,
            revision_conflict: false,
            current: document(revision, 1280),
            candidate: document(revision + 1, candidate),
            diffs: vec![ConfigurationDiffEntry {
                path: "/networks/home/mtu".into(),
                before: json!(1280),
                after: json!(candidate),
                impact: ConfigurationImpact::RestartRequired,
            }],
            validation_errors: Vec::new(),
            has_changes: true,
            can_apply,
        }
    }

    #[test]
    fn apply_requires_a_preview_of_the_current_draft_and_revision() {
        let mut workflow = GpuiConfigurationWorkflow::default();
        workflow.receive_document(document(2, 1280), true);
        workflow.draft = "{\n  \"home\": {\n    \"mtu\": 1400\n  }\n}".into();
        assert!(!workflow.can_apply());

        workflow.receive_preview(preview(2, 1400, true));
        assert!(workflow.can_apply());

        workflow.edit("{\"home\":{\"mtu\":1500}}");
        assert!(!workflow.can_apply());
        assert!(!workflow.preview_is_current());
    }

    #[test]
    fn conflict_invalid_and_blocked_previews_cannot_apply() {
        let mut conflict = preview(2, 1400, true);
        conflict.revision_conflict = true;
        let mut workflow = GpuiConfigurationWorkflow::default();
        workflow.receive_document(document(2, 1280), true);
        workflow.draft = GpuiConfigurationWorkflow::document_json(&conflict.current);
        workflow.receive_preview(conflict);
        assert!(!workflow.can_apply());
        assert_eq!(workflow.apply_block_key(), "configuration.apply.conflict");

        let mut invalid = preview(2, 1400, true);
        invalid
            .validation_errors
            .push(ConfigurationValidationIssue {
                path: "/networks/home/mtu".into(),
                code: crate::config_transaction::ConfigurationValidationCode::InvalidValue,
                message: "MTU 无效".into(),
            });
        workflow.receive_preview(invalid);
        assert!(!workflow.can_apply());
        assert_eq!(workflow.validation_issues.len(), 1);
    }

    #[test]
    fn applied_result_replaces_the_document_and_keeps_an_audit_entry() {
        let mut workflow = GpuiConfigurationWorkflow::default();
        workflow.receive_document(document(2, 1280), true);
        workflow.receive_preview(preview(2, 1400, true));
        let audit = ConfigurationHistoryEntry {
            transaction_id: "tx-3".into(),
            revision: 3,
            previous_revision: 2,
            action: ConfigurationChangeAction::Apply,
            target_revision: None,
            actor: "owner".into(),
            source: "gpui".into(),
            occurred_at: "2026-11-02T00:00:00Z".into(),
            changed_paths: vec!["/networks/home/mtu".into()],
        };
        workflow.receive_applied(ConfigurationApplyResult {
            previous_revision: 2,
            revision: 3,
            no_op: false,
            document: document(3, 1400),
            diffs: preview(2, 1400, true).diffs,
            audit: Some(audit),
        });

        assert_eq!(workflow.document.as_ref().unwrap().revision, 3);
        assert_eq!(
            workflow.draft,
            GpuiConfigurationWorkflow::document_json(&document(3, 1400))
        );
        assert_eq!(workflow.applied.as_ref().unwrap().revision, 3);
        assert!(workflow.preview.is_none());
        assert!(!workflow.can_apply());
    }

    #[test]
    fn imported_draft_accepts_the_networks_wrapper_and_rejects_non_objects() {
        let mut workflow = GpuiConfigurationWorkflow::default();
        workflow
            .import(
                "{\"networks\":{\"home\":{\"mtu\":1400}}}".into(),
                "settings.json".into(),
            )
            .unwrap();
        assert_eq!(workflow.imported_from.as_deref(), Some("settings.json"));
        assert_eq!(
            workflow.draft_networks().unwrap()["home"]["mtu"],
            json!(1400)
        );

        let error = workflow
            .import("{\"home\":42}".into(), "bad.json".into())
            .unwrap_err();
        assert!(error.contains("必须是 JSON 对象"));
    }

    #[test]
    fn rejection_carries_validation_paths_and_blocks_confirmation() {
        let mut workflow = GpuiConfigurationWorkflow {
            confirm_apply: true,
            ..GpuiConfigurationWorkflow::default()
        };
        let problem = NodeProblem::new(NodeProblemCode::InvalidRequest, "配置校验失败")
            .with_validation_issues(vec![NodeValidationIssue {
                path: "/networks/home/mtu".into(),
                code: "invalid_value".into(),
                message: "MTU 超出范围".into(),
            }]);
        workflow.record_rejection(&problem);
        assert_eq!(workflow.validation_issues.len(), 1);
        assert_eq!(
            workflow.active_issue_path.as_deref(),
            Some("/networks/home/mtu")
        );
        assert!(!workflow.confirm_apply);
        assert_eq!(workflow.error.as_deref(), Some("配置校验失败"));
    }

    #[test]
    fn pointer_selection_skips_the_networks_root_and_selects_the_deepest_token() {
        let source =
            "{\n  \"home\": {\n    \"dns\": {\n      \"servers\": [\"1.1.1.1\"]\n    }\n  }\n}";
        let selection = json_pointer_selection(source, "/networks/home/dns/servers/0");
        assert_eq!(&source[selection], "\"servers\"");
    }
}
