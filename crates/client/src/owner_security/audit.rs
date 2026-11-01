//! Append-only security audit for owner initialization.
//!
//! Records intentionally contain only fixed routing metadata and parsed network
//! addresses. Request bodies, headers, initialization codes, recovery codes and
//! Passkey material must never be passed to this module.

use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const AUDIT_SCHEMA_VERSION: u32 = 1;
const AUDIT_FILE: &str = "owner-security-audit.jsonl";

pub(crate) struct OwnerSecurityAudit {
    #[cfg_attr(not(test), allow(dead_code))]
    path: PathBuf,
    file: Mutex<File>,
}

#[derive(Serialize)]
struct AuditRecord<'a> {
    schema_version: u32,
    at_unix_ms: u64,
    channel: &'a str,
    source_ip: Option<String>,
    method: &'a str,
    path: &'a str,
    status: Option<u16>,
    outcome: &'a str,
}

impl OwnerSecurityAudit {
    pub(crate) fn new(config_dir: &Path) -> Result<Self> {
        let path = config_dir.join(AUDIT_FILE);
        let file = open_private_audit_file(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &self,
        channel: &str,
        source_ip: Option<IpAddr>,
        method: &str,
        path: &str,
        status: Option<u16>,
        outcome: &str,
    ) -> Result<()> {
        let at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("系统时间早于 Unix epoch，无法记录安全审计")?
            .as_millis();
        let at_unix_ms = u64::try_from(at_unix_ms).context("安全审计时间超出范围")?;
        let record = AuditRecord {
            schema_version: AUDIT_SCHEMA_VERSION,
            at_unix_ms,
            channel,
            source_ip: source_ip.map(|ip| ip.to_string()),
            method,
            path,
            status,
            outcome,
        };
        let mut line = serde_json::to_vec(&record).context("无法序列化安全审计记录")?;
        line.push(b'\n');

        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("所有者安全审计锁已损坏"))?;
        file.write_all(&line).context("无法追加安全审计记录")?;
        file.sync_data().context("无法同步安全审计记录")?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn open_private_audit_file(path: &Path) -> Result<File> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!("安全审计路径必须是普通文件且不能是符号链接")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("无法检查安全审计文件"),
    }

    let mut options = OpenOptions::new();
    options.append(true).create(true).read(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("无法打开所有者安全审计文件: {}", path.display()))?;

    let opened = file.metadata().context("无法检查已打开的安全审计文件")?;
    if !opened.is_file() {
        anyhow::bail!("安全审计句柄必须指向普通文件");
    }
    let current = std::fs::symlink_metadata(path).context("无法复核安全审计文件")?;
    if current.file_type().is_symlink() || !current.is_file() {
        anyhow::bail!("安全审计路径在打开后变成了非普通文件");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        if opened.dev() != current.dev() || opened.ino() != current.ino() {
            anyhow::bail!("安全审计路径在打开过程中发生变化");
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .context("无法收紧安全审计文件权限")?;
    }

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn audit_is_append_only_private_and_contains_no_request_body() -> Result<()> {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-audit-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir)?;
        let audit = OwnerSecurityAudit::new(&dir)?;
        let source = "203.0.113.9".parse()?;
        audit.record(
            "public",
            Some(source),
            "POST",
            "/api/v1/auth/bootstrap/verify",
            Some(401),
            "invalid_code",
        )?;
        drop(audit);

        let reopened = OwnerSecurityAudit::new(&dir)?;
        reopened.record(
            "public",
            Some(source),
            "GET",
            "/api/v1/auth/bootstrap",
            Some(200),
            "success",
        )?;
        let content = std::fs::read_to_string(reopened.path())?;
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0])?;
        assert_eq!(first["channel"], "public");
        assert_eq!(first["source_ip"], "203.0.113.9");
        assert_eq!(first["status"], 401);
        assert_eq!(first["outcome"], "invalid_code");
        assert!(!content.contains("bootstrap-code"));
        assert!(!content.contains("recovery-code"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(reopened.path())?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn audit_rejects_a_symlink_without_touching_its_target() -> Result<()> {
        use std::os::unix::fs::symlink;

        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-audit-symlink-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir)?;
        let target = dir.join("unrelated.txt");
        std::fs::write(&target, b"must-not-change")?;
        symlink(&target, dir.join(AUDIT_FILE))?;

        let error = OwnerSecurityAudit::new(&dir)
            .err()
            .expect("symlink audit path must fail closed");
        assert!(error.to_string().contains("符号链接"));
        assert_eq!(std::fs::read(&target)?, b"must-not-change");

        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
