use serde::{Deserialize, Serialize};
use std::fmt;

/// The actual rendering surface selected for this node process.
///
/// `WebUi` describes the browser-hosted control surface, even when the browser
/// is launched in app-window mode. It must never be represented as `Gpui`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiMode {
    Gpui,
    WebUi,
    Headless,
}

impl UiMode {
    pub fn is_headless(self) -> bool {
        matches!(self, Self::Headless)
    }

    pub fn is_gpui(self) -> bool {
        matches!(self, Self::Gpui)
    }

    pub fn is_web_ui(self) -> bool {
        matches!(self, Self::WebUi)
    }

    /// Whether this mode owns a desktop window. This compatibility helper does
    /// not claim that a WebUI window is a native GPUI surface.
    pub fn is_desktop_surface(self) -> bool {
        matches!(self, Self::Gpui | Self::WebUi)
    }

    /// Compatibility name retained for existing adapters. Callers should use
    /// `is_desktop_surface`, `is_gpui` or `is_web_ui` when the distinction
    /// matters.
    pub fn is_gui(self) -> bool {
        self.is_desktop_surface()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiModeRequest {
    Auto,
    Gpui,
    WebUi,
    Headless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayEnvironment {
    LinuxX11,
    LinuxWayland,
    WindowsDesktop,
    MacosDesktop,
    Headless,
}

impl DisplayEnvironment {
    pub fn has_desktop(self) -> bool {
        !matches!(self, Self::Headless)
    }

    fn detect_with(mut get_var: impl FnMut(&str) -> Option<String>) -> Self {
        let enabled = |value: Option<String>| {
            value
                .map(|value| {
                    let value = value.trim();
                    !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
                })
                .unwrap_or(false)
        };

        if enabled(get_var("FUCKIPV6_HEADLESS")) {
            return Self::Headless;
        }

        #[cfg(target_os = "linux")]
        {
            if get_var("WAYLAND_DISPLAY")
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            {
                return Self::LinuxWayland;
            }
            if get_var("DISPLAY")
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            {
                return Self::LinuxX11;
            }
            Self::Headless
        }

        #[cfg(windows)]
        {
            if get_var("SESSIONNAME")
                .map(|value| {
                    let value = value.trim();
                    !value.is_empty() && !value.eq_ignore_ascii_case("services")
                })
                .unwrap_or(false)
            {
                Self::WindowsDesktop
            } else {
                Self::Headless
            }
        }

        #[cfg(target_os = "macos")]
        {
            Self::MacosDesktop
        }

        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        {
            Self::Headless
        }
    }

    pub fn detect() -> Self {
        Self::detect_with(|key| std::env::var(key).ok())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiCapabilities {
    pub display: DisplayEnvironment,
    /// Set only after the real GPUI application shell is available.
    pub gpui_available: bool,
}

impl UiCapabilities {
    pub fn detect() -> Self {
        Self {
            display: DisplayEnvironment::detect(),
            gpui_available: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiModeResolution {
    pub requested: UiModeRequest,
    pub effective: UiMode,
    pub display: DisplayEnvironment,
    pub gpui_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiModeError {
    ConflictingFlags,
    GpuiUnavailable,
    DisplayUnavailable,
}

impl fmt::Display for UiModeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingFlags => {
                formatter.write_str("headless, WebUI and GPUI mode flags are mutually exclusive")
            }
            Self::GpuiUnavailable => {
                formatter.write_str("native GPUI mode is not available in this build")
            }
            Self::DisplayUnavailable => {
                formatter.write_str("the requested desktop UI has no display environment")
            }
        }
    }
}

impl std::error::Error for UiModeError {}

pub fn request_from_legacy_flags(
    cli_headless: bool,
    cli_gui: bool,
) -> Result<UiModeRequest, UiModeError> {
    match (cli_headless, cli_gui) {
        (true, true) => Err(UiModeError::ConflictingFlags),
        (true, false) => Ok(UiModeRequest::Headless),
        // The legacy `--gui` flag launches the browser-hosted surface.
        (false, true) => Ok(UiModeRequest::WebUi),
        (false, false) => Ok(UiModeRequest::Auto),
    }
}

pub fn resolve_ui_mode(
    requested: UiModeRequest,
    capabilities: UiCapabilities,
) -> Result<UiModeResolution, UiModeError> {
    let effective = match requested {
        UiModeRequest::Headless => UiMode::Headless,
        UiModeRequest::WebUi if !capabilities.display.has_desktop() => {
            return Err(UiModeError::DisplayUnavailable);
        }
        UiModeRequest::WebUi => UiMode::WebUi,
        UiModeRequest::Gpui if !capabilities.gpui_available => {
            return Err(UiModeError::GpuiUnavailable);
        }
        UiModeRequest::Gpui if !capabilities.display.has_desktop() => {
            return Err(UiModeError::DisplayUnavailable);
        }
        UiModeRequest::Gpui => UiMode::Gpui,
        UiModeRequest::Auto if !capabilities.display.has_desktop() => UiMode::Headless,
        UiModeRequest::Auto if capabilities.gpui_available => UiMode::Gpui,
        UiModeRequest::Auto => UiMode::WebUi,
    };

    Ok(UiModeResolution {
        requested,
        effective,
        display: capabilities.display,
        gpui_available: capabilities.gpui_available,
    })
}

/// Resolve the existing `--headless` / `--gui` flags without allowing the
/// browser-hosted UI to masquerade as native GPUI.
pub fn resolve_legacy_ui_mode(
    cli_headless: bool,
    cli_gui: bool,
) -> Result<UiModeResolution, UiModeError> {
    let mut requested = request_from_legacy_flags(cli_headless, cli_gui)?;
    if requested == UiModeRequest::Auto {
        if environment_enabled("FUCKIPV6_HEADLESS") {
            requested = UiModeRequest::Headless;
        } else if environment_enabled("FUCKIPV6_GUI") {
            requested = UiModeRequest::WebUi;
        }
    }
    resolve_ui_mode(requested, UiCapabilities::detect())
}

fn environment_enabled(key: &str) -> bool {
    std::env::var(key)
        .map(|value| {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

/// Compatibility adapter for existing call sites. New code should call
/// `resolve_legacy_ui_mode` so conflicting or unavailable modes are surfaced as
/// errors instead of silently selecting a different surface.
pub fn detect_ui_mode(cli_headless: bool, cli_gui: bool) -> UiMode {
    resolve_legacy_ui_mode(cli_headless, cli_gui)
        .map(|resolution| resolution.effective)
        .unwrap_or(UiMode::Headless)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: UiCapabilities = UiCapabilities {
        display: DisplayEnvironment::LinuxWayland,
        gpui_available: false,
    };

    #[test]
    fn legacy_gui_flag_resolves_to_webui_not_gpui() {
        let resolution = resolve_ui_mode(
            request_from_legacy_flags(false, true).expect("legacy GUI request"),
            DESKTOP,
        )
        .expect("desktop web UI should resolve");

        assert_eq!(resolution.effective, UiMode::WebUi);
        assert!(resolution.effective.is_web_ui());
        assert!(!resolution.effective.is_gpui());
    }

    #[test]
    fn conflicting_explicit_flags_are_rejected() {
        assert_eq!(
            request_from_legacy_flags(true, true),
            Err(UiModeError::ConflictingFlags)
        );
    }

    #[test]
    fn gpui_requires_an_explicit_available_capability() {
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Gpui,
                UiCapabilities {
                    display: DisplayEnvironment::LinuxX11,
                    gpui_available: false,
                },
            ),
            Err(UiModeError::GpuiUnavailable)
        );

        let resolution = resolve_ui_mode(
            UiModeRequest::Gpui,
            UiCapabilities {
                display: DisplayEnvironment::LinuxX11,
                gpui_available: true,
            },
        )
        .expect("available GPUI should resolve");
        assert_eq!(resolution.effective, UiMode::Gpui);
    }

    #[test]
    fn auto_mode_selects_only_an_available_surface() {
        assert_eq!(
            resolve_ui_mode(UiModeRequest::Auto, DESKTOP)
                .expect("desktop resolution")
                .effective,
            UiMode::WebUi
        );
        assert_eq!(
            resolve_ui_mode(UiModeRequest::Auto, UiCapabilities::detect())
                .map(|resolution| resolution.effective)
                .unwrap_or(UiMode::Headless),
            resolve_legacy_ui_mode(false, false)
                .map(|resolution| resolution.effective)
                .unwrap_or(UiMode::Headless)
        );
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Auto,
                UiCapabilities {
                    display: DisplayEnvironment::Headless,
                    gpui_available: true,
                },
            )
            .expect("headless auto always resolves")
            .effective,
            UiMode::Headless
        );
    }

    #[test]
    fn desktop_requests_fail_without_a_display() {
        let capabilities = UiCapabilities {
            display: DisplayEnvironment::Headless,
            gpui_available: false,
        };
        assert_eq!(
            resolve_ui_mode(UiModeRequest::WebUi, capabilities),
            Err(UiModeError::DisplayUnavailable)
        );
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Gpui,
                UiCapabilities {
                    display: DisplayEnvironment::Headless,
                    gpui_available: true,
                },
            ),
            Err(UiModeError::DisplayUnavailable)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_display_detection_distinguishes_wayland_x11_and_headless() {
        let wayland = DisplayEnvironment::detect_with(|key| match key {
            "WAYLAND_DISPLAY" => Some("wayland-0".into()),
            "DISPLAY" => Some(":0".into()),
            _ => None,
        });
        assert_eq!(wayland, DisplayEnvironment::LinuxWayland);

        let x11 = DisplayEnvironment::detect_with(|key| match key {
            "DISPLAY" => Some(":0".into()),
            _ => None,
        });
        assert_eq!(x11, DisplayEnvironment::LinuxX11);

        let headless = DisplayEnvironment::detect_with(|_| None);
        assert_eq!(headless, DisplayEnvironment::Headless);

        let forced_headless = DisplayEnvironment::detect_with(|key| match key {
            "DISPLAY" => Some(":0".into()),
            "FUCKIPV6_HEADLESS" => Some("true".into()),
            _ => None,
        });
        assert_eq!(forced_headless, DisplayEnvironment::Headless);
    }

    #[cfg(windows)]
    #[test]
    fn windows_service_session_detection_is_headless() {
        let service = DisplayEnvironment::detect_with(|key| match key {
            "SESSIONNAME" => Some("Services".into()),
            _ => None,
        });
        assert_eq!(service, DisplayEnvironment::Headless);

        let desktop = DisplayEnvironment::detect_with(|key| match key {
            "SESSIONNAME" => Some("Console".into()),
            _ => None,
        });
        assert_eq!(desktop, DisplayEnvironment::WindowsDesktop);
    }

    #[test]
    fn mode_status_uses_stable_wire_values() {
        let resolution =
            resolve_ui_mode(UiModeRequest::WebUi, DESKTOP).expect("desktop web UI should resolve");
        let encoded = serde_json::to_value(resolution).expect("serialize UI mode status");
        assert_eq!(encoded["requested"], "web_ui");
        assert_eq!(encoded["effective"], "web_ui");
        assert_eq!(encoded["display"], "linux_wayland");
        assert_eq!(encoded["gpui_available"], false);
    }
}
