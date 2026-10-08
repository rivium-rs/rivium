//! Records the target platform the crate is compiled for: `deps` checks the dependency closure
//! for the platform the calling test runs on.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=RIVIUM_TEST_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
