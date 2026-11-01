use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let output_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: export-browser-assets <output-directory>")?;
    fs::create_dir_all(&output_dir).context("create browser asset output directory")?;
    fs::write(
        output_dir.join("webui.css"),
        fuckipv6_common::ui_components::complete_webui_css(),
    )
    .context("write shared WebUI stylesheet")?;
    fs::write(
        output_dir.join("i18n.js"),
        fuckipv6_common::i18n_browser::browser_runtime_script(),
    )
    .context("write shared browser i18n runtime")?;
    Ok(())
}
