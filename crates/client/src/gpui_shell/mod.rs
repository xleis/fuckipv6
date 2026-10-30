mod session;
mod view;

use anyhow::{bail, Result};

use crate::ui::DisplayEnvironment;

pub use session::NativeNodeSession;

/// True only for a build that contains the native GPUI implementation on a
/// supported desktop target. Display probing is a separate startup gate.
pub const fn is_available() -> bool {
    cfg!(all(
        feature = "gpui-shell",
        any(
            all(target_os = "linux", not(target_env = "musl")),
            windows,
            target_os = "macos"
        )
    ))
}

fn preflight(is_available: bool, display: DisplayEnvironment) -> Result<()> {
    if !is_available {
        bail!("当前构建未包含可用的原生 GPUI 外壳");
    }
    if !display.has_desktop() {
        bail!("当前环境没有可用的桌面显示，拒绝启动 GPUI");
    }
    Ok(())
}

/// Start the real GPUI application on the current (main) thread.
pub fn run(options: crate::cli::commands::RuntimeOptions) -> Result<()> {
    preflight(is_available(), DisplayEnvironment::detect())?;

    let session = NativeNodeSession::start(options)?;
    let state = session.state();
    let commands = session.commands();
    let window_result = view::run_window(state, commands);
    session.shutdown();
    window_result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_rejects_missing_shell_and_display() {
        let unavailable = preflight(false, DisplayEnvironment::LinuxWayland)
            .expect_err("missing native shell must fail");
        assert!(unavailable.to_string().contains("原生 GPUI"));

        let headless =
            preflight(true, DisplayEnvironment::Headless).expect_err("headless launch must fail");
        assert!(headless.to_string().contains("桌面显示"));

        preflight(true, DisplayEnvironment::LinuxWayland)
            .expect("desktop-capable build should pass preflight");
    }
}
