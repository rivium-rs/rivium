//! Published libraries must not ship repository-only validation or internal documents.

#[test]
fn packages_contain_only_their_declared_public_files() {
    for name in [
        "rivium",
        "rivium-util",
        "rivium-host",
        "rivium-http",
        "rivium-jni",
        "rivium-test",
    ] {
        let output = std::process::Command::new(env!("CARGO"))
            .arg("package")
            .args(["--package", name, "--list", "--allow-dirty"])
            .output()
            .expect("list a library package");
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let listing = String::from_utf8(output.stdout).expect("UTF-8 package paths");
        let files: Vec<_> = listing
            .lines()
            .map(|path| path.replace('\\', "/"))
            .collect();
        for required in [
            "Cargo.toml",
            "src/lib.rs",
            "README.md",
            "LICENSE-MIT",
            "LICENSE-APACHE",
        ] {
            assert!(
                files.iter().any(|path| path == required),
                "{name}: missing {required}"
            );
        }
        for path in &files {
            let public = matches!(
                path.as_str(),
                "Cargo.toml"
                    | "Cargo.toml.orig"
                    | "Cargo.lock"
                    | ".cargo_vcs_info.json"
                    | "README.md"
                    | "LICENSE-MIT"
                    | "LICENSE-APACHE"
            ) || path.starts_with("src/")
                || (name == "rivium-test" && path == "build.rs");
            assert!(
                public
                    && !path.split('/').any(|part| matches!(
                        part,
                        "docs" | "planning" | "fixtures" | "scripts" | "target"
                    )),
                "{name}: unexpected packaged path {path}"
            );
        }
    }
}
