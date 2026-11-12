use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

use getrandom::fill;

use super::error::{FileShareError, FileShareErrorCode, FileShareResult};
use super::root::LocalShareRoot;

const TEMP_FILE_ATTEMPTS: usize = 8;
const TEMP_FILE_RANDOM_BYTES: usize = 16;

/// A private, hidden upload staging file. The file is not visible through the
/// normal share namespace and is removed automatically unless it is published.
#[derive(Debug)]
pub struct TemporaryUpload {
    root: LocalShareRoot,
    file: Option<File>,
    temporary_name: String,
}

impl TemporaryUpload {
    pub fn create(root: &LocalShareRoot) -> FileShareResult<Self> {
        for _ in 0..TEMP_FILE_ATTEMPTS {
            let temporary_name = random_temporary_name()?;
            match root.create_temporary_file(&temporary_name) {
                Ok(file) => {
                    return Ok(Self {
                        root: root.clone(),
                        file: Some(file),
                        temporary_name,
                    });
                }
                Err(error) if error.code() == FileShareErrorCode::TargetExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(FileShareError::new(
            FileShareErrorCode::TempUnavailable,
            "temporary upload name collision limit was reached",
        ))
    }

    pub fn file(&self) -> &File {
        self.file
            .as_ref()
            .expect("temporary upload file is present")
    }

    pub fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("temporary upload file is present")
    }

    pub fn temporary_path(&self) -> PathBuf {
        self.root.temporary_file_path(&self.temporary_name)
    }

    /// Publish the complete upload only if the target name still does not exist.
    pub fn persist_new(mut self, target: &SharedPath) -> FileShareResult<()> {
        let file = self
            .file
            .as_mut()
            .expect("temporary upload file is present");
        file.flush().map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "temporary upload could not be flushed",
                error,
            )
        })?;
        file.sync_all().map_err(|error| {
            FileShareError::io(
                FileShareErrorCode::Io,
                "temporary upload could not be synchronized",
                error,
            )
        })?;
        self.file.take();
        self.root
            .link_temporary_file_new(&self.temporary_name, target)?;
        self.temporary_name.clear();
        Ok(())
    }
}

impl Write for TemporaryUpload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file_mut().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file_mut().flush()
    }
}

impl Drop for TemporaryUpload {
    fn drop(&mut self) {
        if !self.temporary_name.is_empty() {
            let _ = self.root.remove_temporary_file(&self.temporary_name);
        }
    }
}

fn random_temporary_name() -> FileShareResult<String> {
    let mut bytes = [0_u8; TEMP_FILE_RANDOM_BYTES];
    fill(&mut bytes).map_err(|error| {
        FileShareError::new(
            FileShareErrorCode::TempUnavailable,
            format!("temporary upload randomness is unavailable: {error}"),
        )
    })?;
    let mut name = String::with_capacity(2 + bytes.len() * 2);
    name.push_str("up-");
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut name, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(name)
}

