use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{ContentHash, IdempotencyKey, NodeId, OperationId};
use super::{DomainError, DomainResult};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Node,
    Network,
    Membership,
    Group,
    SecurityPolicy,
    TrafficPolicy,
    Capability,
    ExitGrant,
    CoordinatorTerm,
    FileShare,
    Operation,
    Audit,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
pub struct DomainObjectRef {
    pub kind: ObjectKind,
    pub id: String,
}

impl DomainObjectRef {
    pub fn validate(&self) -> DomainResult<()> {
        if self.id.is_empty() || self.id.len() > 256 {
            return Err(DomainError::invalid_value("domain object id is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct DomainActorRef {
    pub node_id: NodeId,
    pub display: String,
}

impl DomainActorRef {
    fn validate(&self) -> DomainResult<()> {
        if self.display.is_empty() || self.display.len() > 80 {
            return Err(DomainError::invalid_value("actor display name is invalid"));
        }
        Ok(())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    NodeStart,
    NodeStop,
    NodeRestart,
    NetworkCreate,
    NetworkApply,
    NetworkLeave,
    MembershipApprove,
    MembershipReject,
    MembershipRemove,
    PolicyTest,
    PolicyApply,
    PolicyRollback,
    TrafficRulesApply,
    CapabilityEnable,
    CapabilityDisable,
    ExitSelect,
    ExitRevoke,
    CoordinatorCandidatesUpdate,
    CoordinatorRecover,
    FileShareUpdate,
    FileUploadStart,
    FileDownloadStart,
    OwnerPasskeyRegister,
    OwnerPasskeyRevoke,
    CertificateReload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Queued,
    Running,
    WaitingForReauth,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl OperationState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }

    pub fn can_transition_to(self, target: Self) -> bool {
        matches!(
            (self, target),
            (Self::Queued, Self::Running)
                | (Self::Queued, Self::Failed)
                | (Self::Queued, Self::Cancelled)
                | (Self::Queued, Self::TimedOut)
                | (Self::Running, Self::WaitingForReauth)
                | (Self::Running, Self::Succeeded)
                | (Self::Running, Self::Failed)
                | (Self::Running, Self::Cancelled)
                | (Self::Running, Self::TimedOut)
                | (Self::WaitingForReauth, Self::Running)
                | (Self::WaitingForReauth, Self::Failed)
                | (Self::WaitingForReauth, Self::Cancelled)
                | (Self::WaitingForReauth, Self::TimedOut)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct OperationRecovery {
    pub kind: String,
    pub hint_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct OperationError {
    pub code: String,
    pub message_key: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    pub object: Option<DomainObjectRef>,
    pub retryable: bool,
    pub recovery: Option<OperationRecovery>,
    /// Cancellation is not the same as having no side effects.
    #[serde(default)]
    pub partial_changes: Vec<DomainObjectRef>,
}

impl OperationError {
    pub fn validate(&self) -> DomainResult<()> {
        if !is_stable_symbol(&self.code, 128) || !is_stable_symbol(&self.message_key, 128) {
            return Err(DomainError::invalid_value("operation error is invalid"));
        }
        for (key, value) in &self.params {
            if !is_stable_symbol(key, 128) || value.len() > 2_048 {
                return Err(DomainError::invalid_value(
                    "operation error parameters are invalid",
                ));
            }
        }
        if let Some(recovery) = &self.recovery {
            if !is_stable_token(&recovery.kind, 64) || !is_stable_symbol(&recovery.hint_key, 128) {
                return Err(DomainError::invalid_value(
                    "operation recovery advice is invalid",
                ));
            }
        }
        if let Some(object) = &self.object {
            object.validate()?;
        }
        for object in &self.partial_changes {
            object.validate()?;
        }
        Ok(())
    }
}

fn is_stable_symbol(value: &str, max_len: usize) -> bool {
    is_stable_token(value, max_len) && value.contains('.')
}

fn is_stable_token(value: &str, max_len: usize) -> bool {
    if value.is_empty() || value.len() > max_len {
        return false;
    }
    value.split('.').all(|segment| {
        !segment.is_empty()
            && segment.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
    })
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct Operation {
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub actor: DomainActorRef,
    pub target: DomainObjectRef,
    pub idempotency_key: IdempotencyKey,
    pub state: OperationState,
    pub stage: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    /// The number of execution attempts started. A retry uses the same
    /// operation id and idempotency key but starts a new attempt.
    pub attempt: u32,
    pub max_attempts: u32,
    pub error: Option<OperationError>,
}

impl Operation {
    pub fn validate(&self) -> DomainResult<()> {
        self.actor.validate()?;
        self.target.validate()?;
        if self.stage.is_empty() || self.stage.len() > 96 {
            return Err(DomainError::invalid_value("operation stage is invalid"));
        }
        if self.updated_at_unix_ms < self.created_at_unix_ms {
            return Err(DomainError::invariant(
                "operation update time predates creation",
            ));
        }
        if self.max_attempts == 0 || self.attempt > self.max_attempts {
            return Err(DomainError::invalid_value(
                "operation attempt budget is invalid",
            ));
        }
        if matches!(
            self.state,
            OperationState::Failed | OperationState::TimedOut
        ) && self.error.is_none()
        {
            return Err(DomainError::invariant(
                "failed or timed-out operation requires a stable error",
            ));
        }
        if let Some(error) = &self.error {
            error.validate()?;
        }
        Ok(())
    }

    pub fn transition(
        &mut self,
        state: OperationState,
        stage: impl Into<String>,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        if !self.state.can_transition_to(state) {
            return Err(DomainError::new(
                "domain.invalid_operation_transition",
                format!(
                    "operation cannot transition from {:?} to {:?}",
                    self.state, state
                ),
            ));
        }
        let stage = stage.into();
        if stage.is_empty() || stage.len() > 96 {
            return Err(DomainError::invalid_value("operation stage is invalid"));
        }
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "operation update time moved backwards",
            ));
        }
        let starts_new_attempt =
            state == OperationState::Running && matches!(self.state, OperationState::Queued);
        if starts_new_attempt {
            if self.attempt >= self.max_attempts {
                return Err(DomainError::new(
                    "domain.operation_retry_exhausted",
                    "operation has no attempts remaining",
                ));
            }
            self.attempt += 1;
        }
        self.state = state;
        self.stage = stage;
        self.updated_at_unix_ms = now_unix_ms;
        Ok(())
    }

    pub fn fail(
        &mut self,
        error: OperationError,
        timed_out: bool,
        now_unix_ms: u64,
    ) -> DomainResult<()> {
        error.validate()?;
        let target = if timed_out {
            OperationState::TimedOut
        } else {
            OperationState::Failed
        };
        if !self.state.can_transition_to(target) {
            return Err(DomainError::invalid_transition(format!(
                "operation cannot transition from {:?} to {:?}",
                self.state, target
            )));
        }
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "operation update time moved backwards",
            ));
        }
        self.error = Some(error);
        self.state = target;
        self.stage = if timed_out { "timed_out" } else { "failed" }.into();
        self.updated_at_unix_ms = now_unix_ms;
        Ok(())
    }

    /// Retry a terminal failure without creating a second side effect or
    /// changing the operation/idempotency identity. The original error must
    /// explicitly be retryable and the attempt budget must remain available.
    pub fn retry(&mut self, now_unix_ms: u64) -> DomainResult<()> {
        if !matches!(
            self.state,
            OperationState::Failed | OperationState::TimedOut
        ) {
            return Err(DomainError::invalid_transition(
                "only failed or timed-out operations can be retried",
            ));
        }
        if self.attempt >= self.max_attempts {
            return Err(DomainError::new(
                "domain.operation_retry_exhausted",
                "operation has no attempts remaining",
            ));
        }
        let retryable = self.error.as_ref().is_some_and(|error| error.retryable);
        if !retryable {
            return Err(DomainError::new(
                "domain.operation_retry_not_allowed",
                "operation error is not retryable",
            ));
        }
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "operation update time moved backwards",
            ));
        }
        self.attempt += 1;
        self.state = OperationState::Running;
        self.stage = "retrying".into();
        self.updated_at_unix_ms = now_unix_ms;
        self.error = None;
        Ok(())
    }

