//! Append-only security audit records and their user-safe activity view.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ids::{AuditId, NetworkId, NodeId, OperationId};
use super::{DomainError, DomainResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Succeeded,
    Failed,
    Denied,
}

/// A durable administrative/security record. Details are redacted before the
/// record enters the log, so raw secrets never become part of the append-only
/// history by accident.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AuditEntry {
    pub audit_id: AuditId,
    pub operation_id: Option<OperationId>,
    pub actor: NodeId,
    pub network_id: Option<NetworkId>,
    pub action: String,
    pub resource: String,
    pub outcome: AuditOutcome,
    pub occurred_at_unix_ms: u64,
    pub summary: String,
    pub details: BTreeMap<String, String>,
    /// Additional keys classified as sensitive by a policy layer.
    #[serde(default)]
    pub sensitive_keys: BTreeSet<String>,
}

impl AuditEntry {
    pub fn validate(&self) -> DomainResult<()> {
        if self.action.is_empty() || self.action.len() > 128 {
            return Err(DomainError::invalid_value("audit action is invalid"));
        }
        if self.resource.is_empty() || self.resource.len() > 512 {
            return Err(DomainError::invalid_value("audit resource is invalid"));
        }
        if self.occurred_at_unix_ms == 0 {
            return Err(DomainError::invalid_value(
                "audit occurrence time must be positive",
            ));
        }
        if self.summary.is_empty() || self.summary.len() > 2_048 {
            return Err(DomainError::invalid_value("audit summary is invalid"));
        }
        for (key, value) in &self.details {
            if key.is_empty() || key.len() > 128 {
                return Err(DomainError::invalid_value("audit detail key is invalid"));
            }
            if value.len() > 4_096 {
                return Err(DomainError::invalid_value("audit detail value is too long"));
            }
        }
        if self
            .sensitive_keys
            .iter()
            .any(|key| key.is_empty() || key.len() > 128)
        {
            return Err(DomainError::invalid_value(
                "audit sensitive field declaration is invalid",
            ));
        }
        Ok(())
    }

    /// Redact declared and credential-shaped fields. This method is
    /// idempotent: a second call cannot turn a placeholder back into a secret.
    pub fn redact_sensitive_fields(&mut self) -> usize {
        let declared: BTreeSet<_> = self
            .sensitive_keys
            .iter()
            .map(|key| normalize_key(key))
            .collect();
        let mut redacted = 0;
        for (key, value) in &mut self.details {
            if value != "[REDACTED]" && is_sensitive_key(key, &declared) {
                *value = "[REDACTED]".into();
                redacted += 1;
            }
        }
        redacted
    }

