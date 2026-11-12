//! Durable, verifiable owner-security audit ledger.
//!
//! The short-lived activity feed is a separate projection. This file is the
//! long-term evidence store: records remain on disk across restarts and
//! rotation, carry a global sequence and hash link, and are anchored by an
//! atomically replaced head checkpoint. Writers in different processes are
//! serialized per append, so the WebUI and CLI can share one ledger without
//! holding a process-lifetime lock. Capacity exhaustion fails closed rather
//! than silently deleting old records.

use anyhow::{Context, Result};
use fs2::FileExt;
use fuckipv6_common::domain::{
    verify_audit_chain, AuditChainCheckpoint, AuditChainError, AuditChainRecord,
    AuditChainVerification, AuditEntry, AuditExportFilter, AuditId, AuditOutcome,
    AuditRetentionPolicy, NodeId, AUDIT_GENESIS_HASH,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const AUDIT_FILE_PREFIX: &str = "owner-security-audit";
const LEGACY_AUDIT_FILE: &str = "owner-security-audit.jsonl";
const MIGRATED_AUDIT_FILE: &str = "owner-security-audit.jsonl.migrated";
const CHECKPOINT_FILE: &str = "owner-security-audit.head.json";
const LOCK_FILE: &str = ".owner-security-audit.lock";
const CLOCK_DRIFT_TOLERANCE_MS: u64 = 5 * 60 * 1000;
const MAX_RECORDED_WARNINGS: usize = 32;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) const ENV_AUDIT_RETENTION_DAYS: &str = "FUCKIPV6_AUDIT_RETENTION_DAYS";
pub(crate) const ENV_AUDIT_SEGMENT_RECORDS: &str = "FUCKIPV6_AUDIT_SEGMENT_RECORDS";
pub(crate) const ENV_AUDIT_MAX_SEGMENTS: &str = "FUCKIPV6_AUDIT_MAX_SEGMENTS";

pub(crate) struct OwnerSecurityAudit {
    node_id: NodeId,
    directory: PathBuf,
    policy: AuditRetentionPolicy,
    state: Mutex<LedgerState>,
    warnings: Mutex<Vec<AuditLedgerWarning>>,
    clock: Box<dyn AuditClock>,
}

struct LedgerState {
    active_segment: u32,
    active_record_count: u64,
    next_sequence: u64,
    previous_hash: String,
    checkpoint: AuditChainCheckpoint,
    last_occurred_at_unix_ms: Option<u64>,
    active_file_len: u64,
    segment_count: usize,
    writer_override: Option<Box<dyn AuditLineWriter>>,
    poisoned: Option<String>,
}

trait AuditLineWriter: Send {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn sync_data(&mut self) -> io::Result<()>;
}

struct FileLineWriter(File);

impl AuditLineWriter for FileLineWriter {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)
    }

    fn sync_data(&mut self) -> io::Result<()> {
        self.0.sync_data()
    }
}

trait AuditClock: Send + Sync {
    fn now_unix_ms(&self) -> Result<u64>;
}

struct SystemAuditClock;

