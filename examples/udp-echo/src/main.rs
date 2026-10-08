//! The `udp-echo` program.

fn main() -> std::process::ExitCode {
    rivium::process::run::<udp_echo::Echo>()
}
