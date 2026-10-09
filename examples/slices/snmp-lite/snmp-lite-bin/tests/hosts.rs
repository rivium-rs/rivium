//! The business tests of snmp-lite, the same ones through each host (S-2): the program on the
//! process host, and the JNI library on the embedded host in a desktop JVM, which an Android app
//! loads the same way. A manager queries the agent: the system group, the serial number that the
//! host passed, the polled readings, a walk, an object it does not have, another community.
//! The program's tests are not built for Android, where snmp-lite runs embedded.
#![cfg(not(target_os = "android"))]

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use snmp_lite::mib::{
    ERRORS, POLLS, PRODUCT, READING, SERIAL, SYS_DESCR, SYS_OBJECT_ID, SYS_UP_TIME,
};
use snmp_lite::snmp::{GET, GET_NEXT, Message, Pdu, RESPONSE, V2C, Value};

const BIN: &str = env!("CARGO_BIN_EXE_snmp-lite");

/// A manager: sends requests and reads the answers.
struct Manager {
    socket: UdpSocket,
    agent: SocketAddr,
    next_id: i64,
}

impl Manager {
    fn new(agent: SocketAddr) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        Manager {
            socket,
            agent,
            next_id: 1,
        }
    }

    /// The answer to a request, if one comes within `wait`.
    fn ask(&mut self, community: &[u8], kind: u8, names: &[&[u32]], wait: Duration) -> Option<Pdu> {
        let request_id = self.next_id;
        self.next_id += 1;
        let bindings = names
            .iter()
            .map(|name| (name.to_vec(), Value::Null))
            .collect();
        let request = Message {
            version: V2C,
            community: community.to_vec(),
            pdu: Pdu {
                kind,
                request_id,
                error_status: 0,
                error_index: 0,
                bindings,
            },
        };
        self.socket.set_read_timeout(Some(wait)).unwrap();
        self.socket.send_to(&request.encode(), self.agent).unwrap();
        let mut buffer = [0; 4096];
        let (len, _) = self.socket.recv_from(&mut buffer).ok()?;
        let answer = Message::decode(&buffer[..len]).unwrap();
        assert_eq!(
            (answer.version, answer.community),
            (V2C, community.to_vec())
        );
        assert_eq!(
            (answer.pdu.kind, answer.pdu.request_id),
            (RESPONSE, request_id)
        );
        assert_eq!((answer.pdu.error_status, answer.pdu.error_index), (0, 0));
        Some(answer.pdu)
    }

    fn get(&mut self, names: &[&[u32]]) -> Vec<Value> {
        let answer = self.ask(b"public", GET, names, Duration::from_secs(10));
        let bindings = answer.expect("an answer").bindings;
        assert!(
            bindings
                .iter()
                .map(|(name, _)| name.as_slice())
                .eq(names.iter().copied())
        );
        bindings.into_iter().map(|(_, value)| value).collect()
    }

    /// GETNEXT from `name` until the end of the view.
    fn walk(&mut self, name: &[u32]) -> Vec<(Vec<u32>, Value)> {
        let mut walked = Vec::new();
        let mut name = name.to_vec();
        loop {
            let answer = self.ask(b"public", GET_NEXT, &[&name], Duration::from_secs(10));
            let (next, value) = answer.expect("an answer").bindings.remove(0);
            if value == Value::EndOfMibView {
                return walked;
            }
            assert!(next > name, "{next:?} does not follow {name:?}");
            walked.push((next.clone(), value));
            name = next;
        }
    }
}

/// The business tests, against the agent at `addr` of a device with `serial`.
fn queries(addr: SocketAddr, serial: &str) {
    let mut manager = Manager::new(addr);
    let values = manager.get(&[SYS_DESCR, SYS_OBJECT_ID, SERIAL]);
    let description = format!("snmp-lite {}", env!("CARGO_PKG_VERSION"));
    assert_eq!(values[0], Value::OctetString(description.into_bytes()));
    assert_eq!(values[1], Value::ObjectId(PRODUCT.to_vec()));
    assert_eq!(values[2], Value::OctetString(serial.as_bytes().to_vec()));
    assert_eq!(
        manager.get(&[&[1, 3, 6, 1, 2, 1, 2, 1, 0]]),
        [Value::NoSuchObject]
    );

    // The job polls the device every second: readings arrive.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let values = manager.get(&[POLLS, READING, SYS_UP_TIME]);
        assert!(matches!(values[2], Value::TimeTicks(_)), "{values:?}");
        if let [Value::Counter32(polls), Value::Integer(reading), _] = values[..]
            && polls > 0
        {
            assert!((200..250).contains(&reading), "{values:?}");
            break;
        }
        assert!(Instant::now() < deadline, "no readings: {values:?}");
        std::thread::sleep(Duration::from_millis(200));
    }

    let walked = manager.walk(&[1, 3, 6, 1]);
    let names: Vec<&[u32]> = walked.iter().map(|(name, _)| name.as_slice()).collect();
    let objects = [
        SYS_DESCR,
        SYS_OBJECT_ID,
        SYS_UP_TIME,
        SERIAL,
        READING,
        POLLS,
        ERRORS,
    ];
    assert_eq!(names, objects);

    // Another community gets no answer.
    let other = manager.ask(b"private", GET, &[SERIAL], Duration::from_millis(500));
    assert!(other.is_none());
}

