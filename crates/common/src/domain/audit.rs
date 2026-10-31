//! Append-only audit entries and their user-safe activity projection.

use super::foundation::{require_non_empty, ContractIssue, ContractResult, UnixMillis};
use super::ids::{AuditId, NetworkId, NodeId, OperationId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Succeeded,
    Failed,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: AuditId,
    pub operation_id: Option<OperationId>,
    pub actor: NodeId,
    pub network_id: Option<NetworkId>,
    pub action: String,
    pub resource: String,
    pub outcome: AuditOutcome,
    pub occurred_at: UnixMillis,
    pub summary: String,
    pub details: BTreeMap<String, String>,
    pub sensitive_keys: BTreeSet<String>,
}

impl AuditEntry {
    pub fn validate(&self) -> ContractResult {
        let mut issues = Vec::new();
        require_non_empty(&self.action, "action", 256, &mut issues);
        require_non_empty(&self.resource, "resource", 512, &mut issues);
        require_non_empty(&self.summary, "summary", 2048, &mut issues);
        for key in self.details.keys() {
            require_non_empty(key, "details.key", 128, &mut issues);
        }
        finish(issues)
    }

    /// Redact both declared sensitive keys and common credential-shaped keys.
    /// The operation is idempotent and never mutates the sensitive key set.
    pub fn redact_sensitive_fields(&mut self, additional_keys: &BTreeSet<String>) {
        for (key, value) in &mut self.details {
            let normalized = key.to_ascii_lowercase();
            let sensitive = self.sensitive_keys.contains(key)
                || additional_keys.contains(key)
                || matches!(
                    normalized.as_str(),
                    "password"
                        | "passwd"
                        | "secret"
                        | "token"
                        | "access_token"
                        | "refresh_token"
                        | "private_key"
                        | "recovery_code"
                        | "cookie"
                        | "authorization"
                )
                || normalized.contains("secret")
                || normalized.contains("password");
            if sensitive {
                *value = "[REDACTED]".into();
            }
        }
    }

    pub fn project_activity(&self) -> ActivityEntry {
        let safe_details = self
            .details
            .iter()
            .filter(|(key, value)| {
                !self.sensitive_keys.contains(*key) && value.as_str() != "[REDACTED]"
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        ActivityEntry {
            audit_id: self.id,
            operation_id: self.operation_id,
            actor: self.actor,
            network_id: self.network_id,
            action: self.action.clone(),
            outcome: self.outcome,
            occurred_at: self.occurred_at,
            summary: self.summary.clone(),
            details: safe_details,
        }
    }
}

/// The activity view is intentionally a separate DTO. Adding a field to an
/// audit entry does not accidentally expose it to every activity consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub audit_id: AuditId,
    pub operation_id: Option<OperationId>,
    pub actor: NodeId,
    pub network_id: Option<NetworkId>,
    pub action: String,
    pub outcome: AuditOutcome,
    pub occurred_at: UnixMillis,
    pub summary: String,
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditAppendError {
    Contract(Vec<ContractIssue>),
    DuplicateIdConflict,
}

impl fmt::Display for AuditAppendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(issues) => write!(f, "invalid audit entry: {issues:?}"),
            Self::DuplicateIdConflict => {
                f.write_str("an audit id was reused with a different entry")
            }
        }
    }
}

impl std::error::Error for AuditAppendError {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
    ids: BTreeMap<AuditId, usize>,
}

impl AuditLog {
    pub fn append(&mut self, entry: AuditEntry) -> Result<(), AuditAppendError> {
        entry.validate().map_err(AuditAppendError::Contract)?;
        if let Some(index) = self.ids.get(&entry.id) {
            return if self.entries[*index] == entry {
                Ok(())
            } else {
                Err(AuditAppendError::DuplicateIdConflict)
            };
        }
        self.ids.insert(entry.id, self.entries.len());
        self.entries.push(entry);
        Ok(())
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    pub fn activity(&self) -> Vec<ActivityEntry> {
        self.entries
            .iter()
            .map(AuditEntry::project_activity)
            .collect()
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

    fn entry() -> AuditEntry {
        AuditEntry {
            id: AuditId::from_bytes([1; 16]),
            operation_id: Some(OperationId::from_bytes([2; 16])),
            actor: NodeId::from_bytes([3; 32]),
            network_id: Some(NetworkId::from_bytes([4; 32])),
            action: "update_security_policy".into(),
            resource: "network/home/security".into(),
            outcome: AuditOutcome::Succeeded,
            occurred_at: UnixMillis(1),
            summary: "Security policy updated".into(),
            details: BTreeMap::from([
                ("revision".into(), "2".into()),
                ("token".into(), "raw-token".into()),
                ("custom_hidden".into(), "raw-hidden".into()),
            ]),
            sensitive_keys: BTreeSet::from(["custom_hidden".into()]),
        }
    }

    #[test]
    fn redaction_is_idempotent_and_activity_hides_sensitive_values() {
        let mut entry = entry();
        entry.redact_sensitive_fields(&BTreeSet::new());
        assert_eq!(entry.details["token"], "[REDACTED]");
        assert_eq!(entry.details["custom_hidden"], "[REDACTED]");
        entry.redact_sensitive_fields(&BTreeSet::new());
        assert_eq!(entry.details["token"], "[REDACTED]");

        let activity = entry.project_activity();
        assert_eq!(
            activity.details.get("revision").map(String::as_str),
            Some("2")
        );
        assert!(!activity.details.contains_key("token"));
        assert!(!activity.details.contains_key("custom_hidden"));
    }

    #[test]
    fn audit_is_append_only_and_duplicate_id_is_idempotent() {
        let mut log = AuditLog::default();
        let entry = entry();
        log.append(entry.clone()).expect("append");
        log.append(entry.clone()).expect("idempotent replay");
        assert_eq!(log.entries().len(), 1);

        let mut conflict = entry;
        conflict.summary = "different".into();
        assert_eq!(
            log.append(conflict),
            Err(AuditAppendError::DuplicateIdConflict)
        );
    }
}
