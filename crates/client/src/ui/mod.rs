pub mod mode;
mod surface;

pub(crate) use surface::{acquire as acquire_surface, UiSurfaceKind};

pub use mode::{
    apply_environment_request, detect_ui_mode, resolve_legacy_ui_mode, resolve_ui_mode,
    resolve_ui_request, DisplayEnvironment, UiCapabilities, UiMode, UiModeError, UiModeRequest,
    UiModeResolution,
};

/// WebUI 无头控制服务入口。它只报告服务地址，不打开浏览器或第二套桌面 UI。
/// 有桌面会话的进程必须由 GPUI 入口启动。
pub async fn run_webui_window(webui_port: u16) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{webui_port}");

    println!("========================================================================");
    println!("🚀 fuckipv6 WebUI 控制服务已启动 (无桌面环境)");
    println!("------------------------------------------------------------------------");
    println!("控制台地址: {url}");
    println!("不会自动打开浏览器；可从其他设备访问上述地址。");
    println!("========================================================================");

    tracing::info!("🖥️ WebUI 仅提供服务，不启动桌面窗口: {url}");
    Ok(())
}
