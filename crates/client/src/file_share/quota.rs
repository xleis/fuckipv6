use fuckipv6_common::domain::{FileQuota, FileQuotaError, FileQuotaUsage, SharedPath};

/// Bounds one quota scan so a hostile or accidental directory tree cannot hold
/// the File capability indefinitely. `max_depth` counts nested directories
/// below the selected root; files directly in the root remain visible at zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaScanLimits {
    pub max_depth: usize,
    pub max_entries: u64,
}

impl Default for QuotaScanLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_entries: 100_000,
        }
    }
}

/// Local filesystem usage discovered without following symbolic links. Quota
/// checks combine this with member-specific accounting supplied by the caller;
/// the adapter cannot infer ownership from filesystem metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectoryUsage {
    pub total_bytes: u64,
    pub entries: u64,
    pub symlinks_skipped: u64,
    pub special_files_skipped: u64,
}

/// Request for the File capability's fail-closed upload admission check. The
/// caller supplies the member usage and active-transfer count from the
/// authenticated transfer manager rather than trusting client input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadQuotaRequest {
    pub path: SharedPath,
    pub upload_bytes: u64,
    pub replaced_bytes: u64,
    pub member_bytes: u64,
    pub concurrent_transfers: u32,
    pub scan_limits: QuotaScanLimits,
}

pub(crate) fn check_upload(
    quota: &FileQuota,
    usage: &DirectoryUsage,
    upload_bytes: u64,
    replaced_bytes: u64,
    member_bytes: u64,
    concurrent_transfers: u32,
) -> Result<FileQuotaUsage, FileQuotaError> {
    let quota_usage = FileQuotaUsage {
        total_bytes: usage.total_bytes,
        member_bytes,
        entries: usage.entries,
    };
    quota.check_upload(
        upload_bytes,
        replaced_bytes,
        &quota_usage,
        concurrent_transfers,
    )?;
    Ok(quota_usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_admission_includes_existing_and_replaced_bytes() {
        let quota = FileQuota {
            total_bytes: 1_000,
            per_member_bytes: 500,
            max_file_bytes: 400,
            max_entries: 10,
            reserve_bytes: 100,
            max_concurrent_transfers: 2,
            max_bandwidth_kbps: 0,
        };
        let usage = DirectoryUsage {
            total_bytes: 300,
            entries: 3,
            ..DirectoryUsage::default()
        };

        assert_eq!(
            check_upload(&quota, &usage, 500, 0, 100, 0),
            Err(FileQuotaError::FileTooLarge)
        );
        assert_eq!(
            check_upload(&quota, &usage, 200, 100, 100, 0),
            Ok(FileQuotaUsage {
                total_bytes: 300,
                member_bytes: 100,
                entries: 3,
            })
        );
        assert_eq!(
            check_upload(&quota, &usage, 200, 0, 100, 2),
            Err(FileQuotaError::ConcurrencyLimit)
        );
    }
}
