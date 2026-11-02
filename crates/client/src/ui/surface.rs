//! Runtime-directory-wide exclusivity for the user-visible UI surface.
//!
//! GPUI and WebUI may attach to the same control/node authority, but they are
//! alternative product surfaces. Keep one lock scoped to the local runtime
//! directory so two processes cannot expose both surfaces (or two WebUI
//! listeners) at the same time. The lock is released automatically on exit and
//! never owns the control/node lifetime.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use fs2::FileExt;

const SURFACE_LOCK_FILE: &str = "ui-surface.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiSurfaceKind {
    Gpui,
    WebUi,
}

impl UiSurfaceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Gpui => "GPUI",
            Self::WebUi => "WebUI",
        }
    }
}

/// Exclusive ownership of the local user-visible surface.
///
/// The lease deliberately lives outside control/node locks: closing a GPUI or
/// WebUI process must release only the UI surface while the data plane stays
/// alive.
#[derive(Debug)]
pub(crate) struct SurfaceLease {
    file: File,
}

impl Drop for SurfaceLease {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(%error, "释放 UI 表面独占锁失败");
        }
    }
}

pub(crate) fn acquire(kind: UiSurfaceKind) -> Result<SurfaceLease> {
    let directory = crate::local_ipc::runtime_directory()?;
    acquire_in(&directory, kind)
}

fn acquire_in(directory: &Path, kind: UiSurfaceKind) -> Result<SurfaceLease> {
    let path = directory.join(SURFACE_LOCK_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("无法打开 UI 表面锁 {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("无法收紧 UI 表面锁权限 {}", path.display()))?;
    }

    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            let owner = read_owner(&mut file).unwrap_or_else(|| "另一个".into());
            bail!(
                "已有 {owner} 界面正在使用当前节点；GPUI 与 WebUI 只能启动一个，请先关闭另一个界面"
            );
        }
        Err(error) => {
            return Err(error).with_context(|| format!("无法锁定 UI 表面 {}", path.display()));
        }
    }

    file.set_len(0)
        .with_context(|| format!("无法清空 UI 表面锁 {}", path.display()))?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("无法定位 UI 表面锁 {}", path.display()))?;
    writeln!(file, "{}", kind.label())
        .with_context(|| format!("无法写入 UI 表面锁 {}", path.display()))?;
    file.flush()
        .with_context(|| format!("无法刷新 UI 表面锁 {}", path.display()))?;

    Ok(SurfaceLease { file })
}

fn read_owner(file: &mut File) -> Option<String> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut owner = String::new();
    file.take(64).read_to_string(&mut owner).ok()?;
    let owner = owner.trim();
    matches!(owner, "GPUI" | "WebUI").then(|| owner.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn temporary_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fuckipv6-ui-surface-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temporary runtime directory");
        path
    }

    #[test]
    fn only_one_surface_can_hold_the_runtime_lease() {
        let directory = temporary_directory("exclusive");
        let webui = acquire_in(&directory, UiSurfaceKind::WebUi).expect("first surface lease");

        let error = acquire_in(&directory, UiSurfaceKind::Gpui)
            .expect_err("second surface must fail closed");
        assert!(error.to_string().contains("WebUI"));
        assert!(error.to_string().contains("只能启动一个"));

        drop(webui);
        let gpui = acquire_in(&directory, UiSurfaceKind::Gpui)
            .expect("lease must be reusable after the first surface exits");
        drop(gpui);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn duplicate_webui_surfaces_are_rejected_regardless_of_listener_port() {
        let directory = temporary_directory("webui");
        let _first = acquire_in(&directory, UiSurfaceKind::WebUi).expect("first WebUI lease");

        let error = acquire_in(&directory, UiSurfaceKind::WebUi)
            .expect_err("a second WebUI process must not start");
        assert!(error.to_string().contains("WebUI"));
        assert!(error.to_string().contains("另一个界面"));

        let _ = std::fs::remove_dir_all(directory);
    }
}