    /// Project the audit record into the separate user-facing activity DTO.
    /// Sensitive fields are omitted rather than merely masked, so a future
    /// activity consumer cannot opt back into the raw value.
    pub fn project_activity(&self) -> ActivityEntry {
        let declared: BTreeSet<_> = self
            .sensitive_keys
            .iter()
            .map(|key| normalize_key(key))
            .collect();
        let details = self
            .details
            .iter()
            .filter(|(key, value)| {
                !is_sensitive_key(key, &declared) && value.as_str() != "[REDACTED]"
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let actor_role = self
            .details
            .get("actor_role")
            .or_else(|| self.details.get("role"))
            .cloned();
        let object_kind = self
            .details
            .get("object_kind")
            .cloned()
            .unwrap_or_else(|| infer_activity_object_kind(&self.resource));
        let object_id = self.details.get("object_id").cloned();
        let reason = self
            .details
            .get("reason_code")
            .or_else(|| self.details.get("authorization_code"))
            .cloned();
        ActivityEntry {
            audit_id: self.audit_id.clone(),
            operation_id: self.operation_id.clone(),
            actor: self.actor.clone(),
            network_id: self.network_id.clone(),
            action: self.action.clone(),
            resource: self.resource.clone(),
            outcome: self.outcome,
            occurred_at_unix_ms: self.occurred_at_unix_ms,
            summary: self.summary.clone(),
            category: classify_activity_category(&self.action, self.outcome),
            severity: classify_activity_severity(&self.action, self.outcome),
            actor_role,
            object_kind,
            object_id,
            reason,
            details,
        }
    }
}

/// Activity is an intentionally separate projection. Adding a field to the
/// security audit does not automatically expose that field in product feeds.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ActivityCategory {
    #[default]
    UserActivity,
    SystemEvent,
    SecurityEvent,
    OperationFailure,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ActivitySeverity {
    #[default]
    Info,
    Notice,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct ActivityEntry {
    /// Stable audit identity. Replayed snapshots/events use this same value so
    /// consumers can merge by identity instead of by UI presentation.
    pub audit_id: AuditId,
    pub operation_id: Option<OperationId>,
    pub actor: NodeId,
    pub network_id: Option<NetworkId>,
    pub action: String,
    pub resource: String,
    pub outcome: AuditOutcome,
    pub occurred_at_unix_ms: u64,
    pub summary: String,
    /// Server-classified activity kind. UI clients must not infer this from
    /// localized copy.
    #[serde(default)]
    pub category: ActivityCategory,
    #[serde(default)]
    pub severity: ActivitySeverity,
    /// Stable role label captured at audit time. It is not re-derived from the
    /// viewer's current role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_role: Option<String>,
    /// Stable typed object identity used for filtering and permission-checked
    /// links. `resource` remains the immutable audit resource string.
    #[serde(default)]
    pub object_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
    /// Stable reason/denial code, never arbitrary localized prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub details: BTreeMap<String, String>,
}

fn infer_activity_object_kind(resource: &str) -> String {
    resource
        .split_once(':')
        .map(|(kind, _)| kind)
        .filter(|kind| !kind.is_empty())
        .unwrap_or("resource")
        .to_owned()
}

fn classify_activity_category(action: &str, outcome: AuditOutcome) -> ActivityCategory {
    if outcome == AuditOutcome::Denied {
        return ActivityCategory::SecurityEvent;
    }
    if outcome == AuditOutcome::Failed {
        return ActivityCategory::OperationFailure;
    }
    if action.starts_with("node.")
        || matches!(
            action,
            "network.start" | "network.stop" | "network.restart" | "routing.update"
        )
    {
        return ActivityCategory::SystemEvent;
    }
    if action.starts_with("network.blacklist.")
        || action.starts_with("network.invitation.")
        || action.starts_with("network.join_request.review")
        || action.starts_with("membership.")
        || action.starts_with("node.identity.")
        || action.starts_with("policy.")
    {
        return ActivityCategory::SecurityEvent;
    }
    ActivityCategory::UserActivity
}

fn classify_activity_severity(action: &str, outcome: AuditOutcome) -> ActivitySeverity {
    if outcome == AuditOutcome::Denied {
        return ActivitySeverity::Warning;
    }
    if outcome == AuditOutcome::Failed {
        return ActivitySeverity::Warning;
    }
    if matches!(
        action,
        "node.identity.rotate"
            | "network.blacklist.ban"
            | "network.invitation.revoke"
            | "membership.remove"
            | "policy.apply"
            | "policy.rollback"
    ) {
        return ActivitySeverity::Notice;
    }
    ActivitySeverity::Info
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAppendOutcome {
    Appended,
    DuplicateIgnored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditAppendError {
    Invalid(DomainError),
    DuplicateIdConflict,
}

impl fmt::Display for AuditAppendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => write!(formatter, "{error}"),
            Self::DuplicateIdConflict => {
                formatter.write_str("an audit id was reused with a different entry")
            }
        }
    }
}

impl std::error::Error for AuditAppendError {}

#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
    ids: BTreeMap<AuditId, usize>,
}

impl AuditLog {
    /// Append a sanitized immutable record. Exact retries are idempotent; an
    /// id reused with different content is rejected.
    pub fn append(
        &mut self,
        mut entry: AuditEntry,
    ) -> Result<AuditAppendOutcome, AuditAppendError> {
        entry.validate().map_err(AuditAppendError::Invalid)?;
        entry.redact_sensitive_fields();
        entry.validate().map_err(AuditAppendError::Invalid)?;

        if let Some(index) = self.ids.get(&entry.audit_id) {
            return if self.entries[*index] == entry {
                Ok(AuditAppendOutcome::DuplicateIgnored)
            } else {
                Err(AuditAppendError::DuplicateIdConflict)
            };
        }
        self.ids.insert(entry.audit_id.clone(), self.entries.len());
        self.entries.push(entry);
        Ok(AuditAppendOutcome::Appended)
    }

