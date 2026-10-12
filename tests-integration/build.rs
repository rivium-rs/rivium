//! Build script for tests-integration.
//!
//! Exposes test-app binaries via CARGO_BIN_EXE_* environment variables
//! so integration tests can spawn them as subprocesses.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=../test-app");

    // List of binary names from test-app/Cargo.toml (with hyphens).
    let bins = [
        "host",
        "host-sigint",
        "host-sigquit",
        "host-second-signal",
        "host-deadline",
        "host-fault-ready",
        "host-fault-run",
        "host-fault-build",
        "host-args",
        "host-export",
        "host-notify",
    ];

    // Find the target directory.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_dir = out_dir
        .ancestors()
        .find(|p| p.ends_with("target"))
        .expect("Could not find target directory");

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    // Set CARGO_BIN_EXE_* for each binary.
    for bin in &bins {
        let bin_path = target_dir.join(profile).join(bin);
        let env_name = format!("CARGO_BIN_EXE_{}", bin.replace('-', "_"));

        println!("cargo:rustc-env={}={}", env_name, bin_path.display());
        eprintln!("Setting {} = {}", env_name, bin_path.display());
    }
}
