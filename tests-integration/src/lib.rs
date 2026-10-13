//! Shared test utilities for process-level integration tests.

#![allow(unsafe_code)]

#[cfg(all(feature = "jvm", not(target_os = "android")))]
pub mod jvm;

use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Send a Unix signal to a child process, reporting a failed delivery.
#[cfg(unix)]
pub fn send_signal(pid: u32, sig: libc::c_int) -> std::io::Result<()> {
    // SAFETY: kill only inspects the supplied PID and signal; it does not access memory.
    if unsafe { libc::kill(pid as libc::pid_t, sig) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Get a prebuilt test-app binary; use underscores in place of hyphens in `name`.
pub fn test_binary(name: &str) -> PathBuf {
    // Cargo injects rustc-env at runtime only when it launches tests; standalone runners do not.
    // Every prebuilt test-app binary shares the target/profile directory of host.
    let binary = format!("{}{}", name.replace('_', "-"), env::consts::EXE_SUFFIX);
    let path = Path::new(env!("CARGO_BIN_EXE_host")).with_file_name(binary);
    assert!(
        path.is_file(),
        "{} is missing; run cargo build --package test-app --bins --locked with the same \
         toolchain, target, profile and target directory as the tests",
        path.display()
    );
    path
}

/// A test's isolated root directory, removed when dropped after its child process.
pub struct TestRoot(PathBuf);

/// Create an isolated root without reusing or deleting another test's files.
pub fn test_root() -> TestRoot {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("rivium-integration-{}-{n}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return TestRoot(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("cannot create {}: {error}", path.display()),
        }
    }
}

impl TestRoot {
    /// The directory containing this test's configuration and logs.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Read the application's main log after the process has exited and flushed it.
    pub fn log(&self, name: &str) -> String {
        let file = self.0.join(format!("logs/{name}/{name}.log"));
        std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()))
    }

    /// The command-line arguments selecting this root.
    pub fn args(&self) -> [&str; 2] {
        [
            "--root",
            self.0.to_str().expect("UTF-8 temporary directory"),
        ]
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_paths_work_without_cargo_environment() {
        if env::var_os("RIVIUM_TEST_BINARY_PROBE").is_some() {
            for name in ["host", "host_args"] {
                assert!(test_binary(name).is_file());
            }
            return;
        }

        let mut command = rivium_test::process::command(&env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "tests::binary_paths_work_without_cargo_environment",
            ])
            .env("RIVIUM_TEST_BINARY_PROBE", "1")
            .env_remove("CARGO_BIN_EXE_host")
            .env_remove("CARGO_BIN_EXE_host_args");
        let exited = rivium_test::process::Running::start(command).wait();
        assert_eq!(exited.code, Some(0), "{exited:?}");
    }

    #[test]
    fn roots_are_isolated_and_removed() {
        let first = test_root();
        let second = test_root();
        assert_ne!(first.0, second.0);
        let path = first.0.clone();
        std::fs::write(path.join("log"), "test").unwrap();
        drop(first);
        assert!(!path.exists());
        assert!(second.0.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn invalid_signal_is_reported() {
        assert!(send_signal(std::process::id(), -1).is_err());
    }
}
