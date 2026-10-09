//! The program keeps the lifecycle contract that every program built on Rivium keeps (S-1), with
//! a free port in place of the deployed one, which a test cannot bind. Not built for Android,
//! where snmp-lite runs embedded.
#![cfg(not(target_os = "android"))]

#[test]
fn the_program_keeps_the_lifecycle_contract() {
    let bin = env!("CARGO_BIN_EXE_snmp-lite").as_ref();
    rivium_test::process::lifecycle_contract(bin, &["--set", "snmp.addr=127.0.0.1:0"]);
}
