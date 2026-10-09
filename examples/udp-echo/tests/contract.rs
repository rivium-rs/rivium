//! udp-echo keeps the lifecycle contract that consumers check with the same suite (S-1). Not
//! built for Android, where services run embedded.
#![cfg(not(target_os = "android"))]

#[test]
fn udp_echo_keeps_the_lifecycle_contract() {
    rivium_test::process::lifecycle_contract(env!("CARGO_BIN_EXE_udp-echo").as_ref(), &[]);
}
