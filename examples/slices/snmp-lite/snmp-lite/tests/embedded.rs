//! snmp-lite keeps the lifecycle contract of a program that runs embedded, checked with the
//! suite that consumers run (S-1), with the port that an app passes in place of the deployed
//! one. A test binary of its own: a process installs logging once.

#[test]
fn snmp_lite_keeps_the_embedded_lifecycle_contract() {
    let root = std::env::temp_dir().join(format!("snmp-lite-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let args = ["--set", "snmp.addr=127.0.0.1:0"];
    rivium_test::embedded::lifecycle_contract::<snmp_lite::SnmpLite>(&root, &args);
    let _ = std::fs::remove_dir_all(&root);
}
