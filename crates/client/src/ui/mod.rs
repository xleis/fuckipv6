pub mod mode;

pub use mode::{detect_ui_mode, UiMode};

/// 桌面 GUI 模式入口: 在桌面环境中启动独立的图形窗口 (优先Chromium独立应用模式，降级为系统默认浏览器)
pub async fn run_desktop_gui(webui_port: u16) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{webui_port}");
    tracing::info!("🎨 桌面显示环境已检测，正在调起图形化控制界面: {url}");

    println!("========================================================================");
    println!("🚀 fuckipv6 桌面图形控制中心已启动");
    println!("------------------------------------------------------------------------");
    println!("图形控制台地址: {url}");
    println!("如独立窗口未自动弹出，您可直接在浏览器打开上述地址进行交互。");
    println!("========================================================================");

    launch_gui_window(&url);
    Ok(())
}

/// 启动独立桌面应用窗口
pub fn launch_gui_window(url: &str) {
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
                tracing::info!("已通过 {bin} 创建 Windows 独立图形窗口 (PID: {:?})", child.id());
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
