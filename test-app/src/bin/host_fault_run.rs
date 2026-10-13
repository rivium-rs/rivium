//! Host that fails during service run phase.

fn main() -> std::process::ExitCode {
    test_app::subjects::host_fault::main_run()
}
