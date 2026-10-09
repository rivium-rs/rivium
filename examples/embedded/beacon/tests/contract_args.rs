//! The contract passes its arguments to every start, as an app passes them: here the
//! configuration file's address does not parse, as a deployment's may not run in a test, and
//! only the arguments set one that does. A test binary of its own: a process installs logging
//! once.

#[test]
fn every_start_of_the_contract_gets_its_arguments() {
    let root = std::env::temp_dir().join(format!("beacon-contract-args-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("config.toml"),
        "[beacon]\naddr = \"not an address\"\n",
    )
    .unwrap();
    let args = ["--set", "beacon.addr=127.0.0.1:0"];
    rivium_test::embedded::lifecycle_contract::<beacon::Beacon>(&root, &args);
    let _ = std::fs::remove_dir_all(&root);
}
