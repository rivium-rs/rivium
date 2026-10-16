fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-env=RIVIUM_HELPER_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
