//! The service keeps the lifecycle contract of a program that runs embedded, as the Android app
//! runs it. A test binary of its own: a process installs logging once.

#[test]
fn the_service_keeps_the_embedded_lifecycle_contract() {
    let name = format!("{{project-name}}-contract-{}", std::process::id());
    let root = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // What the defaults need to run in a test, such as a free port in place of the deployment's.
    let args = [{% if http %}"--set", "http.addr=127.0.0.1:0"{% endif %}];
    rivium_test::embedded::lifecycle_contract::<{{crate_name}}::{{project-name | pascal_case}}>(&root, &args);
    let _ = std::fs::remove_dir_all(&root);
}
