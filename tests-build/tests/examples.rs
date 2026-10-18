use std::{collections::BTreeSet, process::Command};

#[test]
fn examples_are_private_targets_and_not_fixture_dependencies() {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    let examples = packages
        .iter()
        .find(|p| p["name"] == "rivium-examples")
        .unwrap();
    assert_eq!(examples["publish"], serde_json::json!([]));
    let targets = examples["targets"].as_array().unwrap();
    assert!(
        targets
            .iter()
            .all(|t| t["kind"] == serde_json::json!(["example"]))
    );
    let names: BTreeSet<_> = targets
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "basic-service",
            "http-server",
            "embedded-host",
            "config-reload",
            "jni-service",
        ])
    );
    assert!(
        examples["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["kind"] == "dev")
    );
    for package in packages {
        assert!(
            package["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["name"] != "rivium-examples"),
            "{} imports the example package",
            package["name"]
        );
    }
}
