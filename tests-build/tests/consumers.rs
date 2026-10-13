//! Supply-chain tests: verify that consumers can use the public API.
//!
//! These tests ensure that the crates can be consumed in different contexts
//! without causing build failures or linker errors.

#[test]
fn can_construct_app() {
    use rivium::{App, AppContext, Result, Service};
    use serde::{Deserialize, Serialize};

    #[derive(Default)]
    struct TestApp;

    #[derive(Default, Serialize, Deserialize)]
    struct Config {}

    impl App for TestApp {
        const NAME: &'static str = "test-app";
        const VERSION: &'static str = "0.0.0";
        type Config = Config;

        fn services(_config: &Self::Config, _ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
            Ok(vec![])
        }
    }

    // Just ensure it compiles and constructs.
    let _app = TestApp;
}

#[test]
fn can_use_error_types() {
    use rivium::error::{kinds, Error};

    let err = Error::explain(kinds::INTERNAL, "test error");
    assert_eq!(err.to_string(), "test error");
}

// No depth limit: a build dependency several edges away is part of the closure too.
fn dependency_names(package: &str, feature: Option<&str>) -> std::collections::BTreeSet<String> {
    let mut command = tests_build::cargo("tree");
    command.args([
        "--package",
        package,
        "--target",
        tests_build::TARGET,
        "--edges",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}",
        "--no-default-features",
    ]);
    if let Some(feature) = feature {
        command.args(["--features", feature]);
    }
    let output = command.output().expect("run cargo tree");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("UTF-8 dependency tree");
    let names: std::collections::BTreeSet<_> = text
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| !name.starts_with('['))
        .map(str::to_owned)
        .collect();
    assert!(names.contains(package), "missing root {package}:\n{text}");
    names
}

fn excludes(names: &std::collections::BTreeSet<String>, forbidden: &[&str]) {
    for name in forbidden {
        assert!(
            !names.contains(*name),
            "unexpected production dependency {name}:\n{names:#?}"
        );
    }
}

#[test]
fn default_consumer_excludes_http_jni_and_zip() {
    let names = dependency_names("tests-build", None);
    assert!(names.contains("rivium"));
    // JNI is deliberately a dev-dependency of this package, not a consumer dependency.
    excludes(
        &names,
        &[
            "rivium-http",
            "axum",
            "hyper",
            "rivium-jni",
            "jni",
            "jni-sys",
            "zip",
        ],
    );
}

#[test]
fn log_export_adds_zip_without_http_or_jni() {
    let names = dependency_names("tests-build", Some("rivium/log-export"));
    assert!(
        names.contains("zip"),
        "the export feature must actually be selected"
    );
    excludes(
        &names,
        &[
            "rivium-http",
            "axum",
            "hyper",
            "rivium-jni",
            "jni",
            "jni-sys",
        ],
    );
}

#[test]
fn adapters_do_not_pull_each_other_in() {
    let jni = dependency_names("rivium-jni", None);
    assert!(jni.contains("jni"));
    excludes(
        &jni,
        &["rivium-http", "axum", "hyper", "java-locator", "libloading"],
    );
    let http = dependency_names("rivium-http", None);
    assert!(http.contains("axum"));
    excludes(&http, &["rivium-jni", "jni", "jni-sys"]);
}
