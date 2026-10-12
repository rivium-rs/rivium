//! Rust code-line budgets for the five library crates, including their tests.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::Value;
use tests_build::workspace_root;

const BUDGETS: &[(&str, u64)] = &[
    ("rivium", 12_635),
    ("rivium-error", 1_031),
    ("rivium-http", 2_854),
    ("rivium-jni", 256),
    ("rivium-test", 935),
];
const TOTAL_BUDGET: u64 = 17_710;

fn rust_code(output: &[u8]) -> Result<u64, String> {
    let value: Value = serde_json::from_slice(output).map_err(|error| error.to_string())?;
    let code = value["Rust"]["code"]
        .as_u64()
        .filter(|code| *code > 0)
        .ok_or("missing or empty Rust code count")?;
    let reports = value["Rust"]["reports"]
        .as_array()
        .filter(|reports| !reports.is_empty())
        .ok_or("missing Rust file reports")?;
    let mut measured = 0u64;
    for report in reports {
        let count = report["stats"]["code"]
            .as_u64()
            .ok_or("missing file code count")?;
        measured = measured.checked_add(count).ok_or("code count overflow")?;
    }
    if measured != code {
        return Err("Rust total differs from file reports".into());
    }
    Ok(code)
}

fn check_budgets(counts: &BTreeMap<&str, u64>) -> Result<(), String> {
    let mut failures = Vec::new();
    for &(name, budget) in BUDGETS {
        match counts.get(name) {
            Some(&count) if count > 0 && count <= budget => {}
            Some(&count) if count > budget => failures.push(format!(
                "{name}: {count} code lines, budget {budget}, over by {}",
                count - budget
            )),
            _ => failures.push(format!("{name}: missing or empty measurement")),
        }
    }
    let total = counts
        .values()
        .fold(0u64, |sum, count| sum.saturating_add(*count));
    if total > TOTAL_BUDGET {
        failures.push(format!("total: {total} code lines, budget {TOTAL_BUDGET}"));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

#[test]
fn library_code_stays_within_reviewed_budgets() {
    let version = Command::new("tokei")
        .arg("--version")
        .output()
        .expect("install tokei 14.0.0 under .tools and add .tools/bin to PATH");
    assert!(version.status.success(), "tokei --version failed");
    assert_eq!(
        String::from_utf8_lossy(&version.stdout)
            .split_whitespace()
            .nth(1),
        Some("14.0.0"),
        "the code counter version is part of the baseline"
    );
    let root = workspace_root();
    let actual: BTreeSet<_> = std::fs::read_dir(root.join("crates"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let expected = BUDGETS.iter().map(|(name, _)| name.to_string()).collect();
    assert_eq!(
        actual, expected,
        "every library crate needs an explicit budget"
    );

    let mut counts = BTreeMap::new();
    for &(name, budget) in BUDGETS {
        // Count each directory separately: no assumptions about relative paths or separators.
        let output = Command::new("tokei")
            .arg(root.join("crates").join(name))
            .args([
                "--types",
                "Rust",
                "--output",
                "json",
                "--no-ignore",
                "--hidden",
            ])
            .output()
            .expect("run tokei");
        assert!(
            output.status.success(),
            "tokei {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let count =
            rust_code(&output.stdout).unwrap_or_else(|error| panic!("tokei {name}: {error}"));
        println!("{name}: {count}/{budget} Rust code lines");
        counts.insert(name, count);
    }
    check_budgets(&counts).unwrap_or_else(|error| {
        panic!(
            "LOC budget failed:\n{error}\nBudget changes require review, not automatic rebasing."
        )
    });
}

#[test]
fn missing_or_inconsistent_counter_output_is_rejected() {
    for bad in [
        "{}",
        r#"{"Rust":{"code":0,"reports":[]}}"#,
        r#"{"Rust":{"code":2,"reports":[{"stats":{"code":1}}]}}"#,
    ] {
        assert!(rust_code(bad.as_bytes()).is_err());
    }
    assert_eq!(
        rust_code(br#"{"Rust":{"code":2,"reports":[{"stats":{"code":2}}]}}"#).unwrap(),
        2
    );
}

#[test]
fn missing_crates_and_individual_or_total_overruns_are_rejected() {
    let mut counts: BTreeMap<_, _> = BUDGETS.iter().map(|(name, _)| (*name, 1)).collect();
    assert!(check_budgets(&counts).is_ok());
    counts.remove("rivium-jni");
    assert!(check_budgets(&counts).unwrap_err().contains("rivium-jni"));
    counts.insert("rivium-jni", 257);
    assert!(check_budgets(&counts).unwrap_err().contains("over by 1"));
    // Individual rounded ceilings sum to 17,711; the independent total cap is 17,710.
    let ceilings = BUDGETS.iter().copied().collect();
    assert!(check_budgets(&ceilings)
        .unwrap_err()
        .contains("total: 17711"));
}
