//! Workspace behavior contracts, including target-aware child process launching.

pub fn command(bin: &str) -> std::process::Command {
    assert!(
        std::path::Path::new(bin).is_file(),
        "build tests-integration/test-host for the same target and profile first: {bin}"
    );
    let mut cmd = rivium_test::command(bin);
    cmd.env_remove("RUST_LOG");
    cmd
}
