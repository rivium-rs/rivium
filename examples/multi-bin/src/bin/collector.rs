//! The `collector` program.

fn main() -> std::process::ExitCode {
    rivium::process::run::<multi_bin::apps::Collector>()
}
