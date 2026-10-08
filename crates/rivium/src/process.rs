//! The process host: runs an [`App`] as a program of its own, under a supervisor such as
//! systemd, launchd or a Windows service wrapper, or from a terminal.
//!
//! ```text
//! <bin> [run] [--root DIR | WorkDir=DIR] [--config FILE] [--set KEY=VALUE]...
//! <bin> check-config [--root DIR | WorkDir=DIR] [--config FILE] [--set KEY=VALUE]...
//! <bin> default-config [--set KEY=VALUE]...
//! <bin> --version | -V | --help | -h
//! ```
//!
//! Without arguments the program runs. SIGTERM and SIGINT stop it, a second one aborts the
//! stop; on Windows CTRL_C and CTRL_BREAK count as SIGINT, CTRL_CLOSE, CTRL_LOGOFF and
//! CTRL_SHUTDOWN as SIGTERM. SIGHUP is logged and ignored: a new configuration takes a restart.
//! The program exits with a [`Code`]; when something went wrong, it says why on stderr, each
//! line after the service's name. After the services' stop deadline (`lifecycle.stop_timeout`)
//! the program takes at most one more second to exit.

pub(crate) mod cli;

use std::ffi::OsString;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::Code;
use crate::app::App;
use crate::config::Paths;
use crate::config::de::format_duration;
use crate::config::load::{Loaded, default_config};
use crate::host::{
    self, AFTER_DEADLINE, EXPORT_CANCEL, End, FLUSH, Failure, RUNTIME_SHUTDOWN, Rounds,
};
use crate::lifecycle::{Restart, StopReason};
use crate::log::{self, payload_text};
use cli::{Command, Options};

/// Runs the program `A` and returns its exit code; `main` returns it:
///
/// ```no_run
/// # struct Agent;
/// # impl rivium::App for Agent {
/// #     const NAME: &'static str = "agent";
/// #     const VERSION: &'static str = "0.1.0";
/// #     type Config = ();
/// #     fn services(_: &(), _: &rivium::AppContext) -> rivium::Result<Vec<Box<dyn rivium::Service>>> {
/// #         Ok(Vec::new())
/// #     }
/// # }
/// fn main() -> std::process::ExitCode {
///     rivium::process::run::<Agent>()
/// }
/// ```
#[must_use]
pub fn run<A: App>() -> ExitCode {
    log::install_panic_hook(A::NAME);
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let code = panic::catch_unwind(AssertUnwindSafe(|| main::<A>(&args, &env))).unwrap_or_else(
        |payload| {
            let why = format!("startup failed: panic: {}", payload_text(&*payload));
            report(A::NAME, &why);
            Code::StartupFailed
        },
    );
    // Every code the process host reports has an exit code.
    ExitCode::from(code.exit_code().unwrap_or(70))
}

fn main<A: App>(args: &[OsString], env: &[(OsString, OsString)]) -> Code {
    let prefix = crate::config::load::env_prefix(A::NAME);
    match cli::parse(args) {
        Err(problem) => {
            report(
                A::NAME,
                &format!("{problem}\nusage: run `{} --help`", A::NAME),
            );
            Code::Usage
        }
        Ok(Command::Version) => print(&format!("{} {}\n", A::NAME, A::VERSION)),
        Ok(Command::Help) => print(&cli::help(A::NAME, A::VERSION, A::CONFIG_FILE, &prefix)),
        Ok(Command::DefaultConfig(sets)) => match default_config::<A::Config>(A::NAME, &sets) {
            Ok(text) => print(&text),
            Err(problems) => {
                report(A::NAME, &problems.to_string());
                Code::Config
            }
        },
        Ok(Command::CheckConfig(options)) => match configure::<A>(&options, env) {
            Ok((_, _, loaded)) => {
                if let Some(missing) = &loaded.missing_file {
                    let why = format!(
                        "no configuration file at {}; the defaults apply",
                        missing.display()
                    );
                    report(A::NAME, &why);
                }
                print(&(loaded.listing().join("\n") + "\n"))
            }
            Err(code) => code,
        },
        Ok(Command::Run(options)) => serve::<A>(options, env),
    }
}

/// Finds the files and loads the configuration, as `run` and `check-config` both do; on
/// failure, says why on stderr and returns the code.
fn configure<A: App>(
    options: &Options,
    env: &[(OsString, OsString)],
) -> Result<(Paths, bool, Loaded<A::Config>), Code> {
    host::configure::<A>(options, Some(env)).map_err(|failure| {
        report(A::NAME, &failure.message);
        failure.code
    })
}

