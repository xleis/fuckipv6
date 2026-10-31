//! Durable operation state, idempotency, retry, and stable errors.

use super::foundation::{require_non_empty, ContractIssue, ContractResult, UnixMillis};
use super::ids::OperationId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractIssue> {
        let value = value.into();
        if value.is_empty() || value.len() > 256 {
            return Err(ContractIssue::new(
                "invalid_idempotency_key",
                "idempotency_key",
                "idempotency key must contain 1-256 characters",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestDigest(String);

impl RequestDigest {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractIssue> {
        let value = value.into();
        let valid = value
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()));
        if !valid {
            return Err(ContractIssue::new(
                "invalid_request_digest",
                "request_digest",
                "request digest must use sha256:<64 lowercase or uppercase hex characters>",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    RollingBack,
    RolledBack,
}

impl OperationState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Cancelled | Self::RolledBack)
    }

    fn allowed_next(self) -> &'static [Self] {
        match self {
            Self::Queued => &[Self::Running, Self::Failed, Self::Cancelled],
            Self::Running => &[
                Self::Succeeded,
                Self::Failed,
                Self::Cancelled,
                Self::RollingBack,
            ],
            Self::Failed => &[Self::RollingBack, Self::Succeeded],
            Self::RollingBack => &[Self::RolledBack, Self::Failed],
            Self::Succeeded | Self::Cancelled | Self::RolledBack => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    Retry,
    RefreshSnapshot,
    Reauthenticate,
    ChangeConfiguration,
    ContactAdministrator,
    NoAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAdvice {
    pub kind: RecoveryKind,
    pub suggested_action: Option<String>,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableError {
    pub code: String,
    pub message: String,
    pub parameters: BTreeMap<String, String>,
    pub recovery: RecoveryAdvice,
}

impl StableError {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        require_non_empty(&self.code, "code", 128, &mut issues);
        require_non_empty(&self.message, "message", 2048, &mut issues);
        if let Some(action) = self.recovery.suggested_action.as_deref() {
            require_non_empty(action, "recovery.suggested_action", 512, &mut issues);
        }
        for key in self.parameters.keys() {
            require_non_empty(key, "parameters.key", 128, &mut issues);
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub retryable_codes: BTreeSet<String>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            retryable_codes: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub id: OperationId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: RequestDigest,
    pub state: OperationState,
    pub attempt: u32,
    pub max_attempts: u32,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub revision: u64,
    pub error: Option<StableError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationError {
    InvalidTransition {
        current: OperationState,
        requested: OperationState,
    },
    StaleRevision {
        expected: u64,
        actual: u64,
    },
    RetryExhausted,
    RetryNotAllowed,
    ErrorRequired,
    StableError(Vec<ContractIssue>),
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransition { current, requested } => {
                write!(
                    f,
                    "invalid operation transition: {current:?} -> {requested:?}"
                )
            }
            Self::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "stale operation revision: expected {expected}, got {actual}"
                )
            }
            Self::RetryExhausted => f.write_str("operation retry limit exhausted"),
            Self::RetryNotAllowed => f.write_str("operation error is not retryable"),
            Self::ErrorRequired => f.write_str("failed operations require a stable error"),
            Self::StableError(issues) => write!(f, "invalid stable error: {issues:?}"),
        }
    }
}

impl std::error::Error for OperationError {}

impl Operation {
    pub fn new(
        id: OperationId,
        idempotency_key: IdempotencyKey,
        request_digest: RequestDigest,
        max_attempts: u32,
        now: UnixMillis,
    ) -> Self {
        Self {
            id,
            idempotency_key,
            request_digest,
            state: OperationState::Queued,
            attempt: 0,
            max_attempts: max_attempts.max(1),
            created_at: now,
            updated_at: now,
            revision: 1,
            error: None,
        }
    }

    pub fn transition(
        &mut self,
        next: OperationState,
        expected_revision: u64,
        error: Option<StableError>,
        now: UnixMillis,
    ) -> Result<(), OperationError> {
        if self.revision != expected_revision {
            return Err(OperationError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if !self.state.allowed_next().contains(&next) {
            return Err(OperationError::InvalidTransition {
                current: self.state,
                requested: next,
            });
        }
        if next == OperationState::Failed && error.is_none() {
            return Err(OperationError::ErrorRequired);
        }
        if let Some(error) = &error {
            error.validate().map_err(OperationError::StableError)?;
        }
        self.state = next;
        self.error = error;
        self.updated_at = now;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    pub fn retry(&mut self, policy: &RetryPolicy, now: UnixMillis) -> Result<(), OperationError> {
        if self.state != OperationState::Failed {
            return Err(OperationError::InvalidTransition {
                current: self.state,
                requested: OperationState::Running,
            });
        }
        if self.attempt >= policy.max_attempts || self.attempt >= self.max_attempts {
            return Err(OperationError::RetryExhausted);
        }
        let error = self.error.as_ref().ok_or(OperationError::ErrorRequired)?;
        if error.recovery.kind != RecoveryKind::Retry
            || !policy.retryable_codes.contains(&error.code)
        {
            return Err(OperationError::RetryNotAllowed);
        }
        self.attempt = self.attempt.saturating_add(1);
        self.state = OperationState::Running;
        self.error = None;
        self.updated_at = now;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "operation", rename_all = "snake_case")]
pub enum IdempotentSubmission {
    Created(Operation),
    Existing(Operation),
    Conflict { existing_operation_id: OperationId },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRegistry {
    operations: BTreeMap<IdempotencyKey, Operation>,
}

impl OperationRegistry {
    pub fn submit(
        &mut self,
        id: OperationId,
        key: IdempotencyKey,
        digest: RequestDigest,
        max_attempts: u32,
        now: UnixMillis,
    ) -> IdempotentSubmission {
        if let Some(existing) = self.operations.get(&key) {
            return if existing.request_digest == digest {
                IdempotentSubmission::Existing(existing.clone())
            } else {
                IdempotentSubmission::Conflict {
                    existing_operation_id: existing.id,
                }
            };
        }
        let operation = Operation::new(id, key.clone(), digest, max_attempts, now);
        self.operations.insert(key, operation.clone());
        IdempotentSubmission::Created(operation)
    }

    pub fn get(&self, key: &IdempotencyKey) -> Option<&Operation> {
        self.operations.get(key)
    }

    pub fn get_mut(&mut self, key: &IdempotencyKey) -> Option<&mut Operation> {
        self.operations.get_mut(key)
    }
}

fn finish(issues: Vec<ContractIssue>) -> ContractResult {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> IdempotencyKey {
        IdempotencyKey::new(value).expect("key")
    }

    fn digest(value: u8) -> RequestDigest {
        RequestDigest::new(format!("sha256:{}", format!("{value:02x}").repeat(32))).expect("digest")
    }

    fn retry_error() -> StableError {
        StableError {
            code: "network_timeout".into(),
            message: "network timed out".into(),
            parameters: BTreeMap::new(),
            recovery: RecoveryAdvice {
                kind: RecoveryKind::Retry,
                suggested_action: None,
                retry_after_ms: Some(100),
            },
        }
    }

    #[test]
    fn idempotency_returns_existing_and_rejects_payload_mismatch() {
        let mut registry = OperationRegistry::default();
        let first = registry.submit(
            OperationId::from_bytes([1; 16]),
            key("request-1"),
            digest(1),
            3,
            UnixMillis(1),
        );
        assert!(matches!(first, IdempotentSubmission::Created(_)));
        let replay = registry.submit(
            OperationId::from_bytes([2; 16]),
            key("request-1"),
            digest(1),
            3,
            UnixMillis(2),
        );
        assert!(
            matches!(replay, IdempotentSubmission::Existing(operation) if operation.id == OperationId::from_bytes([1; 16]))
        );
        let conflict = registry.submit(
            OperationId::from_bytes([3; 16]),
            key("request-1"),
            digest(2),
            3,
            UnixMillis(3),
        );
        assert!(matches!(conflict, IdempotentSubmission::Conflict { .. }));
    }

    #[test]
    fn failed_operation_requires_error_and_only_retries_when_allowed() {
        let mut operation = Operation::new(
            OperationId::from_bytes([1; 16]),
            key("request"),
            digest(1),
            3,
            UnixMillis(1),
        );
        operation
            .transition(OperationState::Running, 1, None, UnixMillis(2))
            .expect("run");
        assert_eq!(
            operation
                .transition(OperationState::Failed, 2, None, UnixMillis(3))
                .expect_err("missing error"),
            OperationError::ErrorRequired
        );
        operation
            .transition(
                OperationState::Failed,
                2,
                Some(retry_error()),
                UnixMillis(3),
            )
            .expect("fail");

        let not_allowed = RetryPolicy {
            max_attempts: 3,
            retryable_codes: BTreeSet::new(),
        };
        assert_eq!(
            operation.retry(&not_allowed, UnixMillis(4)),
            Err(OperationError::RetryNotAllowed)
        );
        let allowed = RetryPolicy {
            max_attempts: 3,
            retryable_codes: BTreeSet::from(["network_timeout".into()]),
        };
        operation.retry(&allowed, UnixMillis(4)).expect("retry");
        assert_eq!(operation.state, OperationState::Running);
        assert_eq!(operation.attempt, 1);
    }

    #[test]
    fn stale_operation_writes_are_rejected() {
        let mut operation = Operation::new(
            OperationId::from_bytes([1; 16]),
            key("request"),
            digest(1),
            3,
            UnixMillis(1),
        );
        assert_eq!(
            operation
                .transition(OperationState::Running, 99, None, UnixMillis(2))
                .expect_err("stale"),
            OperationError::StaleRevision {
                expected: 99,
                actual: 1,
            }
        );
    }
}
