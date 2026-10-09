//! Platform smoke check: runs the HTTP and TLS round trips of the library on a multi-threaded
//! runtime, so that an artifact built for another target or glibc baseline can be exercised
//! where it will run, as on CentOS 7 for the glibc 2.17 builds.
//!
//! ```text
//! smoke check
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args != ["check"] {
        eprintln!("usage: smoke check");
        return ExitCode::from(64);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let http = runtime.block_on(smoke::http_round_trip());
    let tls = smoke::tls_round_trip();
    match (http, tls) {
        (Ok(response), Ok(tls)) if response.starts_with("HTTP/1.1 200 OK") => {
            println!("smoke check ok: http 200, tls {tls}");
            ExitCode::SUCCESS
        }
        (http, tls) => {
            eprintln!("smoke check failed: http {http:?}, tls {tls:?}");
            ExitCode::FAILURE
        }
    }
}