/// `run`: loads the configuration, installs logging, then runs rounds of services until one
/// ends the program.
fn serve<A: App>(options: Options, env: &[(OsString, OsString)]) -> Code {
    let (paths, explicit, loaded) = match configure::<A>(&options, env) {
        Ok(configured) => configured,
        Err(code) => return code,
    };
    if let Err(failure) = host::install_logging::<A>(&paths, &loaded, false) {
        report(A::NAME, &failure.message);
        return failure.code;
    }
    let reserved = &loaded.reserved;
    let lifecycle = &reserved.lifecycle;
    tracing::info!(
        service.name = A::NAME,
        service.version = A::VERSION,
        service.instance = %lifecycle.instance,
        root = %paths.root().display(),
        config.file = loaded.file.as_ref().map(|file| file.display().to_string()),
        overrides = loaded.overrides,
        "starting"
    );
    if let Some(missing) = &loaded.missing_file {
        let file = missing.display();
        tracing::warn!(config.file = %file, "no configuration file; the defaults apply");
    }
    if cfg!(panic = "abort") {
        tracing::warn!(
            "built with panic = \"abort\": a panic ends the process instead of failing its service"
        );
    }
    warn_beyond_kill_limit(lifecycle.stop_timeout);
    let in_process = lifecycle.restart == Restart::InProcess;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name(format!("{}-worker", A::NAME))
        .build();
    let end = match runtime {
        Err(error) => {
            Failure::new(Code::OsError, format!("cannot build the runtime: {error}")).into()
        }
        Ok(runtime) => {
            let mut rounds = Rounds::<A> {
                paths,
                explicit,
                env: Some(env),
                sets: options.sets,
                first: Some(loaded),
            };
            let end = runtime.block_on(async {
                let (stop, mut stops) = mpsc::unbounded_channel();
                match os_stops(stop) {
                    Ok(()) => {
                        let build = |_| rounds.build();
                        host::run(build, in_process, true, &mut stops, |_| {}).await
                    }
                    Err(error) => {
                        let why = format!("cannot handle stop requests: {error}");
                        Failure::new(Code::OsError, why).into()
                    }
                }
            });
            runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
            end
        }
    };
    log::cancel_export(EXPORT_CANCEL);
    finish(A::NAME, &end);
    end.code
}

/// Logs how the program ends, says why on stderr when it went wrong, and waits for the logs.
fn finish(name: &str, end: &End) {
    let (code, exit_code) = (end.code.name(), end.code.exit_code());
    match &end.message {
        None => tracing::info!(code, exit_code, "stopped"),
        Some(why) => {
            tracing::warn!(code, exit_code, reason = %why, "stopped");
            report(name, why);
        }
    }
    let _ = log::flush(FLUSH);
}

/// Warns when the stop budget reaches the time after which this platform's supervisor kills
/// the process: about 5s for launchd, 15s for the Windows service wrapper, 90s for systemd.
fn warn_beyond_kill_limit(stop_timeout: Duration) {
    let limit = if cfg!(target_os = "macos") {
        5
    } else if cfg!(windows) {
        15
    } else if cfg!(target_os = "linux") {
        90
    } else {
        return;
    };
    let budget = stop_timeout + AFTER_DEADLINE;
    if budget >= Duration::from_secs(limit) {
        let (budget, limit) = (format_duration(budget), format!("{limit}s"));
        tracing::warn!(
            budget,
            limit,
            "the stop budget reaches the supervisor's kill limit"
        );
    }
}

/// Passes the operating system's stop requests on: SIGTERM and SIGINT; SIGHUP is logged and
/// ignored.
#[cfg(unix)]
fn os_stops(stop: mpsc::UnboundedSender<StopReason>) -> std::io::Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        loop {
            let (name, number) = tokio::select! {
                _ = terminate.recv() => ("SIGTERM", 15),
                _ = interrupt.recv() => ("SIGINT", 2),
                _ = hangup.recv() => {
                    tracing::warn!(signal = "SIGHUP", "reloading is not supported; ignored");
                    continue;
                }
            };
            if stop.send(StopReason::Signal { name, number }).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// Passes the console's control events on as stop requests.
#[cfg(windows)]
fn os_stops(stop: mpsc::UnboundedSender<StopReason>) -> std::io::Result<()> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
    let (mut c, mut brk) = (ctrl_c()?, ctrl_break()?);
    let (mut close, mut logoff, mut shutdown) = (ctrl_close()?, ctrl_logoff()?, ctrl_shutdown()?);
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                _ = c.recv() => "CTRL_C",
                _ = brk.recv() => "CTRL_BREAK",
                _ = close.recv() => "CTRL_CLOSE",
                _ = logoff.recv() => "CTRL_LOGOFF",
                _ = shutdown.recv() => "CTRL_SHUTDOWN",
            };
            if stop.send(console_event(event)).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// The stop request a console control event stands for: CTRL_C and CTRL_BREAK as SIGINT (2),
/// CTRL_CLOSE, CTRL_LOGOFF and CTRL_SHUTDOWN as SIGTERM (15).
#[cfg(any(windows, test))]
fn console_event(name: &'static str) -> StopReason {
    let number = match name {
        "CTRL_C" | "CTRL_BREAK" => 2,
        _ => 15,
    };
    StopReason::Signal { name, number }
}

/// Writes to stdout; a closed stdout, as with `| head -0`, is ignored.
fn print(text: &str) -> Code {
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
    Code::Ok
}

/// Writes each line of a message to stderr after the service's name.
fn report(name: &str, message: &str) {
    let mut stderr = std::io::stderr().lock();
    for line in message.lines() {
        let _ = writeln!(stderr, "{name}: {line}");
    }
}

#[cfg(test)]
mod tests;