impl AuditClock for SystemAuditClock {
    fn now_unix_ms(&self) -> Result<u64> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("系统时间早于 Unix epoch，无法记录安全审计")?
            .as_millis();
        u64::try_from(millis).context("安全审计时间超出范围")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AuditLedgerWarning {
    pub(crate) code: String,
    pub(crate) occurred_at_unix_ms: u64,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AuditLedgerHealth {
    pub(crate) healthy: bool,
    pub(crate) checkpoint_sequence: u64,
    pub(crate) record_count: u64,
    pub(crate) minimum_retention_days: u32,
    pub(crate) segment_records: u64,
    pub(crate) max_segments: u32,
    pub(crate) warnings: Vec<AuditLedgerWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AuditExportManifest {
    pub(crate) schema_version: u32,
    pub(crate) generated_at_unix_ms: u64,
    pub(crate) filter: AuditExportFilter,
    pub(crate) verification: AuditChainVerification,
    pub(crate) checkpoint: AuditChainCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AuditExportBundle {
    pub(crate) manifest: AuditExportManifest,
    pub(crate) records: Vec<AuditChainRecord>,
}

#[derive(Debug, Error)]
pub(crate) enum AuditLedgerError {
    #[error("安全审计账本已损坏: {0}")]
    Corrupt(String),
    #[error("安全审计链验证失败: {0}")]
    Verification(#[from] AuditChainError),
    #[error("安全审计容量已用尽，拒绝删除历史记录或继续写入")]
    CapacityExceeded,
    #[error("安全审计磁盘空间不足，拒绝继续操作")]
    StorageFull { operation: String },
    #[error("安全审计写入失败 ({operation}): {source}")]
    WriteFailed {
        operation: String,
        #[source]
        source: io::Error,
    },
    #[error("安全审计同步失败 ({operation}): {source}")]
    SyncFailed {
        operation: String,
        #[source]
        source: io::Error,
    },
    #[error("安全审计时钟回退: last={last_unix_ms}, current={current_unix_ms}")]
    ClockDrift {
        last_unix_ms: u64,
        current_unix_ms: u64,
    },
    #[error("安全审计导出范围无效: {0}")]
    InvalidExportFilter(String),
    #[error("无法获取安全审计系统时间: {0}")]
    Clock(String),
}

impl AuditLedgerError {
    fn warning_code(&self) -> &'static str {
        match self {
            Self::StorageFull { .. } => "audit_storage_full",
            Self::WriteFailed { .. } => "audit_write_failed",
            Self::SyncFailed { .. } => "audit_sync_failed",
            Self::ClockDrift { .. } | Self::Clock(_) => "audit_clock_drift",
            Self::CapacityExceeded => "audit_capacity_exhausted",
            Self::Corrupt(_) | Self::Verification(_) => "audit_integrity_failure",
            Self::InvalidExportFilter(_) => "audit_export_invalid",
        }
    }
}

impl OwnerSecurityAudit {
    pub(crate) fn new(config_dir: &Path, node_id: NodeId) -> Result<Self> {
        let policy = retention_policy_from_env()?;
        Self::new_with_policy(config_dir, node_id, policy, Box::new(SystemAuditClock))
    }

    #[cfg(test)]
    pub(crate) fn new_with_retention_policy_for_test(
        config_dir: &Path,
        node_id: NodeId,
        policy: AuditRetentionPolicy,
    ) -> Result<Self> {
        Self::new_with_policy(config_dir, node_id, policy, Box::new(SystemAuditClock))
    }

    fn new_with_policy(
        config_dir: &Path,
        node_id: NodeId,
        policy: AuditRetentionPolicy,
        clock: Box<dyn AuditClock>,
    ) -> Result<Self> {
        policy.validate().context("安全审计保留策略无效")?;
        fs::create_dir_all(config_dir).context("无法创建安全审计目录")?;
        secure_directory(config_dir)?;
        let lock = open_lock_file(&config_dir.join(LOCK_FILE))?;
        FileExt::lock_exclusive(&lock).context("无法锁定安全审计账本进行初始化")?;

        let mut warnings = Vec::new();
        let loaded = initialize_ledger(config_dir, &node_id, policy)?;
        if loaded.recovered_checkpoint {
            warnings.push(recovered_checkpoint_warning());
        }
        drop(lock);
        Ok(Self {
            node_id,
            directory: config_dir.to_path_buf(),
            policy,
            state: Mutex::new(LedgerState::from_loaded(loaded)),
            warnings: Mutex::new(warnings),
            clock,
        })
    }

    /// Append one sanitized request audit record. Write, sync, clock and
    /// capacity failures are returned to the caller and also surface in
    /// `health`, so the control plane can fail closed with an observable alert.
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
        self.append_chain_entry(|sequence, now_unix_ms| {
            security_audit_entry(
                &self.node_id,
                sequence,
                now_unix_ms,
                channel,
                source_ip,
                method,
                path,
                status,
                outcome,
            )
        })
        .map(|_| ())
    }

    /// Append a pre-sanitized command/security entry to the same durable hash
    /// chain. The ledger owns occurrence time and sequence so concurrent
    /// adapters cannot manufacture out-of-order evidence.
    pub(crate) fn append_entry(&self, mut entry: AuditEntry) -> Result<AuditChainRecord> {
        self.append_chain_entry(|_, now_unix_ms| {
            entry.occurred_at_unix_ms = now_unix_ms;
            Ok(entry)
        })
    }

    fn append_chain_entry<F>(&self, build_entry: F) -> Result<AuditChainRecord>
    where
        F: FnOnce(u64, u64) -> Result<AuditEntry>,
    {
        let now_unix_ms = self
            .clock
            .now_unix_ms()
            .map_err(|error| AuditLedgerError::Clock(format!("{error:#}")))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuditLedgerError::Corrupt("安全审计状态锁已损坏".into()))?;
        if let Some(reason) = state.poisoned.as_ref() {
            return Err(AuditLedgerError::Corrupt(format!(
                "安全审计在写入失败后已封闭，必须先修复存储并重启: {reason}"
            ))
            .into());
        }

        let _lock = self.acquire_write_lock()?;
        self.refresh_state_under_lock(&mut state)?;

        if let Some(last_unix_ms) = state.last_occurred_at_unix_ms {
            if now_unix_ms.saturating_add(CLOCK_DRIFT_TOLERANCE_MS) < last_unix_ms {
                let error = AuditLedgerError::ClockDrift {
                    last_unix_ms,
                    current_unix_ms: now_unix_ms,
                };
                self.note_warning(&error, now_unix_ms);
                return Err(error.into());
            }
        }

        if state.next_sequence > self.policy.max_records() {
            let error = AuditLedgerError::CapacityExceeded;
            self.note_warning(&error, now_unix_ms);
            return Err(error.into());
        }
        if state.active_record_count >= self.policy.segment_records {
            let next_segment = state.active_segment.saturating_add(1);
            if next_segment >= self.policy.max_segments {
                let error = AuditLedgerError::CapacityExceeded;
                self.note_warning(&error, now_unix_ms);
                return Err(error.into());
            }
            let path = ledger_segment_path(&self.directory, next_segment);
            drop(create_private_file(&path)?);
            sync_directory(&self.directory)?;
            state.active_segment = next_segment;
            state.active_record_count = 0;
            state.active_file_len = 0;
            state.segment_count = state.segment_count.saturating_add(1);
        }

        let sequence = state.next_sequence;
        let entry = build_entry(sequence, now_unix_ms)?;
        let record = AuditChainRecord::seal(sequence, &state.previous_hash, entry)
            .map_err(|error| AuditLedgerError::Corrupt(error.to_string()))?;
        let mut line = serde_json::to_vec(&record).context("无法序列化安全审计记录")?;
        line.push(b'\n');

        let active_path = ledger_segment_path(&self.directory, state.active_segment);
        let write_result = if let Some(mut writer) = state.writer_override.take() {
            writer
                .write_all(&line)
                .and_then(|_| writer.sync_data())
                .map(|_| state.active_file_len.saturating_add(line.len() as u64))
        } else {
            let file = open_private_append_file(&active_path)?;
            (|| -> io::Result<u64> {
                let mut writer = FileLineWriter(file);
                writer.write_all(&line)?;
                writer.sync_data()?;
                writer.0.metadata().map(|metadata| metadata.len())
            })()
        };
        let active_file_len = match write_result {
            Ok(active_file_len) => active_file_len,
            Err(source) => {
                let error = io_ledger_error("append", source);
                self.note_warning(&error, now_unix_ms);
                state.poisoned = Some(error.to_string());
                return Err(error.into());
            }
        };

        state.active_record_count = state.active_record_count.saturating_add(1);
        state.active_file_len = active_file_len;
        state.next_sequence = sequence.saturating_add(1);
        state.previous_hash = record.entry_hash.clone();
        state.last_occurred_at_unix_ms = Some(now_unix_ms);
        state.checkpoint = AuditChainCheckpoint {
            schema_version: record.schema_version,
            sequence,
            entry_hash: record.entry_hash.clone(),
            record_count: state.next_sequence.saturating_sub(1),
        };

        if let Err(error) = write_checkpoint(&self.directory, &state.checkpoint) {
            let error = ledger_error_from_anyhow("checkpoint", error);
            self.note_warning(&error, now_unix_ms);
            state.poisoned = Some(error.to_string());
            return Err(error.into());
        }
        Ok(record)
    }

    fn acquire_write_lock(&self) -> Result<File> {
        let lock = open_lock_file(&self.directory.join(LOCK_FILE))?;
        FileExt::lock_exclusive(&lock).context("无法锁定安全审计账本进行写入")?;
        Ok(lock)
    }

    /// Re-read the shared ledger before every mutation. The fast path consumes
    /// records appended by another process from the active segment; rotation,
    /// checkpoint recovery and truncation fall back to full verification.
    fn refresh_state_under_lock(&self, state: &mut LedgerState) -> Result<()> {
        if state.poisoned.is_some() {
            return Ok(());
        }
        if refresh_from_active_tail(&self.directory, state)? {
            return Ok(());
        }
        let loaded = initialize_ledger(&self.directory, &self.node_id, self.policy)?;
        if loaded.recovered_checkpoint {
            self.note_warning_detail(
                "audit_checkpoint_recovered",
                unix_time_ms().unwrap_or(1),
                "从完整审计段恢复了落后或缺失的 head checkpoint",
            );
        }
        state.replace_from_loaded(loaded);
        Ok(())
    }

    pub(crate) fn export(&self, filter: &AuditExportFilter) -> Result<AuditExportBundle> {
        filter
            .validate()
            .map_err(|error| AuditLedgerError::InvalidExportFilter(error.to_string()))?;
        let generated_at_unix_ms = self
            .clock
            .now_unix_ms()
            .map_err(|error| AuditLedgerError::Clock(format!("{error:#}")))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| AuditLedgerError::Corrupt("安全审计状态锁已损坏".into()))?;
        let _lock = self.acquire_write_lock()?;
        self.refresh_state_under_lock(&mut state)?;
        let records = read_all_records(&self.directory)?;
        let verification = state.checkpoint.verify(&records)?;
        let records = records
            .into_iter()
            .filter(|record| filter.matches(&record.entry))
            .map(|mut record| {
                // Export is a second redaction boundary. A policy change or a
                // migrated legacy record cannot carry a raw credential out.
                record.entry.redact_sensitive_fields();
                record
            })
            .collect();
        Ok(AuditExportBundle {
            manifest: AuditExportManifest {
                schema_version: record_schema_version(),
                generated_at_unix_ms,
                filter: filter.clone(),
                verification,
                checkpoint: state.checkpoint.clone(),
            },
            records,
        })
    }

    pub(crate) fn health(&self) -> AuditLedgerHealth {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => {
                return AuditLedgerHealth {
                    healthy: false,
                    checkpoint_sequence: 0,
                    record_count: 0,
                    minimum_retention_days: self.policy.minimum_retention_days,
                    segment_records: self.policy.segment_records,
                    max_segments: self.policy.max_segments,
                    warnings: vec![AuditLedgerWarning {
                        code: "audit_integrity_failure".into(),
                        occurred_at_unix_ms: unix_time_ms().unwrap_or(1),
                        detail: "安全审计状态锁已损坏".into(),
                    }],
                };
            }
        };
        if state.poisoned.is_none() {
            let refreshed = self
                .acquire_write_lock()
                .and_then(|_lock| self.refresh_state_under_lock(&mut state));
            if let Err(error) = refreshed {
                self.note_warning_detail(
                    "audit_refresh_failed",
                    unix_time_ms().unwrap_or(1),
                    format!("刷新安全审计状态失败: {error:#}"),
                );
            }
        }
        let warnings = self
            .warnings
            .lock()
            .map(|warnings| warnings.clone())
            .unwrap_or_else(|_| {
                vec![AuditLedgerWarning {
                    code: "audit_integrity_failure".into(),
                    occurred_at_unix_ms: unix_time_ms().unwrap_or(1),
                    detail: "安全审计告警锁已损坏".into(),
                }]
            });
        AuditLedgerHealth {
            healthy: warnings.is_empty(),
            checkpoint_sequence: state.checkpoint.sequence,
            record_count: state.checkpoint.record_count,
            minimum_retention_days: self.policy.minimum_retention_days,
            segment_records: self.policy.segment_records,
            max_segments: self.policy.max_segments,
            warnings,
        }
    }

