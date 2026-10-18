use std::{collections::BTreeSet, process::Command};

fn graph(package: &str, features: &str) -> std::collections::BTreeMap<String, BTreeSet<String>> {
    use std::sync::{Mutex, OnceLock};

    type Graph = std::collections::BTreeMap<String, BTreeSet<String>>;
    static CACHE: OnceLock<Mutex<std::collections::BTreeMap<(String, String), Graph>>> =
        OnceLock::new();

    let cache = CACHE.get_or_init(Default::default);
    let key = (package.to_owned(), features.to_owned());
    let mut cache = cache.lock().unwrap();
    if let Some(found) = cache.get(&key) {
        return found.clone();
    }

    let consumer = tests_build::Consumer::new(package, features);
    tests_build::Consumer::checked(consumer.cargo("check").arg("--lib").output().unwrap());
    let text = tests_build::Consumer::checked(
        consumer
            .cargo("tree")
            .args([
                "--locked",
                "--color",
                "never",
                "-e",
                "normal,build",
                "--prefix",
                "none",
                "--format",
                "{p}|{f}",
            ])
            .output()
            .unwrap(),
    );

    let mut result: Graph = Default::default();
    for line in text.lines() {
        let Some((package, features)) = line.split_once('|') else {
            continue;
        };
        let Some(package) = package.split_whitespace().next() else {
            continue;
        };
        if package == "rivium-contract-probe" {
            continue;
        }
        result.entry(package.to_owned()).or_default().extend(
            features
                .trim_end_matches(" (*)")
                .split(',')
                .filter(|f| !f.is_empty())
                .map(str::to_owned),
        );
    }

    cache.insert(key, result.clone());
    result
}

fn closure(package: &str, features: &str) -> BTreeSet<String> {
    graph(package, features).into_keys().collect()
}

#[test]
fn core_default_has_no_runtime_or_integrations() {
    assert_eq!(closure("rivium", ""), BTreeSet::from(["rivium".into()]));
}

#[test]
fn adapters_do_not_pull_in_siblings() {
    for (package, forbidden) in [
        (
            "rivium-http",
            vec![
                "rivium-host",
                "rivium-jni",
                "jni",
                "toml",
                "flate2",
                "rustls",
                "tokio-rustls",
            ],
        ),
        (
            "rivium-jni",
            vec!["rivium-http", "axum", "java-locator", "libloading"],
        ),
        (
            "rivium-util",
            vec!["tokio", "toml", "tracing-subscriber", "zip"],
        ),
    ] {
        let names = closure(package, "");
        for name in forbidden {
            assert!(
                !names.contains(name),
                "{package} unexpectedly depends on {name}"
            );
        }
    }
}

#[test]
fn utility_features_are_independent() {
    for (feature, present, absent) in [
        ("rivium-util/config", "toml", "flate2"),
        ("rivium-util/log", "flate2", "toml"),
        ("rivium-util/log-export", "zip", "axum"),
        ("rivium-util/task", "tokio", "tracing-subscriber"),
    ] {
        let names = closure("rivium-util", feature);
        assert!(names.contains(present));
        assert!(!names.contains(absent));
    }
}

#[test]
fn test_helpers_keep_logging_dependencies_minimal() {
    let names = closure("rivium-test", "");
    assert!(names.contains("tracing-subscriber"));
    for name in [
        "serde_json",
        "tracing-serde",
        "matchers",
        "regex-automata",
        "nu-ansi-term",
    ] {
        assert!(
            !names.contains(name),
            "rivium-test unexpectedly depends on {name}"
        );
    }
}

#[test]
fn only_six_library_packages_are_publishable() {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: BTreeSet<_> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["publish"] != serde_json::json!([]))
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    let versions: BTreeSet<_> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|package| package["publish"] != serde_json::json!([]))
        .map(|package| package["version"].as_str().unwrap())
        .collect();
    assert_eq!(
        versions,
        BTreeSet::from(["0.2.0"]),
        "all six release crates must share the approved rebuild version"
    );
    assert_eq!(
        names,
        BTreeSet::from([
            "rivium",
            "rivium-host",
            "rivium-http",
            "rivium-jni",
            "rivium-test",
            "rivium-util"
        ])
    );
}

