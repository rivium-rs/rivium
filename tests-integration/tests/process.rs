//! Contracts against a separately built, ordinary process artifact.
use std::path::PathBuf;

#[test]
fn process_contracts() {
    let receipt =
        std::env::var("RIVIUM_TEST_ARTIFACTS").expect("ordinary subject receipt required");
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
    let bin = PathBuf::from(data["subject"]["path"].as_str().expect("subject path"));
    assert!(
        bin.is_absolute() && bin.is_file(),
        "ordinary subject is missing"
    );
    rivium_test::process::lifecycle_contract(&bin, &[]);
}