    fn note_warning(&self, error: &AuditLedgerError, now_unix_ms: u64) {
        self.note_warning_detail(error.warning_code(), now_unix_ms, error.to_string());
    }

    fn note_warning_detail(
        &self,
        code: impl Into<String>,
        now_unix_ms: u64,
        detail: impl Into<String>,
    ) {
        if let Ok(mut warnings) = self.warnings.lock() {
            warnings.push(AuditLedgerWarning {
                code: code.into(),
                occurred_at_unix_ms: now_unix_ms.max(1),
                detail: detail.into(),
            });
            if warnings.len() > MAX_RECORDED_WARNINGS {
                let excess = warnings.len() - MAX_RECORDED_WARNINGS;
                warnings.drain(..excess);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn active_path(&self) -> PathBuf {
        let state = self.state.lock().expect("audit state");
        ledger_segment_path(&self.directory, state.active_segment)
    }

    #[cfg(test)]
    fn replace_writer(&self, writer: Box<dyn AuditLineWriter>) {
        self.state.lock().expect("audit state").writer_override = Some(writer);
    }
}

struct LoadedLedger {
    active_segment: u32,
    active_record_count: u64,
    next_sequence: u64,
    previous_hash: String,
    checkpoint: AuditChainCheckpoint,
    last_occurred_at_unix_ms: Option<u64>,
    active_file_len: u64,
    segment_count: usize,
    recovered_checkpoint: bool,
}

impl LedgerState {
    fn from_loaded(loaded: LoadedLedger) -> Self {
        Self {
            active_segment: loaded.active_segment,
            active_record_count: loaded.active_record_count,
            next_sequence: loaded.next_sequence,
            previous_hash: loaded.previous_hash,
            checkpoint: loaded.checkpoint,
            last_occurred_at_unix_ms: loaded.last_occurred_at_unix_ms,
            active_file_len: loaded.active_file_len,
            segment_count: loaded.segment_count,
            writer_override: None,
            poisoned: None,
        }
    }

    fn replace_from_loaded(&mut self, loaded: LoadedLedger) {
        let writer_override = self.writer_override.take();
        *self = Self::from_loaded(loaded);
        self.writer_override = writer_override;
    }
}

fn recovered_checkpoint_warning() -> AuditLedgerWarning {
    AuditLedgerWarning {
        code: "audit_checkpoint_recovered".to_string(),
        occurred_at_unix_ms: unix_time_ms().unwrap_or(1),
        detail: "从完整审计段恢复了落后或缺失的 head checkpoint".to_string(),
    }
}

/// Consume records appended by another process without re-reading the complete
/// retained history. Returns false when the active segment rotated, the
/// checkpoint is behind/ahead in an unexpected way, or a full integrity check
/// is required.
fn refresh_from_active_tail(directory: &Path, state: &mut LedgerState) -> Result<bool> {
    let segments = audit_segment_paths(directory)?;
    if segments.len() != state.segment_count || state.active_segment as usize + 1 != segments.len()
    {
        return Ok(false);
    }
    let active_path = &segments[state.active_segment as usize];
    let metadata = fs::metadata(active_path).context("无法读取安全审计当前分片元数据")?;
    if metadata.len() < state.active_file_len {
        return Ok(false);
    }
    let checkpoint = read_checkpoint(directory)?;
    if metadata.len() == state.active_file_len {
        return Ok(checkpoint == state.checkpoint);
    }

    let mut file = open_private_read_file(active_path)?;
    file.seek(SeekFrom::Start(state.active_file_len))
        .context("无法定位安全审计追加内容")?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)
        .context("无法读取安全审计追加内容")?;

    let mut expected_sequence = state.next_sequence;
    let mut previous_hash = state.previous_hash.clone();
    let mut appended = 0u64;
    let mut last_occurred_at_unix_ms = None;
    for line in tail.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record: AuditChainRecord =
            serde_json::from_slice(line).context("无法解析其他进程追加的安全审计记录")?;
        record.verify(expected_sequence, &previous_hash)?;
        expected_sequence = expected_sequence.saturating_add(1);
        previous_hash = record.entry_hash;
        last_occurred_at_unix_ms = Some(record.entry.occurred_at_unix_ms);
        appended = appended.saturating_add(1);
    }
    if appended == 0
        || checkpoint.sequence != expected_sequence.saturating_sub(1)
        || checkpoint.record_count != expected_sequence.saturating_sub(1)
        || checkpoint.entry_hash != previous_hash
    {
        return Ok(false);
    }

    state.active_record_count = state.active_record_count.saturating_add(appended);
    state.active_file_len = metadata.len();
    state.next_sequence = expected_sequence;
    state.previous_hash = previous_hash;
    state.last_occurred_at_unix_ms = last_occurred_at_unix_ms;
    state.checkpoint = checkpoint;
    Ok(true)
}

fn initialize_ledger(
    directory: &Path,
    node_id: &NodeId,
    policy: AuditRetentionPolicy,
) -> Result<LoadedLedger> {
    let mut segments = audit_segment_paths(directory)?;
    let legacy_path = directory.join(LEGACY_AUDIT_FILE);
    if segments.is_empty() && path_exists_no_symlink(&legacy_path)? {
        migrate_legacy_ledger(directory, node_id, policy)?;
        segments = audit_segment_paths(directory)?;
    } else if !segments.is_empty() && path_exists_no_symlink(&legacy_path)? {
        return Err(AuditLedgerError::Corrupt(
            "安全审计同时存在旧格式文件和新分片，拒绝猜测权威顺序".into(),
        )
        .into());
    }

    if segments.is_empty() {
        create_private_file(&ledger_segment_path(directory, 0))?;
        write_checkpoint(directory, &AuditChainCheckpoint::default())?;
        sync_directory(directory)?;
        segments.push(ledger_segment_path(directory, 0));
    }

    for (expected, path) in segments.iter().enumerate() {
        let expected_path = ledger_segment_path(directory, expected as u32);
        if path != &expected_path {
            return Err(AuditLedgerError::Corrupt("安全审计分片序号不连续".into()).into());
        }
    }

    let (records, last_segment_count) = read_segment_records(&segments)?;
    if records.len() as u64 > policy.max_records() {
        return Err(AuditLedgerError::CapacityExceeded.into());
    }
    let verification = verify_audit_chain(&records)?;
    let checkpoint = read_checkpoint(directory)?;
    let recovered_checkpoint = match checkpoint.verify(&records) {
        Ok(_) => false,
        Err(AuditChainError::CheckpointMismatch { .. })
            if checkpoint.sequence < verification.last_sequence.unwrap_or(0) =>
        {
            let checkpoint_count = usize::try_from(checkpoint.record_count)
                .map_err(|_| AuditLedgerError::Corrupt("安全审计 checkpoint 计数无效".into()))?;
            if checkpoint_count > records.len() {
                return Err(
                    AuditLedgerError::Corrupt("安全审计 checkpoint 超前于账本".into()).into(),
                );
            }
            checkpoint.verify(&records[..checkpoint_count])?;
            let recovered = AuditChainCheckpoint::from_verification(&verification);
            write_checkpoint(directory, &recovered)?;
            return Ok(LoadedLedger {
                active_segment: (segments.len() - 1) as u32,
                active_record_count: last_segment_count,
                next_sequence: verification.last_sequence.unwrap_or(0).saturating_add(1),
                previous_hash: verification.last_hash,
                checkpoint: recovered,
                last_occurred_at_unix_ms: records
                    .last()
                    .map(|record| record.entry.occurred_at_unix_ms),
                active_file_len: fs::metadata(&segments[segments.len() - 1])
                    .context("无法读取安全审计当前分片元数据")?
                    .len(),
                segment_count: segments.len(),
                recovered_checkpoint: true,
            });
        }
        Err(error) => return Err(error.into()),
    };

    Ok(LoadedLedger {
        active_segment: (segments.len() - 1) as u32,
        active_record_count: last_segment_count,
        next_sequence: verification.last_sequence.unwrap_or(0).saturating_add(1),
        previous_hash: verification.last_hash,
        checkpoint,
        last_occurred_at_unix_ms: records
            .last()
            .map(|record| record.entry.occurred_at_unix_ms),
        active_file_len: fs::metadata(&segments[segments.len() - 1])
            .context("无法读取安全审计当前分片元数据")?
            .len(),
        segment_count: segments.len(),
        recovered_checkpoint,
    })
}

#[allow(clippy::too_many_arguments)]
fn security_audit_entry(
    node_id: &NodeId,
    sequence: u64,
    occurred_at_unix_ms: u64,
    channel: &str,
    source_ip: Option<IpAddr>,
    method: &str,
    path: &str,
    status: Option<u16>,
    outcome: &str,
) -> Result<AuditEntry> {
    let audit_id = AuditId::from_public_key(
        format!("{node_id}:{sequence}:{occurred_at_unix_ms}:{method}:{path}").as_bytes(),
    );
    let outcome_kind = match outcome {
        "success" => AuditOutcome::Succeeded,
        "internal_error" | "write_failed" => AuditOutcome::Failed,
        _ => AuditOutcome::Denied,
    };
    let mut details = BTreeMap::from([
        ("channel".to_string(), channel.to_string()),
        ("method".to_string(), method.to_string()),
        ("path".to_string(), path.to_string()),
        ("outcome".to_string(), outcome.to_string()),
        (
            "object_kind".to_string(),
            "owner_security_endpoint".to_string(),
        ),
        ("object_id".to_string(), path.to_string()),
    ]);
    if let Some(source_ip) = source_ip {
        details.insert("source_ip".to_string(), source_ip.to_string());
    }
    if let Some(status) = status {
        details.insert("status".to_string(), status.to_string());
    }
    let mut entry = AuditEntry {
        audit_id,
        operation_id: None,
        actor: node_id.clone(),
        network_id: None,
        action: format!("owner_security.request.{outcome}"),
        resource: path.to_string(),
        outcome: outcome_kind,
        occurred_at_unix_ms,
        summary: format!("Owner security request {method} {path}"),
        details,
        sensitive_keys: BTreeSet::new(),
    };
    entry
        .validate()
        .map_err(|error| AuditLedgerError::Corrupt(error.to_string()))?;
    entry.redact_sensitive_fields();
    Ok(entry)
}

#[derive(Debug, Deserialize)]
struct LegacyAuditRecord {
    #[serde(default)]
    schema_version: u32,
    at_unix_ms: u64,
    channel: String,
    source_ip: Option<String>,
    method: String,
    path: String,
    status: Option<u16>,
    outcome: String,
}

fn migrate_legacy_ledger(
    directory: &Path,
    node_id: &NodeId,
    policy: AuditRetentionPolicy,
) -> Result<()> {
    let legacy_path = directory.join(LEGACY_AUDIT_FILE);
    let mut file = open_private_read_file(&legacy_path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)
        .context("无法读取旧格式安全审计")?;
    let mut records = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let legacy: LegacyAuditRecord = serde_json::from_str(line)
            .with_context(|| format!("旧格式安全审计第 {} 行无法解析", index.saturating_add(1)))?;
        if legacy.schema_version > 1 {
            return Err(AuditLedgerError::Corrupt(format!(
                "不支持的旧审计 schema {}",
                legacy.schema_version
            ))
            .into());
        }
        let sequence = records.len() as u64 + 1;
        let mut entry = security_audit_entry(
            node_id,
            sequence,
            legacy.at_unix_ms,
            &legacy.channel,
            legacy.source_ip.as_deref().and_then(|ip| ip.parse().ok()),
            &legacy.method,
            &legacy.path,
            legacy.status,
            &legacy.outcome,
        )?;
        entry.audit_id = AuditId::from_public_key(
            format!(
                "legacy:{node_id}:{sequence}:{}:{}:{}",
                legacy.at_unix_ms, legacy.method, legacy.path
            )
            .as_bytes(),
        );
        let previous_hash = records
            .last()
            .map(|record: &AuditChainRecord| record.entry_hash.as_str())
            .unwrap_or(AUDIT_GENESIS_HASH);
        records.push(
            AuditChainRecord::seal(sequence, previous_hash, entry)
                .map_err(|error| AuditLedgerError::Corrupt(error.to_string()))?,
        );
    }
    if records.len() as u64 > policy.max_records() {
        return Err(AuditLedgerError::CapacityExceeded.into());
    }
    let segment_count = records
        .len()
        .div_ceil(policy.segment_records.max(1) as usize);
    if segment_count > policy.max_segments as usize {
        return Err(AuditLedgerError::CapacityExceeded.into());
    }
    for (index, chunk) in records.chunks(policy.segment_records as usize).enumerate() {
        let mut bytes = Vec::new();
        for record in chunk {
            serde_json::to_writer(&mut bytes, record).context("无法序列化迁移后的安全审计")?;
            bytes.push(b'\n');
        }
        write_new_private_file_atomic(&ledger_segment_path(directory, index as u32), &bytes)?;
    }
    if records.is_empty() {
        create_private_file(&ledger_segment_path(directory, 0))?;
    }
    let verification = verify_audit_chain(&records)?;
    write_checkpoint(
        directory,
        &AuditChainCheckpoint::from_verification(&verification),
    )?;
    fs::rename(&legacy_path, directory.join(MIGRATED_AUDIT_FILE))
        .context("无法封存迁移后的旧格式安全审计")?;
    sync_directory(directory)?;
    Ok(())
}

