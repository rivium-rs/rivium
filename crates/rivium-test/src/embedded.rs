//! The lifecycle contract of a program that runs embedded, checked by driving its
//! [`Host`] in the test process.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rivium::embedded::{Host, Status};
use rivium::{App, Code};

/// Checks the lifecycle contract that every program built on Rivium keeps when it runs
/// embedded, by driving a [`Host`] of `A` below `root`, a directory that every start uses:
/// empty, or with just the configuration file that an app would ship. The program must run
/// there with its defaults and `args`, which every start gets after `--root`, as the app would
/// pass them: where the defaults cannot run in a test, such as a privileged port, `args` set
/// what can, such as `["--set", "snmp.addr=127.0.0.1:0"]`. The contract:
///
/// - `start` without `--root`, or with an unknown argument, returns `Usage`; with an invalid
///   configuration it returns `Config` before anything starts; `last_error` says why;
/// - `start` gets the services running: `Ok`, `status()` running and no last error; a second
///   `start` returns `AlreadyRunning`;
/// - `stop` with the default budget of 4 seconds returns `Ok` within it, with `status()` idle
///   and the last lines in the log file; a second `stop` returns `NotRunning`;
/// - three more starts and stops succeed.
///
/// Call it from a test binary of its own: a process installs logging once, below the root of
/// its first start. Once logging is installed, Rivium's panic hook writes panics to the log;
/// this suite writes them to standard error too, so that its failures show.
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
/// #[test]
/// fn the_lifecycle_contract() {
///     let root = std::env::temp_dir().join(format!("agent-contract-{}", std::process::id()));
///     std::fs::create_dir_all(&root).unwrap();
///     rivium_test::embedded::lifecycle_contract::<Agent>(&root, &[]);
/// }
/// ```
///
/// # Panics
///
/// When the program breaks the contract.
pub fn lifecycle_contract<A: App>(root: &Path, args: &[&str]) {
    let host = Host::new::<A>();
    // `args` come before the contract's own settings, which therefore win.
    let rooted = |more: &[&str]| -> Vec<String> {
        let root = root.display().to_string();
        (["--root", root.as_str()].iter().chain(args).chain(more))
            .map(|arg| (*arg).to_string())
            .collect()
    };
    let error = |host: &Host| host.last_error().unwrap_or_default();

    let files = files(root);
    assert_eq!(host.start(&[]), Code::Usage, "without --root");
    let unknown = rooted(&["--unknown"]);
    assert_eq!(host.start(&unknown), Code::Usage, "{unknown:?}");
    assert!(error(&host).starts_with("Usage: "), "{}", error(&host));
    let invalid = rooted(&["--set", "lifecycle.stop_timeout=0s"]);
    assert_eq!(host.start(&invalid), Code::Config, "{invalid:?}");
    let expected = "Config: invalid configuration: lifecycle.stop_timeout";
    assert!(error(&host).starts_with(expected), "{}", error(&host));
    assert_eq!(
        files,
        self::files(root),
        "nothing starts with an invalid configuration"
    );
    assert_eq!(host.status(), Status::Idle);
    failures_to_stderr();

    let log = root.join(format!("logs/{0}/{0}.log", A::NAME));
    for round in 0..4 {
        assert_eq!(
            host.start(&rooted(&[])),
            Code::Ok,
            "start {round}: {}",
            error(&host)
        );
        assert_eq!((host.status(), host.last_error()), (Status::Running, None));
        assert_eq!(host.start(&rooted(&[])), Code::AlreadyRunning);
        let asked = Instant::now();
        assert_eq!(
            host.stop(Duration::from_secs(4)),
            Code::Ok,
            "{}",
            error(&host)
        );
        let took = asked.elapsed();
        assert!(took < Duration::from_secs(4), "the stop took {took:?}");
        assert_eq!(host.status(), Status::Idle);
        let logged = std::fs::read_to_string(&log).unwrap_or_default();
        let stopped = logged
            .lines()
            .filter(|line| line.contains("stopped code=\"Ok\""));
        assert_eq!(
            stopped.count(),
            round + 1,
            "the last lines are in {}",
            log.display()
        );
        assert_eq!(host.stop(Duration::from_secs(4)), Code::NotRunning);
    }
}

/// The files and directories below `root`.
fn files(root: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(root).expect("read the root directory");
    let mut files: Vec<PathBuf> = entries
        .map(|entry| entry.expect("an entry").path())
        .collect();
    files.sort();
    files
}

/// Writes panics to standard error as well as through the panic hook installed so far.
fn failures_to_stderr() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        eprintln!("{info}");
    }));
}
