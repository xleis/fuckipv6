use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
#[cfg(target_os = "linux")]
use std::path::Path;

/// The actual control surface selected for this node process.
///
/// `WebUi` is the browser-hosted control plane for headless hosts. A process
/// with a desktop session must use `Gpui`; it must not expose the WebUI as a
/// second desktop surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
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

    /// Whether this mode owns a native desktop window.
    pub fn is_desktop_surface(self) -> bool {
        matches!(self, Self::Gpui)
    }

    /// Compatibility name retained for existing adapters. Callers should use
    /// `is_desktop_surface`, `is_gpui` or `is_web_ui` when the distinction
    /// matters.
    pub fn is_gui(self) -> bool {
        self.is_desktop_surface()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiModeRequest {
    Auto,
    Gpui,
    WebUi,
    Headless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
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
            let session_type = get_var("XDG_SESSION_TYPE")
                .map(|value| value.trim().to_ascii_lowercase())
                .unwrap_or_default();
            if session_type == "wayland" {
                return Self::LinuxWayland;
            }
            if session_type == "x11" {
                return Self::LinuxX11;
            }
            if ["XDG_CURRENT_DESKTOP", "DESKTOP_SESSION"]
                .iter()
                .any(|key| {
                    get_var(key)
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                })
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
        let environment = Self::detect_with(|key| std::env::var(key).ok());
        if environment.has_desktop() {
            return environment;
        }

        // A system service starts outside the graphical user's environment, so
        // DISPLAY/WAYLAND_DISPLAY are usually absent even when the machine has
        // an active desktop session. Fall back to the display-server sockets so
        // a desktop host still fails closed instead of exposing WebUI.
        #[cfg(target_os = "linux")]
        if let Some(desktop) =
            detect_linux_desktop_sockets(Path::new("/run/user"), Path::new("/tmp/.X11-unix"))
        {
            return desktop;
        }

        environment
    }
}

#[cfg(target_os = "linux")]
fn detect_linux_desktop_sockets(
    runtime_root: &Path,
    x11_socket_root: &Path,
) -> Option<DisplayEnvironment> {
    use std::os::unix::fs::FileTypeExt;

    if let Ok(entries) = std::fs::read_dir(runtime_root) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Ok(session_entries) = std::fs::read_dir(entry.path()) else {
                continue;
            };
            if session_entries.flatten().any(|candidate| {
                candidate
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("wayland-") && !name.ends_with(".lock"))
                    && candidate
                        .file_type()
                        .map(|kind| kind.is_socket())
                        .unwrap_or(false)
            }) {
                return Some(DisplayEnvironment::LinuxWayland);
            }
        }
    }

    std::fs::read_dir(x11_socket_root)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|candidate| {
            candidate
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with('X') && name.len() > 1)
                && candidate
                    .file_type()
                    .map(|kind| kind.is_socket())
                    .unwrap_or(false)
        })
        .then_some(DisplayEnvironment::LinuxX11)
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
            gpui_available: gpui_shell_available(),
        }
    }
}

/// A capability is true only when the native GPUI module was compiled for a
/// supported desktop target. Display availability is checked separately by
/// `resolve_ui_mode` before any platform window is created.
pub const fn gpui_shell_available() -> bool {
    #[cfg(all(
        feature = "gpui-shell",
        any(
            all(target_os = "linux", not(target_env = "musl")),
            windows,
            target_os = "macos"
        )
    ))]
    {
        true
    }
    #[cfg(not(all(
        feature = "gpui-shell",
        any(
            all(target_os = "linux", not(target_env = "musl")),
            windows,
            target_os = "macos"
        )
    )))]
    {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
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
    WebUiRequiresHeadless,
    HeadlessRequiresNoDesktop,
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
            Self::WebUiRequiresHeadless => formatter.write_str(
                "WebUI is only available without a desktop session; use GPUI on desktop hosts",
            ),
            Self::HeadlessRequiresNoDesktop => formatter.write_str(
                "headless node control is only available without a desktop session; use GPUI on desktop hosts",
            ),
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
        // The legacy `--gui` name means a desktop GUI, never the browser-hosted
        // WebUI that is reserved for headless hosts.
        (false, true) => Ok(UiModeRequest::Gpui),
        (false, false) => Ok(UiModeRequest::Auto),
    }
}