fn read_segment_records(segments: &[PathBuf]) -> Result<(Vec<AuditChainRecord>, u64)> {
    let mut records = Vec::new();
    let mut last_count = 0u64;
    for path in segments {
        let mut file = open_private_read_file(path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .with_context(|| format!("无法读取安全审计分片: {}", path.display()))?;
        let mut count = 0u64;
        for (index, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record = serde_json::from_str(line).with_context(|| {
                format!(
                    "安全审计分片 {} 第 {} 行无法解析",
                    path.display(),
                    index.saturating_add(1)
                )
            })?;
            records.push(record);
            count = count.saturating_add(1);
        }
        last_count = count;
    }
    Ok((records, last_count))
}

fn read_all_records(directory: &Path) -> Result<Vec<AuditChainRecord>> {
    Ok(read_segment_records(&audit_segment_paths(directory)?)?.0)
}

fn read_checkpoint(directory: &Path) -> Result<AuditChainCheckpoint> {
    let path = directory.join(CHECKPOINT_FILE);
    if !path_exists_no_symlink(&path)? {
        return Ok(AuditChainCheckpoint::default());
    }
    let mut file = open_private_read_file(&path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .context("无法读取安全审计 head checkpoint")?;
    serde_json::from_slice(&bytes).context("无法解析安全审计 head checkpoint")
}

fn write_checkpoint(directory: &Path, checkpoint: &AuditChainCheckpoint) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(checkpoint).context("无法序列化安全审计 checkpoint")?;
    write_private_file_atomic(&directory.join(CHECKPOINT_FILE), &bytes)
}

fn retention_policy_from_env() -> Result<AuditRetentionPolicy> {
    let mut policy = AuditRetentionPolicy::default();
    if let Some(value) = parse_env_u32(ENV_AUDIT_RETENTION_DAYS)? {
        policy.minimum_retention_days = value;
    }
    if let Some(value) = parse_env_u64(ENV_AUDIT_SEGMENT_RECORDS)? {
        policy.segment_records = value;
    }
    if let Some(value) = parse_env_u32(ENV_AUDIT_MAX_SEGMENTS)? {
        policy.max_segments = value;
    }
    policy.validate().context("安全审计保留策略环境变量无效")?;
    Ok(policy)
}

fn parse_env_u32(name: &str) -> Result<Option<u32>> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map(Some)
            .with_context(|| format!("环境变量 {name} 必须是正整数")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("无法读取环境变量 {name}")),
    }
}

