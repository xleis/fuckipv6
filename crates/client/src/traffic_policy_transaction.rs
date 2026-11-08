//! Atomic traffic-policy drafts, previews, runtime compensation and rollback.
//!
//! The applied policy is an immutable revision. Editors submit a draft without
//! a revision; this module derives the next candidate only after checking the
//! caller's expected revision. Runtime routing, DNS and TUN changes sit behind
//! [`TrafficPolicyRuntimeAdapter`], so the transaction can install all effects,
//! verify them and compensate to the previous policy before moving the
//! authoritative state pointer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use fuckipv6_common::domain::{
    ApplicationIdentityCapability, ContentHash, NetworkId, NodeId, TrafficAction,
    TrafficDefaultPolicy, TrafficFailurePolicy, TrafficPolicy, TrafficRule, TrafficRuleRisk,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const STORE_DIR: &str = ".traffic-policy-transaction";
const STATE_FILE: &str = "state.json";
const PENDING_FILE: &str = "pending.json";
const GENERATIONS_DIR: &str = "generations";
const HISTORY_DIR: &str = "history";
const LOCK_FILE: &str = "lock";
const SCHEMA_VERSION: u16 = 1;

/// Editable traffic policy content. A draft deliberately has no revision:
/// preview computes the successor and apply records the revision only after
/// all runtime effects have been verified.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficPolicyDraft {
    pub node_id: NodeId,
    pub default_policy: TrafficDefaultPolicy,
    #[serde(default)]
    pub failure_policy: TrafficFailurePolicy,
    #[serde(default)]
    pub rules: Vec<TrafficRule>,
}

impl TrafficPolicyDraft {
    pub fn from_applied(policy: &TrafficPolicy) -> Self {
        Self {
            node_id: policy.node_id.clone(),
            default_policy: policy.default_policy.clone(),
            failure_policy: policy.failure_policy.clone(),
            rules: policy.rules.clone(),
        }
    }

