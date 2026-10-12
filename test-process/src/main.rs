//! The ordinary process host, built separately from test drivers.
#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    rivium::process::run::<test_app::Application>()
}
