//! The `http-api` program.

fn main() -> std::process::ExitCode {
    rivium::process::run::<http_api::Api>()
}
