//! Dependency closure checks: optional capabilities, such as an HTTP stack or JNI, stay out of
//! the consumers that do not use them.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Asserts that no banned crate is in the dependency closure of `package` on the target
/// platform the calling test was compiled for.
///
/// The closure is what building `package` compiles: its normal and build dependencies,
/// transitively, as `cargo metadata --filter-platform <target>` resolves them in the workspace
/// of `manifest`. Dev-dependencies are not part of it. A banned name matches the crate of that
/// name; a name ending in `*` matches every crate with that prefix, such as `opentelemetry*`.
///
/// ```no_run
/// # use std::path::Path;
/// let manifest = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
/// rivium_test::deps::assert_closure_excludes(manifest, "my-service", &["axum", "jni"]);
/// ```
///
/// # Panics
///
/// When a banned crate is in the closure (the message shows how `package` depends on it), when
/// `package` is not in the dependency graph, or when `cargo metadata` fails.
pub fn assert_closure_excludes(manifest: &Path, package: &str, banned: &[&str]) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            crate::TARGET,
        ])
        .arg("--manifest-path")
        .arg(manifest)
        .output()
        .expect("run cargo metadata");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cargo metadata failed: {stderr}");
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata output");
    let found = banned_in_closure(&metadata, package, banned);
    assert!(
        found.is_empty(),
        "the dependency closure of {package} on {} contains banned crates:\n{}",
        crate::TARGET,
        found.join("\n")
    );
}

/// The banned crates in the closure of `package`, each shown as the path that reaches it.
fn banned_in_closure(metadata: &Value, package: &str, banned: &[&str]) -> Vec<String> {
    let names: BTreeMap<&str, &str> = items(&metadata["packages"])
        .filter_map(|p| Some((p["id"].as_str()?, p["name"].as_str()?)))
        .collect();
    let nodes: BTreeMap<&str, &Value> = items(&metadata["resolve"]["nodes"])
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect();
    let name = |id| names.get(id).copied().unwrap_or(id);
    let roots: Vec<&str> = nodes
        .keys()
        .copied()
        .filter(|id| name(*id) == package)
        .collect();
    assert!(
        !roots.is_empty(),
        "{package} is not in the dependency graph"
    );

    // Breadth first, so each path shown is a shortest one.
    let mut parent: BTreeMap<&str, Option<&str>> = roots.iter().map(|id| (*id, None)).collect();
    let mut queue: VecDeque<&str> = roots.into_iter().collect();
    let mut found = Vec::new();
    while let Some(id) = queue.pop_front() {
        if parent[id].is_some() && banned.iter().any(|pattern| matches(name(id), pattern)) {
            let mut path = vec![name(id)];
            let mut at = id;
            while let Some(Some(up)) = parent.get(at) {
                path.push(name(up));
                at = up;
            }
            path.reverse();
            found.push(path.join(" -> "));
        }
        let deps = nodes
            .get(id)
            .map(|node| &node["deps"])
            .unwrap_or(&Value::Null);
        for dep in items(deps) {
            let compiled =
                items(&dep["dep_kinds"]).any(|kind| kind["kind"].as_str() != Some("dev"));
            if let Some(next) = dep["pkg"].as_str().filter(|_| compiled)
                && !parent.contains_key(next)
            {
                parent.insert(next, Some(id));
                queue.push_back(next);
            }
        }
    }
    found
}

fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn matches(name: &str, pattern: &str) -> bool {
    pattern
        .strip_suffix('*')
        .map_or(name == pattern, |prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::banned_in_closure;

    /// Metadata for `app` -> `lib` (normal) -> `ring` (normal) -> `cc` (build), with `app`
    /// -> `test-kit` (dev) -> `axum` (normal) and `lib` -> `opentelemetry-sdk` (normal).
    fn metadata() -> Value {
        let dep = |pkg: &str, kind: Value| json!({ "pkg": pkg, "dep_kinds": [{ "kind": kind }] });
        let node = |id: &str, deps: Vec<Value>| json!({ "id": id, "deps": deps });
        let ids = [
            "app",
            "lib",
            "ring",
            "cc",
            "test-kit",
            "axum",
            "opentelemetry-sdk",
        ];
        json!({
            "packages": ids.map(|id| json!({ "id": format!("id:{id}"), "name": id })),
            "resolve": { "nodes": [
                node("id:app", vec![dep("id:lib", Value::Null), dep("id:test-kit", json!("dev"))]),
                node("id:lib", vec![dep("id:ring", Value::Null), dep("id:opentelemetry-sdk", Value::Null)]),
                node("id:ring", vec![dep("id:cc", json!("build"))]),
                node("id:cc", vec![]),
                node("id:test-kit", vec![dep("id:axum", Value::Null)]),
                node("id:axum", vec![]),
                node("id:opentelemetry-sdk", vec![]),
            ]},
        })
    }

    #[test]
    fn normal_and_build_dependencies_are_followed_and_dev_dependencies_are_not() {
        let found = banned_in_closure(&metadata(), "app", &["cc", "axum", "opentelemetry*"]);
        assert_eq!(
            found,
            [
                "app -> lib -> opentelemetry-sdk",
                "app -> lib -> ring -> cc"
            ]
        );
        assert!(banned_in_closure(&metadata(), "test-kit", &["axum"]).len() == 1);
        assert!(banned_in_closure(&metadata(), "app", &["app", "open"]).is_empty());
    }

    #[test]
    #[should_panic(expected = "missing is not in the dependency graph")]
    fn an_unknown_package_fails() {
        banned_in_closure(&metadata(), "missing", &["cc"]);
    }
}
