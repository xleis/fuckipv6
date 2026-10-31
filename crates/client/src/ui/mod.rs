pub mod mode;

pub use mode::{
    detect_ui_mode, resolve_legacy_ui_mode, resolve_ui_mode, DisplayEnvironment, UiCapabilities,
    UiMode, UiModeError, UiModeRequest, UiModeResolution,
};

/// WebUI 桌面窗口入口: 在桌面环境中启动浏览器托管的控制面窗口。
/// 这不是原生 GPUI，运行时模式必须保持为 `UiMode::WebUi`。
pub async fn run_webui_window(webui_port: u16) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{webui_port}");
    let display = DisplayEnvironment::detect();

    println!("========================================================================");
    if should_launch_webui_window(display) {
        println!("🚀 fuckipv6 WebUI 控制窗口已启动");
    } else {
        println!("🚀 fuckipv6 WebUI 控制服务已启动 (无桌面环境)");
    }
    println!("------------------------------------------------------------------------");
    println!("图形控制台地址: {url}");
    if should_launch_webui_window(display) {
        println!("如独立窗口未自动弹出，您可直接在浏览器打开上述地址进行交互。");
    } else {
        println!("当前未检测到图形会话，不会尝试启动浏览器；可从其他设备访问上述地址。");
    }
    println!("========================================================================");

    if should_launch_webui_window(display) {
        tracing::info!("🎨 桌面显示环境已检测，正在调起 WebUI 控制窗口: {url}");
        launch_webui_window(&url);
    } else {
        tracing::info!("🖥️ 无桌面显示环境，WebUI 仅提供服务: {url}");
    }
    Ok(())
}

fn should_launch_webui_window(display: DisplayEnvironment) -> bool {
    display.has_desktop()
}

/// Compatibility wrapper for adapters that still use the historical name.
pub async fn run_desktop_gui(webui_port: u16) -> anyhow::Result<()> {
    run_webui_window(webui_port).await
}

/// 启动浏览器托管的独立 WebUI 窗口
pub fn launch_webui_window(url: &str) {
    #[cfg(target_os = "linux")]
    {
        // 尝试启动主流 Chromium 内核浏览器的 --app 独立轻量模式 (接近原生 Tauri 窗口)
        let candidates = [
            "chromium",
            "google-chrome",
            "google-chrome-stable",
            "brave-browser",
            "microsoft-edge-stable",
        ];
        for bin in candidates {
            if let Ok(child) = std::process::Command::new(bin)
                .arg(format!("--app={url}"))
                .arg("--window-size=1180,820")
                .arg("--no-first-run")
                .spawn()
            {
                tracing::info!("已通过 {bin} 创建独立桌面图形窗口 (PID: {:?})", child.id());
                return;
            }
        }

        // 降级使用 xdg-open
        if let Ok(child) = std::process::Command::new("xdg-open").arg(url).spawn() {
            tracing::info!("已通过 xdg-open 调起系统浏览器 (PID: {:?})", child.id());
        }
    }

    #[cfg(windows)]
    {
        for bin in &["msedge.exe", "msedge", "chrome.exe", "chrome"] {
            if let Ok(child) = std::process::Command::new(bin)
                .arg(format!("--app={url}"))
                .arg("--window-size=1180,820")
                .spawn()
            {
                tracing::info!(
                    "已通过 {bin} 创建 Windows 独立图形窗口 (PID: {:?})",
                    child.id()
                );
                return;
            }
        }
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_webui_never_attempts_to_open_a_browser() {
        assert!(!should_launch_webui_window(DisplayEnvironment::Headless));
        assert!(should_launch_webui_window(DisplayEnvironment::LinuxWayland));
        assert!(should_launch_webui_window(DisplayEnvironment::LinuxX11));
        assert!(should_launch_webui_window(
            DisplayEnvironment::WindowsDesktop
        ));
        assert!(should_launch_webui_window(DisplayEnvironment::MacosDesktop));
    }
}
