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

mod cli;

use std::ffi::OsString;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::Code;
use crate::app::{App, AppContext, Identity};
use crate::config::Paths;
use crate::config::de::format_duration;
use crate::config::load::{FileLayer, Inputs, Loaded, default_config, load};
use crate::host::{self, End, Failure, Overrides, Round};
use crate::lifecycle::{Restart, StopReason, Supervisor};
use crate::log::{self, InstallError, LogInputs, payload_text};
use cli::{Command, Options};

/// How long the runtime may take to shut down once the services have ended.
const RUNTIME_SHUTDOWN: Duration = Duration::from_millis(400);
/// How long the last log lines may take to reach the log files.
const FLUSH: Duration = Duration::from_millis(500);
/// What the host takes after the stop deadline: the runtime, a log export being cancelled
/// (100ms), and the flush.
const AFTER_DEADLINE: Duration = Duration::from_secs(1);

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
    let (root, config) = (options.root.clone(), options.config.clone());
    let (paths, explicit) = Paths::locate(A::NAME, root, config, Some(env), A::CONFIG_FILE)
        .map_err(|error| {
            report(A::NAME, &format!("{error:#}"));
            Code::Usage
        })?;
    let file = FileLayer::Path {
        path: paths.config_file().to_path_buf(),
        explicit,
    };
    let inputs = Inputs {
        name: A::NAME,
        file,
        env: Some(env),
        sets: &options.sets,
        host: false,
    };
    let loaded = load::<A::Config>(&inputs).map_err(|problems| {
        report(A::NAME, &problems.to_string());
        Code::Config
    })?;
    Ok((paths, explicit, loaded))
}

/// `run`: loads the configuration, installs logging, then runs rounds of services until one
/// ends the program.
fn serve<A: App>(options: Options, env: &[(OsString, OsString)]) -> Code {
    let (paths, explicit, loaded) = match configure::<A>(&options, env) {
        Ok(configured) => configured,
        Err(code) => return code,
    };
    let reserved = &loaded.reserved;
    let inputs = LogInputs {
        name: A::NAME.to_string(),
        settings: reserved.log.clone(),
        dir: paths.resolve(&reserved.log.file.dir),
        console: true,
        logcat: false,
        layers: A::log_layers(&loaded.config),
    };
    if let Err(error) = log::install(inputs) {
        report(A::NAME, &error.to_string());
        return match error {
            InstallError::Changed(_) => Code::Config,
            InstallError::Io(_) => Code::CantCreate,
        };
    }
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
                env,
                sets: options.sets,
                first: Some(loaded),
            };
            let end = runtime.block_on(async {
                let (stop, mut stops) = mpsc::unbounded_channel();
                match os_stops(stop) {
                    Ok(()) => host::run(|_| rounds.build(), in_process, &mut stops).await,
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

/// What the process host builds each round from.
struct Rounds<'a, A: App> {
    paths: Paths,
    explicit: bool,
    env: &'a [(OsString, OsString)],
    sets: Vec<(String, String)>,
    /// The configuration of the first round, loaded before logging was installed.
    first: Option<Loaded<A::Config>>,
}

impl<A: App> Rounds<'_, A> {
    /// The next round: its configuration, loaded again after the first round, and its
    /// services.
    fn build(&mut self) -> Result<Round, Failure> {
        let loaded = match self.first.take() {
            Some(loaded) => loaded,
            None => self.reload()?,
        };
        let lifecycle = &loaded.reserved.lifecycle;
        let in_process = lifecycle.restart == Restart::InProcess;
        let supervisor = Supervisor::new(lifecycle.startup_timeout, lifecycle.stop_timeout);
        let identity = Identity {
            name: A::NAME,
            version: A::VERSION,
            instance: lifecycle.instance.clone(),
        };
        let file = self.paths.config_file().to_path_buf();
        let overrides = Overrides {
            env: Some(self.env.to_vec()),
            sets: self.sets.clone(),
            host: false,
        };
        let installed = in_process.then(|| loaded.reserved.log.clone());
        let check = host::check::<A::Config>(A::NAME, file, overrides, installed);
        let ctx = AppContext::new(self.paths.clone(), identity, &supervisor, check);
        let services = A::services(&loaded.config, &ctx).map_err(|error| {
            let why = format!("startup failed: the services cannot be built: {error:#}");
            Failure::new(Code::StartupFailed, why)
        })?;
        let supervisor = services.into_iter().fold(supervisor, Supervisor::with);
        Ok(Round {
            supervisor,
            in_process,
        })
    }

    /// Loads the configuration again and reloads the log filters.
    fn reload(&self) -> Result<Loaded<A::Config>, Failure> {
        let file = FileLayer::Path {
            path: self.paths.config_file().to_path_buf(),
            explicit: self.explicit,
        };
        let inputs = Inputs {
            name: A::NAME,
            file,
            env: Some(self.env),
            sets: &self.sets,
            host: false,
        };
        let loaded = load::<A::Config>(&inputs)
            .map_err(|problems| Failure::new(Code::Config, problems.to_string()))?;
        let settings = &loaded.reserved.log;
        let inputs = LogInputs {
            name: A::NAME.to_string(),
            settings: settings.clone(),
            dir: self.paths.resolve(&settings.file.dir),
            console: true,
            logcat: false,
            layers: Vec::new(),
        };
        log::install(inputs).map_err(|error| Failure::new(Code::Config, error.to_string()))?;
        Ok(loaded)
    }
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
