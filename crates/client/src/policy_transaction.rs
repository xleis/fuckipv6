use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use fuckipv6_common::domain::{
    NetworkAcl, NetworkAclGrant, NetworkId, NodeLocalAccessPolicy, PermissionCondition,
    PermissionEffect, PermissionRule, SubjectSelector,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const STORE_DIR: &str = ".policy-transaction";
const STATE_FILE: &str = "state.json";
const GENERATIONS_DIR: &str = "generations";
const HISTORY_DIR: &str = "history";
const LOCK_FILE: &str = "lock";
const SCHEMA_VERSION: u16 = 1;

/// Complete policy documents governed by one optimistic transaction. Network
/// ACLs are normalized by stable network ID and never by caller-selected order.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicySnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_policy: Option<NodeLocalAccessPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network_acls: Vec<NetworkAcl>,
}

impl PolicySnapshot {
    pub fn normalized(mut self) -> Result<Self> {
        self.network_acls
            .sort_by(|left, right| left.network_id.as_str().cmp(right.network_id.as_str()));
        let mut seen = BTreeSet::new();
        if self
            .network_acls
            .iter()
            .any(|acl| !seen.insert(acl.network_id.clone()))
        {
            anyhow::bail!("策略快照包含重复的网络 ACL");
        }
        Ok(self)
    }

    pub fn validate_at(&self, now_unix_ms: u64) -> Result<()> {
        if let Some(local) = &self.local_policy {
            local
                .validate_at(now_unix_ms)
                .map_err(|error| anyhow::anyhow!("本地策略无效: {error}"))?;
        }
        for acl in &self.network_acls {
            acl.validate_at(now_unix_ms)
                .map_err(|error| anyhow::anyhow!("网络 ACL [{}] 无效: {error}", acl.network_id))?;
        }
        Ok(())
    }

