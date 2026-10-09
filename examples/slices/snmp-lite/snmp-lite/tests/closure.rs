//! Each package compiles only what its form needs (S-4): the service library and the program
//! no JNI and no HTTP stack, the JNI library no HTTP stack, and none of them aws-lc or OpenSSL.

use std::path::Path;

use rivium_test::deps::assert_closure_excludes;

#[test]
fn each_package_compiles_only_what_its_form_needs() {
    let manifest = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let banned = [
        "jni",
        "rivium-jni",
        "axum",
        "hyper",
        "rivium-http",
        "aws-lc*",
        "openssl*",
    ];
    assert_closure_excludes(manifest, "snmp-lite", &banned);
    assert_closure_excludes(manifest, "snmp-lite-bin", &banned);
    assert_closure_excludes(manifest, "snmp-lite-jni", &banned[2..]);
}
