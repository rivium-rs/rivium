#[test]
fn jni_rejects_abort_profiles() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let output = std::process::Command::new(env!("CARGO"))
        .current_dir(root)
        .args([
            "check",
            "-p",
            "rivium-jni",
            "--lib",
            "--locked",
            "--target-dir",
        ])
        .arg(root.join("target/jni-abort-guard"))
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env("RUSTFLAGS", "-Cpanic=abort")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("rivium-jni requires panic=unwind"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn exported_jni_entry_has_checked_types_syntax_and_symbol_uniqueness() {
    let consumer = tests_build::Consumer::new("test-jni", "");
    consumer.set_source(
        r#"#![forbid(unsafe_code)]
        mod nested {
            mod __private {}
            struct JavaVM;
            subject::export!(class = "rivium/tests/Isolated", app = test_app::TestApp);
        }"#,
    );
    tests_build::Consumer::checked(consumer.cargo("build").arg("--lib").output().unwrap());
    for (source, expected) in [
        (
            r#"subject::export!(class = "rivium/tests/Isolated", app = ());"#,
            "trait bound `(): rivium_host::App`",
        ),
        (
            r#"subject::export!(class = "rivium/tests/Isolated", unknown = test_app::TestApp);"#,
            "expected export!",
        ),
        (
            r#"
            mod a { subject::export!(class = "rivium/tests/A", app = test_app::TestApp); }
            mod b { subject::export!(class = "rivium/tests/B", app = test_app::TestApp); }
            "#,
            "JNI_OnLoad",
        ),
    ] {
        consumer.set_source(source);
        let output = consumer.cargo("build").arg("--lib").output().unwrap();
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "invalid export unexpectedly built"
        );
        assert!(diagnostics.contains(expected), "{diagnostics}");
        if expected == "JNI_OnLoad" {
            assert!(
                diagnostics.contains("already defined") || diagnostics.contains("duplicate symbol"),
                "expected a duplicate loader symbol, got: {diagnostics}",
            );
        }
    }
}
