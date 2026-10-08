//! Each package of the example depends only on what its form needs: the service library and the
//! program compile neither JNI nor an HTTP stack, the JNI library no HTTP stack.

use std::path::Path;

use rivium_test::deps::assert_closure_excludes;

#[test]
fn each_package_compiles_only_what_its_form_needs() {
    let manifest = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let banned = ["jni", "rivium-jni", "axum", "hyper", "rivium-http"];
    assert_closure_excludes(manifest, "beacon", &banned);
    assert_closure_excludes(manifest, "beacon-bin", &banned);
    assert_closure_excludes(manifest, "beacon-jni", &banned[2..]);
}
