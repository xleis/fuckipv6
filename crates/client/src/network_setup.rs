//! Stable, non-secret classification for host network setup failures.
//!
//! Lower layers retain the complete error chain for local logs, while command
//! responses select one of these stages and never expose raw command stderr,
//! paths, credentials, interface metadata, or process identities to a public
//! adapter.

use anyhow::Error;

/// User-actionable stage that failed while preparing the host data plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkSetupStage {
    TunCreate,
    AddressConflict,
    RouteSetup,
    DnsSetup,
    PermissionDenied,
    PlatformUnsupported,
    DeviceUnavailable,
    ToolUnavailable,
    Unknown,
}

impl NetworkSetupStage {
    pub(crate) const fn message_key(self) -> &'static str {
        match self {
            Self::TunCreate => "error.network.tun_create_failed",
            Self::AddressConflict => "error.network.address_conflict",
            Self::RouteSetup => "error.network.route_setup_failed",
            Self::DnsSetup => "error.network.dns_setup_failed",
            Self::PermissionDenied => "error.network.permission_denied",
            Self::PlatformUnsupported => "error.network.platform_unsupported",
            Self::DeviceUnavailable => "error.network.device_unavailable",
            Self::ToolUnavailable => "error.network.tool_unavailable",
            Self::Unknown => "error.network.setup_failed",
        }
    }

    /// Locale-independent fallback stored in the canonical error. UI adapters
    /// translate `message_key` and should use this string only as a fallback.
    pub(crate) const fn fallback_message(self) -> &'static str {
        match self {
            Self::TunCreate => "TUN device creation failed.",
            Self::AddressConflict => "The TUN address conflicts with an existing interface.",
            Self::RouteSetup => "TUN route setup failed.",
            Self::DnsSetup => "DNS proxy setup failed.",
            Self::PermissionDenied => {
                "Insufficient privileges to create or configure the TUN device."
            }
            Self::PlatformUnsupported => "TUN is unsupported on this platform.",
            Self::DeviceUnavailable => "The host TUN device is unavailable.",
            Self::ToolUnavailable => "A required host networking tool is unavailable.",
            Self::Unknown => "Host network setup failed.",
        }
    }
}

/// Typed marker carried in an `anyhow` error chain. The detail is kept for
/// local diagnostics but is never copied into a command response.
#[derive(Debug)]
pub(crate) struct NetworkSetupError {
    stage: NetworkSetupStage,
    detail: String,
}

impl NetworkSetupError {
    pub(crate) fn new(stage: NetworkSetupStage, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }

    pub(crate) const fn stage(&self) -> NetworkSetupStage {
        self.stage
    }
}

impl std::fmt::Display for NetworkSetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            formatter.write_str(self.stage.fallback_message())
        } else {
            write!(
                formatter,
                "{} {}",
                self.stage.fallback_message(),
                self.detail
            )
        }
    }
}

impl std::error::Error for NetworkSetupError {}

/// Classify common command failures while retaining the original detail only
/// in the local error chain.
pub(crate) fn command_failure(
    fallback: NetworkSetupStage,
    stderr: impl AsRef<str>,
) -> NetworkSetupError {
    let detail = stderr.as_ref().trim().to_string();
    let lower = detail.to_ascii_lowercase();
    let stage = if lower.contains("operation not permitted")
        || lower.contains("permission denied")
        || lower.contains("not permitted")
        || lower.contains("access denied")
    {
        NetworkSetupStage::PermissionDenied
    } else if fallback == NetworkSetupStage::AddressConflict
        && (lower.contains("file exists")
            || lower.contains("already exists")
            || lower.contains("already assigned")
            || lower.contains("already in use"))
    {
        NetworkSetupStage::AddressConflict
    } else if fallback == NetworkSetupStage::TunCreate
        && (lower.contains("/dev/net/tun")
            || lower.contains("no such file or directory")
            || lower.contains("cannot find device"))
    {
        NetworkSetupStage::DeviceUnavailable
    } else {
        fallback
    };
    NetworkSetupError::new(stage, detail)
}

/// Add a stage marker unless a lower layer already classified the failure.
pub(crate) fn ensure_stage(error: Error, fallback: NetworkSetupStage) -> Error {
    if stage_from_error(&error).is_some() {
        error
    } else {
        error.context(NetworkSetupError::new(fallback, String::new()))
    }
}

pub(crate) fn stage_from_error(error: &Error) -> Option<NetworkSetupStage> {
    error
        .downcast_ref::<NetworkSetupError>()
        .map(NetworkSetupError::stage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_permission_without_exposing_raw_output_in_the_stage() {
        let error = command_failure(
            NetworkSetupStage::RouteSetup,
            "RTNETLINK answers: Operation not permitted",
        );
        assert_eq!(error.stage(), NetworkSetupStage::PermissionDenied);
        assert!(error.to_string().contains("Operation not permitted"));
    }

    #[test]
    fn classifies_address_conflict_only_for_address_operations() {
        let address = command_failure(
            NetworkSetupStage::AddressConflict,
            "RTNETLINK answers: File exists",
        );
        assert_eq!(address.stage(), NetworkSetupStage::AddressConflict);

        let device = command_failure(NetworkSetupStage::TunCreate, "File exists");
        assert_eq!(device.stage(), NetworkSetupStage::TunCreate);
    }

    #[test]
    fn preserves_the_earliest_structured_stage() {
        let error = ensure_stage(
            anyhow::Error::new(NetworkSetupError::new(
                NetworkSetupStage::DnsSetup,
                "resolver write failed",
            )),
            NetworkSetupStage::RouteSetup,
        );
        assert_eq!(stage_from_error(&error), Some(NetworkSetupStage::DnsSetup));
    }

    #[test]
    fn adds_a_fallback_stage_without_dropping_the_source_chain() {
        let error = ensure_stage(anyhow::anyhow!("bind failed"), NetworkSetupStage::DnsSetup);
        assert_eq!(stage_from_error(&error), Some(NetworkSetupStage::DnsSetup));
        assert!(format!("{error:#}").contains("bind failed"));
    }
}
