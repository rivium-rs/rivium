//! The beacon program as a supervisor runs it: the lifecycle contract every program keeps (S-1),
//! and an answer with the serial number of its configuration. Not built for Android, where
//! beacon runs embedded.
#![cfg(not(target_os = "android"))]

use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_beacon");

#[test]
fn beacon_keeps_the_lifecycle_contract() {
    rivium_test::process::lifecycle_contract(BIN.as_ref(), &[]);
}

/// Sends a query and returns the answer.
fn query(addr: SocketAddr) -> String {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    socket.send_to(b"?", addr).unwrap();
    let mut buffer = [0; 512];
    let (len, _) = socket.recv_from(&mut buffer).unwrap();
    String::from_utf8_lossy(&buffer[..len]).into_owned()
}

#[test]
fn the_program_answers_with_the_serial_number_of_its_configuration() {
    let root = std::env::temp_dir().join(format!("beacon-process-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let args = [
        "--root",
        root.to_str().unwrap(),
        "--set",
        "beacon.serial=SN-7",
    ];
    let program = rivium_test::process::spawn(Path::new(BIN), &args);
    let addr = program.addr_of("beacon");
    assert_eq!(query(addr), "SN-7 1");
    #[cfg(unix)]
    {
        program.signal("TERM");
        assert_eq!(program.wait().code, Some(0));
    }
    let _ = std::fs::remove_dir_all(&root);
}
