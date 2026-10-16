//! Aggregate budget with complete, internally consistent per-file measurements.

use std::{
    collections::{BTreeMap, BTreeSet},
    process::Command,
};

const LIBRARIES: [&str; 6] = [
    "rivium",
    "rivium-util",
    "rivium-host",
    "rivium-http",
    "rivium-jni",
    "rivium-test",
];
const TOTAL_BUDGET: u64 = 17_710;

fn rust_code(output: &[u8]) -> Result<u64, String> {
    let value: serde_json::Value =
        serde_json::from_slice(output).map_err(|error| error.to_string())?;
    let code = value["Rust"]["code"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or("missing or empty Rust total")?;
    let reports = value["Rust"]["reports"]
        .as_array()
        .filter(|reports| !reports.is_empty())
        .ok_or("missing Rust file reports")?;
    let mut names = BTreeSet::new();
    let mut sum = 0u64;
    for report in reports {
        let name = report["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or("missing file name")?;
        if !names.insert(name) {
            return Err("duplicate file report".into());
        }
        let count = report["stats"]["code"]
            .as_u64()
            .ok_or("missing file code count")?;
        sum = sum.checked_add(count).ok_or("file count overflow")?;
    }
    if sum != code {
        return Err("Rust total differs from file reports".into());
    }
    Ok(code)
}

fn check_budget(counts: &BTreeMap<&str, u64>) -> Result<u64, String> {
    if counts.keys().copied().collect::<BTreeSet<_>>() != LIBRARIES.into_iter().collect() {
        return Err("missing or unexpected library measurement".into());
    }
    let mut total = 0u64;
    for (name, count) in counts {
        if *count == 0 {
            return Err(format!("empty measurement for {name}"));
        }
        total = total.checked_add(*count).ok_or("total count overflow")?;
    }
    if total > TOTAL_BUDGET {
        return Err(format!(
            "{total} Rust lines exceed the reviewed {TOTAL_BUDGET} aggregate budget"
        ));
    }
    Ok(total)
}

#[test]
fn library_code_stays_within_budget() {
    let version = Command::new("tokei")
        .arg("--version")
        .output()
        .expect("install tokei 14.0.0 and put it first on PATH");
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout)
            .split_whitespace()
            .nth(1),
        Some("14.0.0"),
        "the counter version is part of the reviewed baseline"
    );
    let root = tests_build::workspace().join("crates");
    let actual: BTreeSet<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        actual,
        LIBRARIES.map(str::to_owned).into_iter().collect(),
        "every library directory requires a reviewed measurement"
    );
    let mut counts = BTreeMap::new();
    for name in LIBRARIES {
        let output = Command::new("tokei")
            .arg(root.join(name))
            .args([
                "--types",
                "Rust",
                "--output",
                "json",
                "--no-ignore",
                "--hidden",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let code = rust_code(&output.stdout).unwrap_or_else(|error| panic!("{name}: {error}"));
        println!("{name}: {code} Rust code lines");
        counts.insert(name, code);
    }
    println!("total: {} / {TOTAL_BUDGET}", check_budget(&counts).unwrap());
}

#[test]
fn missing_or_inconsistent_counter_output_is_rejected() {
    for bad in [
        "{}",
        r#"{"Rust":{"code":1,"reports":[]}}"#,
        r#"{"Rust":{"code":0,"reports":[]}}"#,
        r#"{"Rust":{"code":2,"reports":[{"name":"a.rs","stats":{"code":1}}]}}"#,
        r#"{"Rust":{"code":2,"reports":[{"name":"a.rs","stats":{"code":1}},{"name":"a.rs","stats":{"code":1}}]}}"#,
        r#"{"Rust":{"code":1,"reports":[{"stats":{"code":1}}]}}"#,
    ] {
        assert!(
            rust_code(bad.as_bytes()).is_err(),
            "accepted malformed counter output: {bad}"
        );
    }

    assert_eq!(
        rust_code(
            br#"{"Rust":{"code":3,"reports":[{"name":"a.rs","stats":{"code":1}},{"name":"b.rs","stats":{"code":2}}]}}"#
        )
        .unwrap(),
        3
    );
}

#[test]
fn missing_crates_empty_counts_and_total_overruns_are_rejected() {
    let valid: BTreeMap<_, _> = LIBRARIES.map(|name| (name, 1)).into_iter().collect();
    assert_eq!(check_budget(&valid).unwrap(), 6);

    let mut missing = valid.clone();
    missing.remove("rivium-jni");
    assert!(check_budget(&missing).is_err());

    let mut empty = valid.clone();
    empty.insert("rivium-jni", 0);
    assert!(check_budget(&empty).is_err());

    let mut extra = valid.clone();
    extra.insert("unreviewed", 1);
    assert!(check_budget(&extra).is_err());

    let mut over = valid.clone();
    over.insert("rivium", TOTAL_BUDGET);
    assert!(check_budget(&over).is_err());

    over.insert("rivium", u64::MAX);
    assert!(check_budget(&over).is_err());
}
