//! The result codes of the hosts: process exit codes and the embedded host's FFI result codes,
//! from one table.

/// What a host reports: the exit code of the process host, or the result code of the embedded
/// host's calls. The values are public API: a new code is a minor change, a changed value a
/// breaking one.
///
/// | Code | Exit code | FFI code | When |
/// | --- | --- | --- | --- |
/// | `Ok` | 0 | 0 | stopped as asked; `check-config` passed; `--version`, `--help` |
/// | `AlreadyRunning` | — | 1 | start while starting, running or restarting |
/// | `NotRunning` | — | 2 | stop while not running |
/// | `Busy` | — | 3 | start while the last stop is still tearing down |
/// | `Cancelled` | — | 4 | a first start stopped before it was running |
/// | `Usage` | 64 | −64 | the command line or start arguments are not valid |
/// | `StartupFailed` | 69 | −69 | the services did not start; a panic in the host |
/// | `Fault` | 70 | — | a service failed after it was ready |
/// | `OsError` | 71 | −71 | no runtime, thread or signal handler could be created |
/// | `CantCreate` | 73 | −73 | the log directory cannot be created or written |
/// | `Restart` | 75 | — | a service asked for a restart |
/// | `Config` | 78 | −78 | the configuration is not valid |
/// | `StopTimedOut` | 124 | −124 | services were abandoned at the stop deadline |
/// | `Aborted(n)` | 128 + n | — | a second stop request, for signal `n` |
/// | `Panicked` | — | −99 | a panic in an FFI call, caught |
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Code {
    /// Stopped as asked; `check-config` passed; `--version` or `--help`.
    Ok,
    /// Start while starting, running or restarting.
    AlreadyRunning,
    /// Stop while not running.
    NotRunning,
    /// Start while the last stop is still tearing down.
    Busy,
    /// A first start was stopped before the services were running.
    Cancelled,
    /// The command line or the start arguments are not valid.
    Usage,
    /// The services did not start, or the host itself panicked.
    StartupFailed,
    /// A service failed after it was ready.
    Fault,
    /// No runtime, thread or signal handler could be created.
    OsError,
    /// The log directory cannot be created or written.
    CantCreate,
    /// A service asked for a restart.
    Restart,
    /// The configuration is not valid.
    Config,
    /// Services were abandoned at the stop deadline, or a stop did not finish in time.
    StopTimedOut,
    /// A second stop request cut the stop short: the number of its signal, 2 or 15.
    Aborted(u8),
    /// An FFI call panicked; the panic was caught.
    Panicked,
}

impl Code {
    /// The process exit code, for the codes the process host reports.
    #[must_use]
    pub const fn exit_code(self) -> Option<u8> {
        match self {
            Code::Ok => Some(0),
            Code::Usage => Some(64),
            Code::StartupFailed => Some(69),
            Code::Fault => Some(70),
            Code::OsError => Some(71),
            Code::CantCreate => Some(73),
            Code::Restart => Some(75),
            Code::Config => Some(78),
            Code::StopTimedOut => Some(124),
            Code::Aborted(signal) => 128u8.checked_add(signal),
            Code::AlreadyRunning
            | Code::NotRunning
            | Code::Busy
            | Code::Cancelled
            | Code::Panicked => None,
        }
    }

    /// The FFI result code, for the codes the embedded host reports.
    #[must_use]
    pub const fn ffi(self) -> Option<i32> {
        match self {
            Code::Ok => Some(0),
            Code::AlreadyRunning => Some(1),
            Code::NotRunning => Some(2),
            Code::Busy => Some(3),
            Code::Cancelled => Some(4),
            Code::Usage => Some(-64),
            Code::StartupFailed => Some(-69),
            Code::OsError => Some(-71),
            Code::CantCreate => Some(-73),
            Code::Config => Some(-78),
            Code::StopTimedOut => Some(-124),
            Code::Panicked => Some(-99),
            Code::Fault | Code::Restart | Code::Aborted(_) => None,
        }
    }

    /// The name, as the prefix of the embedded host's last error: `"<name>: <message>"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Code::Ok => "Ok",
            Code::AlreadyRunning => "AlreadyRunning",
            Code::NotRunning => "NotRunning",
            Code::Busy => "Busy",
            Code::Cancelled => "Cancelled",
            Code::Usage => "Usage",
            Code::StartupFailed => "StartupFailed",
            Code::Fault => "Fault",
            Code::OsError => "OsError",
            Code::CantCreate => "CantCreate",
            Code::Restart => "Restart",
            Code::Config => "Config",
            Code::StopTimedOut => "StopTimedOut",
            Code::Aborted(_) => "Aborted",
            Code::Panicked => "Panicked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Code;

    #[test]
    fn the_code_table() {
        let table = [
            (Code::Ok, Some(0), Some(0), "Ok"),
            (Code::AlreadyRunning, None, Some(1), "AlreadyRunning"),
            (Code::NotRunning, None, Some(2), "NotRunning"),
            (Code::Busy, None, Some(3), "Busy"),
            (Code::Cancelled, None, Some(4), "Cancelled"),
            (Code::Usage, Some(64), Some(-64), "Usage"),
            (Code::StartupFailed, Some(69), Some(-69), "StartupFailed"),
            (Code::Fault, Some(70), None, "Fault"),
            (Code::OsError, Some(71), Some(-71), "OsError"),
            (Code::CantCreate, Some(73), Some(-73), "CantCreate"),
            (Code::Restart, Some(75), None, "Restart"),
            (Code::Config, Some(78), Some(-78), "Config"),
            (Code::StopTimedOut, Some(124), Some(-124), "StopTimedOut"),
            (Code::Aborted(2), Some(130), None, "Aborted"),
            (Code::Aborted(15), Some(143), None, "Aborted"),
            (Code::Panicked, None, Some(-99), "Panicked"),
        ];
        for (code, exit, ffi, name) in table {
            assert_eq!(
                (code.exit_code(), code.ffi(), code.name()),
                (exit, ffi, name)
            );
        }
    }
}
