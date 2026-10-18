use rivium_host::Code;

#[test]
fn process_and_embedded_codes_keep_their_external_values() {
    for (code, exit, ffi) in [
        (Code::Ok, Some(0), Some(0)),
        (Code::AlreadyRunning, None, Some(1)),
        (Code::NotRunning, None, Some(2)),
        (Code::Busy, None, Some(3)),
        (Code::Cancelled, None, Some(4)),
        (Code::Usage, Some(64), Some(-64)),
        (Code::StartupFailed, Some(69), Some(-69)),
        (Code::Fault, Some(70), None),
        (Code::OsError, Some(71), Some(-71)),
        (Code::CantCreate, Some(73), Some(-73)),
        (Code::Restart, Some(75), None),
        (Code::Config, Some(78), Some(-78)),
        (Code::StopTimedOut, Some(124), Some(-124)),
        (Code::Aborted(2), Some(130), None),
        (Code::Aborted(15), Some(143), None),
        (Code::Panicked, None, Some(-99)),
    ] {
        if let Some(exit) = exit {
            assert_eq!(code.exit(), exit);
        }
        if let Some(ffi) = ffi {
            assert_eq!(code.ffi(), ffi);
        }
    }
}
