//! Shared test utilities for process-level integration tests.

#![allow(unsafe_code)]

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::time::Duration;

#[cfg(unix)]
fn send_signal(pid: u32, sig: libc::c_int) {
    // SAFETY: kill() is called with a valid process ID and signal number.
    // The process may have already exited, in which case kill returns -1.
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

/// Get the path to a test-app binary by name.
///
/// The name should match a [[bin]] target in test-app/Cargo.toml,
/// with hyphens replaced by underscores (e.g., "host_sigint" for "host-sigint").
pub fn test_binary(name: &str) -> PathBuf {
    let env_name = format!("CARGO_BIN_EXE_{}", name);
    PathBuf::from(
        env::var(&env_name)
            .unwrap_or_else(|_| panic!("{} not set. Did you run with cargo test?", env_name)),
    )
}

/// Spawn a test binary and return a handle for interaction.
pub struct TestProcess {
    child: std::process::Child,
}

impl TestProcess {
    /// Spawn a test binary with optional arguments.
    pub fn spawn(binary_name: &str, args: &[&str]) -> std::io::Result<Self> {
        let path = test_binary(binary_name);
        let child = Command::new(path).args(args).spawn()?;
        Ok(Self { child })
    }

    /// Send a signal to the process (Unix only).
    #[cfg(unix)]
    pub fn signal(&mut self, sig: libc::c_int) -> std::io::Result<()> {
        send_signal(self.child.id(), sig);
        Ok(())
    }

    /// Wait for the process to exit with a timeout.
    pub fn wait_timeout(&mut self, timeout: Duration) -> std::io::Result<Option<ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            match self.child.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None if start.elapsed() > timeout => return Ok(None),
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    /// Kill the process.
    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
