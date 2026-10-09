//! Beacon keeps the lifecycle contract of a program that runs embedded, checked with the suite
//! that consumers run (S-1). A test binary of its own: a process installs logging once.

#[test]
fn beacon_keeps_the_embedded_lifecycle_contract() {
    let root = std::env::temp_dir().join(format!("beacon-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    rivium_test::embedded::lifecycle_contract::<beacon::Beacon>(&root, &[]);
    let _ = std::fs::remove_dir_all(&root);
}
