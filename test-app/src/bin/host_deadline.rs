//! Host with a service that doesn't respond to stop (deadline test).

fn main() -> std::process::ExitCode {
    test_app::subjects::host_deadline::main()
}