    pub fn cancel(&mut self, reason: Option<OperationError>, now_unix_ms: u64) -> DomainResult<()> {
        if !self.state.can_transition_to(OperationState::Cancelled) {
            return Err(DomainError::invalid_transition(format!(
                "operation cannot transition from {:?} to {:?}",
                self.state,
                OperationState::Cancelled
            )));
        }
        if now_unix_ms < self.updated_at_unix_ms {
            return Err(DomainError::invariant(
                "operation update time moved backwards",
            ));
        }
        if let Some(error) = reason {
            error.validate()?;
            self.error = Some(error);
        }
        self.state = OperationState::Cancelled;
        self.stage = "cancelled".into();
        self.updated_at_unix_ms = now_unix_ms;
        Ok(())
    }
}

/// Idempotency is scoped by the submitting node. A repeated key returns the
/// original operation only when the request fingerprint matches. Reusing a key
/// for a different payload is a conflict, never a second side effect.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct IdempotencyRegistry {
    entries: BTreeMap<String, IdempotencyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct IdempotencyEntry {
    pub operation_id: OperationId,
    pub request_digest: ContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IdempotentSubmission {
    Created { operation_id: OperationId },
    Existing { operation_id: OperationId },
    Conflict { existing_operation_id: OperationId },
}

impl IdempotencyRegistry {
    pub fn resolve_or_insert(
        &mut self,
        actor: &NodeId,
        key: &IdempotencyKey,
        operation_id: OperationId,
        request_digest: ContentHash,
    ) -> IdempotentSubmission {
        let scoped = format!("{}:{}", actor.as_str(), key.as_str());
        if let Some(existing) = self.entries.get(&scoped) {
            return if existing.request_digest == request_digest {
                IdempotentSubmission::Existing {
                    operation_id: existing.operation_id.clone(),
                }
            } else {
                IdempotentSubmission::Conflict {
                    existing_operation_id: existing.operation_id.clone(),
                }
            };
        }
        self.entries.insert(
            scoped,
            IdempotencyEntry {
                operation_id: operation_id.clone(),
                request_digest,
            },
        );
        IdempotentSubmission::Created { operation_id }
    }

    pub fn get(&self, actor: &NodeId, key: &IdempotencyKey) -> Option<&OperationId> {
        let scoped = format!("{}:{}", actor.as_str(), key.as_str());
        self.entries.get(&scoped).map(|entry| &entry.operation_id)
    }

    pub fn validate(&self) -> DomainResult<()> {
        let ids: BTreeSet<_> = self
            .entries
            .values()
            .map(|entry| &entry.operation_id)
            .collect();
        if ids.len() != self.entries.len() {
            return Err(DomainError::invariant(
                "one operation was registered under multiple idempotency keys",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> DomainActorRef {
        DomainActorRef {
            node_id: NodeId::from_public_key(b"actor"),
            display: "Owner".into(),
        }
    }

    fn operation() -> Operation {
        Operation {
            operation_id: OperationId::from_public_key(b"operation"),
            kind: OperationKind::PolicyApply,
            actor: actor(),
            target: DomainObjectRef {
                kind: ObjectKind::SecurityPolicy,
                id: "policy-1".into(),
            },
            idempotency_key: IdempotencyKey::from_public_key(b"idempotency"),
            state: OperationState::Queued,
            stage: "queued".into(),
            created_at_unix_ms: 10,
            updated_at_unix_ms: 10,
            attempt: 0,
            max_attempts: 3,
            error: None,
        }
    }

    #[test]
    fn idempotency_key_returns_the_same_operation() {
        let actor = NodeId::from_public_key(b"actor");
        let key = IdempotencyKey::from_public_key(b"key");
        let first = OperationId::from_public_key(b"first");
        let second = OperationId::from_public_key(b"second");
        let digest = ContentHash::from_bytes(b"request");
        let mut registry = IdempotencyRegistry::default();
        assert_eq!(
            registry.resolve_or_insert(&actor, &key, first.clone(), digest.clone()),
            IdempotentSubmission::Created {
                operation_id: first.clone()
            }
        );
        assert_eq!(
            registry.resolve_or_insert(&actor, &key, second.clone(), digest.clone()),
            IdempotentSubmission::Existing {
                operation_id: first.clone()
            }
        );
        assert_eq!(
            registry.resolve_or_insert(
                &actor,
                &key,
                second.clone(),
                ContentHash::from_bytes(b"different-request")
            ),
            IdempotentSubmission::Conflict {
                existing_operation_id: first.clone()
            }
        );
        assert_eq!(registry.get(&actor, &key), Some(&first));
        registry.validate().unwrap();
    }

    #[test]
    fn reauthentication_pauses_and_resumes_without_a_terminal_result() {
        let mut operation = operation();
        operation
            .transition(OperationState::Running, "applying", 11)
            .unwrap();
        operation
            .transition(OperationState::WaitingForReauth, "reauth", 12)
            .unwrap();
        operation
            .transition(OperationState::Running, "applying", 13)
            .unwrap();
        operation
            .transition(OperationState::Succeeded, "committed", 14)
            .unwrap();
        assert!(operation
            .transition(OperationState::Running, "retry", 15)
            .is_err());
        operation.validate().unwrap();
    }

    #[test]
    fn failed_operation_requires_actionable_error() {
        let mut operation = operation();
        operation
            .transition(OperationState::Running, "applying", 11)
            .unwrap();
        let error = OperationError {
            code: "policy.conflict".into(),
            message_key: "error.policy_conflict".into(),
            params: BTreeMap::new(),
            object: Some(operation.target.clone()),
            retryable: false,
            recovery: Some(OperationRecovery {
                kind: "refresh_policy".into(),
                hint_key: "recovery.refresh_policy".into(),
            }),
            partial_changes: Vec::new(),
        };
        operation.fail(error, false, 12).unwrap();
        assert_eq!(operation.state, OperationState::Failed);
        assert!(operation.error.is_some());
        operation.validate().unwrap();
    }

    #[test]
    fn cancellation_reports_partial_changes() {
        let mut operation = operation();
        operation
            .transition(OperationState::Running, "applying", 11)
            .unwrap();
        let error = OperationError {
            code: "operation.cancelled".into(),
            message_key: "error.operation_cancelled".into(),
            params: BTreeMap::new(),
            object: None,
            retryable: true,
            recovery: None,
            partial_changes: vec![DomainObjectRef {
                kind: ObjectKind::Capability,
                id: "relay-1".into(),
            }],
        };
        operation.cancel(Some(error), 12).unwrap();
        assert_eq!(operation.state, OperationState::Cancelled);
        assert_eq!(operation.error.as_ref().unwrap().partial_changes.len(), 1);
    }

    #[test]
    fn retry_reuses_operation_identity_and_obeys_attempt_budget() {
        let mut operation = operation();
        operation
            .transition(OperationState::Running, "applying", 11)
            .unwrap();
        assert_eq!(operation.attempt, 1);
        let transient_error = OperationError {
            code: "network.timeout".into(),
            message_key: "error.network_timeout".into(),
            params: BTreeMap::new(),
            object: None,
            retryable: true,
            recovery: Some(OperationRecovery {
                kind: "retry".into(),
                hint_key: "recovery.retry".into(),
            }),
            partial_changes: Vec::new(),
        };
        operation.fail(transient_error, true, 12).unwrap();
        let original_id = operation.operation_id.clone();
        operation.retry(13).unwrap();
        assert_eq!(operation.operation_id, original_id);
        assert_eq!(operation.state, OperationState::Running);
        assert_eq!(operation.attempt, 2);
        assert!(operation.error.is_none());

        operation
            .fail(
                OperationError {
                    code: "network.timeout".into(),
                    message_key: "error.network_timeout".into(),
                    params: BTreeMap::new(),
                    object: None,
                    retryable: true,
                    recovery: None,
                    partial_changes: Vec::new(),
                },
                true,
                14,
            )
            .unwrap();
        operation.retry(15).unwrap();
        assert_eq!(operation.attempt, 3);
        operation
            .fail(
                OperationError {
                    code: "network.timeout".into(),
                    message_key: "error.network_timeout".into(),
                    params: BTreeMap::new(),
                    object: None,
                    retryable: true,
                    recovery: None,
                    partial_changes: Vec::new(),
                },
                true,
                16,
            )
            .unwrap();
        assert_eq!(
            operation.retry(17).unwrap_err().code(),
            "domain.operation_retry_exhausted"
        );
    }

    #[test]
    fn non_retryable_error_cannot_be_retried() {
        let mut operation = operation();
        operation
            .transition(OperationState::Running, "applying", 11)
            .unwrap();
        operation
            .fail(
                OperationError {
                    code: "policy.invalid".into(),
                    message_key: "error.policy_invalid".into(),
                    params: BTreeMap::new(),
                    object: None,
                    retryable: false,
                    recovery: None,
                    partial_changes: Vec::new(),
                },
                false,
                12,
            )
            .unwrap();
        assert_eq!(
            operation.retry(13).unwrap_err().code(),
            "domain.operation_retry_not_allowed"
        );
    }
}