fn parse_env_u64(name: &str) -> Result<Option<u64>> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map(Some)
            .with_context(|| format!("环境变量 {name} 必须是正整数")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("无法读取环境变量 {name}")),
    }
}

fn ledger_segment_path(directory: &Path, segment: u32) -> PathBuf {
    directory.join(format!("{AUDIT_FILE_PREFIX}.{segment:06}.jsonl"))
}

fn audit_segment_paths(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).context("无法枚举安全审计目录")? {
        let entry = entry.context("无法读取安全审计目录项")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(segment) = name
            .strip_prefix(&format!("{AUDIT_FILE_PREFIX}."))
            .and_then(|name| name.strip_suffix(".jsonl"))
            .and_then(|number| number.parse::<u32>().ok())
        else {
            continue;
        };
        if name != format!("{AUDIT_FILE_PREFIX}.{segment:06}.jsonl") {
            return Err(AuditLedgerError::Corrupt("安全审计分片名称不规范".into()).into());
        }
        paths.push((segment, entry.path()));
    }
    paths.sort_by_key(|(segment, _)| *segment);
    Ok(paths.into_iter().map(|(_, path)| path).collect())
}

fn open_lock_file(path: &Path) -> Result<File> {
    let file = open_private_file(path, true, false)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .context("无法收紧安全审计锁文件权限")?;
    }
    Ok(file)
}

