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
    pub member_concurrent_transfers: u32,
    pub directory_concurrent_transfers: u32,
    pub scan_limits: QuotaScanLimits,
}

/// Filesystem usage and authenticated admission counts for one upload. This
/// keeps the quota boundary small enough to audit as a single value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UploadQuotaCheck<'a> {
    pub usage: &'a DirectoryUsage,
    pub upload_bytes: u64,
    pub replaced_bytes: u64,
    pub member_bytes: u64,
    pub concurrent_transfers: u32,
    pub member_concurrent_transfers: u32,
    pub directory_concurrent_transfers: u32,
    pub available_space: u64,
}

pub(crate) fn check_upload(
    quota: &FileQuota,
    check: UploadQuotaCheck<'_>,
) -> Result<FileQuotaUsage, FileQuotaError> {
    let quota_usage = FileQuotaUsage {
        total_bytes: check.usage.total_bytes,
        member_bytes: check.member_bytes,
        entries: check.usage.entries,
    };
    quota.check_upload(
        check.upload_bytes,
        check.replaced_bytes,
        &quota_usage,
        check.concurrent_transfers,
        check.member_concurrent_transfers,
        check.directory_concurrent_transfers,
    )?;
    let required_disk_space = check
        .upload_bytes
        .checked_add(quota.reserve_bytes)
        .ok_or(FileQuotaError::DiskSpaceReserved)?;
    if check.available_space < required_disk_space {
        return Err(FileQuotaError::DiskSpaceReserved);
    }
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
            max_concurrent_transfers_per_member: 2,
            max_concurrent_transfers_per_directory: 2,
            max_bandwidth_kbps: 0,
        };
        let usage = DirectoryUsage {
            total_bytes: 300,
            entries: 3,
            ..DirectoryUsage::default()
        };

        assert_eq!(
            check_upload(
                &quota,
                UploadQuotaCheck {
                    usage: &usage,
                    upload_bytes: 500,
                    replaced_bytes: 0,
                    member_bytes: 100,
                    concurrent_transfers: 0,
                    member_concurrent_transfers: 0,
                    directory_concurrent_transfers: 0,
                    available_space: u64::MAX,
                }
            ),
            Err(FileQuotaError::FileTooLarge)
        );
        assert_eq!(
            check_upload(
                &quota,
                UploadQuotaCheck {
                    usage: &usage,
                    upload_bytes: 200,
                    replaced_bytes: 100,
                    member_bytes: 100,
                    concurrent_transfers: 0,
                    member_concurrent_transfers: 0,
                    directory_concurrent_transfers: 0,
                    available_space: u64::MAX,
                }
            ),
            Ok(FileQuotaUsage {
                total_bytes: 300,
                member_bytes: 100,
                entries: 3,
            })
        );
        assert_eq!(
            check_upload(
                &quota,
                UploadQuotaCheck {
                    usage: &usage,
                    upload_bytes: 200,
                    replaced_bytes: 0,
                    member_bytes: 100,
                    concurrent_transfers: 2,
                    member_concurrent_transfers: 0,
                    directory_concurrent_transfers: 0,
                    available_space: u64::MAX,
                }
            ),
            Err(FileQuotaError::ConcurrencyLimit)
        );
        assert_eq!(
            check_upload(
                &quota,
                UploadQuotaCheck {
                    usage: &usage,
                    upload_bytes: 200,
                    replaced_bytes: 0,
                    member_bytes: 100,
                    concurrent_transfers: 0,
                    member_concurrent_transfers: 2,
                    directory_concurrent_transfers: 0,
                    available_space: u64::MAX,
                }
            ),
            Err(FileQuotaError::MemberConcurrencyLimit)
        );
        assert_eq!(
            check_upload(
                &quota,
                UploadQuotaCheck {
                    usage: &usage,
                    upload_bytes: 200,
                    replaced_bytes: 0,
                    member_bytes: 100,
                    concurrent_transfers: 0,
                    member_concurrent_transfers: 0,
                    directory_concurrent_transfers: 2,
                    available_space: u64::MAX,
                }
            ),
            Err(FileQuotaError::DirectoryConcurrencyLimit)
        );
        assert_eq!(
            check_upload(
                &quota,
                UploadQuotaCheck {
                    usage: &usage,
                    upload_bytes: 200,
                    replaced_bytes: 0,
                    member_bytes: 100,
                    concurrent_transfers: 0,
                    member_concurrent_transfers: 0,
                    directory_concurrent_transfers: 0,
                    available_space: 299,
                }
            ),
            Err(FileQuotaError::DiskSpaceReserved)
        );
    }
}
