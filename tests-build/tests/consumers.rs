//! Two isolated core graphs share one reviewed seed lock.
use std::path::PathBuf;
use std::process::Command;

fn cargo(args: &[&str]) -> serde_json::Value {
    let output = Command::new(std::env::var_os("CARGO").unwrap()).args(args).output().unwrap();
    assert!(output.status.success(), "cargo: {}", String::from_utf8_lossy(&output.stderr));
    if output.stdout.is_empty() { serde_json::Value::Null } else {
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn core_consumers() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_owned();
    let target = std::env::var("RIVIUM_CONSUMER_TARGET").expect("explicit consumer target required");
    let cases = root.join("tests-build/cases/core");
    let seed = std::fs::read(cases.join("seed.lock")).unwrap();
    for export in [false, true] {
        let mode = if export { "export" } else { "default" };
        let dir = root.join("target/consumers").join(mode);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let manifest = std::fs::read_to_string(cases.join("manifest.toml.in")).unwrap()
            .replace("@CORE_PATH@", &serde_json::to_string(&root.join("crates/rivium")).unwrap())
            .replace("@EXPORT@", if export { ", features = ["log-export"]" } else { "" });
        std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
        std::fs::copy(cases.join("main.rs"), dir.join("src/main.rs")).unwrap();
        std::fs::write(dir.join("Cargo.lock"), &seed).unwrap();
        let manifest = dir.join("Cargo.toml");
        let build = dir.join("build");
        cargo(&["check", "--manifest-path", manifest.to_str().unwrap(), "--target", &target,
            "--target-dir", build.to_str().unwrap(), "--locked", "--offline", "--quiet"]);
        let data = cargo(&["metadata", "--manifest-path", manifest.to_str().unwrap(),
            "--format-version", "1", "--filter-platform", &target, "--locked", "--offline"]);
        let packages = data["packages"].as_array().unwrap();
        let core = packages.iter().find(|p| p["name"] == "rivium").unwrap();
        assert_eq!(PathBuf::from(core["manifest_path"].as_str().unwrap()).canonicalize().unwrap(),
            root.join("crates/rivium/Cargo.toml").canonicalize().unwrap(), "core source identity");
        let nodes = data["resolve"]["nodes"].as_array().unwrap();
        let zip = packages.iter().find(|p| p["name"] == "zip");
        assert_eq!(zip.is_some_and(|p| nodes.iter().any(|n| n["id"] == p["id"])), export, "zip feature graph");
        rivium_test::deps::assert_closure_excludes(&manifest, "core-consumer",
            &["axum", "hyper", "jni", "rivium-test", "opentelemetry*", "cc", "cmake", "aws-lc*", "openssl*"]);
        assert_eq!(std::fs::read(dir.join("Cargo.lock")).unwrap(), seed, "consumer lock changed");
    }
}