use fuckipv6_common::domain::SharedPath;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read;

    use fuckipv6_common::domain::{SharedPath, SymlinkPolicy};

    use super::*;

    fn root() -> (tempfile::TempDir, LocalShareRoot) {
        let temporary = tempfile::tempdir().expect("tempdir");
        let selected = LocalShareRoot::select(
            temporary.path().to_str().expect("utf8 temp path"),
            SymlinkPolicy::Reject,
        )
        .expect("share root");
        (temporary, selected)
    }

    #[test]
    fn temporary_upload_is_published_only_after_completion() {
        let (temporary, share) = root();
        let target = SharedPath::parse("result.txt").expect("path");
        let mut upload = TemporaryUpload::create(&share).expect("upload");
        upload.write_all(b"complete").expect("write");
        assert!(!temporary.path().join("result.txt").exists());

        upload.persist_new(&target).expect("persist");
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("result.txt")).expect("read"),
            "complete"
        );
    }

    #[test]
    fn failed_or_dropped_upload_is_removed() {
        let (_temporary, share) = root();
        let mut upload = TemporaryUpload::create(&share).expect("upload");
        upload.write_all(b"partial").expect("write");
        let path = upload.temporary_path();
        assert!(path.exists());
        drop(upload);
        assert!(!path.exists());
    }

    #[test]
    fn completed_upload_never_replaces_an_existing_file() {
        let (temporary, share) = root();
        std::fs::write(temporary.path().join("result.txt"), b"original").expect("existing");
        let target = SharedPath::parse("result.txt").expect("path");
        let mut upload = TemporaryUpload::create(&share).expect("upload");
        upload.write_all(b"replacement").expect("write");

        let error = upload
            .persist_new(&target)
            .expect_err("existing target must fail");
        assert_eq!(error.code(), FileShareErrorCode::TargetExists);
        assert_eq!(
            std::fs::read_to_string(temporary.path().join("result.txt")).expect("read"),
            "original"
        );
    }

    #[cfg(unix)]
    #[test]
    fn publication_rejects_a_target_parent_swapped_to_an_outside_symlink() {
        let (temporary, share) = root();
        let target_parent = temporary.path().join("incoming");
        fs::create_dir(&target_parent).expect("target parent");
        let parked_parent = temporary.path().join("parked");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("result.txt"), b"outside-original").expect("outside file");

        let target = SharedPath::parse("incoming/result.txt").expect("path");
        let mut upload = TemporaryUpload::create(&share).expect("upload");
        upload.write_all(b"replacement").expect("write");

        fs::rename(&target_parent, &parked_parent).expect("move inside parent");
        std::os::unix::fs::symlink(outside.path(), &target_parent).expect("outside symlink");
        let error = upload
            .persist_new(&target)
            .expect_err("swapped target parent must fail closed");
        assert_eq!(error.code(), FileShareErrorCode::SymlinkRejected);
        assert_eq!(
            fs::read(outside.path().join("result.txt")).expect("outside file remains"),
            b"outside-original"
        );
        assert!(!parked_parent.join("result.txt").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn concurrent_directory_exchange_never_publishes_outside_root() {
        use std::ffi::CString;
        use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};
        use std::thread;

        let (temporary, share) = root();
        let pivot = temporary.path().join("pivot");
        let alternate = temporary.path().join("alternate");
        let outside = tempfile::tempdir().expect("outside");
        fs::create_dir(&pivot).expect("pivot directory");
        fs::write(outside.path().join("existing.txt"), b"outside-original").expect("outside file");
        std::os::unix::fs::symlink(outside.path(), &alternate).expect("outside symlink");

        let pivot_c = CString::new(pivot.to_str().expect("utf8 path")).expect("path without NUL");
        let alternate_c =
            CString::new(alternate.to_str().expect("utf8 path")).expect("path without NUL");
        let stop = Arc::new(AtomicBool::new(false));
        let swaps = Arc::new(AtomicUsize::new(0));
        let rename_error = Arc::new(AtomicI32::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let attacker = {
            let stop = stop.clone();
            let swaps = swaps.clone();
            let rename_error = rename_error.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                while !stop.load(Ordering::Relaxed) {
                    let result = unsafe {
                        libc::renameat2(
                            libc::AT_FDCWD,
                            pivot_c.as_ptr(),
                            libc::AT_FDCWD,
                            alternate_c.as_ptr(),
                            libc::RENAME_EXCHANGE,
                        )
                    };
                    if result != 0 {
                        rename_error.store(
                            std::io::Error::last_os_error().raw_os_error().unwrap_or(-1),
                            Ordering::Relaxed,
                        );
                        break;
                    }
                    swaps.fetch_add(1, Ordering::Relaxed);
                    thread::yield_now();
                }
            })
        };

        barrier.wait();
        let mut published = Vec::new();
        for index in 0..256 {
            let target_name = format!("pivot/race-{index}.txt");
            let target = SharedPath::parse(&target_name).expect("target path");
            let mut upload = TemporaryUpload::create(&share).expect("upload");
            upload.write_all(b"inside-publish").expect("write");
            match upload.persist_new(&target) {
                Ok(()) => published.push(format!("race-{index}.txt")),
                Err(error) => assert!(matches!(
                    error.code(),
                    FileShareErrorCode::SymlinkRejected
                        | FileShareErrorCode::ParentUnavailable
                        | FileShareErrorCode::Io
                )),
            }
            thread::yield_now();
        }
        stop.store(true, Ordering::Relaxed);
        attacker.join().expect("attacker thread");

        assert_eq!(
            rename_error.load(Ordering::Relaxed),
            0,
            "rename exchange must be supported"
        );
        assert!(swaps.load(Ordering::Relaxed) > 0);
        assert!(!published.is_empty());
        assert_eq!(
            fs::read(outside.path().join("existing.txt")).expect("outside file remains"),
            b"outside-original"
        );
        assert_eq!(
            fs::read_dir(outside.path())
                .expect("outside directory")
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>(),
            vec![std::ffi::OsString::from("existing.txt")]
        );

        let inside = if fs::symlink_metadata(&pivot)
            .expect("pivot metadata")
            .is_dir()
        {
            pivot
        } else {
            alternate
        };
        for file_name in published {
            assert_eq!(
                fs::read(inside.join(file_name)).expect("published file"),
                b"inside-publish"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn temporary_directory_and_file_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let (temporary, share) = root();
        let mut upload = TemporaryUpload::create(&share).expect("upload");
        upload.write_all(b"private").expect("write");
        let directory = temporary
            .path()
            .join(super::super::root::FILE_TEMP_DIRECTORY);
        let directory_mode = std::fs::metadata(&directory)
            .expect("directory")
            .permissions()
            .mode()
            & 0o777;
        let file_mode = std::fs::metadata(upload.temporary_path())
            .expect("file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(file_mode, 0o600);

        let mut file = std::fs::File::open(upload.temporary_path()).expect("open");
        let mut content = String::new();
        file.read_to_string(&mut content).expect("read");
        assert_eq!(content, "private");
    }
}
