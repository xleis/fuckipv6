#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Headless,
    Gui,
}

impl UiMode {
    pub fn is_headless(&self) -> bool {
        matches!(self, Self::Headless)
    }

    pub fn is_gui(&self) -> bool {
        matches!(self, Self::Gui)
    }
}

/// 自适应检测当前系统的运行时显示环境
///
/// 优先级:
/// 1. 用户显式命令行参数: `--headless` 强制无头, `--gui` 强制桌面
/// 2. 操作系统桌面环境自适应探测:
///    - Linux: 检测环境变量 `DISPLAY` 或 `WAYLAND_DISPLAY`
///    - Windows/macOS: 默认桌面会话, 处于 CI/无显示控制台时回退无头
pub fn detect_ui_mode(cli_headless: bool, cli_gui: bool) -> UiMode {
    if cli_headless {
        return UiMode::Headless;
    }
    if cli_gui {
        return UiMode::Gui;
    }

    // 环境变量覆盖
    if std::env::var("FUCKIPV6_HEADLESS").map(|v| v == "1" || v == "true").unwrap_or(false) {
        return UiMode::Headless;
    }
    if std::env::var("FUCKIPV6_GUI").map(|v| v == "1" || v == "true").unwrap_or(false) {
        return UiMode::Gui;
    }

    #[cfg(target_os = "linux")]
    {
        let has_x11 = std::env::var("DISPLAY").map(|v| !v.trim().is_empty()).unwrap_or(false);
        let has_wayland = std::env::var("WAYLAND_DISPLAY").map(|v| !v.trim().is_empty()).unwrap_or(false);
        if has_x11 || has_wayland {
            UiMode::Gui
        } else {
            UiMode::Headless
        }
    }

    #[cfg(windows)]
    {
        UiMode::Gui
    }

    #[cfg(target_os = "macos")]
    {
        UiMode::Gui
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        UiMode::Headless
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_cli_flags() {
        assert_eq!(detect_ui_mode(true, false), UiMode::Headless);
        assert_eq!(detect_ui_mode(false, true), UiMode::Gui);
        // Headless flag takes precedence if both somehow set
        assert_eq!(detect_ui_mode(true, true), UiMode::Headless);
    }
}
