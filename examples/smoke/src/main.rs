//! Smoke service for the CI baseline: a long-running process that stops gracefully when the
//! platform asks it to (SIGTERM/SIGINT, or a Windows console control event), and a mode that exits
//! with a given code. The hosting jobs run it under systemd, WinSW and launchd.
//!
//! ```text
//! smoke serve [--log FILE] [--exit-once CODE --marker FILE]
//! smoke exit CODE
//! smoke check
//! ```
//!
//! `check` runs the HTTP and TLS round trips of the library on a multi-threaded runtime, so an
//! artifact built for another target or glibc baseline can be exercised where it will run.
//!
//! `serve` prints `ready` on stdout and to the log once its stop handlers are installed. With `--exit-once`, the
//! first run (when MARKER does not exist yet) creates MARKER and exits with CODE, so a supervisor's
//! restart can be observed; later runs serve normally.

use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match args.first().map(String::as_str) {
        Some("exit") => args
            .get(1)
            .and_then(|code| code.parse::<u8>().ok())
            .map(ExitCode::from),
        Some("serve") => Serve::parse(&args[1..]).map(Serve::run),
        Some("check") if args.len() == 1 => Some(check()),
        _ => None,
    };
    parsed.unwrap_or_else(|| {
        eprintln!("usage: smoke serve [--log FILE] [--exit-once CODE --marker FILE]");
        eprintln!("       smoke exit CODE | smoke check");
        ExitCode::from(64)
    })
}

fn check() -> ExitCode {
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

#[derive(Default)]
struct Serve {
    log: Option<PathBuf>,
    exit_once: Option<(u8, PathBuf)>,
}

impl Serve {
    fn parse(args: &[String]) -> Option<Self> {
        let (mut serve, mut code, mut marker) = (Serve::default(), None, None);
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            let value = args.next()?;
            match flag.as_str() {
                "--log" => serve.log = Some(value.into()),
                "--exit-once" => code = Some(value.parse().ok()?),
                "--marker" => marker = Some(PathBuf::from(value)),
                _ => return None,
            }
        }
        match (code, marker) {
            (Some(code), Some(marker)) => serve.exit_once = Some((code, marker)),
            (None, None) => {}
            _ => return None,
        }
        Some(serve)
    }

    fn run(self) -> ExitCode {
        self.log(&format!("started pid={}", std::process::id()));
        if let Some((code, marker)) = &self.exit_once
            && !marker.exists()
        {
            std::fs::write(marker, b"").expect("create marker file");
            self.log(&format!("exiting code={code}"));
            return ExitCode::from(*code);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("smoke-worker")
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let signal = runtime.block_on(async {
            let stop = stop_requested().expect("install stop handlers");
            println!("ready pid={}", std::process::id());
            self.log("ready");
            stop.await
        });
        self.log(&format!("stop signal={signal}"));
        runtime.shutdown_timeout(Duration::from_millis(400));
        self.log("stopped");
        ExitCode::SUCCESS
    }

    fn log(&self, line: &str) {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let line = format!("{millis} {line}\n");
        match &self.log {
            Some(path) => OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| file.write_all(line.as_bytes()))
                .expect("write log file"),
            None => eprint!("{line}"),
        }
    }
}

/// Installs the stop handlers now and returns a future that resolves with the first request.
#[cfg(unix)]
fn stop_requested() -> std::io::Result<impl Future<Output = &'static str>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = term.recv() => "SIGTERM",
            _ = int.recv() => "SIGINT",
        }
    })
}

/// Installs the stop handlers now and returns a future that resolves with the first request.
#[cfg(windows)]
fn stop_requested() -> std::io::Result<impl Future<Output = &'static str>> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
    let (mut c, mut brk) = (ctrl_c()?, ctrl_break()?);
    let (mut close, mut logoff, mut shutdown) = (ctrl_close()?, ctrl_logoff()?, ctrl_shutdown()?);
    Ok(async move {
        tokio::select! {
            _ = c.recv() => "CTRL_C",
            _ = brk.recv() => "CTRL_BREAK",
            _ = close.recv() => "CTRL_CLOSE",
            _ = logoff.recv() => "CTRL_LOGOFF",
            _ = shutdown.recv() => "CTRL_SHUTDOWN",
        }
    })
}
