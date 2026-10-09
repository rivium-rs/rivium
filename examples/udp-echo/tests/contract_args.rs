//! The contract passes its arguments to every run that loads a configuration: here the
//! deployment's address does not parse, as a deployment's default may not run in a test, and
//! only the arguments set one that does. A test binary of its own, because it sets the
//! environment of the programs it starts. Not built for Android, where services run embedded.
#![cfg(not(target_os = "android"))]

#[test]
fn every_run_of_the_contract_gets_its_arguments() {
    // SAFETY: the only test of this binary sets the variable before it starts any thread.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("UDP_ECHO_ECHO__ADDR", "not an address");
    }
    let bin = env!("CARGO_BIN_EXE_udp-echo").as_ref();
    rivium_test::process::lifecycle_contract(bin, &["--set", "echo.addr=127.0.0.1:0"]);
}