    pub fn candidate(&self, revision: u64) -> Result<TrafficPolicy, TrafficPolicyTransactionError> {
        let candidate = TrafficPolicy {
            node_id: self.node_id.clone(),
            revision,
            default_policy: self.default_policy.clone(),
            failure_policy: self.failure_policy.clone(),
            rules: self.rules.clone(),
        };
        candidate
            .validate()
            .map_err(|error| TrafficPolicyTransactionError::InvalidCandidate(error.to_string()))?;
        Ok(candidate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyCurrent {
    pub revision: u64,
    pub policy: TrafficPolicy,
    pub content_hash: ContentHash,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficPolicyChangeKind {
    RuleAdded,
    RuleRemoved,
    RuleUpdated,
    RulesReordered,
    DefaultPolicyChanged,
    FailurePolicyChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyDiffEntry {
    /// Stable JSON Pointer rooted at the applied policy content.
    pub path: String,
    pub change: TrafficPolicyChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TrafficPolicyObjectKind {
    Rule,
    DefaultPolicy,
    FailurePolicy,
    Network,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyAffectedObject {
    pub kind: TrafficPolicyObjectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
}

/// Trusted material from the command bus. `Denied` is a terminal server-side
/// authorization result and cannot be converted into an apply request.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TrafficPolicyAuthorization {
    Allowed {
        actor: String,
        source: String,
    },
    Denied {
        reason_code: String,
        message: String,
    },
}

impl TrafficPolicyAuthorization {
    pub fn allowed(actor: impl Into<String>, source: impl Into<String>) -> Self {
        Self::Allowed {
            actor: actor.into(),
            source: source.into(),
        }
    }

    pub fn denied(reason_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Denied {
            reason_code: reason_code.into(),
            message: message.into(),
        }
    }

    fn allowed_identity(&self) -> Result<(&str, &str), TrafficPolicyTransactionError> {
        match self {
            Self::Allowed { actor, source } => {
                validate_audit_identity("actor", actor)?;
                validate_audit_identity("source", source)?;
                Ok((actor, source))
            }
            Self::Denied {
                reason_code,
                message,
            } => Err(TrafficPolicyTransactionError::PermissionDenied {
                reason_code: reason_code.clone(),
                message: message.clone(),
            }),
        }
    }
}

fn validate_audit_identity(field: &str, value: &str) -> Result<(), TrafficPolicyTransactionError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(TrafficPolicyTransactionError::InvalidAuthorization(
            format!("{field} identity is invalid"),
        ));
    }
    Ok(())
}

/// Server-owned target and platform truth used before a draft can be applied.
/// Callers build this from authoritative node/network snapshots, never from
/// fields supplied in the draft request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficPolicyValidationContext {
    pub node_id: NodeId,
    pub known_network_ids: BTreeSet<NetworkId>,
    pub known_exit_node_ids: BTreeSet<NodeId>,
    /// Full route/Exit actions require a usable TUN data plane.
    pub tun_available: bool,
    pub tun_unavailable_reason: String,
    pub application_identity: ApplicationIdentityCapability,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyPreview {
    pub expected_revision: u64,
    pub current_revision: u64,
    pub candidate_revision: u64,
    pub revision_conflict: bool,
    pub current: TrafficPolicy,
    pub candidate: TrafficPolicy,
    pub has_changes: bool,
    pub can_apply: bool,
    pub diffs: Vec<TrafficPolicyDiffEntry>,
    pub affected_objects: Vec<TrafficPolicyAffectedObject>,
    pub risks: Vec<TrafficRuleRisk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_target_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficPolicyChangeAction {
    Apply,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficPolicyHistoryOutcome {
    Applied,
    RolledBack,
    Failed,
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyHistoryEntry {
    pub transaction_id: String,
    pub previous_revision: u64,
    pub candidate_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_revision: Option<u64>,
    pub action: TrafficPolicyChangeAction,
    pub outcome: TrafficPolicyHistoryOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_revision: Option<u64>,
    pub actor: String,
    pub source: String,
    pub occurred_at: String,
    pub content_hash: ContentHash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub diffs: Vec<TrafficPolicyDiffEntry>,
    pub affected_objects: Vec<TrafficPolicyAffectedObject>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyApplyResult {
    pub previous_revision: u64,
    pub revision: u64,
    pub no_op: bool,
    pub policy: TrafficPolicy,
    pub content_hash: ContentHash,
    pub diffs: Vec<TrafficPolicyDiffEntry>,
    pub affected_objects: Vec<TrafficPolicyAffectedObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<TrafficPolicyHistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyRollbackResult {
    pub previous_revision: u64,
    pub revision: u64,
    pub target_revision: u64,
    pub policy: TrafficPolicy,
    pub content_hash: ContentHash,
    pub diffs: Vec<TrafficPolicyDiffEntry>,
    pub affected_objects: Vec<TrafficPolicyAffectedObject>,
    pub history: TrafficPolicyHistoryEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyHistorySnapshot {
    pub current_revision: u64,
    pub entries: Vec<TrafficPolicyHistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TrafficPolicyRecoveryReport {
    pub transaction_id: Option<String>,
    pub rolled_back_to_revision: Option<u64>,
    pub history: Option<TrafficPolicyHistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficPolicyRuntimeFailure {
    pub stage: String,
    pub reason_code: String,
    pub message: String,
}

impl TrafficPolicyRuntimeFailure {
    pub fn new(
        stage: impl Into<String>,
        reason_code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            stage: stage.into(),
            reason_code: reason_code.into(),
            message: message.into(),
        }
    }
}

/// Route, DNS and TUN side effects are implemented behind this boundary. The
/// transaction owns ordering, verification, crash journaling and compensation;
/// implementations must not advance authority or claim verification success.
pub trait TrafficPolicyRuntimeAdapter: Send + Sync {
    fn apply(&self, plan: &TrafficPolicyRuntimePlan) -> Result<(), TrafficPolicyRuntimeFailure>;
    fn verify(&self, plan: &TrafficPolicyRuntimePlan) -> Result<(), TrafficPolicyRuntimeFailure>;
    fn restore(&self, previous: &TrafficPolicy) -> Result<(), TrafficPolicyRuntimeFailure>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficPolicyRuntimePlan {
    pub transaction_id: String,
    pub previous: TrafficPolicy,
    pub candidate: TrafficPolicy,
    pub affected_networks: Vec<NetworkId>,
    pub affected_exits: Vec<NodeId>,
}

/// Structured details when both the runtime operation and its compensation
/// fail. Keeping this payload boxed keeps the common transaction `Result`
/// small while preserving the original and rollback diagnostics.
#[derive(Debug, thiserror::Error)]
#[error("原错误 [{original_stage}/{original_code}]: {original_message}；补偿错误 [{rollback_stage}/{rollback_code}]: {rollback_message}")]
pub struct TrafficPolicyCompensationFailure {
    pub original_stage: String,
    pub original_code: String,
    pub original_message: String,
    pub rollback_stage: String,
    pub rollback_code: String,
    pub rollback_message: String,
}

impl TrafficPolicyCompensationFailure {
    fn new(
        original_stage: impl Into<String>,
        original_code: impl Into<String>,
        original_message: impl Into<String>,
        rollback: TrafficPolicyRuntimeFailure,
    ) -> Box<Self> {
        Box::new(Self {
            original_stage: original_stage.into(),
            original_code: original_code.into(),
            original_message: original_message.into(),
            rollback_stage: rollback.stage,
            rollback_code: rollback.reason_code,
            rollback_message: rollback.message,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TrafficPolicyTransactionError {
    #[error("流量策略 revision 冲突：期望 {expected}，当前 {current}")]
    RevisionConflict { expected: u64, current: u64 },
    #[error("流量策略候选无效: {0}")]
    InvalidCandidate(String),
    #[error("流量策略授权失败 [{reason_code}]: {message}")]
    PermissionDenied {
        reason_code: String,
        message: String,
    },
    #[error("流量策略授权上下文无效: {0}")]
    InvalidAuthorization(String),
    #[error("流量策略目标不存在: {kind} [{id}]")]
    MissingTarget { kind: &'static str, id: String },
    #[error("流量策略平台能力不可用: {0}")]
    CapabilityUnavailable(String),
    #[error("流量策略 revision {0} 不存在")]
    RevisionNotFound(u64),
    #[error("流量策略事务 {0} 需要恢复，拒绝读取半配置状态")]
    RecoveryRequired(String),
    #[error("运行时 {stage} 失败 [{reason_code}]: {message}")]
    RuntimeFailed {
        stage: String,
        reason_code: String,
        message: String,
    },
    #[error("运行时补偿失败: {0}")]
    CompensationFailed(Box<TrafficPolicyCompensationFailure>),
    #[error("流量策略持久化失败: {0}")]
    Persistence(String),
}

#[derive(Debug, Clone)]
pub struct TrafficPolicyTransactionStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredState {
    schema_version: u16,
    revision: u64,
    generation: String,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_transaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGeneration {
    schema_version: u16,
    policy: TrafficPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingTransaction {
    schema_version: u16,
    transaction_id: String,
    previous: TrafficPolicy,
    candidate: TrafficPolicy,
    action: TrafficPolicyChangeAction,
    target_revision: Option<u64>,
    actor: String,
    source: String,
    occurred_at: String,
    content_hash: ContentHash,
    diffs: Vec<TrafficPolicyDiffEntry>,
    affected_objects: Vec<TrafficPolicyAffectedObject>,
    affected_networks: Vec<NetworkId>,
    affected_exits: Vec<NodeId>,
}

struct LockedStore {
    _lock: File,
}

type PolicyDiff = (
    Vec<TrafficPolicyDiffEntry>,
    Vec<TrafficPolicyAffectedObject>,
    Vec<NetworkId>,
    Vec<NodeId>,
);

struct PreparedChange {
    current: TrafficPolicy,
    candidate: TrafficPolicy,
    diffs: Vec<TrafficPolicyDiffEntry>,
    affected_objects: Vec<TrafficPolicyAffectedObject>,
    affected_networks: Vec<NetworkId>,
    affected_exits: Vec<NodeId>,
    risks: Vec<TrafficRuleRisk>,
    content_hash: ContentHash,
}

impl TrafficPolicyTransactionStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn current(
        &self,
        fallback: &TrafficPolicy,
    ) -> Result<TrafficPolicyCurrent, TrafficPolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.load_state_locked(fallback)?;
        let policy = self.read_generation_locked(&state.generation)?;
        Ok(TrafficPolicyCurrent {
            revision: state.revision,
            content_hash: policy.content_hash().map_err(invalid_candidate)?,
            policy,
        })
    }

    pub fn preview(
        &self,
        fallback: &TrafficPolicy,
        expected_revision: u64,
        draft: TrafficPolicyDraft,
        context: &TrafficPolicyValidationContext,
    ) -> Result<TrafficPolicyPreview, TrafficPolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.load_state_locked(fallback)?;
        let current = self.read_generation_locked(&state.generation)?;
        let candidate = draft.candidate(next_revision(state.revision)?)?;
        let prepared = prepare_change(current, candidate, context)?;
        Ok(preview_from_prepared(
            expected_revision,
            state.revision,
            prepared,
            None,
        ))
    }

    pub fn preview_rollback(
        &self,
        fallback: &TrafficPolicy,
        expected_revision: u64,
        target_revision: u64,
        context: &TrafficPolicyValidationContext,
    ) -> Result<TrafficPolicyPreview, TrafficPolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.load_state_locked(fallback)?;
        if target_revision == 0 || target_revision > state.revision {
            return Err(TrafficPolicyTransactionError::RevisionNotFound(
                target_revision,
            ));
        }
        let current = self.read_generation_locked(&state.generation)?;
        let target = self.read_generation_locked(&target_revision.to_string())?;
        let candidate = restore_as_new_revision(&current, &target)?;
        let prepared = prepare_change(current, candidate, context)?;
        Ok(preview_from_prepared(
            expected_revision,
            state.revision,
            prepared,
            Some(target_revision),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &self,
        fallback: &TrafficPolicy,
        expected_revision: u64,
        draft: TrafficPolicyDraft,
        context: &TrafficPolicyValidationContext,
        authorization: &TrafficPolicyAuthorization,
        runtime: &dyn TrafficPolicyRuntimeAdapter,
    ) -> Result<TrafficPolicyApplyResult, TrafficPolicyTransactionError> {
        let (actor, source) = authorization.allowed_identity()?;
        let _locked = self.lock()?;
        let state = self.load_state_locked(fallback)?;
        if expected_revision != state.revision {
            return Err(TrafficPolicyTransactionError::RevisionConflict {
                expected: expected_revision,
                current: state.revision,
            });
        }
        let current = self.read_generation_locked(&state.generation)?;
        let candidate = draft.candidate(next_revision(state.revision)?)?;
        let prepared = prepare_change(current, candidate, context)?;
        if prepared.diffs.is_empty() {
            return Ok(TrafficPolicyApplyResult {
                previous_revision: state.revision,
                revision: state.revision,
                no_op: true,
                content_hash: prepared.content_hash,
                policy: prepared.current,
                diffs: Vec::new(),
                affected_objects: Vec::new(),
                history: None,
            });
        }
        let transaction_id = transaction_id(
            state.revision,
            prepared.candidate.revision,
            &prepared.content_hash,
            actor,
        );
        let occurred_at = now_string();
        let history = history_entry(
            &transaction_id,
            state.revision,
            &prepared,
            TrafficPolicyChangeAction::Apply,
            None,
            actor,
            source,
            &occurred_at,
        );
        let pending = pending_from_prepared(
            transaction_id.clone(),
            &prepared,
            TrafficPolicyChangeAction::Apply,
            None,
            actor,
            source,
            &occurred_at,
        );
        let history =
            self.execute_runtime_transaction_locked(&prepared, &pending, history, runtime)?;
        Ok(TrafficPolicyApplyResult {
            previous_revision: state.revision,
            revision: prepared.candidate.revision,
            no_op: false,
            policy: prepared.candidate,
            content_hash: prepared.content_hash,
            diffs: prepared.diffs,
            affected_objects: prepared.affected_objects,
            history: Some(history),
        })
    }

    pub fn rollback(
        &self,
        fallback: &TrafficPolicy,
        expected_revision: u64,
        target_revision: u64,
        context: &TrafficPolicyValidationContext,
        authorization: &TrafficPolicyAuthorization,
        runtime: &dyn TrafficPolicyRuntimeAdapter,
    ) -> Result<TrafficPolicyRollbackResult, TrafficPolicyTransactionError> {
        let (actor, source) = authorization.allowed_identity()?;
        let _locked = self.lock()?;
        let state = self.load_state_locked(fallback)?;
        if expected_revision != state.revision {
            return Err(TrafficPolicyTransactionError::RevisionConflict {
                expected: expected_revision,
                current: state.revision,
            });
        }
        if target_revision == 0 || target_revision > state.revision {
            return Err(TrafficPolicyTransactionError::RevisionNotFound(
                target_revision,
            ));
        }
        let current = self.read_generation_locked(&state.generation)?;
        let target = self.read_generation_locked(&target_revision.to_string())?;
        let candidate = restore_as_new_revision(&current, &target)?;
        let prepared = prepare_change(current, candidate, context)?;
        let transaction_id = transaction_id(
            state.revision,
            prepared.candidate.revision,
            &prepared.content_hash,
            actor,
        );
        let occurred_at = now_string();
        let history = history_entry(
            &transaction_id,
            state.revision,
            &prepared,
            TrafficPolicyChangeAction::Rollback,
            Some(target_revision),
            actor,
            source,
            &occurred_at,
        );
        let pending = pending_from_prepared(
            transaction_id,
            &prepared,
            TrafficPolicyChangeAction::Rollback,
            Some(target_revision),
            actor,
            source,
            &occurred_at,
        );
        let history =
            self.execute_runtime_transaction_locked(&prepared, &pending, history, runtime)?;
        Ok(TrafficPolicyRollbackResult {
            previous_revision: state.revision,
            revision: prepared.candidate.revision,
            target_revision,
            policy: prepared.candidate,
            content_hash: prepared.content_hash,
            diffs: prepared.diffs,
            affected_objects: prepared.affected_objects,
            history,
        })
    }

    /// Recover a crash or disconnect boundary before serving current state.
    /// A pending transaction whose id is already authoritative was committed;
    /// otherwise runtime effects are restored to the previous policy.
    pub fn recover_pending(
        &self,
        fallback: &TrafficPolicy,
        runtime: &dyn TrafficPolicyRuntimeAdapter,
    ) -> Result<TrafficPolicyRecoveryReport, TrafficPolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.ensure_initialized_locked(fallback)?;
        let Some(pending) = self.read_pending_locked()? else {
            self.cleanup_staging_locked()?;
            return Ok(TrafficPolicyRecoveryReport {
                transaction_id: None,
                rolled_back_to_revision: None,
                history: None,
            });
        };
        if pending.schema_version != SCHEMA_VERSION {
            return Err(TrafficPolicyTransactionError::Persistence(format!(
                "不支持的待恢复事务 schema {}",
                pending.schema_version
            )));
        }
        if state.last_transaction_id.as_deref() == Some(pending.transaction_id.as_str()) {
            self.remove_pending_locked()?;
            return Ok(TrafficPolicyRecoveryReport {
                transaction_id: Some(pending.transaction_id),
                rolled_back_to_revision: None,
                history: None,
            });
        }
        runtime
            .restore(&pending.previous)
            .map_err(|failure| runtime_error("recovery_restore", failure))?;
        let mut history = history_from_pending(
            &pending,
            TrafficPolicyHistoryOutcome::Recovered,
            Some("runtime_transaction_recovered"),
            Some("检测到未提交事务，已恢复上一可用流量策略"),
        );
        history.occurred_at = now_string();
        self.write_history_locked(&history)?;
        self.remove_pending_locked()?;
        Ok(TrafficPolicyRecoveryReport {
            transaction_id: Some(pending.transaction_id),
            rolled_back_to_revision: Some(pending.previous.revision),
            history: Some(history),
        })
    }

    pub fn history_snapshot(
        &self,
        fallback: &TrafficPolicy,
    ) -> Result<TrafficPolicyHistorySnapshot, TrafficPolicyTransactionError> {
        let _locked = self.lock()?;
        let state = self.load_state_locked(fallback)?;
        let mut entries = self.read_history_locked()?;
        entries.sort_by(|left, right| {
            right
                .occurred_at
                .cmp(&left.occurred_at)
                .then_with(|| right.transaction_id.cmp(&left.transaction_id))
        });
        entries.truncate(256);
        Ok(TrafficPolicyHistorySnapshot {
            current_revision: state.revision,
            entries,
        })
    }

    fn execute_runtime_transaction_locked(
        &self,
        prepared: &PreparedChange,
        pending: &PendingTransaction,
        mut history: TrafficPolicyHistoryEntry,
        runtime: &dyn TrafficPolicyRuntimeAdapter,
    ) -> Result<TrafficPolicyHistoryEntry, TrafficPolicyTransactionError> {
        // The durable intent is written before route/DNS/TUN mutation. On
        // restart it tells recovery exactly which previous policy to restore.
        self.write_pending_locked(pending)?;
        let plan = TrafficPolicyRuntimePlan {
            transaction_id: pending.transaction_id.clone(),
            previous: prepared.current.clone(),
            candidate: prepared.candidate.clone(),
            affected_networks: prepared.affected_networks.clone(),
            affected_exits: prepared.affected_exits.clone(),
        };
        if let Err(failure) = runtime.apply(&plan) {
            return self.compensate_failed_transaction_locked(
                pending,
                history,
                "runtime_apply",
                failure,
                runtime,
            );
        }
        if let Err(failure) = runtime.verify(&plan) {
            return self.compensate_failed_transaction_locked(
                pending,
                history,
                "runtime_verify",
                failure,
                runtime,
            );
        }

        history.outcome = match pending.action {
            TrafficPolicyChangeAction::Apply => TrafficPolicyHistoryOutcome::Applied,
            TrafficPolicyChangeAction::Rollback => TrafficPolicyHistoryOutcome::RolledBack,
        };
        history.applied_revision = Some(prepared.candidate.revision);
        if let Err(error) = self.commit_locked(
            prepared.candidate.revision,
            &prepared.candidate,
            &history,
            Some(pending.transaction_id.clone()),
        ) {
            // A failed persistence step after runtime mutation must not leave
            // the new policy live. If compensation itself fails, preserve the
            // pending journal and report a diagnostic failure.
            if let Err(failure) = runtime.restore(&prepared.current) {
                return Err(TrafficPolicyTransactionError::CompensationFailed(
                    TrafficPolicyCompensationFailure::new(
                        "persistence_commit",
                        "persistence_commit_failed",
                        error.to_string(),
                        failure,
                    ),
                ));
            }
            return Err(error);
        }
        self.remove_pending_locked()?;
        Ok(history)
    }

    fn compensate_failed_transaction_locked(
        &self,
        pending: &PendingTransaction,
        mut history: TrafficPolicyHistoryEntry,
        failed_stage: &str,
        failure: TrafficPolicyRuntimeFailure,
        runtime: &dyn TrafficPolicyRuntimeAdapter,
    ) -> Result<TrafficPolicyHistoryEntry, TrafficPolicyTransactionError> {
        if let Err(rollback) = runtime.restore(&pending.previous) {
            return Err(TrafficPolicyTransactionError::CompensationFailed(
                TrafficPolicyCompensationFailure::new(
                    failed_stage,
                    failure.reason_code.clone(),
                    failure.message.clone(),
                    rollback,
                ),
            ));
        }
        history.outcome = TrafficPolicyHistoryOutcome::Failed;
        history.applied_revision = None;
        history.reason_code = Some(format!("{failed_stage}_{}", failure.reason_code));
        history.reason = Some(failure.message.clone());
        self.write_history_locked(&history)?;
        self.remove_pending_locked()?;
        Err(TrafficPolicyTransactionError::RuntimeFailed {
            stage: failed_stage.into(),
            reason_code: failure.reason_code,
            message: failure.message,
        })
    }

    fn lock(&self) -> Result<LockedStore, TrafficPolicyTransactionError> {
        let store_dir = self.root.join(STORE_DIR);
        fs::create_dir_all(&store_dir).map_err(persistence)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(store_dir.join(LOCK_FILE))
            .map_err(persistence)?;
        lock.lock_exclusive().map_err(persistence)?;
        Ok(LockedStore { _lock: lock })
    }

    fn load_state_locked(
        &self,
        fallback: &TrafficPolicy,
    ) -> Result<StoredState, TrafficPolicyTransactionError> {
        let state = self.ensure_initialized_locked(fallback)?;
        if let Some(pending) = self.read_pending_locked()? {
            if state.last_transaction_id.as_deref() == Some(pending.transaction_id.as_str()) {
                self.remove_pending_locked()?;
            } else {
                return Err(TrafficPolicyTransactionError::RecoveryRequired(
                    pending.transaction_id,
                ));
            }
        }
        Ok(state)
    }

    fn ensure_initialized_locked(
        &self,
        fallback: &TrafficPolicy,
    ) -> Result<StoredState, TrafficPolicyTransactionError> {
        let store_dir = self.root.join(STORE_DIR);
        fs::create_dir_all(store_dir.join(GENERATIONS_DIR)).map_err(persistence)?;
        fs::create_dir_all(store_dir.join(HISTORY_DIR)).map_err(persistence)?;
        self.cleanup_staging_locked()?;
        let state_path = store_dir.join(STATE_FILE);
        if state_path.exists() {
            let state = read_json::<StoredState>(&state_path)?;
            if state.schema_version != SCHEMA_VERSION {
                return Err(TrafficPolicyTransactionError::Persistence(format!(
                    "不支持的流量策略事务 schema {}",
                    state.schema_version
                )));
            }
            if !self.generation_path(&state.generation).exists() {
                return Err(TrafficPolicyTransactionError::Persistence(format!(
                    "权威流量策略 generation {} 缺失",
                    state.generation
                )));
            }
            self.cleanup_unpublished_locked(&state)?;
            return Ok(state);
        }

        fallback
            .validate()
            .map_err(|error| TrafficPolicyTransactionError::InvalidCandidate(error.to_string()))?;
        let generation = fallback.revision.to_string();
        write_atomic(
            &self.generation_path(&generation),
            &StoredGeneration {
                schema_version: SCHEMA_VERSION,
                policy: fallback.clone(),
            },
        )?;
        let state = StoredState {
            schema_version: SCHEMA_VERSION,
            revision: fallback.revision,
            generation,
            updated_at: now_string(),
            last_transaction_id: None,
        };
        write_atomic(&state_path, &state)?;
        sync_dir(&store_dir)?;
        Ok(state)
    }

    fn read_generation_locked(
        &self,
        generation: &str,
    ) -> Result<TrafficPolicy, TrafficPolicyTransactionError> {
        let stored = read_json::<StoredGeneration>(&self.generation_path(generation))?;
        if stored.schema_version != SCHEMA_VERSION {
            return Err(TrafficPolicyTransactionError::Persistence(format!(
                "不支持的流量策略 generation schema {}",
                stored.schema_version
            )));
        }
        stored
            .policy
            .validate()
            .map_err(|error| TrafficPolicyTransactionError::Persistence(error.to_string()))?;
        Ok(stored.policy)
    }

    fn write_pending_locked(
        &self,
        pending: &PendingTransaction,
    ) -> Result<(), TrafficPolicyTransactionError> {
        write_atomic(&self.pending_path(), pending)?;
        sync_dir(&self.root.join(STORE_DIR))?;
        Ok(())
    }

    fn read_pending_locked(
        &self,
    ) -> Result<Option<PendingTransaction>, TrafficPolicyTransactionError> {
        let path = self.pending_path();
        if !path.exists() {
            return Ok(None);
        }
        read_json(&path).map(Some)
    }

    fn remove_pending_locked(&self) -> Result<(), TrafficPolicyTransactionError> {
        let path = self.pending_path();
        if path.exists() {
            fs::remove_file(&path).map_err(persistence)?;
            sync_dir(&self.root.join(STORE_DIR))?;
        }
        Ok(())
    }

    fn write_history_locked(
        &self,
        history: &TrafficPolicyHistoryEntry,
    ) -> Result<(), TrafficPolicyTransactionError> {
        write_atomic(&self.history_path(&history.transaction_id), history)?;
        sync_dir(&self.root.join(STORE_DIR).join(HISTORY_DIR))
    }

    fn read_history_locked(
        &self,
    ) -> Result<Vec<TrafficPolicyHistoryEntry>, TrafficPolicyTransactionError> {
        let history_dir = self.root.join(STORE_DIR).join(HISTORY_DIR);
        let mut entries = Vec::new();
        let Ok(paths) = fs::read_dir(history_dir) else {
            return Ok(entries);
        };
        for path in paths.flatten() {
            let path = path.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            entries.push(read_json(&path)?);
        }
        Ok(entries)
    }

    fn commit_locked(
        &self,
        revision: u64,
        policy: &TrafficPolicy,
        history: &TrafficPolicyHistoryEntry,
        transaction_id: Option<String>,
    ) -> Result<(), TrafficPolicyTransactionError> {
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
                policy: policy.clone(),
            },
        )?;
        write_json_sync(&staged_history, history)?;
        let state = StoredState {
            schema_version: SCHEMA_VERSION,
            revision,
            generation: revision.to_string(),
            updated_at: history.occurred_at.clone(),
            last_transaction_id: transaction_id,
        };
        write_json_sync(&staged_state, &state)?;

        fs::rename(
            &staged_generation,
            self.generation_path(&revision.to_string()),
        )
        .map_err(persistence)?;
        fs::rename(&staged_history, self.history_path(&history.transaction_id))
            .map_err(persistence)?;
        sync_dir(&store_dir.join(GENERATIONS_DIR))?;
        sync_dir(&store_dir.join(HISTORY_DIR))?;

        // The authority pointer is the commit point. Everything before this
        // rename is unreachable and is cleaned on restart.
        fs::rename(&staged_state, store_dir.join(STATE_FILE)).map_err(persistence)?;
        sync_dir(&store_dir)?;
        let _ = fs::remove_dir_all(&staging_dir);
        Ok(())
    }

    fn cleanup_unpublished_locked(
        &self,
        state: &StoredState,
    ) -> Result<(), TrafficPolicyTransactionError> {
        self.cleanup_staging_locked()?;
        let generations = self.root.join(STORE_DIR).join(GENERATIONS_DIR);
        if let Ok(entries) = fs::read_dir(generations) {
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

    fn cleanup_staging_locked(&self) -> Result<(), TrafficPolicyTransactionError> {
        let store_dir = self.root.join(STORE_DIR);
        if let Ok(entries) = fs::read_dir(store_dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(".staging-") {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        Ok(())
    }

    fn pending_path(&self) -> PathBuf {
        self.root.join(STORE_DIR).join(PENDING_FILE)
    }

    fn generation_path(&self, generation: &str) -> PathBuf {
        self.root
            .join(STORE_DIR)
            .join(GENERATIONS_DIR)
            .join(format!("{generation}.json"))
    }

    fn history_path(&self, transaction_id: &str) -> PathBuf {
        self.root
            .join(STORE_DIR)
            .join(HISTORY_DIR)
            .join(format!("{transaction_id}.json"))
    }
}

fn prepare_change(
    current: TrafficPolicy,
    candidate: TrafficPolicy,
    context: &TrafficPolicyValidationContext,
) -> Result<PreparedChange, TrafficPolicyTransactionError> {
    current
        .validate()
        .map_err(|error| TrafficPolicyTransactionError::Persistence(error.to_string()))?;
    current
        .validate_successor(&candidate)
        .map_err(|error| TrafficPolicyTransactionError::InvalidCandidate(error.to_string()))?;
    validate_context(&candidate, context)?;
    let (diffs, affected_objects, affected_networks, affected_exits) =
        diff_policies(&current, &candidate)?;
    let risks = candidate
        .risk_report()
        .map_err(|error| TrafficPolicyTransactionError::InvalidCandidate(error.to_string()))?;
    let content_hash = candidate
        .content_hash()
        .map_err(|error| TrafficPolicyTransactionError::InvalidCandidate(error.to_string()))?;
    Ok(PreparedChange {
        current,
        candidate,
        diffs,
        affected_objects,
        affected_networks,
        affected_exits,
        risks,
        content_hash,
    })
}

fn validate_context(
    policy: &TrafficPolicy,
    context: &TrafficPolicyValidationContext,
) -> Result<(), TrafficPolicyTransactionError> {
    if policy.node_id != context.node_id {
        return Err(TrafficPolicyTransactionError::InvalidCandidate(
            "validation context does not belong to the policy node".into(),
        ));
    }
    let mut needs_tun = action_needs_tun(&policy.default_policy.action);
    for rule in &policy.rules {
        rule.matcher
            .validate_for_application_capability(&context.application_identity)
            .map_err(|error| {
                TrafficPolicyTransactionError::CapabilityUnavailable(error.to_string())
            })?;
        match &rule.action {
            TrafficAction::Network(network_id) => {
                needs_tun = true;
                if !context.known_network_ids.contains(network_id) {
                    return Err(TrafficPolicyTransactionError::MissingTarget {
                        kind: "network",
                        id: network_id.to_string(),
                    });
                }
            }
            TrafficAction::Exit(node_id) => {
                needs_tun = true;
                if !context.known_exit_node_ids.contains(node_id) {
                    return Err(TrafficPolicyTransactionError::MissingTarget {
                        kind: "exit",
                        id: node_id.to_string(),
                    });
                }
            }
            TrafficAction::Direct | TrafficAction::Reject => {}
        }
    }
    match &policy.default_policy.action {
        TrafficAction::Network(network_id) if !context.known_network_ids.contains(network_id) => {
            return Err(TrafficPolicyTransactionError::MissingTarget {
                kind: "network",
                id: network_id.to_string(),
            });
        }
        TrafficAction::Exit(node_id) if !context.known_exit_node_ids.contains(node_id) => {
            return Err(TrafficPolicyTransactionError::MissingTarget {
                kind: "exit",
                id: node_id.to_string(),
            });
        }
        TrafficAction::Direct | TrafficAction::Network(_) | TrafficAction::Exit(_) => {}
        TrafficAction::Reject => {}
    }
    if needs_tun && !context.tun_available {
        let reason = if context.tun_unavailable_reason.trim().is_empty() {
            "当前平台没有可用的 TUN 数据面".to_string()
        } else {
            context.tun_unavailable_reason.clone()
        };
        return Err(TrafficPolicyTransactionError::CapabilityUnavailable(reason));
    }
    Ok(())
}

fn action_needs_tun(action: &TrafficAction) -> bool {
    matches!(action, TrafficAction::Network(_) | TrafficAction::Exit(_))
}

fn diff_policies(
    current: &TrafficPolicy,
    candidate: &TrafficPolicy,
) -> Result<PolicyDiff, TrafficPolicyTransactionError> {
    let mut diffs = Vec::new();
    let mut affected = BTreeSet::new();
    let mut networks = BTreeSet::new();
    let mut exits = BTreeSet::new();

    if current.default_policy != candidate.default_policy {
        diffs.push(TrafficPolicyDiffEntry {
            path: "/default_policy".into(),
            change: TrafficPolicyChangeKind::DefaultPolicyChanged,
            before: Some(serde_json::to_value(&current.default_policy).map_err(persistence)?),
            after: Some(serde_json::to_value(&candidate.default_policy).map_err(persistence)?),
        });
        affected.insert(TrafficPolicyAffectedObject {
            kind: TrafficPolicyObjectKind::DefaultPolicy,
            object_id: None,
        });
        collect_actions(
            [
                &current.default_policy.action,
                &candidate.default_policy.action,
            ],
            &mut networks,
            &mut exits,
        );
    }
    if current.failure_policy != candidate.failure_policy {
        diffs.push(TrafficPolicyDiffEntry {
            path: "/failure_policy".into(),
            change: TrafficPolicyChangeKind::FailurePolicyChanged,
            before: Some(serde_json::to_value(&current.failure_policy).map_err(persistence)?),
            after: Some(serde_json::to_value(&candidate.failure_policy).map_err(persistence)?),
        });
        affected.insert(TrafficPolicyAffectedObject {
            kind: TrafficPolicyObjectKind::FailurePolicy,
            object_id: None,
        });
    }

    let before = current
        .rules
        .iter()
        .map(|rule| (rule.rule_id.clone(), rule))
        .collect::<BTreeMap<_, _>>();
    let after = candidate
        .rules
        .iter()
        .map(|rule| (rule.rule_id.clone(), rule))
        .collect::<BTreeMap<_, _>>();
    let ids = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for rule_id in ids {
        let before_rule = before.get(&rule_id).copied();
        let after_rule = after.get(&rule_id).copied();
        if before_rule == after_rule {
            continue;
        }
        let change = if before_rule.is_none() {
            TrafficPolicyChangeKind::RuleAdded
        } else if after_rule.is_none() {
            TrafficPolicyChangeKind::RuleRemoved
        } else {
            TrafficPolicyChangeKind::RuleUpdated
        };
        diffs.push(TrafficPolicyDiffEntry {
            path: format!("/rules/{}", escape_json_pointer(rule_id.as_str())),
            change,
            before: option_value(&before_rule)?,
            after: option_value(&after_rule)?,
        });
        affected.insert(TrafficPolicyAffectedObject {
            kind: TrafficPolicyObjectKind::Rule,
            object_id: Some(rule_id.to_string()),
        });
        if let Some(rule) = before_rule {
            collect_actions([&rule.action], &mut networks, &mut exits);
        }
        if let Some(rule) = after_rule {
            collect_actions([&rule.action], &mut networks, &mut exits);
        }
    }

    let common_before = current
        .rules
        .iter()
        .filter(|rule| after.contains_key(&rule.rule_id))
        .map(|rule| rule.rule_id.clone())
        .collect::<Vec<_>>();
    let common_after = candidate
        .rules
        .iter()
        .filter(|rule| before.contains_key(&rule.rule_id))
        .map(|rule| rule.rule_id.clone())
        .collect::<Vec<_>>();
    if common_before != common_after {
        let before_value = serde_json::to_value(&common_before).map_err(persistence)?;
        let after_value = serde_json::to_value(&common_after).map_err(persistence)?;
        diffs.push(TrafficPolicyDiffEntry {
            path: "/rules".into(),
            change: TrafficPolicyChangeKind::RulesReordered,
            before: Some(before_value),
            after: Some(after_value),
        });
        for rule in &candidate.rules {
            if rule.enabled {
                affected.insert(TrafficPolicyAffectedObject {
                    kind: TrafficPolicyObjectKind::Rule,
                    object_id: Some(rule.rule_id.to_string()),
                });
                collect_actions([&rule.action], &mut networks, &mut exits);
            }
        }
    }

    for network_id in &networks {
        affected.insert(TrafficPolicyAffectedObject {
            kind: TrafficPolicyObjectKind::Network,
            object_id: Some(network_id.to_string()),
        });
    }
    for exit_id in &exits {
        affected.insert(TrafficPolicyAffectedObject {
            kind: TrafficPolicyObjectKind::Exit,
            object_id: Some(exit_id.to_string()),
        });
    }
    Ok((
        diffs,
        affected.into_iter().collect(),
        networks.into_iter().collect(),
        exits.into_iter().collect(),
    ))
}

fn collect_actions<'a>(
    actions: impl IntoIterator<Item = &'a TrafficAction>,
    networks: &mut BTreeSet<NetworkId>,
    exits: &mut BTreeSet<NodeId>,
) {
    for action in actions {
        match action {
            TrafficAction::Network(network_id) => {
                networks.insert(network_id.clone());
            }
            TrafficAction::Exit(node_id) => {
                exits.insert(node_id.clone());
            }
            TrafficAction::Direct | TrafficAction::Reject => {}
        }
    }
}

fn preview_from_prepared(
    expected_revision: u64,
    current_revision: u64,
    prepared: PreparedChange,
    rollback_target_revision: Option<u64>,
) -> TrafficPolicyPreview {
    let revision_conflict = expected_revision != current_revision;
    TrafficPolicyPreview {
        expected_revision,
        current_revision,
        candidate_revision: prepared.candidate.revision,
        revision_conflict,
        current: prepared.current,
        candidate: prepared.candidate,
        has_changes: !prepared.diffs.is_empty(),
        can_apply: !revision_conflict,
        diffs: prepared.diffs,
        affected_objects: prepared.affected_objects,
        risks: prepared.risks,
        rollback_target_revision,
    }
}

fn restore_as_new_revision(
    current: &TrafficPolicy,
    target: &TrafficPolicy,
) -> Result<TrafficPolicy, TrafficPolicyTransactionError> {
    target
        .validate()
        .map_err(|error| TrafficPolicyTransactionError::InvalidCandidate(error.to_string()))?;
    let candidate = TrafficPolicy {
        node_id: current.node_id.clone(),
        revision: next_revision(current.revision)?,
        default_policy: target.default_policy.clone(),
        failure_policy: target.failure_policy.clone(),
        rules: target
            .rules
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                let mut rule = rule.clone();
                rule.priority = u32::try_from(index + 1).map_err(|_| {
                    TrafficPolicyTransactionError::InvalidCandidate("流量策略规则数量过多".into())
                })?;
                Ok(rule)
            })
            .collect::<Result<Vec<_>, TrafficPolicyTransactionError>>()?,
    };
    current
        .validate_successor(&candidate)
        .map_err(|error| TrafficPolicyTransactionError::InvalidCandidate(error.to_string()))?;
    Ok(candidate)
}

fn next_revision(current: u64) -> Result<u64, TrafficPolicyTransactionError> {
    current.checked_add(1).ok_or_else(|| {
        TrafficPolicyTransactionError::InvalidCandidate("流量策略 revision 已溢出".into())
    })
}

fn pending_from_prepared(
    transaction_id: String,
    prepared: &PreparedChange,
    action: TrafficPolicyChangeAction,
    target_revision: Option<u64>,
    actor: &str,
    source: &str,
    occurred_at: &str,
) -> PendingTransaction {
    PendingTransaction {
        schema_version: SCHEMA_VERSION,
        transaction_id,
        previous: prepared.current.clone(),
        candidate: prepared.candidate.clone(),
        action,
        target_revision,
        actor: actor.into(),
        source: source.into(),
        occurred_at: occurred_at.into(),
        content_hash: prepared.content_hash.clone(),
        diffs: prepared.diffs.clone(),
        affected_objects: prepared.affected_objects.clone(),
        affected_networks: prepared.affected_networks.clone(),
        affected_exits: prepared.affected_exits.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn history_entry(
    transaction_id: &str,
    previous_revision: u64,
    prepared: &PreparedChange,
    action: TrafficPolicyChangeAction,
    target_revision: Option<u64>,
    actor: &str,
    source: &str,
    occurred_at: &str,
) -> TrafficPolicyHistoryEntry {
    TrafficPolicyHistoryEntry {
        transaction_id: transaction_id.into(),
        previous_revision,
        candidate_revision: prepared.candidate.revision,
        applied_revision: None,
        action,
        outcome: TrafficPolicyHistoryOutcome::Failed,
        target_revision,
        actor: actor.into(),
        source: source.into(),
        occurred_at: occurred_at.into(),
        content_hash: prepared.content_hash.clone(),
        reason_code: None,
        reason: None,
        diffs: prepared.diffs.clone(),
        affected_objects: prepared.affected_objects.clone(),
    }
}

fn history_from_pending(
    pending: &PendingTransaction,
    outcome: TrafficPolicyHistoryOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
) -> TrafficPolicyHistoryEntry {
    TrafficPolicyHistoryEntry {
        transaction_id: pending.transaction_id.clone(),
        previous_revision: pending.previous.revision,
        candidate_revision: pending.candidate.revision,
        applied_revision: None,
        action: pending.action,
        outcome,
        target_revision: pending.target_revision,
        actor: pending.actor.clone(),
        source: pending.source.clone(),
        occurred_at: pending.occurred_at.clone(),
        content_hash: pending.content_hash.clone(),
        reason_code: reason_code.map(str::to_owned),
        reason: reason.map(str::to_owned),
        diffs: pending.diffs.clone(),
        affected_objects: pending.affected_objects.clone(),
    }
}

fn transaction_id(
    previous_revision: u64,
    candidate_revision: u64,
    hash: &ContentHash,
    actor: &str,
) -> String {
    let digest = blake3::hash(
        format!(
            "{previous_revision}:{candidate_revision}:{}:{actor}",
            hash.as_str()
        )
        .as_bytes(),
    );
    format!("traffic_tx_{}", digest.to_hex())
}

fn option_value<T: Serialize>(
    value: &Option<T>,
) -> Result<Option<Value>, TrafficPolicyTransactionError> {
    value
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(persistence)
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn runtime_error(
    stage: &str,
    failure: TrafficPolicyRuntimeFailure,
) -> TrafficPolicyTransactionError {
    TrafficPolicyTransactionError::RuntimeFailed {
        stage: stage.into(),
        reason_code: failure.reason_code,
        message: failure.message,
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<T, TrafficPolicyTransactionError> {
    let bytes = fs::read(path).map_err(persistence)?;
    serde_json::from_slice(&bytes).map_err(persistence)
}

fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), TrafficPolicyTransactionError> {
    let parent = path
        .parent()
        .ok_or_else(|| TrafficPolicyTransactionError::Persistence("路径没有父目录".into()))?;
    fs::create_dir_all(parent).map_err(persistence)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("traffic-policy"),
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
) -> Result<(), TrafficPolicyTransactionError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(persistence)?;
    let mut file = File::create(path).map_err(persistence)?;
    file.write_all(&bytes).map_err(persistence)?;
    file.write_all(b"\n").map_err(persistence)?;
    file.sync_all().map_err(persistence)?;
    Ok(())
}

fn sync_dir(path: &Path) -> Result<(), TrafficPolicyTransactionError> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(persistence)?;
    }
    Ok(())
}

fn persistence(error: impl std::fmt::Display) -> TrafficPolicyTransactionError {
    TrafficPolicyTransactionError::Persistence(error.to_string())
}

fn invalid_candidate(error: impl std::fmt::Display) -> TrafficPolicyTransactionError {
    TrafficPolicyTransactionError::InvalidCandidate(error.to_string())
}

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use fuckipv6_common::domain::{
        ApplicationIdentity, ApplicationIdentityCapability, ApplicationIdentityPlatform,
        DomainName, PortRange, TrafficDefaultConfirmedBy, TrafficMatch, TrafficProtocol,
        TrafficRuleId,
    };

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum RuntimeCall {
        Apply(String),
        Verify(String),
        Restore(u64),
    }

    #[derive(Default)]
    struct FakeRuntime {
        calls: Mutex<Vec<RuntimeCall>>,
        fail_apply: bool,
        fail_verify: bool,
        fail_restore: bool,
    }

    impl FakeRuntime {
        fn calls(&self) -> Vec<RuntimeCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl TrafficPolicyRuntimeAdapter for FakeRuntime {
        fn apply(
            &self,
            plan: &TrafficPolicyRuntimePlan,
        ) -> Result<(), TrafficPolicyRuntimeFailure> {
            self.calls
                .lock()
                .unwrap()
                .push(RuntimeCall::Apply(plan.transaction_id.clone()));
            if self.fail_apply {
                Err(TrafficPolicyRuntimeFailure::new(
                    "apply",
                    "disconnected",
                    "control channel disconnected",
                ))
            } else {
                Ok(())
            }
        }

        fn verify(
            &self,
            plan: &TrafficPolicyRuntimePlan,
        ) -> Result<(), TrafficPolicyRuntimeFailure> {
            self.calls
                .lock()
                .unwrap()
                .push(RuntimeCall::Verify(plan.transaction_id.clone()));
            if self.fail_verify {
                Err(TrafficPolicyRuntimeFailure::new(
                    "verify",
                    "dns_timeout",
                    "DNS verification timed out",
                ))
            } else {
                Ok(())
            }
        }

        fn restore(&self, previous: &TrafficPolicy) -> Result<(), TrafficPolicyRuntimeFailure> {
            self.calls
                .lock()
                .unwrap()
                .push(RuntimeCall::Restore(previous.revision));
            if self.fail_restore {
                Err(TrafficPolicyRuntimeFailure::new(
                    "restore",
                    "disconnected",
                    "control channel disconnected",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fuckipv6-traffic-policy-{name}-{}-{}",
            std::process::id(),
            now_string().replace([':', '.'], "-")
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn node() -> NodeId {
        NodeId::from_public_key(b"traffic-policy-node")
    }

    fn network(name: &str) -> NetworkId {
        NetworkId::from_public_key(name.as_bytes())
    }

    fn exit(name: &str) -> NodeId {
        NodeId::from_public_key(name.as_bytes())
    }

    fn rule(name: &str, destination: &str, action: TrafficAction) -> TrafficRule {
        TrafficRule {
            rule_id: TrafficRuleId::from_public_key(name.as_bytes()),
            priority: 1,
            enabled: true,
            matcher: TrafficMatch {
                exact_domains: vec![DomainName::parse(destination).unwrap()],
                domain_suffixes: Vec::new(),
                destination_cidrs: Vec::new(),
                destination_ports: vec![PortRange {
                    start: 443,
                    end: 443,
                }],
                protocols: vec![TrafficProtocol::Tcp],
                applications: Vec::new(),
            },
            action,
            description: name.into(),
        }
    }

    fn policy() -> TrafficPolicy {
        TrafficPolicy {
            node_id: node(),
            revision: 1,
            default_policy: TrafficDefaultPolicy {
                action: TrafficAction::Direct,
                confirmed_by: TrafficDefaultConfirmedBy::Owner,
                confirmed_at_unix_ms: 1_700_000_000_000,
            },
            failure_policy: TrafficFailurePolicy::fail_closed(),
            rules: vec![rule(
                "chat",
                "chat.example.com",
                TrafficAction::Network(network("home")),
            )],
        }
    }

    fn context(tun_available: bool) -> TrafficPolicyValidationContext {
        TrafficPolicyValidationContext {
            node_id: node(),
            known_network_ids: [network("home"), network("work")].into_iter().collect(),
            known_exit_node_ids: [exit("exit-a"), exit("exit-b")].into_iter().collect(),
            tun_available,
            tun_unavailable_reason: "TUN unavailable in test".into(),
            application_identity: ApplicationIdentityCapability::available(
                ApplicationIdentityPlatform::Linux,
                [fuckipv6_common::domain::ApplicationIdentityFormat::ExecutablePath],
            )
            .unwrap(),
        }
    }

    fn draft(mut applied: TrafficPolicy) -> TrafficPolicyDraft {
        applied.revision = 0;
        TrafficPolicyDraft {
            node_id: applied.node_id,
            default_policy: applied.default_policy,
            failure_policy: applied.failure_policy,
            rules: applied.rules,
        }
    }

    #[test]
    fn preview_is_side_effect_free_and_does_not_change_authority() {
        let root = temp_root("preview");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let mut candidate = current.clone();
        candidate.rules[0].action = TrafficAction::Exit(exit("exit-a"));
        let mut video = rule("video", "video.example.com", TrafficAction::Reject);
        video.priority = 2;
        candidate.rules.push(video);
        let preview = store
            .preview(&current, 1, draft(candidate), &context(true))
            .unwrap();
        assert!(preview.has_changes);
        assert!(preview.can_apply);
        assert_eq!(preview.current_revision, 1);
        assert_eq!(preview.candidate_revision, 2);
        assert!(preview
            .diffs
            .iter()
            .any(|diff| diff.change == TrafficPolicyChangeKind::RuleUpdated));
        assert!(preview
            .diffs
            .iter()
            .any(|diff| diff.change == TrafficPolicyChangeKind::RuleAdded));
        assert!(preview.affected_objects.iter().any(|object| {
            object.kind == TrafficPolicyObjectKind::Exit
                && object.object_id == Some(exit("exit-a").to_string())
        }));
        assert_eq!(store.current(&current).unwrap().revision, 1);
        assert!(store.history_snapshot(&current).unwrap().entries.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn diff_reports_default_policy_changes_and_target_impacts() {
        let current = policy();
        let mut candidate = current.clone();
        candidate.revision = 2;
        candidate.default_policy.action = TrafficAction::Exit(exit("exit-b"));
        let (diffs, affected, networks, exits) = diff_policies(&current, &candidate).unwrap();
        assert!(diffs
            .iter()
            .any(|diff| diff.change == TrafficPolicyChangeKind::DefaultPolicyChanged));
        assert!(affected.iter().any(|object| {
            object.kind == TrafficPolicyObjectKind::DefaultPolicy && object.object_id.is_none()
        }));
        assert!(affected.iter().any(|object| {
            object.kind == TrafficPolicyObjectKind::Exit
                && object.object_id == Some(exit("exit-b").to_string())
        }));
        assert!(networks.is_empty());
        assert_eq!(exits, vec![exit("exit-b")]);
    }

    #[test]
    fn apply_verifies_then_advances_one_authoritative_revision() {
        let root = temp_root("apply");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let mut candidate = current.clone();
        candidate.rules[0].enabled = false;
        let runtime = FakeRuntime::default();
        let applied = store
            .apply(
                &current,
                1,
                draft(candidate),
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap();
        assert!(!applied.no_op);
        assert_eq!(applied.previous_revision, 1);
        assert_eq!(applied.revision, 2);
        assert_eq!(store.current(&current).unwrap().revision, 2);
        assert!(matches!(
            runtime.calls().as_slice(),
            [RuntimeCall::Apply(_), RuntimeCall::Verify(_)]
        ));
        let history = store.history_snapshot(&current).unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(
            history.entries[0].outcome,
            TrafficPolicyHistoryOutcome::Applied
        );
        assert_eq!(history.entries[0].actor, "owner");
        assert_eq!(history.entries[0].applied_revision, Some(2));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn no_op_does_not_advance_revision_or_runtime() {
        let root = temp_root("noop");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let runtime = FakeRuntime::default();
        let applied = store
            .apply(
                &current,
                1,
                draft(current.clone()),
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap();
        assert!(applied.no_op);
        assert_eq!(applied.revision, 1);
        assert_eq!(store.current(&current).unwrap().revision, 1);
        assert!(runtime.calls().is_empty());
        assert!(store.history_snapshot(&current).unwrap().entries.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stale_expected_revision_is_rejected_without_side_effects() {
        let root = temp_root("conflict");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let mut candidate = current.clone();
        candidate.rules[0].enabled = false;
        let runtime = FakeRuntime::default();
        let error = store
            .apply(
                &current,
                0,
                draft(candidate),
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::RevisionConflict {
                expected: 0,
                current: 1
            }
        ));
        assert!(runtime.calls().is_empty());
        assert_eq!(store.current(&current).unwrap().revision, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn denied_authorization_missing_targets_and_tun_are_rejected() {
        let root = temp_root("validation");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let runtime = FakeRuntime::default();
        let denied = store
            .apply(
                &current,
                1,
                draft(current.clone()),
                &context(true),
                &TrafficPolicyAuthorization::denied("forbidden", "no manage permission"),
                &runtime,
            )
            .unwrap_err();
        assert!(matches!(
            denied,
            TrafficPolicyTransactionError::PermissionDenied { .. }
        ));

        let mut missing = current.clone();
        missing.rules[0].action = TrafficAction::Network(network("missing"));
        let error = store
            .preview(&current, 1, draft(missing), &context(true))
            .unwrap_err();
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::MissingTarget {
                kind: "network",
                ..
            }
        ));

        let mut routed = current.clone();
        routed.rules[0].action = TrafficAction::Exit(exit("exit-a"));
        let error = store
            .preview(&current, 1, draft(routed), &context(false))
            .unwrap_err();
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::CapabilityUnavailable(_)
        ));
        assert_eq!(store.current(&current).unwrap().revision, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn apply_or_verify_failure_restores_previous_version_and_records_reason() {
        for (name, fail_apply, fail_verify) in [
            ("apply-failure", true, false),
            ("verify-failure", false, true),
        ] {
            let root = temp_root(name);
            let store = TrafficPolicyTransactionStore::new(root.clone());
            let current = policy();
            let mut candidate = current.clone();
            candidate.rules[0].action = TrafficAction::Exit(exit("exit-a"));
            let runtime = FakeRuntime {
                fail_apply,
                fail_verify,
                ..FakeRuntime::default()
            };
            let error = store
                .apply(
                    &current,
                    1,
                    draft(candidate),
                    &context(true),
                    &TrafficPolicyAuthorization::allowed("owner", "test"),
                    &runtime,
                )
                .unwrap_err();
            assert!(matches!(
                error,
                TrafficPolicyTransactionError::RuntimeFailed { .. }
            ));
            assert_eq!(runtime.calls().last(), Some(&RuntimeCall::Restore(1)));
            assert_eq!(store.current(&current).unwrap().revision, 1);
            let history = store.history_snapshot(&current).unwrap();
            assert_eq!(
                history.entries[0].outcome,
                TrafficPolicyHistoryOutcome::Failed
            );
            assert!(history.entries[0].reason_code.is_some());
            assert!(history.entries[0].reason.is_some());
            assert!(!root.join(STORE_DIR).join(PENDING_FILE).exists());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn compensation_failure_is_diagnostic_and_keeps_pending_journal() {
        let root = temp_root("compensation-failure");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let mut candidate = current.clone();
        candidate.rules[0].enabled = false;
        let runtime = FakeRuntime {
            fail_apply: true,
            fail_restore: true,
            ..FakeRuntime::default()
        };
        let error = store
            .apply(
                &current,
                1,
                draft(candidate),
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::CompensationFailed(_)
        ));
        assert!(root.join(STORE_DIR).join(PENDING_FILE).exists());
        assert!(matches!(
            store.current(&current),
            Err(TrafficPolicyTransactionError::RecoveryRequired(_))
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crash_recovery_restores_previous_policy_then_cleans_journal() {
        let root = temp_root("recovery");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        // Initialize and construct the durable journal exactly as a crash
        // between pending write and authoritative state switch would.
        store.current(&current).unwrap();
        let mut candidate = current.clone();
        candidate.revision = 2;
        candidate.rules[0].enabled = false;
        let (diffs, affected, networks, exits) = diff_policies(&current, &candidate).unwrap();
        let pending = PendingTransaction {
            schema_version: SCHEMA_VERSION,
            transaction_id: "traffic_tx_crash".into(),
            previous: current.clone(),
            candidate: candidate.clone(),
            action: TrafficPolicyChangeAction::Apply,
            target_revision: None,
            actor: "owner".into(),
            source: "test".into(),
            occurred_at: now_string(),
            content_hash: candidate.content_hash().unwrap(),
            diffs,
            affected_objects: affected,
            affected_networks: networks,
            affected_exits: exits,
        };
        store.write_pending_locked(&pending).unwrap();
        assert!(matches!(
            store.current(&current),
            Err(TrafficPolicyTransactionError::RecoveryRequired(_))
        ));
        let runtime = FakeRuntime::default();
        let report = store.recover_pending(&current, &runtime).unwrap();
        assert_eq!(report.rolled_back_to_revision, Some(1));
        assert_eq!(runtime.calls(), vec![RuntimeCall::Restore(1)]);
        assert_eq!(store.current(&current).unwrap().revision, 1);
        assert!(!root.join(STORE_DIR).join(PENDING_FILE).exists());
        let history = store.history_snapshot(&current).unwrap();
        assert_eq!(
            history.entries[0].outcome,
            TrafficPolicyHistoryOutcome::Recovered
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn committed_transaction_pending_cleanup_does_not_restore_a_half_state() {
        let root = temp_root("post-commit-crash");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        store.current(&current).unwrap();
        let mut candidate = current.clone();
        candidate.revision = 2;
        candidate.rules[0].enabled = false;
        let prepared = prepare_change(current.clone(), candidate.clone(), &context(true)).unwrap();
        let occurred_at = now_string();
        let transaction_id = "traffic_tx_committed".to_string();
        let mut history = history_entry(
            &transaction_id,
            1,
            &prepared,
            TrafficPolicyChangeAction::Apply,
            None,
            "owner",
            "test",
            &occurred_at,
        );
        history.outcome = TrafficPolicyHistoryOutcome::Applied;
        history.applied_revision = Some(2);
        let pending = pending_from_prepared(
            transaction_id.clone(),
            &prepared,
            TrafficPolicyChangeAction::Apply,
            None,
            "owner",
            "test",
            &occurred_at,
        );
        store.write_pending_locked(&pending).unwrap();
        store
            .commit_locked(2, &candidate, &history, Some(transaction_id.clone()))
            .unwrap();

        let runtime = FakeRuntime::default();
        let report = store.recover_pending(&current, &runtime).unwrap();
        assert_eq!(report.transaction_id, Some(transaction_id));
        assert_eq!(report.rolled_back_to_revision, None);
        assert!(runtime.calls().is_empty());
        assert_eq!(store.current(&current).unwrap().revision, 2);
        assert!(!root.join(STORE_DIR).join(PENDING_FILE).exists());
    }

    #[test]
    fn rollback_is_a_new_verified_revision_with_target_audit() {
        let root = temp_root("rollback");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let runtime = FakeRuntime::default();
        let mut changed = current.clone();
        changed.rules[0].enabled = false;
        store
            .apply(
                &current,
                1,
                draft(changed),
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap();
        let rolled_back = store
            .rollback(
                &current,
                2,
                1,
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap();
        assert_eq!(rolled_back.previous_revision, 2);
        assert_eq!(rolled_back.revision, 3);
        assert_eq!(rolled_back.target_revision, 1);
        assert!(rolled_back.policy.rules[0].enabled);
        assert_eq!(store.current(&current).unwrap().revision, 3);
        assert_eq!(
            rolled_back.history.action,
            TrafficPolicyChangeAction::Rollback
        );
        assert_eq!(rolled_back.history.target_revision, Some(1));
        assert_eq!(
            rolled_back.history.outcome,
            TrafficPolicyHistoryOutcome::RolledBack
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_rollback_records_target_and_runtime_reason_without_advancing() {
        let root = temp_root("rollback-failure");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let mut changed = current.clone();
        changed.rules[0].enabled = false;
        store
            .apply(
                &current,
                1,
                draft(changed),
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &FakeRuntime::default(),
            )
            .unwrap();
        let runtime = FakeRuntime {
            fail_apply: true,
            ..FakeRuntime::default()
        };
        let error = store
            .rollback(
                &current,
                2,
                1,
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::RuntimeFailed { .. }
        ));
        assert_eq!(store.current(&current).unwrap().revision, 2);
        let history = store.history_snapshot(&current).unwrap();
        let failed = history
            .entries
            .iter()
            .find(|entry| entry.action == TrafficPolicyChangeAction::Rollback)
            .unwrap();
        assert_eq!(failed.target_revision, Some(1));
        assert_eq!(failed.outcome, TrafficPolicyHistoryOutcome::Failed);
        assert_eq!(failed.applied_revision, None);
        assert_eq!(
            failed.reason_code.as_deref(),
            Some("runtime_apply_disconnected")
        );
        assert!(failed.reason.is_some());
        assert!(!root.join(STORE_DIR).join(PENDING_FILE).exists());
    }

    #[test]
    fn reorder_diff_uses_stable_ids_and_reports_all_exit_impacts() {
        let mut current = policy();
        let mut video = rule(
            "video",
            "video.example.com",
            TrafficAction::Exit(exit("exit-b")),
        );
        video.priority = 2;
        current.rules.push(video);
        let mut candidate = current.clone();
        candidate.rules = vec![
            rule(
                "video",
                "video.example.com",
                TrafficAction::Exit(exit("exit-b")),
            ),
            rule(
                "chat",
                "chat.example.com",
                TrafficAction::Exit(exit("exit-a")),
            ),
        ];
        candidate.rules[0].priority = 1;
        candidate.rules[1].priority = 2;
        let (diffs, affected, networks, exits) = diff_policies(&current, &candidate).unwrap();
        assert!(diffs
            .iter()
            .any(|diff| diff.change == TrafficPolicyChangeKind::RulesReordered));
        assert!(affected.iter().any(|object| {
            object.kind == TrafficPolicyObjectKind::Exit
                && object.object_id == Some(exit("exit-b").to_string())
        }));
        assert_eq!(exits.len(), 2);
        assert!(exits.contains(&exit("exit-a")));
        assert!(exits.contains(&exit("exit-b")));
        assert!(networks.contains(&network("home")));
    }

    #[test]
    fn unsupported_application_capability_is_rejected_before_apply() {
        let root = temp_root("application-capability");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let mut candidate = current.clone();
        candidate.rules[0].matcher.applications =
            vec![ApplicationIdentity::PackageId("org.example.app".into())];
        let mut validation = context(true);
        validation.application_identity = ApplicationIdentityCapability::unsupported(
            ApplicationIdentityPlatform::Linux,
            "package identity is unsupported",
        )
        .unwrap();
        let error = store
            .preview(&current, 1, draft(candidate), &validation)
            .unwrap_err();
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::CapabilityUnavailable(_)
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_or_non_successor_candidates_never_replace_authority() {
        let root = temp_root("invalid");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        let runtime = FakeRuntime::default();
        let mut duplicate = current.clone();
        duplicate.rules.push(duplicate.rules[0].clone());
        duplicate.rules[1].priority = 2;
        let error = store
            .apply(
                &current,
                1,
                draft(duplicate),
                &context(true),
                &TrafficPolicyAuthorization::allowed("owner", "test"),
                &runtime,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TrafficPolicyTransactionError::InvalidCandidate(_)
        ));
        assert_eq!(store.current(&current).unwrap().revision, 1);
        assert!(runtime.calls().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn transaction_files_are_isolated_from_security_policy_store() {
        let root = temp_root("isolation");
        let store = TrafficPolicyTransactionStore::new(root.clone());
        let current = policy();
        store.current(&current).unwrap();
        assert!(root.join(STORE_DIR).join(STATE_FILE).exists());
        assert!(!root.join(".policy-transaction").exists());
        assert_eq!(
            TrafficPolicyTestContract::only_one_applied(&store, &current),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    struct TrafficPolicyTestContract;

    impl TrafficPolicyTestContract {
        fn only_one_applied(
            store: &TrafficPolicyTransactionStore,
            fallback: &TrafficPolicy,
        ) -> usize {
            let current = store.current(fallback).unwrap();
            assert_eq!(current.policy.revision, current.revision);
            usize::from(store.history_snapshot(fallback).unwrap().current_revision == 1)
        }
    }
}
