//! Host that responds to SIGQUIT (exits immediately with 124).

fn main() -> std::process::ExitCode {
    test_app::subjects::host::main()
}
