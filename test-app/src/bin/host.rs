//! The basic host: starts, stops on SIGTERM with 0, and flushes logs.

fn main() -> std::process::ExitCode {
    test_app::subjects::host::main()
}