fn open_private_append_file(path: &Path) -> Result<File> {
    open_private_file(path, true, true)
}

fn create_private_file(path: &Path) -> Result<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(AuditLedgerError::Corrupt(format!(
                "安全审计路径必须是普通文件且不能是符号链接: {}",
                path.display()
            ))
            .into());
        }
        Ok(_) => {
            return Err(AuditLedgerError::Corrupt(format!(
                "安全审计分片已存在，拒绝覆盖: {}",
                path.display()
            ))
            .into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("无法检查安全审计分片"),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("无法创建安全审计分片: {}", path.display()))?;
    secure_open_file(path, &file)?;
    Ok(file)
}

fn open_private_file(path: &Path, create: bool, append: bool) -> Result<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!("安全审计路径必须是普通文件且不能是符号链接")
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {}
        Err(error) => return Err(error).context("无法检查安全审计文件"),
    }

    let mut options = OpenOptions::new();
    options.write(true).read(false).create(create);
    if append {
        options.append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("无法打开安全审计文件: {}", path.display()))?;
    secure_open_file(path, &file)?;
    Ok(file)
}

fn open_private_read_file(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path).context("无法检查安全审计文件")?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("安全审计路径必须是普通文件且不能是符号链接");
    }
    let mut options = OpenOptions::new();
    options.read(true).write(false);
    #[cfg(unix)]
    {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
    }
    let file = options
        .open(path)
        .with_context(|| format!("无法读取安全审计文件: {}", path.display()))?;
    secure_open_file(path, &file)?;
    Ok(file)
}

fn secure_open_file(path: &Path, file: &File) -> Result<()> {
    let opened = file.metadata().context("无法检查已打开的安全审计文件")?;
    if !opened.is_file() {
        anyhow::bail!("安全审计句柄必须指向普通文件");
    }
    let current = fs::symlink_metadata(path).context("无法复核安全审计文件")?;
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
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .context("无法收紧安全审计文件权限")?;
    }
    Ok(())
}

fn temporary_file_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().context("安全审计文件缺少父目录")?;
    let nonce = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".tmp-{}-{}-{}-{nonce}",
        std::process::id(),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audit"),
        unix_time_ms().unwrap_or_default()
    )))
}

fn write_new_private_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("安全审计文件缺少父目录")?;
    let temporary = temporary_file_path(path)?;
    let result = (|| -> Result<()> {
        let mut file = create_private_file(&temporary)?;
        file.write_all(bytes).context("无法写入安全审计临时文件")?;
        file.sync_all().context("无法同步安全审计临时文件")?;
        fs::rename(&temporary, path).context("无法原子发布安全审计分片")?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_private_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("安全审计文件缺少父目录")?;
    let temporary = temporary_file_path(path)?;
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("无法创建安全审计临时文件: {}", temporary.display()))?;
        file.write_all(bytes).context("无法写入安全审计临时文件")?;
        file.sync_all().context("无法同步安全审计临时文件")?;
        fs::rename(&temporary, path).context("无法原子替换安全审计 checkpoint")?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn secure_directory(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("无法收紧安全审计目录权限: {}", _path.display()))?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("无法同步安全审计目录: {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn path_exists_no_symlink(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(AuditLedgerError::Corrupt(format!(
                "安全审计路径必须是普通文件且不能是符号链接: {}",
                path.display()
            ))
            .into())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("无法检查安全审计文件"),
    }
}

fn ledger_error_from_anyhow(operation: &str, error: anyhow::Error) -> AuditLedgerError {
    if let Some(source) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<io::Error>())
    {
        return io_ledger_error(operation, io::Error::new(source.kind(), source.to_string()));
    }
    AuditLedgerError::Corrupt(format!("{error:#}"))
}

fn io_ledger_error(operation: &str, source: io::Error) -> AuditLedgerError {
    if is_storage_full(&source) {
        return AuditLedgerError::StorageFull {
            operation: operation.to_string(),
        };
    }
    if operation == "sync" {
        AuditLedgerError::SyncFailed {
            operation: operation.to_string(),
            source,
        }
    } else {
        AuditLedgerError::WriteFailed {
            operation: operation.to_string(),
            source,
        }
    }
}

fn is_storage_full(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::StorageFull
        || matches!(error.raw_os_error(), Some(28 | 39 | 112 | 122))
}

fn record_schema_version() -> u32 {
    fuckipv6_common::domain::AUDIT_CHAIN_SCHEMA_VERSION
}