pub fn resolve_ui_mode(
    requested: UiModeRequest,
    capabilities: UiCapabilities,
) -> Result<UiModeResolution, UiModeError> {
    let effective = match requested {
        // Browser-hosted and explicitly headless control are allowed only on a
        // host without a desktop session. A desktop host always uses GPUI.
        UiModeRequest::WebUi if capabilities.display.has_desktop() => {
            return Err(UiModeError::WebUiRequiresHeadless);
        }
        UiModeRequest::Headless if capabilities.display.has_desktop() => {
            return Err(UiModeError::HeadlessRequiresNoDesktop);
        }
        UiModeRequest::Auto | UiModeRequest::Gpui if capabilities.display.has_desktop() => {
            if !capabilities.gpui_available {
                return Err(UiModeError::GpuiUnavailable);
            }
            UiMode::Gpui
        }
        UiModeRequest::Auto => UiMode::Headless,
        UiModeRequest::WebUi => UiMode::WebUi,
        UiModeRequest::Headless => UiMode::Headless,
        UiModeRequest::Gpui => {
            if !capabilities.display.has_desktop() {
                return Err(UiModeError::DisplayUnavailable);
            }
            if !capabilities.gpui_available {
                return Err(UiModeError::GpuiUnavailable);
            }
            UiMode::Gpui
        }
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
    let requested = apply_environment_request(request_from_legacy_flags(cli_headless, cli_gui)?)?;
    resolve_ui_mode(requested, UiCapabilities::detect())
}

/// Apply legacy process-level UI overrides without changing the detected
/// display environment. An override may request a mode, but it can never make
/// a desktop host look headless or turn that host into a WebUI process.
pub fn apply_environment_request(requested: UiModeRequest) -> Result<UiModeRequest, UiModeError> {
    if requested != UiModeRequest::Auto {
        if environment_enabled("FUCKIPV6_HEADLESS") || environment_enabled("FUCKIPV6_GUI") {
            return Err(UiModeError::ConflictingFlags);
        }
        return Ok(requested);
    }

    let headless = environment_enabled("FUCKIPV6_HEADLESS");
    let gpui = environment_enabled("FUCKIPV6_GUI");
    match (headless, gpui) {
        (true, true) => Err(UiModeError::ConflictingFlags),
        (true, false) => Ok(UiModeRequest::Headless),
        (false, true) => Ok(UiModeRequest::Gpui),
        (false, false) => Ok(UiModeRequest::Auto),
    }
}

/// Resolve a parsed request after applying process-level compatibility
/// overrides, then enforce the desktop/headless surface policy.
pub fn resolve_ui_request(
    requested: UiModeRequest,
    capabilities: UiCapabilities,
) -> Result<UiModeResolution, UiModeError> {
    resolve_ui_mode(apply_environment_request(requested)?, capabilities)
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
    fn legacy_gui_flag_resolves_to_native_gpui_not_webui() {
        let resolution = resolve_ui_mode(
            request_from_legacy_flags(false, true).expect("legacy GUI request"),
            UiCapabilities {
                display: DisplayEnvironment::LinuxWayland,
                gpui_available: true,
            },
        )
        .expect("desktop GPUI should resolve");

        assert_eq!(resolution.effective, UiMode::Gpui);
        assert!(resolution.effective.is_gpui());
        assert!(!resolution.effective.is_web_ui());
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
    fn gpui_capability_matches_the_compiled_supported_feature() {
        let expected = cfg!(all(
            feature = "gpui-shell",
            any(
                all(target_os = "linux", not(target_env = "musl")),
                windows,
                target_os = "macos"
            )
        ));
        assert_eq!(gpui_shell_available(), expected);
    }

    #[test]
    fn desktop_auto_mode_never_falls_back_to_webui() {
        for display in [
            DisplayEnvironment::LinuxX11,
            DisplayEnvironment::LinuxWayland,
            DisplayEnvironment::WindowsDesktop,
            DisplayEnvironment::MacosDesktop,
        ] {
            assert_eq!(
                resolve_ui_mode(
                    UiModeRequest::Auto,
                    UiCapabilities {
                        display,
                        gpui_available: true,
                    },
                )
                .expect("desktop GPUI resolution")
                .effective,
                UiMode::Gpui
            );
            assert_eq!(
                resolve_ui_mode(
                    UiModeRequest::Auto,
                    UiCapabilities {
                        display,
                        gpui_available: false,
                    },
                ),
                Err(UiModeError::GpuiUnavailable)
            );
        }
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Auto,
                UiCapabilities {
                    display: DisplayEnvironment::Headless,
                    gpui_available: true,
                },
            )
            .expect("headless auto resolves without a desktop surface")
            .effective,
            UiMode::Headless
        );
    }

    #[test]
    fn explicit_webui_is_rejected_on_a_desktop_host() {
        assert_eq!(
            resolve_ui_mode(UiModeRequest::WebUi, DESKTOP),
            Err(UiModeError::WebUiRequiresHeadless)
        );
    }

    #[test]
    fn explicit_headless_is_rejected_on_a_desktop_host() {
        assert_eq!(
            resolve_ui_mode(UiModeRequest::Headless, DESKTOP),
            Err(UiModeError::HeadlessRequiresNoDesktop)
        );
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Headless,
                UiCapabilities {
                    display: DisplayEnvironment::Headless,
                    gpui_available: false,
                },
            )
            .expect("headless host should resolve headless control")
            .effective,
            UiMode::Headless
        );
    }

    #[test]
    fn webui_is_available_without_a_display_but_gpui_is_not() {
        let capabilities = UiCapabilities {
            display: DisplayEnvironment::Headless,
            gpui_available: false,
        };
        assert_eq!(
            resolve_ui_mode(UiModeRequest::WebUi, capabilities)
                .expect("headless WebUI server should resolve")
                .effective,
            UiMode::WebUi
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
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Auto,
                UiCapabilities {
                    display: wayland,
                    gpui_available: true,
                },
            )
            .expect("Wayland automatic mode")
            .effective,
            UiMode::Gpui
        );

        let x11 = DisplayEnvironment::detect_with(|key| match key {
            "DISPLAY" => Some(":0".into()),
            _ => None,
        });
        assert_eq!(x11, DisplayEnvironment::LinuxX11);
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Auto,
                UiCapabilities {
                    display: x11,
                    gpui_available: true,
                },
            )
            .expect("X11 automatic mode")
            .effective,
            UiMode::Gpui
        );

        let headless = DisplayEnvironment::detect_with(|_| None);
        assert_eq!(headless, DisplayEnvironment::Headless);
        assert_eq!(
            resolve_ui_mode(
                UiModeRequest::Auto,
                UiCapabilities {
                    display: headless,
                    gpui_available: true,
                },
            )
            .expect("headless automatic mode")
            .effective,
            UiMode::Headless
        );

        let public_ip_does_not_select_a_display = DisplayEnvironment::detect_with(|key| {
            (key == "FUCKIPV6_PUBLIC_IP").then(|| "203.0.113.10".to_string())
        });
        assert_eq!(
            public_ip_does_not_select_a_display,
            DisplayEnvironment::Headless
        );

        let headless_environment_variable_cannot_mask_desktop =
            DisplayEnvironment::detect_with(|key| match key {
                "DISPLAY" => Some(":0".into()),
                "FUCKIPV6_HEADLESS" => Some("true".into()),
                _ => None,
            });
        assert_eq!(
            headless_environment_variable_cannot_mask_desktop,
            DisplayEnvironment::LinuxX11
        );

        let desktop_session_without_display_variables =
            DisplayEnvironment::detect_with(|key| match key {
                "XDG_CURRENT_DESKTOP" => Some("niri".into()),
                "XDG_SESSION_TYPE" => Some("wayland".into()),
                _ => None,
            });
        assert_eq!(
            desktop_session_without_display_variables,
            DisplayEnvironment::LinuxWayland
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_session_socket_detection_survives_a_headless_service_environment() {
        use std::os::unix::net::UnixListener;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        fn temporary_directory(label: &str) -> std::path::PathBuf {
            let path = std::env::temp_dir().join(format!(
                "fuckipv6-ui-mode-{label}-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("temporary UI mode directory");
            path
        }

        let runtime_root = temporary_directory("wayland-run");
        let wayland_session = runtime_root.join("1000");
        std::fs::create_dir_all(&wayland_session).expect("Wayland session directory");
        let _wayland_socket =
            UnixListener::bind(wayland_session.join("wayland-1")).expect("Wayland session socket");
        let empty_x11 = temporary_directory("empty-x11");
        assert_eq!(
            detect_linux_desktop_sockets(&runtime_root, &empty_x11),
            Some(DisplayEnvironment::LinuxWayland)
        );
        drop(_wayland_socket);
        let _ = std::fs::remove_dir_all(runtime_root);

        let empty_runtime = temporary_directory("empty-run");
        let x11_root = temporary_directory("x11");
        let _x11_socket = UnixListener::bind(x11_root.join("X0")).expect("X11 session socket");
        assert_eq!(
            detect_linux_desktop_sockets(&empty_runtime, &x11_root),
            Some(DisplayEnvironment::LinuxX11)
        );
        drop(_x11_socket);
        let _ = std::fs::remove_dir_all(empty_runtime);
        let _ = std::fs::remove_dir_all(empty_x11);
        let _ = std::fs::remove_dir_all(x11_root);
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
        let resolution = resolve_ui_mode(
            UiModeRequest::WebUi,
            UiCapabilities {
                display: DisplayEnvironment::Headless,
                gpui_available: false,
            },
        )
        .expect("headless web UI should resolve");
        let encoded = serde_json::to_value(resolution).expect("serialize UI mode status");
        assert_eq!(encoded["requested"], "web_ui");
        assert_eq!(encoded["effective"], "web_ui");
        assert_eq!(encoded["display"], "headless");
        assert_eq!(encoded["gpui_available"], false);
    }
}
