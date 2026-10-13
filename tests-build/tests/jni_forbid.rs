//! The JNI export compiles in an unsafe-forbidden consumer; aborting panics are rejected.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

struct Guarded;

impl rivium::App for Guarded {
    const NAME: &'static str = "jni-guard";
    const VERSION: &'static str = "0.0.0";
    type Config = BTreeMap<String, String>;

    fn services(
        _: &Self::Config,
        _: &rivium::AppContext,
    ) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
        Ok(Vec::new())
    }
}

rivium_jni::export!(class = "rivium/tests/GuardedBridge", app = Guarded);

#[test]
fn export_compiles_with_unsafe_forbidden() {
    // This is a compile guard, not a JVM invocation or an unsafe call from the consumer.
    let _entry_point = JNI_OnLoad;
}

#[test]
fn aborting_panics_are_rejected_for_the_documented_reason() {
    let executable = std::env::current_exe().expect("test executable");
    let target = executable
        .parent()
        .expect("test directory")
        .join("jni-abort");
    let output = tests_build::cargo("check")
        .args([
            "--package",
            "rivium-jni",
            "--lib",
            "--target",
            tests_build::TARGET,
            "--profile",
            "dev",
        ])
        .env("CARGO_PROFILE_DEV_PANIC", "abort")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        // Do not reuse or lock the outer test run's build directory.
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("check the JNI library with aborting panics");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "JNI must reject panic=abort");
    assert!(
        stderr.contains("rivium-jni needs panic = \"unwind\""),
        "failed for an unrelated reason:\n{stderr}"
    );
}
