use rivium_test::TempDir;

#[test]
fn atomic_replacement_preserves_old_data_and_backup() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("config");
    rivium_util::fs::atomic_write(&path, b"old", false).unwrap();
    rivium_util::fs::atomic_write(&path, b"new", true).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"new");
    assert_eq!(std::fs::read(path.with_extension("bak")).unwrap(), b"old");
    assert!(rivium_util::fs::atomic_write(temp.path(), b"bad", false).is_err());
}

#[cfg(unix)]
#[test]
fn new_config_and_backups_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("secret");
    rivium_util::fs::atomic_write(&path, b"old", false).unwrap();
    rivium_util::fs::atomic_write(&path, b"new", true).unwrap();
    for file in [&path, &path.with_extension("bak")] {
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn replacement_failure_is_explicitly_uncommitted() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let error = rivium_util::fs::atomic_write(&target, b"new", false).unwrap_err();
    assert!(!error.committed());
    assert_eq!(error.stage(), rivium_util::fs::AtomicWriteStage::Replace);
    assert!(target.is_dir());
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn backup_path_must_differ_from_destination() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join("already.bak");
    std::fs::write(&target, b"old").unwrap();
    let error = rivium_util::fs::atomic_write(&target, b"new", true).unwrap_err();
    assert!(!error.committed());
    assert_eq!(error.stage(), rivium_util::fs::AtomicWriteStage::Backup);
    assert_eq!(std::fs::read(target).unwrap(), b"old");
}

#[cfg(unix)]
#[test]
fn a_directory_sync_failure_reports_already_committed_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let directory = temp.path().join("restricted");
    std::fs::create_dir(&directory).unwrap();
    let target = directory.join("config");
    std::fs::write(&target, b"old").unwrap();

    struct Restore(std::path::PathBuf);

    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let _restore = Restore(directory.clone());
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o300)).unwrap();
    // A superuser can bypass mode checks and cannot exercise this OS failure.
    if std::fs::File::open(&directory).is_ok() {
        return;
    }
    let error = rivium_util::fs::atomic_write(&target, b"new", false).unwrap_err();
    assert!(error.committed());
    assert_eq!(
        error.stage(),
        rivium_util::fs::AtomicWriteStage::SyncDirectory
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
}
