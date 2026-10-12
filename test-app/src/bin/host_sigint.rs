//! Host that responds to SIGINT (same behavior as SIGTERM).

fn main() -> std::process::ExitCode {
    test_app::subjects::host::main()
}
