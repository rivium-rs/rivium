//! Records the target whose dependency closure the tests inspect.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-env=TESTS_BUILD_TARGET={}",
        std::env::var("TARGET").expect("Cargo target")
    );
}
