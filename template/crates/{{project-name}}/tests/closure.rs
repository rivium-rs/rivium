//! Each package compiles only what its form needs: the program no JNI{% unless http %}, nothing an HTTP stack{% endunless %},
//! and no package aws-lc, OpenSSL or OpenTelemetry. The closures are those of the platform that
//! the test is compiled for.

use std::path::Path;

use rivium_test::deps::assert_closure_excludes;

#[test]
fn each_package_compiles_only_what_its_form_needs() {
    let manifest = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    let mut banned = vec!["aws-lc*", "openssl*", "opentelemetry*"];
{%- unless http %}
    banned.extend(["axum", "hyper", "rivium-http"]);
{%- endunless %}{% if jni %}
    assert_closure_excludes(manifest, "{{project-name}}-jni", &banned);{% endif %}
    banned.extend(["jni", "rivium-jni"]);
    assert_closure_excludes(manifest, "{{project-name}}", &banned);
    assert_closure_excludes(manifest, "{{project-name}}-bin", &banned);
}
