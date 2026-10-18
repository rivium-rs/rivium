//! Atomic single-file replacement for trusted paths.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// The operation that failed during an atomic replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicWriteStage {
    /// Allocate a private temporary file beside the destination.
    CreateTemp,
    /// Write the new bytes.
    Write,
    /// Flush the new file to storage.
    SyncFile,
    /// Save the existing destination under its `.bak` extension.
    Backup,
    /// Replace the destination with the complete new file.
    Replace,
    /// Persist the directory entry after replacement (Unix only).
    SyncDirectory,
}

/// A replacement failure, including whether the destination already changed.
#[derive(Debug)]
pub struct AtomicWriteError {
    stage: AtomicWriteStage,
    committed: bool,
    source: io::Error,
}

impl AtomicWriteError {
    /// The operation that failed.
    pub fn stage(&self) -> AtomicWriteStage {
        self.stage
    }

    /// Whether the final rename succeeded before the failure.
    ///
    /// `true` means the caller must not assume the original data remains or
    /// blindly retry an associated business operation.
    pub fn committed(&self) -> bool {
        self.committed
    }

    /// The underlying filesystem error.
    pub fn io_error(&self) -> &io::Error {
        &self.source
    }

    fn at(stage: AtomicWriteStage, source: io::Error) -> Self {
        Self {
            stage,
            committed: stage == AtomicWriteStage::SyncDirectory,
            source,
        }
    }
}

impl fmt::Display for AtomicWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "atomic write failed at {:?} (committed={}): {}",
            self.stage, self.committed, self.source
        )
    }
}

impl std::error::Error for AtomicWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Write bytes via a private, synchronized temporary file in the same directory.
///
/// If requested, the previous contents are saved to `path.with_extension("bak")`
/// before replacing the destination. A successful backup does not imply a
/// successful destination replacement. On Unix the containing directory is
/// synchronized after rename; its failure is returned with `committed() == true`.
/// On other platforms no directory durability guarantee is made.
///
/// Paths must already be authorized by the caller. Parent directories must exist.
pub fn atomic_write(
    path: impl AsRef<Path>,
    bytes: &[u8],
    backup: bool,
) -> Result<(), AtomicWriteError> {
    #[cfg(test)]
    {
        atomic_write_impl(path.as_ref(), bytes, backup, |_| Ok(()))
    }
    #[cfg(not(test))]
    {
        atomic_write_impl(path.as_ref(), bytes, backup)
    }
}

fn atomic_write_impl(
    path: &Path,
    bytes: &[u8],
    backup: bool,
    #[cfg(test)] mut before: impl FnMut(AtomicWriteStage) -> io::Result<()>,
) -> Result<(), AtomicWriteError> {
    if backup && path.with_extension("bak") == path {
        return Err(AtomicWriteError::at(
            AtomicWriteStage::Backup,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "backup path equals destination",
            ),
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    #[cfg(test)]
    before(AtomicWriteStage::CreateTemp)
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::CreateTemp, e))?;
    let (pending, mut file) = temporary_file(parent)
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::CreateTemp, e))?;
    #[cfg(test)]
    before(AtomicWriteStage::Write)
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::Write, e))?;
    file.write_all(bytes)
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::Write, e))?;
    #[cfg(test)]
    before(AtomicWriteStage::SyncFile)
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::SyncFile, e))?;
    file.sync_all()
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::SyncFile, e))?;
    drop(file);

    if backup {
        #[cfg(test)]
        before(AtomicWriteStage::Backup)
            .map_err(|e| AtomicWriteError::at(AtomicWriteStage::Backup, e))?;
        match File::open(path) {
            Ok(mut previous) => {
                let (saved, mut destination) = temporary_file(parent)
                    .map_err(|e| AtomicWriteError::at(AtomicWriteStage::Backup, e))?;
                io::copy(&mut previous, &mut destination)
                    .and_then(|_| destination.sync_all())
                    .map_err(|e| AtomicWriteError::at(AtomicWriteStage::Backup, e))?;
                drop(destination);
                fs::rename(&saved.0, path.with_extension("bak"))
                    .map_err(|e| AtomicWriteError::at(AtomicWriteStage::Backup, e))?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(AtomicWriteError::at(AtomicWriteStage::Backup, error)),
        }
    }
    #[cfg(test)]
    before(AtomicWriteStage::Replace)
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::Replace, e))?;
    fs::rename(&pending.0, path).map_err(|e| AtomicWriteError::at(AtomicWriteStage::Replace, e))?;
    #[cfg(all(test, unix))]
    before(AtomicWriteStage::SyncDirectory)
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::SyncDirectory, e))?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| AtomicWriteError::at(AtomicWriteStage::SyncDirectory, e))?;
    Ok(())
}

struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn temporary_file(parent: &Path) -> io::Result<(Temporary, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..128 {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".rivium-{}-{timestamp}-{serial}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((Temporary(path), file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_failure(
        stage: AtomicWriteStage,
        committed: bool,
        target_bytes: &[u8],
        backup_bytes: &[u8],
    ) {
        let directory = rivium_test::TempDir::new().unwrap();
        let target = directory.path().join("config");
        let backup = target.with_extension("bak");
        fs::write(&target, b"old").unwrap();
        fs::write(&backup, b"earlier backup").unwrap();
        let error = atomic_write_impl(&target, b"new", true, |current| {
            if current == stage {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected filesystem failure",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.stage(), stage);
        assert_eq!(error.committed(), committed);
        assert_eq!(error.io_error().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&target).unwrap(), target_bytes);
        assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
        let mut names: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["config", "config.bak"],
            "temporary files survived the failed operation"
        );
    }

    #[test]
    fn create_failure_keeps_original_files() {
        assert_failure(
            AtomicWriteStage::CreateTemp,
            false,
            b"old",
            b"earlier backup",
        );
    }

    #[test]
    fn write_failure_removes_the_temporary_file() {
        assert_failure(AtomicWriteStage::Write, false, b"old", b"earlier backup");
    }

    #[test]
    fn file_sync_failure_does_not_replace_or_back_up_the_target() {
        assert_failure(AtomicWriteStage::SyncFile, false, b"old", b"earlier backup");
    }

    #[test]
    fn backup_failure_keeps_original_files() {
        assert_failure(AtomicWriteStage::Backup, false, b"old", b"earlier backup");
    }

    #[test]
    fn rename_failure_keeps_target_and_completed_backup() {
        assert_failure(AtomicWriteStage::Replace, false, b"old", b"old");
    }

    #[cfg(unix)]
    #[test]
    fn directory_sync_failure_preserves_the_committed_new_target() {
        assert_failure(AtomicWriteStage::SyncDirectory, true, b"new", b"old");
    }
}
