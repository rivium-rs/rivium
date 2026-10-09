//! The program keeps the lifecycle contract that every program built on Rivium keeps (S-1), with
//! a free port in place of the deployed one. Not built for Android, where services run embedded.
#![cfg(not(target_os = "android"))]

#[test]
fn the_program_keeps_the_lifecycle_contract() {
    let bin = env!("CARGO_BIN_EXE_edge-lite").as_ref();
    rivium_test::process::lifecycle_contract(bin, &["--set", "http.addr=127.0.0.1:0"]);
}
