//! Host that keeps the default Unix SIGQUIT termination behavior.

fn main() -> std::process::ExitCode {
    test_app::subjects::host::main()
}
