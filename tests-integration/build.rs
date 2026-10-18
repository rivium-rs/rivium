fn main() {
    println!(
        "cargo:rustc-env=RIVIUM_TEST_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    println!("cargo:rerun-if-changed=build.rs");
}
