//! The program.

fn main() -> std::process::ExitCode {
    rivium::process::run::<consumer_process::Consumer>()
}
