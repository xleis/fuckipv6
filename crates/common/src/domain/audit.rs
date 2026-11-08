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

pub const AUDIT_CHAIN_SCHEMA_VERSION: u32 = 1;
pub const AUDIT_GENESIS_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// One immutable link in the security-audit hash chain. The sequence is
/// global across rotated segments, and every exported record carries enough
/// evidence for an offline verifier to check ordering and content integrity.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AuditChainRecord {
    pub schema_version: u32,
    pub sequence: u64,
    pub previous_hash: String,
    pub entry: AuditEntry,
    pub entry_hash: String,
}

impl AuditChainRecord {
    pub fn seal(
        sequence: u64,
        previous_hash: impl Into<String>,
        mut entry: AuditEntry,
    ) -> Result<Self, AuditAppendError> {
        if sequence == 0 {
            return Err(AuditAppendError::Invalid(DomainError::invalid_value(
                "audit sequence must be positive",
            )));
        }
        entry.validate().map_err(AuditAppendError::Invalid)?;
        entry.redact_sensitive_fields();
        entry.validate().map_err(AuditAppendError::Invalid)?;
        let previous_hash = previous_hash.into();
        if !is_sha256_hex(&previous_hash) {
            return Err(AuditAppendError::Invalid(DomainError::invalid_value(
                "audit previous hash is invalid",
            )));
        }
        let mut record = Self {
            schema_version: AUDIT_CHAIN_SCHEMA_VERSION,
            sequence,
            previous_hash,
            entry,
            entry_hash: String::new(),
        };
        record.entry_hash = record
            .compute_entry_hash()
            .map_err(AuditAppendError::Invalid)?;
        Ok(record)
    }

