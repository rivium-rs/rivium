//! Shared paths and locked Cargo invocations for repository checks.

use std::path::PathBuf;
use std::process::Command;

/// The platform whose dependency graph these checks validate.
pub const TARGET: &str = env!("TESTS_BUILD_TARGET");

/// The repository workspace, independent of the test's working directory.
pub fn workspace_root() -> PathBuf {
    // A cached test binary may have been compiled in a different checkout.
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("run repository checks through Cargo or nextest"),
    );
    assert!(
        manifest.is_absolute(),
        "Cargo must supply an absolute manifest directory"
    );
    manifest.parent().expect("workspace parent").to_owned()
}

/// A Cargo command using the calling toolchain and the existing workspace lockfile.
pub fn cargo(subcommand: &str) -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .arg(subcommand)
        .arg("--locked")
        .current_dir(workspace_root())
        .env("CARGO_TERM_COLOR", "never");
    command
}
