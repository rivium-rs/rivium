//! Consumer-side test tools. Add this crate only as a development dependency.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// An exclusively created temporary directory removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> io::Result<Self> {
        loop {
            let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("rivium-{}-{serial}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Shared in-memory output for small test fixtures, not a production logging sink.
#[derive(Clone)]
pub struct Logs {
    bytes: Arc<Mutex<Vec<u8>>>,
    // Keep tracing's multi-dispatch callsite registration active across threads.
    _callsites: tracing::Dispatch,
}

impl Default for Logs {
    fn default() -> Self {
        Self {
            bytes: Arc::default(),
            _callsites: tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
        }
    }
}

impl Logs {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }

    pub fn dispatch(&self) -> tracing::Dispatch {
        tracing::Dispatch::new(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(self.clone())
                .finish(),
        )
    }
}

impl Write for Logs {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.bytes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

pub fn idle(name: &str, kind: rivium::Kind) -> Box<dyn rivium::Service> {
    rivium::service(name, kind, |context| async move {
        context.ready();
        context.stopped().await;
        Ok(())
    })
}

pub async fn until_ready(state: &mut rivium::Readiness) -> rivium::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !state.is_ready() {
            if state.phase() == rivium::Phase::Stopped || state.changed().await.is_none() {
                return Err(rivium::Error::internal("service ended before readiness"));
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| rivium::Error::internal("readiness deadline exceeded"))?
}

/// Launch a target executable with Cargo's whitespace-separated runner arguments.
/// Windows children execute directly inside Wine. Runner selection uses the compiled
/// target, including when `cargo --target` overrides `CARGO_BUILD_TARGET`.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let target = env!("RIVIUM_HELPER_TARGET");
    let key = format!(
        "CARGO_TARGET_{}_RUNNER",
        target.to_uppercase().replace('-', "_")
    );
    let runner = if cfg!(windows) {
        None
    } else {
        std::env::var(key).ok()
    };
    command_with_runner(program.as_ref(), runner.as_deref())
}

fn command_with_runner(program: &std::ffi::OsStr, runner: Option<&str>) -> std::process::Command {
    if let Some(runner) = runner {
        let mut words = runner.split_whitespace();
        let mut command = std::process::Command::new(words.next().expect("target runner is empty"));
        command.args(words).arg(program);
        command
    } else {
        std::process::Command::new(program)
    }
}

#[cfg(test)]
mod contracts {
    use super::*;

    #[test]
    fn temporary_directories_are_unique_and_release_their_contents() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        assert_ne!(a.path(), b.path());
        let path = a.path().to_owned();
        fs::write(path.join("payload"), b"test").unwrap();
        drop(a);
        assert!(!path.exists());
        assert!(b.path().is_dir());
    }

    #[test]
    fn captured_logs_retain_fields_without_terminal_escapes() {
        let output = Logs::default();
        tracing::dispatcher::with_default(&output.dispatch(), || {
            tracing::info!(attempt = 3, "observed")
        });
        let text = output.text();
        assert!(text.contains("observed") && text.contains("attempt=3"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn target_commands_preserve_runner_prefix_and_reject_an_empty_runner() {
        let executable = std::ffi::OsStr::new("target/app with spaces");
        let command = command_with_runner(executable, Some("qemu-arm -L /sysroot"));
        assert_eq!(command.get_program(), "qemu-arm");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-L", "/sysroot", "target/app with spaces"]
        );
        assert_eq!(
            command_with_runner(executable, None).get_program(),
            executable
        );
        assert!(std::panic::catch_unwind(|| command_with_runner(executable, Some("  "))).is_err());
    }

    #[test]
    #[cfg(not(windows))]
    fn compiled_target_wins_over_runtime_build_target() {
        if std::env::var_os("RIVIUM_RUNNER_SELECTION_CHILD").is_some() {
            assert_eq!(
                command("target-child").get_program(),
                "actual-target-runner"
            );
            return;
        }

        let runner_key = format!(
            "CARGO_TARGET_{}_RUNNER",
            env!("RIVIUM_HELPER_TARGET")
                .to_uppercase()
                .replace('-', "_")
        );
        let output = command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "contracts::compiled_target_wins_over_runtime_build_target",
            ])
            .env("RIVIUM_RUNNER_SELECTION_CHILD", "1")
            .env("CARGO_BUILD_TARGET", "different-target")
            .env("CARGO_TARGET_DIFFERENT_TARGET_RUNNER", "wrong-runner")
            .env(runner_key, "actual-target-runner")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
