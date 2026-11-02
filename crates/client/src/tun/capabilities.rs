use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TUN_CAPABILITY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunPlatform {
    Linux,
    Macos,
    Windows,
    Other,
}

impl TunPlatform {
    pub const fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "macos")]
        {
            Self::Macos
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Self::Other
        }
    }

    const fn is_supported(self) -> bool {
        matches!(self, Self::Linux | Self::Macos | Self::Windows)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunCapabilityReason {
    PlatformUnsupported,
    PermissionDenied,
    DeviceUnavailable,
    ToolUnavailable,
    Unknown,
}

impl TunCapabilityReason {
    pub const fn message_key(self) -> &'static str {
        match self {
            Self::PlatformUnsupported => "error.network.platform_unsupported",
            Self::PermissionDenied => "error.network.permission_denied",
            Self::DeviceUnavailable => "error.network.device_unavailable",
            Self::ToolUnavailable => "error.network.tool_unavailable",
            Self::Unknown => "error.network.setup_failed",
        }
    }

    pub(crate) const fn network_setup_stage(self) -> crate::network_setup::NetworkSetupStage {
        use crate::network_setup::NetworkSetupStage;

        match self {
            Self::PlatformUnsupported => NetworkSetupStage::PlatformUnsupported,
            Self::PermissionDenied => NetworkSetupStage::PermissionDenied,
            Self::DeviceUnavailable => NetworkSetupStage::DeviceUnavailable,
            Self::ToolUnavailable => NetworkSetupStage::ToolUnavailable,
            Self::Unknown => NetworkSetupStage::Unknown,
        }
    }
}

/// Redacted TUN readiness for the current node process. This is deliberately
/// derived from capability metadata rather than a real TUN creation probe.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
pub struct TunCapabilities {
    pub schema_version: u16,
    pub platform: TunPlatform,
    pub supported: bool,
    pub can_rebuild: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<TunCapabilityReason>,
}

impl TunCapabilities {
    pub fn detect() -> Self {
        Self::for_probe(HostProbe::detect())
    }

    pub fn blocking_reason(&self) -> Option<TunCapabilityReason> {
        if !self.supported {
            return Some(TunCapabilityReason::PlatformUnsupported);
        }
        if self.can_rebuild {
            None
        } else {
            Some(
                self.reasons
                    .first()
                    .copied()
                    .unwrap_or(TunCapabilityReason::Unknown),
            )
        }
    }

    fn for_probe(probe: HostProbe) -> Self {
        if !probe.platform.is_supported() {
            return Self {
                schema_version: TUN_CAPABILITY_SCHEMA_VERSION,
                platform: probe.platform,
                supported: false,
                can_rebuild: false,
                reasons: vec![TunCapabilityReason::PlatformUnsupported],
            };
        }

        let mut reasons = Vec::new();
        if !probe.privileged {
            reasons.push(TunCapabilityReason::PermissionDenied);
        }
        if !probe.device_available {
            reasons.push(TunCapabilityReason::DeviceUnavailable);
        }
        if !probe.routing_tool_available {
            reasons.push(TunCapabilityReason::ToolUnavailable);
        }
        if reasons.is_empty() && !probe.preflight_known {
            reasons.push(TunCapabilityReason::Unknown);
        }

        Self {
            schema_version: TUN_CAPABILITY_SCHEMA_VERSION,
            platform: probe.platform,
            supported: true,
            can_rebuild: reasons.is_empty(),
            reasons,
        }
    }
}

