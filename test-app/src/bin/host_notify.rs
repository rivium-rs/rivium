//! Host that sends systemd notifications (sd-notify test).

fn main() -> std::process::ExitCode {
    test_app::subjects::host::main()
}