    fn acl(&self, network_id: &NetworkId) -> Option<&NetworkAcl> {
        self.network_acls
            .iter()
            .find(|acl| &acl.network_id == network_id)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PolicyObjectKind {
    LocalPolicy,
    NetworkAcl,
    PermissionRule,
    NetworkAclGrant,
    ContributionVeto,
    NetworkScope,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PolicyChangeKind {
    LocalPolicyAdded,
    LocalPolicyUpdated,
    LocalPolicyRemoved,
    NetworkAclAdded,
    NetworkAclUpdated,
    NetworkAclRemoved,
    PermissionRuleAdded,
    PermissionRuleUpdated,
    PermissionRuleRemoved,
    GrantAdded,
    GrantUpdated,
    GrantRemoved,
    VetoAdded,
    VetoUpdated,
    VetoRemoved,
    NetworkScopeChanged,
    ValidityChanged,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyDiffEntry {
    /// Stable JSON Pointer rooted at the policy snapshot.
    pub path: String,
    pub change: PolicyChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize)]
pub struct PolicyAffectedObject {
    pub kind: PolicyObjectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRiskReason {
    AllowRuleAdded,
    DenyRuleRemoved,
    PermissionRuleExpanded,
    NetworkAclGrantAdded,
    NetworkAclGrantExpanded,
    GrantReauthenticationRemoved,
    GrantValidityExtended,
    LocalPolicyRemoved,
    NetworkAclRemoved,
    NetworkScopeExpanded,
    ContributionVetoRemoved,
    PolicyValidityExtended,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRiskLevel {
    #[default]
    None,
    Elevated,
    High,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyRiskSummary {
    pub level: PolicyRiskLevel,
    pub privilege_expansion: bool,
    pub requires_recent_reauthentication: bool,
    pub requires_explicit_confirmation: bool,
    pub reasons: Vec<PolicyRiskReason>,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyCurrent {
    pub revision: u64,
    pub snapshot: PolicySnapshot,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyHistorySnapshot {
    pub current_revision: u64,
    pub entries: Vec<PolicyHistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyPreview {
    pub expected_revision: u64,
    pub current_revision: u64,
    pub revision_conflict: bool,
    pub current: PolicySnapshot,
    pub candidate: PolicySnapshot,
    pub network_ids: Vec<String>,
    pub includes_local_policy: bool,
    pub diffs: Vec<PolicyDiffEntry>,
    pub affected_objects: Vec<PolicyAffectedObject>,
    pub risk: PolicyRiskSummary,
    pub has_changes: bool,
    pub can_apply: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_target_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyChangeAction {
    Apply,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyHistoryEntry {
    pub transaction_id: String,
    pub revision: u64,
    pub previous_revision: u64,
    pub action: PolicyChangeAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_revision: Option<u64>,
    pub actor: String,
    pub source: String,
    pub occurred_at: String,
    pub local_policy_revision: Option<u64>,
    pub network_acl_revisions: BTreeMap<String, u64>,
    pub diffs: Vec<PolicyDiffEntry>,
    pub affected_objects: Vec<PolicyAffectedObject>,
    pub risk: PolicyRiskSummary,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyApplyResult {
    pub previous_revision: u64,
    pub revision: u64,
    pub no_op: bool,
    pub snapshot: PolicySnapshot,
    pub diffs: Vec<PolicyDiffEntry>,
    pub affected_objects: Vec<PolicyAffectedObject>,
    pub risk: PolicyRiskSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit: Option<PolicyHistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, JsonSchema, Serialize, Deserialize)]
pub struct PolicyRollbackResult {
    pub previous_revision: u64,
    pub revision: u64,
    pub target_revision: u64,
    pub snapshot: PolicySnapshot,
    pub diffs: Vec<PolicyDiffEntry>,
    pub affected_objects: Vec<PolicyAffectedObject>,
    pub risk: PolicyRiskSummary,
    pub audit: PolicyHistoryEntry,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyTransactionError {
    #[error("策略 revision 冲突：期望 {expected}，当前 {current}")]
    RevisionConflict { expected: u64, current: u64 },
    #[error("策略变更范围与候选内容不一致：声明 {declared:?}，实际 {actual:?}")]
    ScopeMismatch {
        declared: Vec<String>,
        actual: Vec<String>,
    },
    #[error("策略候选无效: {0}")]
    InvalidCandidate(String),
    #[error("高风险权限扩大必须显式确认")]
    ExplicitConfirmationRequired,
    #[error("策略 revision {0} 不存在")]
    RevisionNotFound(u64),
    #[error("策略持久化失败: {0}")]
    Persistence(String),
}

#[derive(Debug, Clone)]
pub(crate) struct PolicyTransactionStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredState {
    schema_version: u16,
    revision: u64,
    generation: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGeneration {
    schema_version: u16,
    snapshot: PolicySnapshot,
}

struct LockedStore {
    _lock: File,
}

struct PreparedChange {
    current: PolicySnapshot,
    candidate: PolicySnapshot,
    diffs: Vec<PolicyDiffEntry>,
    affected_objects: Vec<PolicyAffectedObject>,
    network_ids: Vec<String>,
    includes_local_policy: bool,
    risk: PolicyRiskSummary,
}

impl PolicyTransactionStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn current(
        &self,
        fallback: &PolicySnapshot,
    ) -> std::result::Result<PolicyCurrent, PolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(fallback)?;
        Ok(PolicyCurrent {
            revision: state.revision,
            snapshot: self.read_generation_locked(&state.generation)?,
        })
    }

    pub(crate) fn current_snapshot(
        &self,
        fallback: &PolicySnapshot,
    ) -> std::result::Result<PolicySnapshot, PolicyTransactionError> {
        Ok(self.current(fallback)?.snapshot)
    }

    pub(crate) fn preview(
        &self,
        fallback: &PolicySnapshot,
        expected_revision: u64,
        candidate: PolicySnapshot,
    ) -> std::result::Result<PolicyPreview, PolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(fallback)?;
        let current = self.read_generation_locked(&state.generation)?;
        let now = unix_time_ms();
        let prepared = prepare_candidate_change(current, candidate, now)?;
        Ok(preview_from_prepared(
            expected_revision,
            state.revision,
            prepared,
            None,
        ))
    }

    pub(crate) fn preview_rollback(
        &self,
        fallback: &PolicySnapshot,
        expected_revision: u64,
        target_revision: u64,
    ) -> std::result::Result<PolicyPreview, PolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(fallback)?;
        if target_revision == 0 || target_revision > state.revision {
            return Err(PolicyTransactionError::RevisionNotFound(target_revision));
        }
        let current = self.read_generation_locked(&state.generation)?;
        let target = self.read_generation_locked(&target_revision.to_string())?;
        let now = unix_time_ms();
        let candidate = restore_as_new_revision(&current, &target, now)?;
        let prepared = prepare_candidate_change(current, candidate, now)?;
        Ok(preview_from_prepared(
            expected_revision,
            state.revision,
            prepared,
            Some(target_revision),
        ))
    }

    // The scope, confirmation and audit inputs form one atomic authorization
    // boundary; grouping them would obscure which values are checked together.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply(
        &self,
        fallback: &PolicySnapshot,
        expected_revision: u64,
        candidate: PolicySnapshot,
        network_ids: &[String],
        includes_local_policy: bool,
        confirm_privilege_expansion: bool,
        actor: &str,
        source: &str,
    ) -> std::result::Result<PolicyApplyResult, PolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(fallback)?;
        if expected_revision != state.revision {
            return Err(PolicyTransactionError::RevisionConflict {
                expected: expected_revision,
                current: state.revision,
            });
        }
        let current = self.read_generation_locked(&state.generation)?;
        let now = unix_time_ms();
        let prepared = prepare_candidate_change(current, candidate, now)?;
        validate_declared_scope(&prepared, network_ids, includes_local_policy)?;
        if prepared.risk.requires_explicit_confirmation && !confirm_privilege_expansion {
            return Err(PolicyTransactionError::ExplicitConfirmationRequired);
        }
        if prepared.diffs.is_empty() {
            return Ok(PolicyApplyResult {
                previous_revision: state.revision,
                revision: state.revision,
                no_op: true,
                snapshot: prepared.current,
                diffs: Vec::new(),
                affected_objects: Vec::new(),
                risk: PolicyRiskSummary::default(),
                audit: None,
            });
        }

        let next_revision = state.revision.saturating_add(1);
        let occurred_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let transaction_id =
            transaction_id(next_revision, &prepared.candidate, actor, &occurred_at);
        let history = history_entry(
            &transaction_id,
            next_revision,
            state.revision,
            PolicyChangeAction::Apply,
            None,
            actor,
            source,
            occurred_at,
            &prepared,
        );
        self.commit_locked(next_revision, &prepared.candidate, &history)?;
        Ok(PolicyApplyResult {
            previous_revision: state.revision,
            revision: next_revision,
            no_op: false,
            snapshot: prepared.candidate,
            diffs: prepared.diffs,
            affected_objects: prepared.affected_objects,
            risk: prepared.risk,
            audit: Some(history),
        })
    }

    // Rollback carries the same checked authorization and audit boundary as
    // apply, with the target revision added explicitly.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rollback(
        &self,
        fallback: &PolicySnapshot,
        expected_revision: u64,
        target_revision: u64,
        network_ids: &[String],
        includes_local_policy: bool,
        confirm_privilege_expansion: bool,
        actor: &str,
        source: &str,
    ) -> std::result::Result<PolicyRollbackResult, PolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(fallback)?;
        if expected_revision != state.revision {
            return Err(PolicyTransactionError::RevisionConflict {
                expected: expected_revision,
                current: state.revision,
            });
        }
        if target_revision == 0 || target_revision > state.revision {
            return Err(PolicyTransactionError::RevisionNotFound(target_revision));
        }
        let current = self.read_generation_locked(&state.generation)?;
        let target = self.read_generation_locked(&target_revision.to_string())?;
        let now = unix_time_ms();
        let candidate = restore_as_new_revision(&current, &target, now)?;
        let prepared = prepare_candidate_change(current, candidate, now)?;
        validate_declared_scope(&prepared, network_ids, includes_local_policy)?;
        if prepared.risk.requires_explicit_confirmation && !confirm_privilege_expansion {
            return Err(PolicyTransactionError::ExplicitConfirmationRequired);
        }

        let next_revision = state.revision.saturating_add(1);
        let occurred_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let transaction_id =
            transaction_id(next_revision, &prepared.candidate, actor, &occurred_at);
        let history = history_entry(
            &transaction_id,
            next_revision,
            state.revision,
            PolicyChangeAction::Rollback,
            Some(target_revision),
            actor,
            source,
            occurred_at,
            &prepared,
        );
        self.commit_locked(next_revision, &prepared.candidate, &history)?;
        Ok(PolicyRollbackResult {
            previous_revision: state.revision,
            revision: next_revision,
            target_revision,
            snapshot: prepared.candidate,
            diffs: prepared.diffs,
            affected_objects: prepared.affected_objects,
            risk: prepared.risk,
            audit: history,
        })
    }

    #[cfg(test)]
    pub(crate) fn history(
        &self,
        fallback: &PolicySnapshot,
    ) -> std::result::Result<Vec<PolicyHistoryEntry>, PolicyTransactionError> {
        Ok(self.history_snapshot(fallback)?.entries)
    }

    pub(crate) fn history_snapshot(
        &self,
        fallback: &PolicySnapshot,
    ) -> std::result::Result<PolicyHistorySnapshot, PolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(fallback)?;
        let mut entries = Vec::new();
        for revision in (1..=state.revision).rev() {
            entries.push(self.read_history_locked(revision)?);
        }
        Ok(PolicyHistorySnapshot {
            current_revision: state.revision,
            entries,
        })
    }

    fn lock(&self) -> std::result::Result<LockedStore, PolicyTransactionError> {
        let store_dir = self.root.join(STORE_DIR);
        fs::create_dir_all(&store_dir).map_err(persistence)?;
        let lock_path = store_dir.join(LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(persistence)?;
        lock.lock_exclusive().map_err(persistence)?;
        Ok(LockedStore { _lock: lock })
    }

    fn ensure_initialized_locked(
        &self,
        fallback: &PolicySnapshot,
    ) -> std::result::Result<StoredState, PolicyTransactionError> {
        let store_dir = self.root.join(STORE_DIR);
        fs::create_dir_all(store_dir.join(GENERATIONS_DIR)).map_err(persistence)?;
        fs::create_dir_all(store_dir.join(HISTORY_DIR)).map_err(persistence)?;
        let state_path = store_dir.join(STATE_FILE);
        if state_path.exists() {
            let state = read_json::<StoredState>(&state_path)?;
            if state.schema_version != SCHEMA_VERSION {
                return Err(PolicyTransactionError::Persistence(format!(
                    "不支持的策略事务 schema {}",
                    state.schema_version
                )));
            }
            let generation_path = self.generation_path(&state.generation);
            if !generation_path.exists() {
                return Err(PolicyTransactionError::Persistence(format!(
                    "权威策略 generation {} 缺失",
                    state.generation
                )));
            }
            self.cleanup_unpublished_locked(&state)?;
            return Ok(state);
        }

        let snapshot = fallback.clone().normalized().map_err(invalid_candidate)?;
        snapshot
            .validate_at(unix_time_ms())
            .map_err(invalid_candidate)?;
        self.cleanup_staging_locked()?;
        let generation = "0".to_string();
        write_atomic(
            &self.generation_path(&generation),
            &StoredGeneration {
                schema_version: SCHEMA_VERSION,
                snapshot,
            },
        )?;
        let state = StoredState {
            schema_version: SCHEMA_VERSION,
            revision: 0,
            generation,
            updated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        };
        write_atomic(&state_path, &state)?;
        sync_dir(&store_dir)?;
        Ok(state)
    }

    fn read_generation_locked(
        &self,
        generation: &str,
    ) -> std::result::Result<PolicySnapshot, PolicyTransactionError> {
        let stored = read_json::<StoredGeneration>(&self.generation_path(generation))?;
        if stored.schema_version != SCHEMA_VERSION {
            return Err(PolicyTransactionError::Persistence(format!(
                "不支持的策略 generation schema {}",
                stored.schema_version
            )));
        }
        stored.snapshot.normalized().map_err(persistence)
    }

    fn read_history_locked(
        &self,
        revision: u64,
    ) -> std::result::Result<PolicyHistoryEntry, PolicyTransactionError> {
        read_json(&self.history_path(revision))
    }

    fn commit_locked(
        &self,
        revision: u64,
        snapshot: &PolicySnapshot,
        history: &PolicyHistoryEntry,
    ) -> std::result::Result<(), PolicyTransactionError> {
        let store_dir = self.root.join(STORE_DIR);
        let staging_dir = store_dir.join(format!(".staging-{revision}"));
        if staging_dir.exists() {
            fs::remove_dir_all(&staging_dir).map_err(persistence)?;
        }
        fs::create_dir_all(&staging_dir).map_err(persistence)?;
        let staged_generation = staging_dir.join("generation.json");
        let staged_history = staging_dir.join("history.json");
        let staged_state = staging_dir.join(STATE_FILE);
        write_json_sync(
            &staged_generation,
            &StoredGeneration {
                schema_version: SCHEMA_VERSION,
                snapshot: snapshot.clone(),
            },
        )?;
        write_json_sync(&staged_history, history)?;
        let state = StoredState {
            schema_version: SCHEMA_VERSION,
            revision,
            generation: revision.to_string(),
            updated_at: history.occurred_at.clone(),
        };
        write_json_sync(&staged_state, &state)?;

        fs::create_dir_all(store_dir.join(GENERATIONS_DIR)).map_err(persistence)?;
        fs::create_dir_all(store_dir.join(HISTORY_DIR)).map_err(persistence)?;
        fs::rename(
            &staged_generation,
            self.generation_path(&revision.to_string()),
        )
        .map_err(persistence)?;
        fs::rename(&staged_history, self.history_path(revision)).map_err(persistence)?;
        sync_dir(&store_dir.join(GENERATIONS_DIR))?;
        sync_dir(&store_dir.join(HISTORY_DIR))?;

        // The authoritative pointer moves last. A crash before this rename
        // leaves the previous generation live; the unpublished files are
        // removed during the next recovery pass.
        fs::rename(&staged_state, store_dir.join(STATE_FILE)).map_err(persistence)?;
        sync_dir(&store_dir)?;
        fs::remove_dir_all(&staging_dir).map_err(persistence)?;
        Ok(())
    }

    fn cleanup_unpublished_locked(
        &self,
        state: &StoredState,
    ) -> std::result::Result<(), PolicyTransactionError> {
        self.cleanup_staging_locked()?;
        let generations = self.root.join(STORE_DIR).join(GENERATIONS_DIR);
        if let Ok(entries) = fs::read_dir(&generations) {
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let Some(revision) = name
                    .strip_suffix(".json")
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    continue;
                };
                if revision > state.revision {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        let history = self.root.join(STORE_DIR).join(HISTORY_DIR);
        if let Ok(entries) = fs::read_dir(&history) {
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let Some(revision) = name
                    .strip_suffix(".json")
                    .and_then(|value| value.parse::<u64>().ok())
                else {
                    continue;
                };
                if revision > state.revision {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        Ok(())
    }

    fn cleanup_staging_locked(&self) -> std::result::Result<(), PolicyTransactionError> {
        let store_dir = self.root.join(STORE_DIR);
        if let Ok(entries) = fs::read_dir(&store_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with(".staging-") {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        Ok(())
    }

    fn generation_path(&self, generation: &str) -> PathBuf {
        self.root
            .join(STORE_DIR)
            .join(GENERATIONS_DIR)
            .join(format!("{generation}.json"))
    }

    fn history_path(&self, revision: u64) -> PathBuf {
        self.root
            .join(STORE_DIR)
            .join(HISTORY_DIR)
            .join(format!("{revision}.json"))
    }
}

fn prepare_candidate_change(
    current: PolicySnapshot,
    candidate: PolicySnapshot,
    now_unix_ms: u64,
) -> std::result::Result<PreparedChange, PolicyTransactionError> {
    let prepared = calculate_change(current, candidate, now_unix_ms)?;
    if !prepared.diffs.is_empty() {
        validate_candidate_transition(&prepared.current, &prepared.candidate, now_unix_ms)?;
    }
    Ok(prepared)
}

fn calculate_change(
    current: PolicySnapshot,
    candidate: PolicySnapshot,
    now_unix_ms: u64,
) -> std::result::Result<PreparedChange, PolicyTransactionError> {
    let current = current.normalized().map_err(persistence)?;
    let candidate = candidate.normalized().map_err(invalid_candidate)?;
    candidate
        .validate_at(now_unix_ms)
        .map_err(invalid_candidate)?;
    let (diffs, affected_objects) = diff_snapshots(&current, &candidate)?;
    let network_ids = affected_network_ids(&affected_objects);
    let includes_local_policy = affected_objects.iter().any(|object| {
        object.kind == PolicyObjectKind::LocalPolicy
            || object.kind == PolicyObjectKind::NetworkScope
    });
    let risk = risk_summary(&current, &candidate);
    Ok(PreparedChange {
        current,
        candidate,
        diffs,
        affected_objects,
        network_ids,
        includes_local_policy,
        risk,
    })
}

fn validate_declared_scope(
    prepared: &PreparedChange,
    declared_network_ids: &[String],
    includes_local_policy: bool,
) -> std::result::Result<(), PolicyTransactionError> {
    let declared_network_ids = normalized_network_ids(declared_network_ids)?;
    if prepared.network_ids != declared_network_ids
        || prepared.includes_local_policy != includes_local_policy
    {
        return Err(PolicyTransactionError::ScopeMismatch {
            declared: declared_network_ids,
            actual: prepared.network_ids.clone(),
        });
    }
    Ok(())
}

fn preview_from_prepared(
    expected_revision: u64,
    current_revision: u64,
    prepared: PreparedChange,
    rollback_target_revision: Option<u64>,
) -> PolicyPreview {
    let revision_conflict = expected_revision != current_revision;
    PolicyPreview {
        expected_revision,
        current_revision,
        revision_conflict,
        current: prepared.current,
        candidate: prepared.candidate,
        network_ids: prepared.network_ids,
        includes_local_policy: prepared.includes_local_policy,
        has_changes: !prepared.diffs.is_empty(),
        can_apply: !revision_conflict,
        diffs: prepared.diffs,
        affected_objects: prepared.affected_objects,
        risk: prepared.risk,
        rollback_target_revision,
    }
}

fn normalized_network_ids(
    network_ids: &[String],
) -> std::result::Result<Vec<String>, PolicyTransactionError> {
    let mut normalized = Vec::new();
    for network_id in network_ids {
        NetworkId::parse(network_id.clone()).map_err(|error| {
            PolicyTransactionError::InvalidCandidate(format!(
                "网络 ID [{network_id}] 无效: {error}"
            ))
        })?;
        normalized.push(network_id.clone());
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn affected_network_ids(affected: &[PolicyAffectedObject]) -> Vec<String> {
    affected
        .iter()
        .filter_map(|object| object.network_id.clone())
        .filter(|network_id| object_is_top_level_acl(affected, network_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn object_is_top_level_acl(affected: &[PolicyAffectedObject], network_id: &str) -> bool {
    affected.iter().any(|object| {
        object.network_id.as_deref() == Some(network_id)
            && object.kind == PolicyObjectKind::NetworkAcl
    })
}

fn diff_snapshots(
    current: &PolicySnapshot,
    candidate: &PolicySnapshot,
) -> std::result::Result<(Vec<PolicyDiffEntry>, Vec<PolicyAffectedObject>), PolicyTransactionError>
{
    let mut diffs = Vec::new();
    let mut affected = BTreeSet::new();

    if current.local_policy != candidate.local_policy {
        let change = if current.local_policy.is_none() {
            PolicyChangeKind::LocalPolicyAdded
        } else if candidate.local_policy.is_none() {
            PolicyChangeKind::LocalPolicyRemoved
        } else {
            PolicyChangeKind::LocalPolicyUpdated
        };
        diffs.push(PolicyDiffEntry {
            path: "/local_policy".into(),
            change,
            before: option_value(&current.local_policy)?,
            after: option_value(&candidate.local_policy)?,
        });
        affected.insert(PolicyAffectedObject {
            kind: PolicyObjectKind::LocalPolicy,
            network_id: None,
            object_id: current
                .local_policy
                .as_ref()
                .or(candidate.local_policy.as_ref())
                .map(|policy| policy.node_id.to_string()),
        });
        diff_local_details(
            current.local_policy.as_ref(),
            candidate.local_policy.as_ref(),
            &mut diffs,
            &mut affected,
        )?;
    }

    let current_acls = current
        .network_acls
        .iter()
        .map(|acl| (acl.network_id.clone(), acl))
        .collect::<BTreeMap<_, _>>();
    let candidate_acls = candidate
        .network_acls
        .iter()
        .map(|acl| (acl.network_id.clone(), acl))
        .collect::<BTreeMap<_, _>>();
    let network_ids = current_acls
        .keys()
        .chain(candidate_acls.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for network_id in network_ids {
        let before = current_acls.get(&network_id).copied();
        let after = candidate_acls.get(&network_id).copied();
        if before == after {
            continue;
        }
        let change = if before.is_none() {
            PolicyChangeKind::NetworkAclAdded
        } else if after.is_none() {
            PolicyChangeKind::NetworkAclRemoved
        } else {
            PolicyChangeKind::NetworkAclUpdated
        };
        diffs.push(PolicyDiffEntry {
            path: format!("/network_acls/{}", escape_json_pointer(network_id.as_str())),
            change,
            before: option_value(&before)?,
            after: option_value(&after)?,
        });
        affected.insert(PolicyAffectedObject {
            kind: PolicyObjectKind::NetworkAcl,
            network_id: Some(network_id.to_string()),
            object_id: None,
        });
        diff_acl_details(&network_id, before, after, &mut diffs, &mut affected)?;
    }

    Ok((diffs, affected.into_iter().collect()))
}

fn diff_local_details(
    before: Option<&NodeLocalAccessPolicy>,
    after: Option<&NodeLocalAccessPolicy>,
    diffs: &mut Vec<PolicyDiffEntry>,
    affected: &mut BTreeSet<PolicyAffectedObject>,
) -> std::result::Result<(), PolicyTransactionError> {
    let (Some(before), Some(after)) = (before, after) else {
        return Ok(());
    };
    if before.network_scope != after.network_scope {
        diffs.push(PolicyDiffEntry {
            path: "/local_policy/network_scope".into(),
            change: PolicyChangeKind::NetworkScopeChanged,
            before: Some(serde_json::to_value(&before.network_scope).map_err(persistence)?),
            after: Some(serde_json::to_value(&after.network_scope).map_err(persistence)?),
        });
        affected.insert(PolicyAffectedObject {
            kind: PolicyObjectKind::NetworkScope,
            network_id: None,
            object_id: None,
        });
    }
    if before.issued_at_unix_ms != after.issued_at_unix_ms
        || before.expires_at_unix_ms != after.expires_at_unix_ms
    {
        diffs.push(PolicyDiffEntry {
            path: "/local_policy/validity".into(),
            change: PolicyChangeKind::ValidityChanged,
            before: Some(serde_json::json!({
                "issued_at_unix_ms": before.issued_at_unix_ms,
                "expires_at_unix_ms": before.expires_at_unix_ms,
            })),
            after: Some(serde_json::json!({
                "issued_at_unix_ms": after.issued_at_unix_ms,
                "expires_at_unix_ms": after.expires_at_unix_ms,
            })),
        });
    }
    diff_rules(
        "/local_policy/rules",
        before.rules.iter().map(|rule| (&rule.rule_id, rule)),
        after.rules.iter().map(|rule| (&rule.rule_id, rule)),
        diffs,
        affected,
        None,
    )?;
    diff_vetoes(before, after, diffs, affected)?;
    Ok(())
}

fn diff_acl_details(
    network_id: &NetworkId,
    before: Option<&NetworkAcl>,
    after: Option<&NetworkAcl>,
    diffs: &mut Vec<PolicyDiffEntry>,
    affected: &mut BTreeSet<PolicyAffectedObject>,
) -> std::result::Result<(), PolicyTransactionError> {
    let (Some(before), Some(after)) = (before, after) else {
        return Ok(());
    };
    let prefix = format!(
        "/network_acls/{}/",
        escape_json_pointer(network_id.as_str())
    );
    if before.issued_at_unix_ms != after.issued_at_unix_ms
        || before.expires_at_unix_ms != after.expires_at_unix_ms
    {
        diffs.push(PolicyDiffEntry {
            path: format!("{prefix}validity"),
            change: PolicyChangeKind::ValidityChanged,
            before: Some(serde_json::json!({
                "issued_at_unix_ms": before.issued_at_unix_ms,
                "expires_at_unix_ms": before.expires_at_unix_ms,
            })),
            after: Some(serde_json::json!({
                "issued_at_unix_ms": after.issued_at_unix_ms,
                "expires_at_unix_ms": after.expires_at_unix_ms,
            })),
        });
    }
    diff_rules(
        &format!("{prefix}rules"),
        before.rules.iter().map(|rule| (&rule.rule_id, rule)),
        after.rules.iter().map(|rule| (&rule.rule_id, rule)),
        diffs,
        affected,
        Some(network_id),
    )?;
    diff_grants(
        network_id,
        &prefix,
        before.grants.iter().map(|grant| (&grant.grant_id, grant)),
        after.grants.iter().map(|grant| (&grant.grant_id, grant)),
        diffs,
        affected,
    )?;
    Ok(())
}

fn diff_rules<'a>(
    path: &str,
    before: impl Iterator<
        Item = (
            &'a fuckipv6_common::domain::SecurityRuleId,
            &'a PermissionRule,
        ),
    >,
    after: impl Iterator<
        Item = (
            &'a fuckipv6_common::domain::SecurityRuleId,
            &'a PermissionRule,
        ),
    >,
    diffs: &mut Vec<PolicyDiffEntry>,
    affected: &mut BTreeSet<PolicyAffectedObject>,
    network_id: Option<&NetworkId>,
) -> std::result::Result<(), PolicyTransactionError> {
    let before = before
        .map(|(id, rule)| (id.to_string(), rule))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .map(|(id, rule)| (id.to_string(), rule))
        .collect::<BTreeMap<_, _>>();
    let ids = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for id in ids {
        let before_rule = before.get(&id).copied();
        let after_rule = after.get(&id).copied();
        if before_rule == after_rule {
            continue;
        }
        let change = if before_rule.is_none() {
            PolicyChangeKind::PermissionRuleAdded
        } else if after_rule.is_none() {
            PolicyChangeKind::PermissionRuleRemoved
        } else {
            PolicyChangeKind::PermissionRuleUpdated
        };
        diffs.push(PolicyDiffEntry {
            path: format!("{path}/{}", escape_json_pointer(&id)),
            change,
            before: option_value(&before_rule)?,
            after: option_value(&after_rule)?,
        });
        affected.insert(PolicyAffectedObject {
            kind: PolicyObjectKind::PermissionRule,
            network_id: network_id.map(ToString::to_string),
            object_id: Some(id),
        });
    }
    Ok(())
}

fn diff_grants<'a>(
    network_id: &NetworkId,
    prefix: &str,
    before: impl Iterator<
        Item = (
            &'a fuckipv6_common::domain::SecurityRuleId,
            &'a NetworkAclGrant,
        ),
    >,
    after: impl Iterator<
        Item = (
            &'a fuckipv6_common::domain::SecurityRuleId,
            &'a NetworkAclGrant,
        ),
    >,
    diffs: &mut Vec<PolicyDiffEntry>,
    affected: &mut BTreeSet<PolicyAffectedObject>,
) -> std::result::Result<(), PolicyTransactionError> {
    let before = before
        .map(|(id, grant)| (id.to_string(), grant))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .map(|(id, grant)| (id.to_string(), grant))
        .collect::<BTreeMap<_, _>>();
    let ids = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for id in ids {
        let before_grant = before.get(&id).copied();
        let after_grant = after.get(&id).copied();
        if before_grant == after_grant {
            continue;
        }
        let change = if before_grant.is_none() {
            PolicyChangeKind::GrantAdded
        } else if after_grant.is_none() {
            PolicyChangeKind::GrantRemoved
        } else {
            PolicyChangeKind::GrantUpdated
        };
        diffs.push(PolicyDiffEntry {
            path: format!("{prefix}grants/{}", escape_json_pointer(&id)),
            change,
            before: option_value(&before_grant)?,
            after: option_value(&after_grant)?,
        });
        affected.insert(PolicyAffectedObject {
            kind: PolicyObjectKind::NetworkAclGrant,
            network_id: Some(network_id.to_string()),
            object_id: Some(id),
        });
    }
    Ok(())
}

fn diff_vetoes(
    before: &NodeLocalAccessPolicy,
    after: &NodeLocalAccessPolicy,
    diffs: &mut Vec<PolicyDiffEntry>,
    affected: &mut BTreeSet<PolicyAffectedObject>,
) -> std::result::Result<(), PolicyTransactionError> {
    let before = before
        .contribution_vetoes
        .iter()
        .map(|veto| (veto.veto_id.to_string(), veto))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .contribution_vetoes
        .iter()
        .map(|veto| (veto.veto_id.to_string(), veto))
        .collect::<BTreeMap<_, _>>();
    let ids = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for id in ids {
        let before_veto = before.get(&id).copied();
        let after_veto = after.get(&id).copied();
        if before_veto == after_veto {
            continue;
        }
        let change = if before_veto.is_none() {
            PolicyChangeKind::VetoAdded
        } else if after_veto.is_none() {
            PolicyChangeKind::VetoRemoved
        } else {
            PolicyChangeKind::VetoUpdated
        };
        diffs.push(PolicyDiffEntry {
            path: format!(
                "/local_policy/contribution_vetoes/{}",
                escape_json_pointer(&id)
            ),
            change,
            before: option_value(&before_veto)?,
            after: option_value(&after_veto)?,
        });
        affected.insert(PolicyAffectedObject {
            kind: PolicyObjectKind::ContributionVeto,
            network_id: None,
            object_id: Some(id),
        });
    }
    Ok(())
}

fn risk_summary(current: &PolicySnapshot, candidate: &PolicySnapshot) -> PolicyRiskSummary {
    let mut reasons = BTreeSet::new();
    match (&current.local_policy, &candidate.local_policy) {
        (Some(_), None) => {
            reasons.insert(PolicyRiskReason::LocalPolicyRemoved);
        }
        (before, Some(after)) => {
            if let Some(before) = before {
                if scope_expanded(&before.network_scope, &after.network_scope) {
                    reasons.insert(PolicyRiskReason::NetworkScopeExpanded);
                }
                let before_vetoes = before
                    .contribution_vetoes
                    .iter()
                    .map(|veto| &veto.veto_id)
                    .collect::<BTreeSet<_>>();
                if after
                    .contribution_vetoes
                    .iter()
                    .any(|veto| !before_vetoes.contains(&veto.veto_id))
                {
                    // This is a restriction, not an expansion.
                }
                if before.contribution_vetoes.iter().any(|veto| {
                    !after
                        .contribution_vetoes
                        .iter()
                        .any(|next| next.veto_id == veto.veto_id)
                }) {
                    reasons.insert(PolicyRiskReason::ContributionVetoRemoved);
                }
                if after.expires_at_unix_ms > before.expires_at_unix_ms {
                    reasons.insert(PolicyRiskReason::PolicyValidityExtended);
                }
                risk_rules(&before.rules, &after.rules, &mut reasons);
            } else {
                if after
                    .rules
                    .iter()
                    .any(|rule| rule.effect == PermissionEffect::Allow)
                {
                    reasons.insert(PolicyRiskReason::AllowRuleAdded);
                }
                if after.contribution_vetoes.is_empty() {
                    // A newly added local policy without vetoes is not by itself
                    // evidence of expansion; its rules are checked above.
                }
            }
        }
        (None, None) => {}
    }

    let current_acls = current
        .network_acls
        .iter()
        .map(|acl| (&acl.network_id, acl))
        .collect::<BTreeMap<_, _>>();
    let candidate_acls = candidate
        .network_acls
        .iter()
        .map(|acl| (&acl.network_id, acl))
        .collect::<BTreeMap<_, _>>();
    for network_id in current_acls.keys().chain(candidate_acls.keys()) {
        match (current_acls.get(network_id), candidate_acls.get(network_id)) {
            (Some(_), None) => {
                reasons.insert(PolicyRiskReason::NetworkAclRemoved);
            }
            (Some(before), Some(after)) => {
                if after.expires_at_unix_ms > before.expires_at_unix_ms {
                    reasons.insert(PolicyRiskReason::PolicyValidityExtended);
                }
                risk_rules(&before.rules, &after.rules, &mut reasons);
                risk_grants(&before.grants, &after.grants, &mut reasons);
            }
            (None, Some(after)) => {
                if after
                    .rules
                    .iter()
                    .any(|rule| rule.effect == PermissionEffect::Allow)
                    || after.grants.iter().any(|grant| {
                        grant.revoked_at_unix_ms.is_none()
                            && grant.expires_at_unix_ms > unix_time_ms()
                    })
                {
                    reasons.insert(PolicyRiskReason::AllowRuleAdded);
                }
            }
            (None, None) => {}
        }
    }

    let reasons = reasons.into_iter().collect::<Vec<_>>();
    let privilege_expansion = !reasons.is_empty();
    PolicyRiskSummary {
        level: if privilege_expansion {
            PolicyRiskLevel::High
        } else if current != candidate {
            PolicyRiskLevel::Elevated
        } else {
            PolicyRiskLevel::None
        },
        privilege_expansion,
        requires_recent_reauthentication: privilege_expansion,
        requires_explicit_confirmation: privilege_expansion,
        reasons,
    }
}

fn risk_rules(
    before: &[PermissionRule],
    after: &[PermissionRule],
    reasons: &mut BTreeSet<PolicyRiskReason>,
) {
    let before = before
        .iter()
        .map(|rule| (&rule.rule_id, rule))
        .collect::<BTreeMap<_, _>>();
    for next in after {
        match before.get(&next.rule_id) {
            None if next.effect == PermissionEffect::Allow => {
                reasons.insert(PolicyRiskReason::AllowRuleAdded);
            }
            Some(previous) => {
                if previous.effect == PermissionEffect::Deny
                    && next.effect == PermissionEffect::Allow
                {
                    reasons.insert(PolicyRiskReason::DenyRuleRemoved);
                    continue;
                }
                if next.effect == PermissionEffect::Allow && rule_expanded(previous, next) {
                    reasons.insert(PolicyRiskReason::PermissionRuleExpanded);
                    if previous
                        .conditions
                        .contains(&PermissionCondition::RecentReauthentication)
                        && !next
                            .conditions
                            .contains(&PermissionCondition::RecentReauthentication)
                    {
                        reasons.insert(PolicyRiskReason::GrantReauthenticationRemoved);
                    }
                }
                if previous.effect == PermissionEffect::Deny
                    && (next.subjects.len() < previous.subjects.len()
                        || next.resources.len() < previous.resources.len()
                        || next.actions.len() < previous.actions.len())
                {
                    reasons.insert(PolicyRiskReason::DenyRuleRemoved);
                }
            }
            None => {}
        }
    }
    let after_ids = after
        .iter()
        .map(|rule| &rule.rule_id)
        .collect::<BTreeSet<_>>();
    if before
        .values()
        .any(|rule| rule.effect == PermissionEffect::Deny && !after_ids.contains(&rule.rule_id))
    {
        reasons.insert(PolicyRiskReason::DenyRuleRemoved);
    }
}

fn risk_grants(
    before: &[NetworkAclGrant],
    after: &[NetworkAclGrant],
    reasons: &mut BTreeSet<PolicyRiskReason>,
) {
    let before = before
        .iter()
        .map(|grant| (&grant.grant_id, grant))
        .collect::<BTreeMap<_, _>>();
    for next in after {
        match before.get(&next.grant_id) {
            None => {
                reasons.insert(PolicyRiskReason::NetworkAclGrantAdded);
            }
            Some(previous) => {
                if previous.revoked_at_unix_ms.is_some() && next.revoked_at_unix_ms.is_none() {
                    reasons.insert(PolicyRiskReason::NetworkAclGrantExpanded);
                }
                if selector_set_expanded(&previous.subjects, &next.subjects)
                    || next.resources.len() > previous.resources.len()
                    || next.actions.len() > previous.actions.len()
                {
                    reasons.insert(PolicyRiskReason::NetworkAclGrantExpanded);
                }
                if previous.requires_recent_reauthentication
                    && !next.requires_recent_reauthentication
                {
                    reasons.insert(PolicyRiskReason::GrantReauthenticationRemoved);
                }
                if next.expires_at_unix_ms > previous.expires_at_unix_ms {
                    reasons.insert(PolicyRiskReason::GrantValidityExtended);
                }
            }
        }
    }
}

fn rule_expanded(before: &PermissionRule, after: &PermissionRule) -> bool {
    selector_set_expanded(&before.subjects, &after.subjects)
        || after
            .resources
            .iter()
            .any(|resource| !before.resources.contains(resource))
        || after
            .actions
            .iter()
            .any(|action| !before.actions.contains(action))
        || before
            .conditions
            .iter()
            .any(|condition| !after.conditions.contains(condition))
}

fn selector_set_expanded(before: &[SubjectSelector], after: &[SubjectSelector]) -> bool {
    let before = before.iter().collect::<BTreeSet<_>>();
    after.iter().any(|subject| match subject {
        SubjectSelector::AnyMember => !before.contains(subject),
        SubjectSelector::Role(_) | SubjectSelector::Node(_) | SubjectSelector::Group(_) => {
            !before.contains(subject)
        }
    })
}

fn scope_expanded(
    before: &fuckipv6_common::domain::NodeNetworkScope,
    after: &fuckipv6_common::domain::NodeNetworkScope,
) -> bool {
    use fuckipv6_common::domain::NodeNetworkScope;
    match (before, after) {
        (NodeNetworkScope::All, _) => false,
        (NodeNetworkScope::Only(_), NodeNetworkScope::All) => true,
        (NodeNetworkScope::Only(before), NodeNetworkScope::Only(after)) => {
            after.iter().any(|network| !before.contains(network))
        }
    }
}

fn validate_candidate_transition(
    current: &PolicySnapshot,
    candidate: &PolicySnapshot,
    now_unix_ms: u64,
) -> std::result::Result<(), PolicyTransactionError> {
    candidate
        .validate_at(now_unix_ms)
        .map_err(invalid_candidate)?;
    match (&current.local_policy, &candidate.local_policy) {
        (Some(before), Some(after)) => {
            before
                .prepare_update(after.clone(), now_unix_ms)
                .map_err(invalid_candidate)?;
        }
        (None, Some(after)) if after.revision != 1 => {
            return Err(PolicyTransactionError::InvalidCandidate(
                "新增本地策略 revision 必须为 1".into(),
            ));
        }
        _ => {}
    }
    for acl in &candidate.network_acls {
        match current.acl(&acl.network_id) {
            Some(before) => {
                before
                    .prepare_update(acl.clone(), now_unix_ms)
                    .map_err(invalid_candidate)?;
            }
            None if acl.revision != 1 => {
                return Err(PolicyTransactionError::InvalidCandidate(format!(
                    "新增网络 ACL [{}] revision 必须为 1",
                    acl.network_id
                )));
            }
            None => {}
        }
    }
    Ok(())
}

fn restore_as_new_revision(
    current: &PolicySnapshot,
    target: &PolicySnapshot,
    now_unix_ms: u64,
) -> std::result::Result<PolicySnapshot, PolicyTransactionError> {
    let mut restored = target.clone().normalized().map_err(persistence)?;
    if let Some(local) = restored.local_policy.as_mut() {
        let duration = local
            .expires_at_unix_ms
            .saturating_sub(local.issued_at_unix_ms)
            .max(1);
        local.revision = current
            .local_policy
            .as_ref()
            .map(|policy| policy.revision.saturating_add(1))
            .unwrap_or(1);
        local.issued_at_unix_ms = now_unix_ms;
        local.expires_at_unix_ms = now_unix_ms.saturating_add(duration);
    }
    for acl in &mut restored.network_acls {
        let duration = acl
            .expires_at_unix_ms
            .saturating_sub(acl.issued_at_unix_ms)
            .max(1);
        let offset = now_unix_ms.saturating_sub(acl.issued_at_unix_ms);
        acl.revision = current
            .acl(&acl.network_id)
            .map(|policy| policy.revision.saturating_add(1))
            .unwrap_or(1);
        acl.issued_at_unix_ms = now_unix_ms;
        acl.expires_at_unix_ms = now_unix_ms.saturating_add(duration);
        // The domain invariant requires every grant to be issued no earlier
        // than its containing ACL revision. Move the complete grant window by
        // the same offset so restoring an old ACL neither violates that
        // invariant nor silently shortens/lengthens a grant.
        for grant in &mut acl.grants {
            grant.issued_at_unix_ms = grant.issued_at_unix_ms.saturating_add(offset);
            grant.expires_at_unix_ms = grant.expires_at_unix_ms.saturating_add(offset);
            if let Some(revoked_at) = grant.revoked_at_unix_ms.as_mut() {
                *revoked_at = revoked_at.saturating_add(offset);
            }
        }
    }
    restored
        .validate_at(now_unix_ms)
        .map_err(invalid_candidate)?;
    Ok(restored)
}

// Keeping the complete audit record construction explicit prevents a caller
// from deciding which trust-boundary fields are omitted.
#[allow(clippy::too_many_arguments)]
fn history_entry(
    transaction_id: &str,
    revision: u64,
    previous_revision: u64,
    action: PolicyChangeAction,
    target_revision: Option<u64>,
    actor: &str,
    source: &str,
    occurred_at: String,
    prepared: &PreparedChange,
) -> PolicyHistoryEntry {
    PolicyHistoryEntry {
        transaction_id: transaction_id.into(),
        revision,
        previous_revision,
        action,
        target_revision,
        actor: actor.into(),
        source: source.into(),
        occurred_at,
        local_policy_revision: prepared
            .candidate
            .local_policy
            .as_ref()
            .map(|policy| policy.revision),
        network_acl_revisions: prepared
            .candidate
            .network_acls
            .iter()
            .map(|acl| (acl.network_id.to_string(), acl.revision))
            .collect(),
        diffs: prepared.diffs.clone(),
        affected_objects: prepared.affected_objects.clone(),
        risk: prepared.risk.clone(),
    }
}

fn transaction_id(
    revision: u64,
    snapshot: &PolicySnapshot,
    actor: &str,
    occurred_at: &str,
) -> String {
    let encoded = serde_json::to_vec(snapshot).unwrap_or_default();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&encoded);
    hasher.update(actor.as_bytes());
    hasher.update(occurred_at.as_bytes());
    format!(
        "policy-txn-{revision}-{}",
        &hasher.finalize().to_hex()[..16]
    )
}

fn option_value<T: Serialize>(
    value: &Option<T>,
) -> std::result::Result<Option<Value>, PolicyTransactionError> {
    value
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(persistence)
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn read_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> std::result::Result<T, PolicyTransactionError> {
    let bytes = fs::read(path).map_err(persistence)?;
    serde_json::from_slice(&bytes).map_err(persistence)
}

fn write_atomic<T: Serialize>(
    path: &Path,
    value: &T,
) -> std::result::Result<(), PolicyTransactionError> {
    let parent = path
        .parent()
        .ok_or_else(|| PolicyTransactionError::Persistence("路径没有父目录".into()))?;
    fs::create_dir_all(parent).map_err(persistence)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("policy"),
        std::process::id()
    ));
    write_json_sync(&temporary, value)?;
    fs::rename(&temporary, path).map_err(persistence)?;
    sync_dir(parent)?;
    Ok(())
}

fn write_json_sync<T: Serialize>(
    path: &Path,
    value: &T,
) -> std::result::Result<(), PolicyTransactionError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(persistence)?;
    let mut file = File::create(path).map_err(persistence)?;
    file.write_all(&bytes).map_err(persistence)?;
    file.write_all(b"\n").map_err(persistence)?;
    file.sync_all().map_err(persistence)?;
    Ok(())
}

fn sync_dir(path: &Path) -> std::result::Result<(), PolicyTransactionError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(persistence)?;
    }
    Ok(())
}

fn persistence(error: impl std::fmt::Display) -> PolicyTransactionError {
    PolicyTransactionError::Persistence(error.to_string())
}

fn invalid_candidate(error: impl std::fmt::Display) -> PolicyTransactionError {
    PolicyTransactionError::InvalidCandidate(error.to_string())
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{NetworkId, NodeId, PolicyAction, ResourceKind, SecurityRuleId};

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fuckipv6-policy-txn-{name}-{}-{}",
            std::process::id(),
            unix_time_ms()
        ))
    }

    fn network_id(name: &str) -> NetworkId {
        NetworkId::from_public_key(name.as_bytes())
    }

    fn rule(
        name: &str,
        effect: PermissionEffect,
        action: PolicyAction,
        resource: ResourceKind,
        conditions: Vec<PermissionCondition>,
    ) -> PermissionRule {
        PermissionRule {
            rule_id: SecurityRuleId::from_public_key(name.as_bytes()),
            effect,
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![resource],
            actions: vec![action],
            conditions,
        }
    }

    fn local(revision: u64, rules: Vec<PermissionRule>) -> NodeLocalAccessPolicy {
        let now = unix_time_ms();
        NodeLocalAccessPolicy {
            node_id: NodeId::from_public_key(b"policy-owner"),
            revision,
            issued_at_unix_ms: now.saturating_sub(60_000),
            expires_at_unix_ms: now + 86_400_000,
            network_scope: fuckipv6_common::domain::NodeNetworkScope::Only(vec![network_id(
                "home",
            )]),
            contribution_vetoes: Vec::new(),
            exit_contribution: None,
            relay_limits: Default::default(),
            rules,
        }
    }

    fn acl(revision: u64, rules: Vec<PermissionRule>) -> NetworkAcl {
        let now = unix_time_ms();
        NetworkAcl {
            network_id: network_id("home"),
            revision,
            issued_at_unix_ms: now.saturating_sub(60_000),
            expires_at_unix_ms: now + 86_400_000,
            rules,
            grants: Vec::new(),
            relay_limits: Default::default(),
        }
    }

    fn initial() -> PolicySnapshot {
        PolicySnapshot {
            local_policy: Some(local(
                1,
                vec![rule(
                    "local-read",
                    PermissionEffect::Allow,
                    PolicyAction::Read,
                    ResourceKind::Network,
                    vec![PermissionCondition::RecentReauthentication],
                )],
            )),
            network_acls: vec![acl(
                1,
                vec![rule(
                    "acl-read",
                    PermissionEffect::Allow,
                    PolicyAction::Read,
                    ResourceKind::Network,
                    Vec::new(),
                )],
            )],
        }
    }

    fn expanded_candidate() -> PolicySnapshot {
        let mut candidate = initial();
        candidate.local_policy.as_mut().unwrap().revision = 2;
        candidate.local_policy.as_mut().unwrap().rules = vec![rule(
            "local-read",
            PermissionEffect::Allow,
            PolicyAction::Update,
            ResourceKind::SecurityPolicy,
            Vec::new(),
        )];
        let acl = candidate.network_acls.get_mut(0).unwrap();
        acl.revision = 2;
        acl.grants.push(NetworkAclGrant {
            grant_id: SecurityRuleId::from_public_key(b"grant"),
            subjects: vec![SubjectSelector::AnyMember],
            resources: vec![ResourceKind::SecurityPolicy],
            actions: vec![PolicyAction::Update],
            issued_at_unix_ms: acl.issued_at_unix_ms,
            expires_at_unix_ms: acl.expires_at_unix_ms,
            revoked_at_unix_ms: None,
            revoked_by: None,
            requires_recent_reauthentication: true,
        });
        candidate
    }

    #[test]
    fn preview_reports_diff_scope_and_high_risk_expansion() {
        let root = temp_root("preview");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let preview = store.preview(&base, 0, expanded_candidate()).unwrap();
        assert!(!preview.revision_conflict);
        assert!(preview.has_changes);
        assert!(preview.can_apply);
        assert_eq!(preview.risk.level, PolicyRiskLevel::High);
        assert!(preview
            .risk
            .reasons
            .contains(&PolicyRiskReason::PermissionRuleExpanded));
        assert!(preview
            .risk
            .reasons
            .contains(&PolicyRiskReason::NetworkAclGrantAdded));
        assert!(preview
            .affected_objects
            .iter()
            .any(|object| object.network_id.as_deref() == Some(network_id("home").as_str())));
        assert!(root.join(STORE_DIR).join(STATE_FILE).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn first_preview_reports_actual_scope_without_requiring_declared_scope() {
        let root = temp_root("preview-scope");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        let candidate = expanded_candidate();

        let preview = store.preview(&base, 0, candidate.clone()).unwrap();

        assert_eq!(preview.network_ids, vec![network_id("home").to_string()]);
        assert!(preview.includes_local_policy);
        assert!(preview.has_changes);
        assert_eq!(preview.current, base);
        assert_eq!(preview.candidate, candidate);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_rollback_reports_new_revision_diff_scope_and_risk_without_writing() {
        let root = temp_root("rollback-preview");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let applied = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "apply",
            )
            .unwrap();

        let preview = store.preview_rollback(&base, applied.revision, 1).unwrap();

        assert_eq!(preview.rollback_target_revision, Some(1));
        assert_eq!(preview.current_revision, applied.revision);
        assert!(preview.has_changes);
        assert!(preview.can_apply);
        assert!(!preview.diffs.is_empty());
        assert_eq!(preview.risk.level, PolicyRiskLevel::High);
        assert!(preview.affected_objects.iter().any(|object| {
            object.kind == PolicyObjectKind::LocalPolicy && object.object_id.is_some()
        }));
        assert_eq!(store.current(&base).unwrap().revision, applied.revision);
        assert_eq!(store.history(&base).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn high_risk_rollback_requires_explicit_confirmation_and_keeps_current() {
        let root = temp_root("rollback-confirm");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let applied = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "apply",
            )
            .unwrap();

        let error = store
            .rollback(
                &base,
                applied.revision,
                1,
                &[network_id("home").to_string()],
                true,
                false,
                "owner",
                "rollback",
            )
            .unwrap_err();

        assert!(matches!(
            error,
            PolicyTransactionError::ExplicitConfirmationRequired
        ));
        assert_eq!(store.current_snapshot(&base).unwrap(), applied.snapshot);
        assert_eq!(store.history(&base).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_preview_reports_conflict_and_cannot_apply() {
        let root = temp_root("stale-preview");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let applied = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "apply",
            )
            .unwrap();

        let mut candidate = applied.snapshot.clone();
        candidate.local_policy.as_mut().unwrap().revision += 1;
        candidate.network_acls[0].revision += 1;
        candidate.network_acls[0].rules[0].effect = PermissionEffect::Deny;

        let preview = store.preview(&base, 0, candidate).unwrap();

        assert!(preview.revision_conflict);
        assert!(!preview.can_apply);
        assert_eq!(preview.expected_revision, 0);
        assert_eq!(preview.current_revision, applied.revision);
        assert_eq!(store.current_snapshot(&base).unwrap(), applied.snapshot);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn high_risk_requires_explicit_confirmation_and_failure_keeps_old_snapshot() {
        let root = temp_root("confirm");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        assert_eq!(store.current_snapshot(&base).unwrap(), base);
        let error = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                false,
                "owner",
                "test",
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PolicyTransactionError::ExplicitConfirmationRequired
        ));
        assert_eq!(store.current_snapshot(&base).unwrap(), base);
        assert!(store.history(&base).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn conditional_apply_rejects_concurrent_overwrite() {
        let root = temp_root("race");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let first = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "first",
            )
            .unwrap();
        assert_eq!(first.revision, 1);
        let stale = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "stale",
            )
            .unwrap_err();
        assert!(matches!(
            stale,
            PolicyTransactionError::RevisionConflict {
                expected: 0,
                current: 1
            }
        ));
        assert_eq!(store.current_snapshot(&base).unwrap(), first.snapshot);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rollback_restores_content_as_new_revision_and_records_audit() {
        let root = temp_root("rollback");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let applied = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "test",
            )
            .unwrap();
        let rolled_back = store
            .rollback(
                &base,
                1,
                1,
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "test",
            )
            .unwrap();
        assert_eq!(rolled_back.target_revision, 1);
        assert_eq!(rolled_back.revision, 2);
        assert_eq!(rolled_back.audit.action, PolicyChangeAction::Rollback);
        assert_eq!(rolled_back.audit.target_revision, Some(1));
        assert_eq!(
            rolled_back.snapshot.local_policy.as_ref().unwrap().rules,
            applied.snapshot.local_policy.as_ref().unwrap().rules
        );
        assert_eq!(
            rolled_back.snapshot.local_policy.as_ref().unwrap().revision,
            3
        );
        let history = store.history(&base).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].action, PolicyChangeAction::Rollback);
        assert_eq!(history[1].action, PolicyChangeAction::Apply);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crash_orphans_are_cleaned_without_changing_authoritative_pointer() {
        let root = temp_root("crash");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let store_dir = root.join(STORE_DIR);
        fs::create_dir_all(store_dir.join(".staging-99")).unwrap();
        fs::write(store_dir.join("generations/99.json"), b"{}").unwrap();
        fs::write(store_dir.join("history/99.json"), b"{}").unwrap();
        let current = store.current_snapshot(&base).unwrap();
        assert_eq!(current, base);
        assert!(!store_dir.join(".staging-99").exists());
        assert!(!store_dir.join("generations/99.json").exists());
        assert!(!store_dir.join("history/99.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_and_acl_updates_require_the_next_document_revision() {
        let root = temp_root("document-revisions");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();

        let mut stale_local = base.clone();
        stale_local.local_policy.as_mut().unwrap().rules = vec![rule(
            "local-read",
            PermissionEffect::Deny,
            PolicyAction::Read,
            ResourceKind::Network,
            Vec::new(),
        )];
        let error = store
            .apply(&base, 0, stale_local, &[], true, false, "owner", "test")
            .unwrap_err();
        assert!(matches!(error, PolicyTransactionError::InvalidCandidate(_)));
        assert_eq!(store.current_snapshot(&base).unwrap(), base);

        let mut stale_acl = base.clone();
        stale_acl.network_acls[0].rules = vec![rule(
            "acl-read",
            PermissionEffect::Deny,
            PolicyAction::Read,
            ResourceKind::Network,
            Vec::new(),
        )];
        let error = store
            .apply(
                &base,
                0,
                stale_acl,
                &[network_id("home").to_string()],
                false,
                false,
                "owner",
                "test",
            )
            .unwrap_err();
        assert!(matches!(error, PolicyTransactionError::InvalidCandidate(_)));
        assert_eq!(store.current_snapshot(&base).unwrap(), base);
        assert!(store.history(&base).unwrap().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rollback_bumps_acl_revision_and_refreshes_its_validity_window() {
        let root = temp_root("rollback-acl-clock");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let applied = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "test",
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let rolled_back = store
            .rollback(
                &base,
                1,
                1,
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "test",
            )
            .unwrap();
        let before = applied.snapshot.network_acls.first().unwrap();
        let after = rolled_back.snapshot.network_acls.first().unwrap();
        assert_eq!(after.revision, before.revision.saturating_add(1));
        assert!(after.issued_at_unix_ms >= before.issued_at_unix_ms);
        assert!(after.expires_at_unix_ms > after.issued_at_unix_ms);
        assert_eq!(
            rolled_back.audit.network_acl_revisions[&network_id("home").to_string()],
            after.revision
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reopened_store_recovers_the_committed_snapshot_and_history() {
        let root = temp_root("reopen");
        let base = initial();
        let first_store = PolicyTransactionStore::new(root.clone());
        let applied = first_store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "test",
            )
            .unwrap();
        drop(first_store);

        let reopened = PolicyTransactionStore::new(root.clone());
        let current = reopened.current(&base).unwrap();
        assert_eq!(current.revision, applied.revision);
        assert_eq!(current.snapshot, applied.snapshot);
        let history = reopened.history_snapshot(&base).unwrap();
        assert_eq!(history.current_revision, applied.revision);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].revision, applied.revision);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_rollback_keeps_the_current_authoritative_snapshot() {
        let root = temp_root("rollback-failure");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let applied = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "test",
            )
            .unwrap();
        let error = store
            .rollback(
                &base,
                1,
                99,
                &[network_id("home").to_string()],
                true,
                true,
                "owner",
                "test",
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PolicyTransactionError::RevisionNotFound(99)
        ));
        assert_eq!(store.current_snapshot(&base).unwrap(), applied.snapshot);
        assert_eq!(store.history(&base).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn two_writers_with_the_same_expected_revision_have_one_winner() {
        let root = temp_root("writer-race");
        let base = initial();
        PolicyTransactionStore::new(root.clone())
            .current_snapshot(&base)
            .unwrap();
        let mut handles = Vec::new();
        for source in ["writer-a", "writer-b"] {
            let root = root.clone();
            let base = base.clone();
            handles.push(std::thread::spawn(move || {
                PolicyTransactionStore::new(root)
                    .apply(
                        &base,
                        0,
                        expanded_candidate(),
                        &[network_id("home").to_string()],
                        true,
                        true,
                        "owner",
                        source,
                    )
                    .is_ok()
            }));
        }
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| **result).count(), 1);
        let reopened = PolicyTransactionStore::new(root.clone());
        assert_eq!(reopened.current(&base).unwrap().revision, 1);
        assert_eq!(reopened.history(&base).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scope_mismatch_rejects_undeclared_acl_change() {
        let root = temp_root("scope");
        let store = PolicyTransactionStore::new(root.clone());
        let base = initial();
        store.current_snapshot(&base).unwrap();
        let error = store
            .apply(
                &base,
                0,
                expanded_candidate(),
                &[],
                false,
                true,
                "owner",
                "test",
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PolicyTransactionError::ScopeMismatch { .. }
        ));
        let _ = fs::remove_dir_all(root);
    }
}
