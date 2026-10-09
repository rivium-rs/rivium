//! A clone of the `AppContext` outlives `services()`: a service that takes a new configuration
//! keeps one, checks candidates as the next start would load them, writes the valid one and
//! asks for a restart, which loads it. A test binary of its own: a process installs logging
//! once.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use rivium::embedded::{Host, Status};
use rivium::{App, AppContext, Code, Result, Service, ServiceKind};
use serde::{Deserialize, Serialize};

/// The port of each round built so far.
static PORTS: Mutex<Vec<u16>> = Mutex::new(Vec::new());
/// What the first round's checks found.
static CHECKED: Mutex<Vec<std::result::Result<(), String>>> = Mutex::new(Vec::new());

struct Configured;

#[derive(Default, Serialize, Deserialize)]
struct Config {
    port: u16,
}

impl App for Configured {
    const NAME: &'static str = "configured";
    const VERSION: &'static str = "1.0.0";
    type Config = Config;

    fn services(config: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        PORTS.lock().unwrap().push(config.port);
        let (ctx, first) = (ctx.clone(), config.port == 1);
        let api = rivium::service("api", ServiceKind::Frontline, move |run| async move {
            run.ready();
            if first {
                // As an API does: the candidates arrive once the services run.
                for candidate in ["port = \"x\"\n", "port = 2\n"] {
                    let checked = ctx.check_config(candidate).map_err(|e| e.to_string());
                    CHECKED.lock().unwrap().push(checked);
                }
                let file = ctx.paths().config_file();
                if rivium::fs::atomic_write(file, b"port = 2\n", true).is_ok() {
                    ctx.restarter().request("configuration changed");
                }
            }
            run.stopped().await;
            Ok(())
        });
        Ok(vec![api])
    }
}

#[test]
fn a_clone_of_the_context_checks_a_new_configuration_and_restarts_the_run() {
    let root = std::env::temp_dir().join(format!("rivium-context-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), "port = 1\n").unwrap();
    let host = Host::new::<Configured>();
    let args = ["--root".to_string(), root.display().to_string()];
    assert_eq!(host.start(&args), Code::Ok, "{:?}", host.last_error());
    // Rivium's panic hook writes panics to the log once logging is installed: the test's own
    // failures go to standard error as well.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        eprintln!("{info}");
    }));
    // A restart asked for so soon after the start counts as a failure: the next round is built
    // after a backoff of a second.
    let deadline = Instant::now() + Duration::from_secs(10);
    while (PORTS.lock().unwrap().len() < 2 || host.status() != Status::Running)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(*PORTS.lock().unwrap(), [1, 2], "{:?}", host.last_error());
    let checked = CHECKED.lock().unwrap().clone();
    let invalid = checked[0].as_ref().unwrap_err();
    assert!(
        invalid.starts_with("invalid configuration: port (file "),
        "{checked:?}"
    );
    assert_eq!(checked[1], Ok(()));
    let backup = std::fs::read_to_string(root.join("config.toml.bak")).unwrap();
    assert_eq!(backup, "port = 1\n");
    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
    let _ = std::fs::remove_dir_all(&root);
}
