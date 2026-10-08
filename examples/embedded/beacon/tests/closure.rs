//! The service library and the program compile neither JNI nor an HTTP stack: each package of
//! the example depends only on what its form needs.

use std::path::Path;

use rivium_test::deps::assert_closure_excludes;

#[test]
fn the_library_and_the_program_compile_no_jni_or_http() {
    let manifest = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let banned = ["jni", "rivium-jni", "axum", "hyper", "rivium-http"];
    assert_closure_excludes(manifest, "beacon", &banned);
    assert_closure_excludes(manifest, "beacon-bin", &banned);
}