fn unix_time_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("系统时间超出范围")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuckipv6_common::domain::{AuditChainVerification, AuditExportFilter};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestClock(Arc<AtomicU64>);

    impl AuditClock for TestClock {
        fn now_unix_ms(&self) -> Result<u64> {
            Ok(self.0.load(Ordering::Relaxed))
        }
    }

    struct FullWriter;

    impl AuditLineWriter for FullWriter {
        fn write_all(&mut self, _bytes: &[u8]) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::StorageFull))
        }

        fn sync_data(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "fuckipv6-owner-audit-{name}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp audit dir");
        dir
    }

    fn test_ledger(
        dir: &Path,
        policy: AuditRetentionPolicy,
        now: u64,
    ) -> (OwnerSecurityAudit, Arc<AtomicU64>) {
        let clock = Arc::new(AtomicU64::new(now));
        let ledger = OwnerSecurityAudit::new_with_policy(
            dir,
            NodeId::from_public_key(b"audit-node"),
            policy,
            Box::new(TestClock(clock.clone())),
        )
        .expect("test audit ledger");
        (ledger, clock)
    }

    fn record(ledger: &OwnerSecurityAudit, path: &str, outcome: &str) -> Result<()> {
        ledger.record(
            "local",
            Some("203.0.113.9".parse().expect("source IP")),
            "POST",
            path,
            Some(200),
            outcome,
        )
    }

    fn all_records(ledger: &OwnerSecurityAudit) -> Vec<AuditChainRecord> {
        read_all_records(&ledger.directory).expect("audit records")
    }

    #[test]
    fn audit_is_private_append_only_and_contains_no_request_body() -> Result<()> {
        let dir = temp_dir("private");
        let (ledger, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_000);
        record(&ledger, "/api/v1/auth/bootstrap/verify", "invalid_code")?;
        record(&ledger, "/api/v1/auth/bootstrap", "success")?;
        let active_path = ledger.active_path();
        drop(ledger);

        let (reopened, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_100);
        let records = all_records(&reopened);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sequence, 1);
        assert_eq!(records[1].sequence, 2);
        assert_eq!(records[0].entry.details["channel"], "local");
        assert_eq!(records[0].entry.details["source_ip"], "203.0.113.9");
        assert_eq!(records[0].entry.details["status"], "200");
        assert_eq!(records[0].entry.details["outcome"], "invalid_code");
        let content = std::fs::read_to_string(active_path)?;
        assert!(!content.contains("bootstrap-code"));
        assert!(!content.contains("recovery-code"));
        assert!(verify_audit_chain(&records).is_ok());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(reopened.active_path())?
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn independent_ledger_handles_share_one_chain_without_lifetime_lock() -> Result<()> {
        let dir = temp_dir("shared-handles");
        let (webui, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_500);
        let (cli, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_500);

        record(&webui, "/webui/start", "success")?;
        record(&cli, "/cli/status", "success")?;
        record(&webui, "/webui/owner", "success")?;

        assert_eq!(webui.health().record_count, 3);
        assert_eq!(cli.health().record_count, 3);
        let bundle = cli.export(&AuditExportFilter::default())?;
        assert_eq!(
            bundle
                .records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(bundle.records[1].entry.resource, "/cli/status");

        drop(webui);
        drop(cli);
        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn stale_checkpoint_recovery_is_not_reported_as_integrity_failure() -> Result<()> {
        let dir = temp_dir("checkpoint-recovery");
        let (writer, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_550);
        let (reader, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_550);
        record(&writer, "/one", "success")?;
        write_checkpoint(&dir, &AuditChainCheckpoint::default())?;

        let health = reader.health();
        assert_eq!(health.checkpoint_sequence, 1);
        assert!(health
            .warnings
            .iter()
            .any(|warning| warning.code == "audit_checkpoint_recovered"));
        assert!(!health
            .warnings
            .iter()
            .any(|warning| warning.code == "audit_integrity_failure"));

        drop(writer);
        drop(reader);
        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn parallel_ledger_handles_append_an_unbroken_chain() -> Result<()> {
        let dir = temp_dir("parallel-handles");
        let (webui, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_600);
        let (cli, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_600);
        let webui = Arc::new(webui);
        let cli = Arc::new(cli);

        let webui_writer = {
            let ledger = webui.clone();
            std::thread::spawn(move || -> Result<()> {
                for index in 0..8 {
                    record(&ledger, &format!("/webui/{index}"), "success")?;
                }
                Ok(())
            })
        };
        let cli_writer = {
            let ledger = cli.clone();
            std::thread::spawn(move || -> Result<()> {
                for index in 0..8 {
                    record(&ledger, &format!("/cli/{index}"), "success")?;
                }
                Ok(())
            })
        };
        webui_writer.join().expect("webui audit writer")?;
        cli_writer.join().expect("cli audit writer")?;

        let records = read_all_records(&dir)?;
        assert_eq!(records.len(), 16);
        assert_eq!(
            records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            (1..=16).collect::<Vec<_>>()
        );
        assert!(verify_audit_chain(&records).is_ok());

        drop(webui);
        drop(cli);
        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn checkpoint_detects_deleted_tail_and_content_tampering() -> Result<()> {
        let dir = temp_dir("tamper");
        let (ledger, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 1_000);
        record(&ledger, "/one", "success")?;
        record(&ledger, "/two", "success")?;
        drop(ledger);

        let active = ledger_segment_path(&dir, 0);
        let content = std::fs::read_to_string(&active)?;
        std::fs::write(&active, content.lines().next().unwrap().to_string() + "\n")?;

        let error = OwnerSecurityAudit::new_with_policy(
            &dir,
            NodeId::from_public_key(b"audit-node"),
            AuditRetentionPolicy::default(),
            Box::new(TestClock(Arc::new(AtomicU64::new(1_100)))),
        )
        .err()
        .expect("tail deletion must fail closed");
        assert!(format!("{error:#}").contains("checkpoint mismatch"));

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn rotation_keeps_global_order_and_verifies_after_reopen() -> Result<()> {
        let dir = temp_dir("rotation");
        let policy = AuditRetentionPolicy {
            minimum_retention_days: 30,
            segment_records: 2,
            max_segments: 4,
        };
        let (ledger, clock) = test_ledger(&dir, policy, 2_000);
        record(&ledger, "/one", "success")?;
        record(&ledger, "/two", "success")?;
        clock.store(2_001, Ordering::Relaxed);
        record(&ledger, "/three", "success")?;
        drop(ledger);

        let (reopened, _) = test_ledger(&dir, policy, 2_002);
        let records = all_records(&reopened);
        assert_eq!(
            records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(reopened
            .active_path()
            .ends_with("owner-security-audit.000001.jsonl"));
        let bundle = reopened.export(&AuditExportFilter::default())?;
        assert_eq!(bundle.records.len(), 3);
        assert_eq!(bundle.manifest.verification.last_sequence, Some(3));

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn capacity_exhaustion_never_silently_deletes_history() -> Result<()> {
        let dir = temp_dir("capacity");
        let policy = AuditRetentionPolicy {
            minimum_retention_days: 30,
            segment_records: 1,
            max_segments: 2,
        };
        let (ledger, clock) = test_ledger(&dir, policy, 3_000);
        record(&ledger, "/one", "success")?;
        clock.store(3_001, Ordering::Relaxed);
        record(&ledger, "/two", "success")?;
        clock.store(3_002, Ordering::Relaxed);
        let error = record(&ledger, "/three", "success").expect_err("capacity must fail closed");
        assert!(format!("{error:#}").contains("容量已用尽"));
        assert_eq!(all_records(&ledger).len(), 2);
        assert!(!ledger.health().healthy);

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn disk_full_and_write_failure_raise_health_alerts() -> Result<()> {
        let dir = temp_dir("disk-full");
        let (ledger, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 4_000);
        ledger.replace_writer(Box::new(FullWriter));
        let error = record(&ledger, "/full", "success").expect_err("disk full must fail");
        assert!(format!("{error:#}").contains("磁盘空间不足"));
        let health = ledger.health();
        assert!(!health.healthy);
        assert_eq!(health.warnings.last().unwrap().code, "audit_storage_full");

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn clock_regression_is_detected_before_append() -> Result<()> {
        let dir = temp_dir("clock");
        let (ledger, clock) = test_ledger(&dir, AuditRetentionPolicy::default(), 10 * 60 * 1000);
        record(&ledger, "/one", "success")?;
        clock.store(1_000, Ordering::Relaxed);
        let error = record(&ledger, "/two", "success").expect_err("clock drift must fail");
        assert!(format!("{error:#}").contains("时钟回退"));
        assert_eq!(all_records(&ledger).len(), 1);
        assert!(!ledger.health().healthy);

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn export_applies_time_object_scope_and_redacts_sensitive_fields() -> Result<()> {
        let dir = temp_dir("export");
        let (ledger, clock) = test_ledger(&dir, AuditRetentionPolicy::default(), 5_000);
        record(&ledger, "/one", "success")?;
        clock.store(6_000, Ordering::Relaxed);
        record(&ledger, "/two", "success")?;
        let filter = AuditExportFilter {
            from_unix_ms: Some(6_000),
            to_unix_ms: Some(6_000),
            object_kind: Some("owner_security_endpoint".into()),
            object_id: Some("/two".into()),
            ..AuditExportFilter::default()
        };
        let bundle = ledger.export(&filter)?;
        assert_eq!(bundle.records.len(), 1);
        assert_eq!(bundle.records[0].entry.resource, "/two");
        assert!(bundle
            .records
            .iter()
            .all(|record| !record.entry.details.contains_key("token")));

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn mixed_request_and_command_entries_share_chain_and_verify_after_reopen() -> Result<()> {
        let dir = temp_dir("mixed-chain");
        let (ledger, clock) = test_ledger(&dir, AuditRetentionPolicy::default(), 5_500);
        record(&ledger, "/owner-endpoint", "success")?;

        let mut command_entry = security_audit_entry(
            &ledger.node_id,
            999,
            1,
            "command",
            None,
            "COMMAND",
            "StartNode",
            None,
            "success",
        )?;
        command_entry.audit_id = AuditId::from_public_key(b"mixed-command-entry");
        command_entry.action = "node.start".into();
        command_entry.resource = "node_command:start_node".into();
        let appended = ledger.append_entry(command_entry)?;
        assert_eq!(appended.sequence, 2);
        assert_eq!(appended.previous_hash, all_records(&ledger)[0].entry_hash);
        assert_eq!(appended.entry.occurred_at_unix_ms, 5_500);

        clock.store(5_501, Ordering::Relaxed);
        record(&ledger, "/owner-endpoint-two", "success")?;
        drop(ledger);

        let (reopened, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 5_600);
        let bundle = reopened.export(&AuditExportFilter::default())?;
        assert_eq!(
            bundle
                .records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(bundle.records[1].entry.action, "node.start");
        assert_eq!(bundle.manifest.verification.record_count, 3);
        assert!(verify_audit_chain(&bundle.records).is_ok());

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn legacy_audit_migrates_with_global_sequence_and_remains_verifiable() -> Result<()> {
        let dir = temp_dir("migration");
        let legacy = [
            r#"{"schema_version":1,"at_unix_ms":100,"channel":"public","source_ip":"203.0.113.1","method":"POST","path":"/api/v1/auth/bootstrap/verify","status":401,"outcome":"invalid_code"}"#,
            r#"{"schema_version":1,"at_unix_ms":200,"channel":"local","source_ip":null,"method":"GET","path":"/api/v1/auth/bootstrap","status":200,"outcome":"success"}"#,
        ]
        .join("\n");
        std::fs::write(dir.join(LEGACY_AUDIT_FILE), legacy + "\n")?;

        let (ledger, _) = test_ledger(&dir, AuditRetentionPolicy::default(), 300);
        let records = all_records(&ledger);
        assert_eq!(
            records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(records[0].entry.occurred_at_unix_ms, 100);
        assert_eq!(records[1].entry.resource, "/api/v1/auth/bootstrap");
        assert!(dir.join(MIGRATED_AUDIT_FILE).exists());
        let health = ledger.health();
        assert_eq!(health.checkpoint_sequence, 2);
        assert_eq!(health.record_count, 2);

        #[cfg(unix)]
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn audit_rejects_a_symlink_without_touching_its_target() -> Result<()> {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("symlink");
        let target = dir.join("unrelated.txt");
        std::fs::write(&target, b"must-not-change")?;
        symlink(&target, dir.join(LOCK_FILE))?;
        let error = OwnerSecurityAudit::new_with_policy(
            &dir,
            NodeId::from_public_key(b"audit-node"),
            AuditRetentionPolicy::default(),
            Box::new(TestClock(Arc::new(AtomicU64::new(1_000)))),
        )
        .err()
        .expect("symlink audit path must fail closed");
        assert!(format!("{error:#}").contains("符号链接"));
        assert_eq!(std::fs::read(&target)?, b"must-not-change");

        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn checkpoint_export_type_remains_serializable() {
        let checkpoint = AuditChainCheckpoint::default();
        let json = serde_json::to_value(checkpoint).expect("checkpoint JSON");
        assert_eq!(json["sequence"], 0);
        let verification = AuditChainVerification {
            first_sequence: None,
            last_sequence: None,
            last_hash: AUDIT_GENESIS_HASH.to_string(),
            record_count: 0,
        };
        assert_eq!(verification.record_count, 0);
    }
}
