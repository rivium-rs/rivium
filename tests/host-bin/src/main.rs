//! Ordinary process subject; the application is shared independently of its host.
#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    rivium::process::run::<rivium_verify_app::VerifyApp>()
}
