//! The program keeps the lifecycle contract that every program built on Rivium keeps: what a
//! supervisor relies on when it starts and stops it. Not built for Android, where the service
//! runs embedded.
#![cfg(not(target_os = "android"))]

#[test]
fn the_program_keeps_the_lifecycle_contract() {
    let bin = env!("CARGO_BIN_EXE_{{project-name}}").as_ref();
    // What the defaults need to run in a test, such as a free port in place of the deployment's.
    let args = [{% if http %}"--set", "http.addr=127.0.0.1:0"{% endif %}];
    rivium_test::process::lifecycle_contract(bin, &args);
}