#[test]
fn release_packages_exclude_internal_material() {
    for package in [
        "rivium",
        "rivium-util",
        "rivium-host",
        "rivium-http",
        "rivium-jni",
        "rivium-test",
    ] {
        let out = Command::new(env!("CARGO"))
            .args([
                "package",
                "-p",
                package,
                "--list",
                "--allow-dirty",
                "--locked",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let files = String::from_utf8(out.stdout).unwrap();
        assert!(files.lines().any(|f| f == "src/lib.rs"));
        assert!(files.lines().any(|f| f == "LICENSE-MIT"));
        assert_eq!(
            files.lines().any(|f| f == "build.rs"),
            package == "rivium-test",
            "only the target-aware test helper has a packaged build script"
        );
        assert!(!files.lines().any(|f| f.starts_with("tests/")
            || f.starts_with("docs/design/")
            || f.starts_with(".tools/")));
    }
}

fn dependency_features(package: &str, features: &str, dependency: &str) -> BTreeSet<String> {
    graph(package, features)
        .remove(dependency)
        .unwrap_or_default()
}

#[test]
fn jni_fixture_verifies_the_minimal_embedded_host() {
    let names = closure("test-jni", "");
    for forbidden in [
        "tests-integration",
        "rivium-test",
        "rivium-http",
        "axum",
        "zip",
    ] {
        assert!(
            !names.contains(forbidden),
            "JNI fixture pulls in {forbidden}"
        );
    }
    assert_eq!(
        dependency_features("test-jni", "", "rivium-host"),
        BTreeSet::from(["embedded".into()])
    );
    let tokio = dependency_features("test-jni", "", "tokio");
    for forbidden in ["signal", "test-util", "net"] {
        assert!(
            !tokio.contains(forbidden),
            "JNI fixture enables Tokio {forbidden}"
        );
    }
}

#[test]
fn task_tools_do_not_enable_the_core_supervisor() {
    assert_eq!(
        dependency_features("rivium-util", "task", "rivium"),
        BTreeSet::from(["task".into()])
    );
    assert!(dependency_features("rivium-util", "task", "tokio-util").contains("rt"));
    let names = closure("rivium-util", "task");
    assert!(!names.contains("tracing"));
}

#[test]
fn core_scopes_use_task_tracking_without_supervisor_logging() {
    assert!(dependency_features("rivium", "task", "tokio-util").contains("rt"));
    assert!(!closure("rivium", "task").contains("tracing"));
    assert!(dependency_features("rivium", "rt", "tokio-util").contains("rt"));
}

#[test]
fn http_default_is_vocabulary_only() {
    let names = closure("rivium-http", "");
    for forbidden in ["tokio", "hyper", "tower-http", "uuid"] {
        assert!(
            !names.contains(forbidden),
            "HTTP vocabulary pulls in {forbidden}"
        );
    }
    assert!(dependency_features("rivium-http", "", "rivium").is_empty());
}

#[test]
fn http_contract_does_not_enable_a_listener_or_file_io() {
    let names = closure("rivium-http", "contract");
    assert!(names.contains("tokio"));
    assert!(names.contains("uuid"));
    assert!(!names.contains("rustls"));
    assert!(!names.contains("hyper"));
    assert!(dependency_features("rivium-http", "contract", "rivium").is_empty());
    let tokio = dependency_features("rivium-http", "contract", "tokio");
    for forbidden in ["net", "fs", "rt"] {
        assert!(
            !tokio.contains(forbidden),
            "HTTP contract enables Tokio {forbidden}"
        );
    }
}

#[test]
fn http_server_and_file_capabilities_are_independent() {
    let server = dependency_features("rivium-http", "server", "tokio");
    assert!(server.contains("net"));
    assert!(!server.contains("fs"));
    let files = dependency_features("rivium-http", "fs", "tokio");
    assert!(files.contains("fs"));
    assert!(!files.contains("net"));
    assert!(dependency_features("rivium-http", "fs", "rivium").is_empty());
}

#[test]
fn library_contracts_live_with_their_crates() {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for (name, required) in [
        ("rivium", vec!["lifecycle"]),
        (
            "rivium-util",
            vec!["config", "log", "log_export", "fs", "task"],
        ),
        (
            "rivium-http",
            vec!["vocabulary", "contract", "server", "files"],
        ),
    ] {
        let package = metadata["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap();
        let tests: BTreeSet<_> = package["targets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["kind"] == serde_json::json!(["test"]))
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for required in required {
            assert!(
                tests.contains(required),
                "{name} does not own its {required} contract"
            );
        }
    }
}
