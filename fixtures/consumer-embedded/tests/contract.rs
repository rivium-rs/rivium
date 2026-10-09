//! The embedded lifecycle contract, as a consumer checks it.

#[test]
fn the_service_keeps_the_embedded_lifecycle_contract() {
    let root = std::env::temp_dir().join(format!("consumer-embedded-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // The deployment's port is not for tests.
    let args = ["--set", "addr=127.0.0.1:0"];
    rivium_test::embedded::lifecycle_contract::<consumer_embedded::Consumer>(&root, &args);
    let _ = std::fs::remove_dir_all(&root);
}
