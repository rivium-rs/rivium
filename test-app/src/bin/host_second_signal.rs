//! Host that stays stopping until a second signal aborts it (128 + that signal).

fn main() -> std::process::ExitCode {
    test_app::subjects::host_deadline::main()
}