    /// Only sanitized records are ever returned from the audit log.
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

fn normalize_key(key: &str) -> String {
    let mut normalized = String::with_capacity(key.len());
    let mut separator = false;
    for character in key.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push('_');
            }
            normalized.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    normalized
}

fn is_sensitive_key(key: &str, declared: &BTreeSet<String>) -> bool {
    let normalized = normalize_key(key);
    if declared.contains(&normalized) {
        return true;
    }
    matches!(
        normalized.as_str(),
        "password"
            | "passwd"
            | "passphrase"
            | "secret"
            | "client_secret"
            | "token"
            | "access_token"
            | "refresh_token"
            | "id_token"
            | "bearer"
            | "authorization"
            | "cookie"
            | "set_cookie"
            | "private_key"
            | "privatekey"
            | "pre_shared_key"
            | "psk"
            | "recovery_code"
            | "recoverycode"
            | "invite"
            | "invitation"
            | "invite_code"
            | "ticket"
            | "api_key"
            | "apikey"
            | "credential"
            | "credentials"
    ) || normalized.contains("password")
        || normalized.contains("passwd")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("private_key")
        || normalized.contains("privatekey")
        || normalized.contains("recovery_code")
        || normalized.contains("recoverycode")
        || normalized.contains("invite_code")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.ends_with("_credential")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> AuditEntry {
        AuditEntry {
            audit_id: AuditId::from_public_key(b"audit-1"),
            operation_id: Some(OperationId::from_public_key(b"operation-1")),
            actor: NodeId::from_public_key(b"actor-1"),
            network_id: Some(NetworkId::from_public_key(b"network-1")),
            action: "security_policy.update".into(),
            resource: "network/home/security-policy".into(),
            outcome: AuditOutcome::Succeeded,
            occurred_at_unix_ms: 100,
            summary: "Security policy updated".into(),
            details: BTreeMap::from([
                ("revision".into(), "2".into()),
                ("accessToken".into(), "raw-access-token".into()),
                ("custom-hidden".into(), "raw-hidden".into()),
                ("safe".into(), "visible".into()),
            ]),
            sensitive_keys: BTreeSet::from(["custom_hidden".into()]),
        }
    }

    #[test]
    fn known_and_declared_sensitive_fields_are_redacted_idempotently() {
        let mut entry = entry();
        assert_eq!(entry.redact_sensitive_fields(), 2);
        assert_eq!(entry.details["accessToken"], "[REDACTED]");
        assert_eq!(entry.details["custom-hidden"], "[REDACTED]");
        assert_eq!(entry.details["safe"], "visible");
        assert_eq!(entry.redact_sensitive_fields(), 0);
    }

    #[test]
    fn activity_projection_omits_sensitive_fields() {
        let mut entry = entry();
        entry.redact_sensitive_fields();
        let activity = entry.project_activity();
        assert_eq!(
            activity.details.get("revision").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            activity.details.get("safe").map(String::as_str),
            Some("visible")
        );
        assert!(!activity.details.contains_key("accessToken"));
        assert!(!activity.details.contains_key("custom-hidden"));
    }

    #[test]
    fn audit_log_sanitizes_entries_and_is_idempotent() {
        let mut log = AuditLog::default();
        assert_eq!(log.append(entry()).unwrap(), AuditAppendOutcome::Appended);
        assert_eq!(log.entries()[0].details["accessToken"], "[REDACTED]");
        assert_eq!(
            log.append(entry()).unwrap(),
            AuditAppendOutcome::DuplicateIgnored
        );
        assert_eq!(log.entries().len(), 1);
    }

    #[test]
    fn reusing_an_audit_id_with_different_content_is_rejected() {
        let mut log = AuditLog::default();
        log.append(entry()).unwrap();
        let mut changed = entry();
        changed.summary = "Different outcome".into();
        assert_eq!(
            log.append(changed),
            Err(AuditAppendError::DuplicateIdConflict)
        );
    }
}
