//! The `beacon` program: the beacon service library on the process host.

fn main() -> std::process::ExitCode {
    rivium::process::run::<beacon::Beacon>()
}
