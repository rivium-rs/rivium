//! Records the target triple, so the process tests can find cargo's target runner.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=SMOKE_TARGET={target}");
}
