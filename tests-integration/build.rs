//! Build script for tests-integration.
//!
//! Exposes test-app binaries via CARGO_BIN_EXE_* environment variables
//! so integration tests can spawn them as subprocesses. Build the binaries first
//! with `cargo build -p test-app --bins` using the same toolchain, target and profile.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(qemu)");
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

    // OUT_DIR is <target-dir>/<optional-triple>/<profile>/build/<package>/out.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let bin_dir = out_dir.ancestors().nth(3).expect("Cargo profile directory");
    let suffix = if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        ".exe"
    } else {
        ""
    };

    // Set CARGO_BIN_EXE_* for each binary.
    for bin in &bins {
        let bin_path = bin_dir.join(format!("{bin}{suffix}"));
        let env_name = format!("CARGO_BIN_EXE_{}", bin.replace('-', "_"));

        println!("cargo:rustc-env={}={}", env_name, bin_path.display());
    }
}
