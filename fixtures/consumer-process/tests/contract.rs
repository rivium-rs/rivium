//! The lifecycle contract, as a consumer checks it.

#[test]
fn the_program_keeps_the_lifecycle_contract() {
    let bin = env!("CARGO_BIN_EXE_consumer-process").as_ref();
    // The deployment's port is not for tests.
    rivium_test::process::lifecycle_contract(bin, &["--set", "addr=127.0.0.1:0"]);
}
