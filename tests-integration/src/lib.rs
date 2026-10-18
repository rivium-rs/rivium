//! Workspace behavior contracts, including target-aware child process launching.

pub fn command() -> std::process::Command {
    let exe = std::env::current_exe().expect("test executable");
    let bin = exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) {
            "test-host.exe"
        } else {
            "test-host"
        });
    assert!(
        bin.is_file(),
        "build tests-integration/test-host for the same target and profile first: {}",
        bin.display()
    );
    let mut cmd = rivium_test::command(bin);
    cmd.env_remove("RUST_LOG");
    cmd
}
