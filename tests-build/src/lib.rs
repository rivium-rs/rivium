//! Independent consumers for dependency and compile contracts.

use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

/// A real package outside the workspace with no development dependencies.
pub struct Consumer {
    root: PathBuf,
}

impl Consumer {
    pub fn set_source(&self, source: &str) {
        fs::write(self.root.join("src/lib.rs"), source).unwrap();
    }

    pub fn new(package: &str, features: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "rivium-consumer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("src")).unwrap();

        let target_package = if package == "test-jni" {
            "rivium-jni"
        } else {
            package
        };
        let dependency = workspace().join("crates").join(target_package);
        let features: Vec<_> = features
            .split(',')
            .filter(|f| !f.is_empty())
            .map(|f| f.rsplit('/').next().unwrap())
            .collect();
        let list = features
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(",");

        let mut manifest = format!(
            "[package]\nname=\"rivium-contract-probe\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[lib]\ncrate-type=[\"rlib\",\"cdylib\"]\n[dependencies]\nsubject={{package=\"{target_package}\",path={:?},default-features=false,features=[{list}]}}\n",
            dependency.to_str().unwrap()
        );

        let source = if package == "test-jni" {
            manifest.push_str(&format!(
                "test-app={{path={:?}}}\n",
                workspace()
                    .join("tests-integration/test-app")
                    .to_str()
                    .unwrap()
            ));
            "#![forbid(unsafe_code)]\nsubject::export!(class = \"rivium/tests/Isolated\", app = test_app::TestApp);\n"
        } else if package == "rivium" && features.is_empty() {
            "pub fn error() -> subject::Error { subject::Error::invalid(\"caller detail\").with_public_message(\"safe\") }\n"
        } else {
            "pub fn consumer_compiles() {}\n"
        };

        fs::write(root.join("Cargo.toml"), manifest).unwrap();
        fs::write(root.join("src/lib.rs"), source).unwrap();
        fs::copy(workspace().join("Cargo.lock"), root.join("Cargo.lock")).unwrap();
        Self { root }
    }

    pub fn cargo(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO"));
        command
            .current_dir(&self.root)
            .args([operation, "--manifest-path"])
            .arg(self.root.join("Cargo.toml"))
            .arg("--offline");
        command.env(
            "CARGO_TARGET_DIR",
            workspace().join("target/isolated-consumers"),
        );
        command
    }

    pub fn checked(output: Output) -> String {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
