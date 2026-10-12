//! Host that fails during service construction (build phase).

fn main() -> std::process::ExitCode {
    test_app::subjects::host_fault::main_build()
}
