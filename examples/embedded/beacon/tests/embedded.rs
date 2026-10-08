//! Beacon embedded, as an Android app runs it: the app passes the root and the device's serial
//! number, the service answers, and it starts again after a stop.

use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::Duration;

use beacon::Beacon;
use rivium::Code;
use rivium::embedded::Host;

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

/// The address of the last `listening` event in the log file.
fn listening(root: &Path) -> SocketAddr {
    rivium::log::flush(Duration::from_secs(5)).unwrap();
    let log = std::fs::read_to_string(root.join("logs/beacon/beacon.log")).unwrap();
    let line = log
        .lines()
        .rfind(|line| line.contains(" listening "))
        .unwrap();
    let addr = line.split("listen.addr=").nth(1).unwrap();
    addr.split_whitespace().next().unwrap().parse().unwrap()
}

#[test]
fn the_embedded_beacon_answers_with_the_serial_number_the_app_passes() {
    let root = std::env::temp_dir().join(format!("beacon-embedded-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let host = Host::new::<Beacon>();
    let root_arg = root.display().to_string();
    let args = ["--root", &root_arg, "--set", "beacon.serial=SN-42"].map(String::from);
    // Rivium's panic hook writes panics to the log once logging is installed: the test's own
    // failures go to standard error as well.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        eprintln!("{info}");
    }));
    for _ in 0..2 {
        assert_eq!(host.start(&args), Code::Ok, "{:?}", host.last_error());
        let addr = listening(&root);
        // Each start builds new services, with a new count.
        assert_eq!(query(addr), "SN-42 1");
        assert_eq!(query(addr), "SN-42 2");
        assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
    }
    let _ = std::fs::remove_dir_all(&root);
}
