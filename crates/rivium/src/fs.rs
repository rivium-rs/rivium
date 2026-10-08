//! Files written so that a failure or a crash never leaves them missing or half written.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Replaces the file at `path` with `bytes`, atomically.
///
/// With `backup`, an existing file is first copied to `<path>.bak`. The bytes go to a temporary
/// file in the same directory, which is synced, given the permissions of the file it replaces,
/// and renamed over `path`; on Unix the directory is synced as well. If any step fails, the
/// temporary file is removed and `path` is left as it was.
///
/// ```no_run
/// rivium::fs::atomic_write("configs/default.toml".as_ref(), b"[log]\nfilter = \"debug\"\n", true)?;
/// # Ok::<(), std::io::Error>(())
/// ```
///
/// # Errors
///
/// When the backup, the temporary file, the rename or the directory sync fails.
pub fn atomic_write(path: &Path, bytes: &[u8], backup: bool) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("the path names no file"))?;
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let existing = fs::metadata(path).ok();
    if backup && existing.is_some() {
        let mut copy = path.as_os_str().to_owned();
        copy.push(".bak");
        fs::copy(path, copy)?;
    }
    let (tmp, file) = temporary(dir, &name.to_string_lossy())?;
    let written = (|| {
        let mut file = file;
        file.write_all(bytes)?;
        if let Some(existing) = &existing {
            file.set_permissions(existing.permissions())?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    sync_dir(dir)
}

/// A new temporary file next to `name`, created without replacing anything.
fn temporary(dir: &Path, name: &str) -> io::Result<(std::path::PathBuf, File)> {
    for attempt in 0.. {
        let tmp = dir.join(format!(".{name}.{}.{attempt}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!("an attempt counter without end")
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::atomic_write;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rivium-fs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &PathBuf) -> Vec<String> {
        let mut names: Vec<String> = (fs::read_dir(dir).unwrap())
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_new_files_and_replaces_old_ones_with_a_backup() {
        let dir = dir("replace");
        let path = dir.join("config.toml");
        atomic_write(&path, b"one", true).unwrap();
        assert_eq!(
            (fs::read(&path).unwrap(), names(&dir)),
            (b"one".to_vec(), vec!["config.toml".to_string()])
        );
        atomic_write(&path, b"two", false).unwrap();
        atomic_write(&path, b"three", true).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"three");
        assert_eq!(fs::read(dir.join("config.toml.bak")).unwrap(), b"two");
        assert_eq!(names(&dir), ["config.toml", "config.toml.bak"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_permissions_of_the_file_it_replaces() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir("mode");
        let path = dir.join("secret.toml");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        atomic_write(&path, b"new", false).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_write_leaves_the_target_and_no_temporary_file() {
        let dir = dir("fail");
        // A directory cannot be replaced by a file: the rename fails.
        let target = dir.join("taken");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside"), b"kept").unwrap();
        assert!(atomic_write(&target, b"new", false).is_err());
        assert_eq!(fs::read(target.join("inside")).unwrap(), b"kept");
        assert_eq!(names(&dir), ["taken"]);
        assert!(atomic_write(&dir.join("missing/dir/file"), b"x", true).is_err());
        assert!(atomic_write(&dir, b"x", false).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
