//! Small shared value types used by the domain contracts.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
///
/// Time is represented as data rather than a localized date string so policy
/// expiry, discovery rollback, audit ordering, and leases share one comparison
/// rule across Rust, TypeScript, and Android.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UnixMillis(pub u64);

impl UnixMillis {
    pub const ZERO: Self = Self(0);

    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        Self(u64::try_from(millis).unwrap_or(u64::MAX))
    }

    pub fn saturating_add_secs(self, seconds: u64) -> Self {
        Self(self.0.saturating_add(seconds.saturating_mul(1_000)))
    }

    pub fn is_before(self, other: Self) -> bool {
        self.0 < other.0
    }
}

impl fmt::Display for UnixMillis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A machine-readable contract violation.
///
/// `code` is stable across adapters. `field` is a JSON-style path and `message`
/// is safe to surface to an operator after localization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractIssue {
    pub code: String,
    pub field: String,
    pub message: String,
}

impl ContractIssue {
    pub fn new(
        code: impl Into<String>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ContractIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.field, self.message)
    }
}

impl std::error::Error for ContractIssue {}

pub type ContractResult = Result<(), Vec<ContractIssue>>;

pub fn require_non_empty(
    value: &str,
    field: &str,
    max_chars: usize,
    issues: &mut Vec<ContractIssue>,
) {
    let length = value.chars().count();
    if value.trim().is_empty() {
        issues.push(ContractIssue::new(
            "value_required",
            field,
            "value must not be empty",
        ));
    } else if length > max_chars {
        issues.push(ContractIssue::new(
            "value_too_long",
            field,
            format!("value must not exceed {max_chars} characters"),
        ));
    }
}

pub fn require_unique<T: Ord + Clone>(values: &[T], field: &str, issues: &mut Vec<ContractIssue>) {
    let mut seen = std::collections::BTreeSet::new();
    if values.iter().any(|value| !seen.insert(value.clone())) {
        issues.push(ContractIssue::new(
            "duplicate_value",
            field,
            "values must be unique",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_required_bounded_and_unique_values() {
        let mut issues = Vec::new();
        require_non_empty(" ", "name", 8, &mut issues);
        require_non_empty("123456789", "name", 8, &mut issues);
        require_unique(&[1, 2, 1], "items", &mut issues);
        assert_eq!(
            issues
                .iter()
                .map(|issue| issue.code.as_str())
                .collect::<Vec<_>>(),
            ["value_required", "value_too_long", "duplicate_value"]
        );
    }
}
