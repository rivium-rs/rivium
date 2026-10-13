//! Plain host reserved for hosting checks; it does not implement sd-notify.

fn main() -> std::process::ExitCode {
    test_app::subjects::host::main()
}
