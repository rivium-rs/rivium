//! A consumer that needs neither HTTP nor JNI does not compile them, and the library crates keep
//! to their dependency rules: rivium compiles no C code and rivium-error depends on the tracing
//! facade only. The closures are computed for the target platform this test is compiled for, so
//! every test job checks its own target.

use std::path::Path;

use rivium_test::deps::assert_closure_excludes;

fn manifest() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
}

#[test]
fn a_minimal_consumer_compiles_no_http_jni_or_telemetry_stack() {
    let banned = [
        "axum",
        "hyper",
        "tower-http",
        "jni",
        "opentelemetry*",
        "aws-lc*",
        "openssl*",
        "rivium-http",
        "rivium-jni",
        "rivium-test",
    ];
    assert_closure_excludes(manifest(), "minimal", &banned);
}

#[test]
fn rivium_compiles_no_c_code() {
    let banned = [
        "cc",
        "cmake",
        "aws-lc*",
        "openssl*",
        "libz-sys",
        "libz-ng-sys",
    ];
    assert_closure_excludes(manifest(), "rivium", &banned);
}

#[test]
fn rivium_error_stays_light() {
    let banned = [
        "tokio",
        "serde",
        "axum",
        "hyper",
        "tracing-subscriber",
        "jni",
    ];
    assert_closure_excludes(manifest(), "rivium-error", &banned);
}
