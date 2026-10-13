//! Host that fails during service ready phase.

fn main() -> std::process::ExitCode {
    test_app::subjects::host_fault::main_ready()
}
