//! The service library and the program compile no JNI, and neither aws-lc nor OpenSSL: the HTTP
//! stack is rustls-free here, and TLS, where a service adds it, uses rustls with ring.

use std::path::Path;

use rivium_test::deps::assert_closure_excludes;

#[test]
fn each_package_compiles_only_what_its_form_needs() {
    let manifest = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let banned = ["jni", "rivium-jni", "aws-lc*", "openssl*", "opentelemetry*"];
    assert_closure_excludes(manifest, "edge-lite", &banned);
    assert_closure_excludes(manifest, "edge-lite-bin", &banned);
}