    /// Recompute the hash from the canonical JSON payload. `entry_hash` itself
    /// is deliberately excluded so verification detects changes to the entry,
    /// sequence or predecessor link.
    pub fn compute_entry_hash(&self) -> DomainResult<String> {
        #[derive(Serialize)]
        struct CanonicalAuditPayload<'a> {
            schema_version: u32,
            sequence: u64,
            previous_hash: &'a str,
            entry: &'a AuditEntry,
        }
        let payload = serde_json::to_vec(&CanonicalAuditPayload {
            schema_version: self.schema_version,
            sequence: self.sequence,
            previous_hash: &self.previous_hash,
            entry: &self.entry,
        })
        .map_err(|_| DomainError::invalid_value("audit record could not be canonicalized"))?;
        Ok(blake3::hash(&payload).to_hex().to_string())
    }

    pub fn verify(
        &self,
        expected_sequence: u64,
        expected_previous_hash: &str,
    ) -> Result<(), AuditChainError> {
        if self.schema_version != AUDIT_CHAIN_SCHEMA_VERSION {
            return Err(AuditChainError::UnsupportedSchema {
                sequence: self.sequence,
                schema_version: self.schema_version,
            });
        }
        if self.sequence != expected_sequence {
            return Err(AuditChainError::SequenceMismatch {
                expected: expected_sequence,
                actual: self.sequence,
            });
        }
        if self.previous_hash != expected_previous_hash {
            return Err(AuditChainError::PreviousHashMismatch {
                sequence: self.sequence,
            });
        }
        self.entry
            .validate()
            .map_err(|_| AuditChainError::InvalidEntry {
                sequence: self.sequence,
            })?;
        if !is_sha256_hex(&self.entry_hash) {
            return Err(AuditChainError::HashMismatch {
                sequence: self.sequence,
            });
        }
        let actual = self
            .compute_entry_hash()
            .map_err(|_| AuditChainError::InvalidEntry {
                sequence: self.sequence,
            })?;
        if actual != self.entry_hash {
            return Err(AuditChainError::HashMismatch {
                sequence: self.sequence,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditChainError {
    UnsupportedSchema { sequence: u64, schema_version: u32 },
    SequenceMismatch { expected: u64, actual: u64 },
    PreviousHashMismatch { sequence: u64 },
    HashMismatch { sequence: u64 },
    InvalidEntry { sequence: u64 },
}

impl fmt::Display for AuditChainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema {
                sequence,
                schema_version,
            } => write!(
                formatter,
                "audit record {sequence} uses unsupported schema {schema_version}"
            ),
            Self::SequenceMismatch { expected, actual } => write!(
                formatter,
                "audit sequence discontinuity: expected {expected}, got {actual}"
            ),
            Self::PreviousHashMismatch { sequence } => {
                write!(
                    formatter,
                    "audit record {sequence} has a broken predecessor link"
                )
            }
            Self::HashMismatch { sequence } => {
                write!(formatter, "audit record {sequence} failed its content hash")
            }
            Self::InvalidEntry { sequence } => {
                write!(
                    formatter,
                    "audit record {sequence} contains an invalid entry"
                )
            }
        }
    }
}

impl std::error::Error for AuditChainError {}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct AuditChainVerification {
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub last_hash: String,
    pub record_count: u64,
}

/// Verify one uninterrupted append-only chain. Rotation is a storage detail:
/// callers must verify all segment records together.
pub fn verify_audit_chain(
    records: &[AuditChainRecord],
) -> Result<AuditChainVerification, AuditChainError> {
    let mut expected_sequence = 1;
    let mut previous_hash = AUDIT_GENESIS_HASH.to_owned();
    for record in records {
        record.verify(expected_sequence, &previous_hash)?;
        expected_sequence = expected_sequence.saturating_add(1);
        previous_hash = record.entry_hash.clone();
    }
    Ok(AuditChainVerification {
        first_sequence: (!records.is_empty()).then_some(1),
        last_sequence: records.last().map(|record| record.sequence),
        last_hash: previous_hash,
        record_count: records.len() as u64,
    })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

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

    fn chain() -> Vec<AuditChainRecord> {
        let mut records = Vec::new();
        for index in 0..3 {
            let mut item = entry();
            item.audit_id = AuditId::from_public_key(format!("audit-{index}").as_bytes());
            item.summary = format!("Audit {index}");
            item.occurred_at_unix_ms += index;
            let sequence = records.len() as u64 + 1;
            let previous_hash = records
                .last()
                .map(|record: &AuditChainRecord| record.entry_hash.clone())
                .unwrap_or_else(|| AUDIT_GENESIS_HASH.to_owned());
            records.push(AuditChainRecord::seal(sequence, previous_hash, item).unwrap());
        }
        records
    }

    #[test]
    fn hash_chain_verifies_and_detects_content_tampering() {
        let mut records = chain();
        let verified = verify_audit_chain(&records).unwrap();
        assert_eq!(verified.first_sequence, Some(1));
        assert_eq!(verified.last_sequence, Some(3));
        assert_eq!(verified.record_count, 3);
        assert_eq!(verified.last_hash, records[2].entry_hash);

        records[1].entry.summary = "tampered".into();
        assert_eq!(
            verify_audit_chain(&records),
            Err(AuditChainError::HashMismatch { sequence: 2 })
        );

        let mut records = chain();
        records[2].entry_hash = "f".repeat(64);
        assert_eq!(
            verify_audit_chain(&records),
            Err(AuditChainError::HashMismatch { sequence: 3 })
        );
    }

    #[test]
    fn hash_chain_detects_sequence_gaps_and_broken_links() {
        let mut records = chain();
        records.remove(1);
        assert_eq!(
            verify_audit_chain(&records),
            Err(AuditChainError::SequenceMismatch {
                expected: 2,
                actual: 3,
            })
        );

        let mut records = chain();
        records[1].previous_hash = "0".repeat(64);
        assert_eq!(
            verify_audit_chain(&records),
            Err(AuditChainError::PreviousHashMismatch { sequence: 2 })
        );
    }

    #[test]
    fn sealing_redacts_sensitive_values_before_hashing() {
        let record = AuditChainRecord::seal(1, AUDIT_GENESIS_HASH, entry()).unwrap();
        assert_eq!(record.entry.details["accessToken"], "[REDACTED]");
        assert_eq!(record.entry.details["custom-hidden"], "[REDACTED]");
        assert!(verify_audit_chain(&[record]).is_ok());
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