/// A new, empty root directory for one test.
fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snmp-lite-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_program_answers_the_queries_and_stops_on_sigterm() {
    let root = root("process");
    let args = [
        "--root",
        root.to_str().unwrap(),
        "--set",
        "device.serial=SN-PROCESS",
        "--set",
        "snmp.addr=127.0.0.1:0",
    ];
    let program = rivium_test::process::spawn(Path::new(BIN), &args);
    queries(program.addr_of("agent"), "SN-PROCESS");
    #[cfg(unix)]
    {
        program.signal("TERM");
        assert_eq!(program.wait().code, Some(0));
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_device_that_does_not_answer_is_counted_and_does_not_hold_up_the_stop() {
    let root = root("slow");
    let args = [
        "--root",
        root.to_str().unwrap(),
        "--set",
        "snmp.addr=127.0.0.1:0",
        "--set",
        "vendor.latency_ms=60000",
    ];
    let program = rivium_test::process::spawn(Path::new(BIN), &args);
    let mut manager = Manager::new(program.addr_of("agent"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while manager.get(&[ERRORS]) == [Value::Counter32(0)] {
        assert!(Instant::now() < deadline, "no failed polls");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(manager.get(&[POLLS]), [Value::Counter32(0)]);
    #[cfg(unix)]
    {
        // The device's blocking read gives up after its read timeout, a second, within the
        // stop timeout of 3 s.
        program.signal("TERM");
        let exited = program.wait();
        assert_eq!(exited.code, Some(0), "{exited:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The address of the `round`th `listening` event in the log file below `root`.
fn listening(root: &Path, round: usize) -> SocketAddr {
    let log = root.join("logs/snmp-lite/snmp-lite.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let found = text
            .lines()
            .filter(|line| line.contains(" listening "))
            .nth(round - 1);
        if let Some(line) = found {
            let addr = line.split("listen.addr=").nth(1).unwrap();
            return addr.split_whitespace().next().unwrap().parse().unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "no listening event in {}",
            log.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "needs a JDK 17 and the JNI library: SNMP_LITE_JNI=<its directory> (ci/slices)"]
fn the_app_answers_the_same_queries_in_a_jvm_and_starts_again() {
    let lib = std::env::var("SNMP_LITE_JNI").expect("SNMP_LITE_JNI: the JNI library's directory");
    let lib = std::fs::canonicalize(&lib).unwrap_or_else(|e| panic!("{lib}: {e}"));
    let work = root("jvm");
    let (classes, root, src) = (work.join("classes"), work.join("root"), work.join("src"));
    std::fs::create_dir_all(&root).unwrap();
    // The bridge class as rivium-jni declares it, and the driver, in the bridge's package.
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = [
        here.join("../../../../crates/rivium-jni/tests/jvm/RiviumBridge.java"),
        here.join("../snmp-lite-jni/jvm-test/Slice.java"),
    ];
    let package = src.join("com/example/snmplite");
    std::fs::create_dir_all(&package).unwrap();
    for source in &sources {
        let text = std::fs::read_to_string(source).unwrap();
        let file = package.join(source.file_name().unwrap());
        std::fs::write(file, format!("package com.example.snmplite;\n\n{text}")).unwrap();
    }
    let javac = Command::new("javac")
        .arg("-d")
        .arg(&classes)
        .args(
            sources
                .iter()
                .map(|source| package.join(source.file_name().unwrap())),
        )
        .status()
        .expect("javac");
    assert!(javac.success());

    let serial = "SN-JVM";
    let mut java = Command::new("java")
        .arg(format!("-Djava.library.path={}", lib.display()))
        .arg("-cp")
        .arg(&classes)
        .args(["com.example.snmplite.Slice", "snmp_lite_jni", "--root"])
        .arg(&root)
        .args(["--set", &format!("device.serial={serial}")])
        .args(["--set", "snmp.addr=127.0.0.1:0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("java");
    let mut stdin = java.stdin.take().unwrap();
    let mut lines = BufReader::new(java.stdout.take().unwrap()).lines();
    let mut line = || lines.next().map(Result::unwrap).unwrap_or_default();
    for round in 1..=2 {
        assert_eq!(line(), "running", "the JVM failed: see its standard error");
        queries(listening(&root, round), serial);
        writeln!(stdin, "{}", if round == 1 { "again" } else { "done" }).unwrap();
        assert_eq!(line(), "stopped", "the JVM failed: see its standard error");
    }
    assert!(java.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&work);
}