impl Default for TunCapabilities {
    fn default() -> Self {
        // Missing or older snapshot data means capability is unknown. Never let
        // deserialization defaults or test fixtures accidentally enable a host
        // network mutation. The production node store performs an explicit
        // detection when it constructs the canonical snapshot.
        Self {
            schema_version: TUN_CAPABILITY_SCHEMA_VERSION,
            platform: TunPlatform::current(),
            supported: TunPlatform::current().is_supported(),
            can_rebuild: false,
            reasons: vec![TunCapabilityReason::Unknown],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HostProbe {
    platform: TunPlatform,
    privileged: bool,
    device_available: bool,
    routing_tool_available: bool,
    preflight_known: bool,
}

impl HostProbe {
    fn detect() -> Self {
        let platform = TunPlatform::current();
        match platform {
            TunPlatform::Linux => {
                let privileged = process_has_network_admin_capability();
                let device_available = std::path::Path::new("/dev/net/tun").exists();
                let routing_tool_available = executable_available("ip");
                Self {
                    platform,
                    privileged,
                    device_available,
                    routing_tool_available,
                    preflight_known: true,
                }
            }
            TunPlatform::Macos => Self {
                platform,
                privileged: effective_uid() == Some(0),
                device_available: std::path::Path::new("/dev").is_dir(),
                routing_tool_available: executable_available("ifconfig")
                    && executable_available("route"),
                preflight_known: true,
            },
            TunPlatform::Windows => Self {
                platform,
                privileged: windows::is_elevated(),
                device_available: true,
                routing_tool_available: true,
                preflight_known: true,
            },
            TunPlatform::Other => Self {
                platform,
                privileged: false,
                device_available: false,
                routing_tool_available: false,
                preflight_known: false,
            },
        }
    }
}

#[cfg(target_os = "linux")]
fn process_has_network_admin_capability() -> bool {
    if effective_uid() == Some(0) {
        return true;
    }
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    let Some(hex) = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:").map(str::trim))
    else {
        return false;
    };
    u64::from_str_radix(hex, 16)
        .map(|capabilities| capabilities & (1_u64 << 12) != 0)
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn effective_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let mut fields = line.strip_prefix("Uid:")?.split_whitespace();
        fields.nth(1)?.parse().ok()
    })
}

#[cfg(target_os = "macos")]
fn effective_uid() -> Option<u32> {
    let uid = unsafe { libc::geteuid() };
    Some(uid)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn effective_uid() -> Option<u32> {
    None
}

#[cfg(target_os = "windows")]
mod windows {
    pub(super) fn is_elevated() -> bool {
        crate::tun::windows::is_elevated()
    }
}

#[cfg(not(target_os = "windows"))]
mod windows {
    pub(super) fn is_elevated() -> bool {
        false
    }
}

fn executable_available(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| {
        let candidate = directory.join(name);
        candidate.is_file()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(platform: TunPlatform) -> HostProbe {
        HostProbe {
            platform,
            privileged: true,
            device_available: true,
            routing_tool_available: true,
            preflight_known: true,
        }
    }

    #[test]
    fn ready_linux_probe_allows_rebuild() {
        let capabilities = TunCapabilities::for_probe(probe(TunPlatform::Linux));
        assert!(capabilities.supported);
        assert!(capabilities.can_rebuild);
        assert_eq!(capabilities.blocking_reason(), None);
    }

    #[test]
    fn permission_failure_fails_closed_with_a_stable_reason() {
        let mut probe = probe(TunPlatform::Linux);
        probe.privileged = false;
        let capabilities = TunCapabilities::for_probe(probe);
        assert!(!capabilities.can_rebuild);
        assert_eq!(
            capabilities.blocking_reason(),
            Some(TunCapabilityReason::PermissionDenied)
        );
    }

    #[test]
    fn default_capabilities_fail_closed() {
        let capabilities = TunCapabilities::default();
        assert!(!capabilities.can_rebuild);
        assert_eq!(
            capabilities.blocking_reason(),
            Some(TunCapabilityReason::Unknown)
        );
    }

    #[test]
    fn device_and_tool_failures_are_reported_together() {
        let mut probe = probe(TunPlatform::Linux);
        probe.device_available = false;
        probe.routing_tool_available = false;
        let capabilities = TunCapabilities::for_probe(probe);
        assert_eq!(
            capabilities.reasons,
            vec![
                TunCapabilityReason::DeviceUnavailable,
                TunCapabilityReason::ToolUnavailable
            ]
        );
    }

    #[test]
    fn unknown_platform_is_explicitly_unsupported() {
        let capabilities = TunCapabilities::for_probe(probe(TunPlatform::Other));
        assert!(!capabilities.supported);
        assert!(!capabilities.can_rebuild);
        assert_eq!(
            capabilities.blocking_reason(),
            Some(TunCapabilityReason::PlatformUnsupported)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn capability_metadata_round_trips_without_host_details() {
        let capabilities = TunCapabilities::for_probe(probe(TunPlatform::Linux));
        let encoded = serde_json::to_string(&capabilities).expect("serialize capabilities");
        assert!(!encoded.contains("/dev/net/tun"));
        assert!(!encoded.contains("PATH"));
        assert!(!encoded.contains("uid"));
        let decoded: TunCapabilities =
            serde_json::from_str(&encoded).expect("deserialize capabilities");
        assert_eq!(decoded, capabilities);
    }
}
