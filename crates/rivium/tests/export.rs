//! Log export end to end, through the embedded host (R-15): the exporter the services get, the
//! share of the budget kept for archives, an export that outlives a restart in the process, one
//! export at a time, and the cancellation of an export in progress when the host stops. One test
//! runs every scenario in order, because a process installs logging once, below the root of its
//! first start.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rivium::embedded::{Host, Status};
use rivium::error::Class;
use rivium::log::{Date, ExportId, ExportRequest, ExportState, LogExporter, Progress};
use rivium::{App, AppContext, Code, Restarter, Result, Service, ServiceKind};
use rivium_test::{ScriptedService, Step};
use serde::{Deserialize, Serialize};

/// What the last round got from its context.
static CONTEXT: Mutex<Option<(LogExporter, Restarter)>> = Mutex::new(None);

struct Probe;

#[derive(Default, Serialize, Deserialize)]
struct Config {}

impl App for Probe {
    const NAME: &'static str = "probe";
    const VERSION: &'static str = "1.0.0";
    type Config = Config;

    fn services(_: &Config, ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
        *CONTEXT.lock().unwrap() = Some((ctx.log_exporter().clone(), ctx.restarter().clone()));
        let steps = vec![Step::Ready, Step::UntilStopped];
        let service = ScriptedService::new("serve", ServiceKind::Frontline, steps);
        Ok(vec![Box::new(service)])
    }
}

fn current_exporter() -> LogExporter {
    CONTEXT.lock().unwrap().as_ref().unwrap().0.clone()
}

fn todays_logs() -> ExportRequest {
    ExportRequest {
        from: Date::today(),
        to: Date::today(),
        sinks: Vec::new(),
    }
}

/// Waits until the export is no longer running.
fn finished(exporter: &LogExporter, id: ExportId) -> Progress {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let progress = exporter.progress(id).unwrap();
        if progress.state != ExportState::Running || Instant::now() > deadline {
            return progress;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until the host is in `status`.
fn reaches(host: &Host, status: Status) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while host.status() != status {
        assert!(
            Instant::now() < deadline,
            "not {status:?}: {:?}",
            host.status()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The names in the directory, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = (fs::read_dir(dir).unwrap())
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Bytes that deflate poorly, so that packing them takes a while.
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            b"0123456789abcdef"[(state & 15) as usize]
        })
        .collect()
}

/// Rivium's panic hook writes panics to the log once logging is installed: this test's own
/// failures go to standard error as well.
fn failures_to_stderr() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        hook(info);
        eprintln!("{info}");
    }));
}

#[test]
fn log_export() {
    let root: PathBuf = std::env::temp_dir().join(format!("rivium-export-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let logs = root.join("logs/probe");
    fs::create_dir_all(&logs).unwrap();
    // Twenty archives of 1 MiB: the log files keep 32 MiB less 1 MiB for compression and 16 MiB
    // for log export, so starting deletes the five oldest.
    for seq in 1..=20 {
        let name = format!("probe.2026-10-01.{seq}.log.gz");
        fs::write(logs.join(name), vec![0; 1 << 20]).unwrap();
    }
    let host = Host::new::<Probe>();
    let mut args: Vec<String> = ["--root", root.to_str().unwrap()].map(String::from).into();
    for set in [
        "log.file.max_file_size=1MiB",
        "log.file.max_total_size=32MiB",
        "log.export.max_size=16MiB",
        r#"log.files=[{ name = "audit", filter = "audit=info" }]"#,
    ] {
        args.extend(["--set".into(), set.into()]);
    }
    assert_eq!(host.start(&args), Code::Ok, "{:?}", host.last_error());
    failures_to_stderr();
    let mut kept: Vec<u32> = (names(&logs).iter())
        .filter_map(|name| {
            name.strip_suffix(".log.gz")?
                .rsplit('.')
                .next()?
                .parse()
                .ok()
        })
        .collect();
    kept.sort_unstable();
    assert_eq!(kept, (6..=20).collect::<Vec<u32>>(), "{:?}", names(&logs));

    // The exporter the services get packs the lines logged before the start.
    let exporter = current_exporter();
    assert_eq!(exporter.sinks(), ["probe", "audit"]);
    tracing::info!(target: "audit", "an audited line");
    tracing::info!("the last line before the export");
    let id = exporter.start(todays_logs()).unwrap();
    assert_eq!(finished(&exporter, id).state, ExportState::Done);
    let archive = exporter.archive(id).unwrap();
    let today = Date::today();
    assert_eq!(archive.file_name, format!("probe-logs-{today}_{today}.zip"));
    let mut zip = zip::ZipArchive::new(fs::File::open(&archive.path).unwrap()).unwrap();
    let mut main = String::new();
    zip.by_name("probe.log")
        .unwrap()
        .read_to_string(&mut main)
        .unwrap();
    assert!(main.contains("the last line before the export"), "{main}");
    let packed: Vec<&str> = zip.file_names().collect();
    assert!(
        packed.contains(&"audit.log") && packed.contains(&"manifest.txt"),
        "{packed:?}"
    );

    // An export in progress outlives a restart in the process, and refuses another one.
    fs::write(logs.join(format!("probe.{today}.90.log")), noise(12 << 20)).unwrap();
    let id = exporter.start(todays_logs()).unwrap();
    let conflict = exporter.start(todays_logs()).unwrap_err();
    assert_eq!(conflict.class(), Class::Conflict, "{conflict}");
    let restarter = CONTEXT.lock().unwrap().as_ref().unwrap().1.clone();
    restarter.request("a new configuration");
    reaches(&host, Status::Restarting);
    reaches(&host, Status::Running);
    let rebuilt = current_exporter();
    assert_eq!(finished(&rebuilt, id).state, ExportState::Done);
    assert!(rebuilt.archive(id).unwrap().path.exists());

    // Stopping the host cancels an export in progress, but the exporter stays.
    let id = exporter.start(todays_logs()).unwrap();
    assert_eq!(host.stop(Duration::from_secs(4)), Code::Ok);
    assert_eq!(exporter.progress(id).unwrap().state, ExportState::Cancelled);
    let left: Vec<String> = names(&logs)
        .into_iter()
        .filter(|name| name.starts_with("export-"))
        .collect();
    assert_eq!(left, Vec::<String>::new());
    let log = fs::read_to_string(logs.join("probe.log")).unwrap();
    for event in [
        "log export started",
        "log export finished",
        "log export cancelled",
    ] {
        assert!(log.contains(event), "{event} in {log}");
    }
    let _ = fs::remove_dir_all(&root);
}
