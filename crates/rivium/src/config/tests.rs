//! Loading: layers and their priority, text values, sources, problems and the reserved sections.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::load::{FileLayer, Inputs, Loaded, default_config, load};
use super::{Paths, Report, Source};
use crate::lifecycle::Restart;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Config {
    device: Device,
    poll: Poll,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Device {
    serial: Option<String>,
    port: Option<u16>,
    enabled: bool,
    mode: Mode,
    tags: Vec<String>,
    aliases: BTreeMap<String, String>,
    targets: Vec<Target>,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    #[default]
    Active,
    Passive,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Target {
    host: String,
    port: u16,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Poll {
    #[serde(
        serialize_with = "crate::config::de::serialize_duration",
        deserialize_with = "crate::config::de::duration::<_, 1, 60>"
    )]
    every: Duration,
    retries: u8,
}

impl Default for Poll {
    fn default() -> Self {
        Poll {
            every: Duration::from_secs(5),
            retries: 3,
        }
    }
}

const FILE: &str = "/srv/app/config.toml";

fn load_with(
    file: Option<&str>,
    env: &[(&str, &str)],
    sets: &[(&str, &str)],
) -> Result<Loaded<Config>, Report> {
    let env: Vec<(OsString, OsString)> = env.iter().map(|(k, v)| (k.into(), v.into())).collect();
    let sets: Vec<(String, String)> = sets
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let file = match file {
        Some(text) => FileLayer::Text {
            path: FILE.into(),
            text: text.into(),
        },
        None => FileLayer::None,
    };
    load(&Inputs {
        name: "app",
        file,
        env: Some(&env),
        sets: &sets,
        host: false,
    })
}

/// The `check-config` line of a key.
fn line(loaded: &Loaded<Config>, key: &str) -> String {
    let prefix = format!("{key} = ");
    let mut lines = loaded
        .listing()
        .into_iter()
        .filter(|line| line.starts_with(&prefix));
    lines.next().unwrap_or_else(|| panic!("no line for {key}"))
}

/// The loaded configuration, or a panic that shows every problem.
fn ok(result: Result<Loaded<Config>, Report>) -> Loaded<Config> {
    result.unwrap_or_else(|report| panic!("unexpected problems:\n{report}"))
}

fn problems(report: &Report) -> Vec<String> {
    report.problems().iter().map(ToString::to_string).collect()
}

#[test]
fn options_maps_lists_and_enums_come_from_the_file_and_the_environment() {
    let file = "[device]\nserial = \"A-7\"\n[device.aliases]\nfront = \"10.0.0.1\"\n";
    let env = [
        ("APP_DEVICE__PORT", "1161"),
        ("APP_DEVICE__ENABLED", "true"),
        ("APP_DEVICE__MODE", "passive"),
        ("APP_DEVICE__TAGS", r#"["a", "b"]"#),
        ("APP_DEVICE__ALIASES__BACK", "10.0.0.2"),
        ("APP_DEVICE__TARGETS", r#"[{ host = "h", port = 1 }]"#),
        ("APP_POLL__EVERY", "1500ms"),
    ];
    let loaded = ok(load_with(Some(file), &env, &[]));
    let device = &loaded.config.device;
    assert_eq!(
        (device.serial.as_deref(), device.port, device.enabled),
        (Some("A-7"), Some(1161), true)
    );
    assert_eq!(
        (&device.mode, device.tags.as_slice()),
        (&Mode::Passive, ["a", "b"].map(String::from).as_slice())
    );
    let aliases: Vec<(&str, &str)> = device
        .aliases
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(aliases, [("back", "10.0.0.2"), ("front", "10.0.0.1")]);
    assert_eq!(
        device.targets,
        [Target {
            host: "h".into(),
            port: 1
        }]
    );
    assert_eq!(loaded.config.poll.every, Duration::from_millis(1500));
    assert_eq!(loaded.overrides, env.len());
    // A value below the 1s minimum is still a valid form; the range check rejects it below.
    assert!(load_with(None, &[("APP_POLL__EVERY", "999ms")], &[]).is_err());

    assert_eq!(
        line(&loaded, "device.serial"),
        r#"device.serial = "A-7" (file /srv/app/config.toml)"#
    );
    assert_eq!(
        line(&loaded, "device.port"),
        "device.port = 1161 (env APP_DEVICE__PORT)"
    );
    assert_eq!(
        line(&loaded, "device.aliases.front"),
        r#"device.aliases.front = "10.0.0.1" (file /srv/app/config.toml)"#
    );
    assert_eq!(
        line(&loaded, "device.aliases.back"),
        r#"device.aliases.back = "10.0.0.2" (env APP_DEVICE__ALIASES__BACK)"#
    );
    assert_eq!(
        line(&loaded, "device.targets"),
        r#"device.targets = [{ host = "h", port = 1 }] (env APP_DEVICE__TARGETS)"#
    );
    assert_eq!(line(&loaded, "poll.retries"), "poll.retries = 3 (default)");
}

#[test]
fn every_problem_is_reported_once() {
    let file = r#"
[device]
serail = "A-7"
port = "abc"
mode = "turbo"

[[device.targets]]
port = 161

[poll]
every = "0s"

[extra]
x = 1
"#;
    let report = load_with(Some(file), &[], &[]).unwrap_err();
    let source = "(file /srv/app/config.toml)";
    let expected = [
        format!(
            "invalid configuration: device.mode {source}: unknown variant `turbo`, expected `active` or `passive`"
        ),
        format!(
            "invalid configuration: device.port {source}: invalid type: string \"abc\", expected u16"
        ),
        format!(
            "invalid configuration: device.serail {source}: unknown key; did you mean `serial`?"
        ),
        format!("invalid configuration: device.targets[0].host {source}: missing field `host`"),
        format!("invalid configuration: extra {source}: unknown key"),
        format!(
            "invalid configuration: poll.every {source}: must be between 1s and 1m, got \"0s\""
        ),
    ];
    assert_eq!(problems(&report), expected);
}

#[test]
fn later_layers_win() {
    let filter = |file: Option<&str>, env: &[(&str, &str)], sets: &[(&str, &str)]| {
        let loaded = ok(load_with(file, env, sets));
        line(&loaded, "log.filter")
    };
    let file = Some("[log]\nfilter = \"debug\"\n");
    let alias = ("RUST_LOG", "trace");
    let prefixed = ("APP_LOG__FILTER", "warn");
    assert_eq!(filter(None, &[], &[]), r#"log.filter = "info" (default)"#);
    assert_eq!(
        filter(file, &[], &[]),
        r#"log.filter = "debug" (file /srv/app/config.toml)"#
    );
    assert_eq!(
        filter(file, &[alias], &[]),
        r#"log.filter = "trace" (env RUST_LOG)"#
    );
    assert_eq!(
        filter(file, &[prefixed, alias], &[]),
        r#"log.filter = "warn" (env APP_LOG__FILTER)"#
    );
    let set = [("log.filter", "error")];
    assert_eq!(
        filter(file, &[prefixed, alias], &set),
        r#"log.filter = "error" (cli --set log.filter)"#
    );
    // Empty variables are unset; names without `__` are not keys.
    let unset = [
        ("APP_LOG__FILTER", ""),
        ("RUST_LOG", ""),
        ("APP_ROOT", "/x"),
        ("APP_SERVICE_HOST", "y"),
    ];
    assert_eq!(
        filter(file, &unset, &[]),
        r#"log.filter = "debug" (file /srv/app/config.toml)"#
    );
}

#[test]
fn the_embedded_host_reads_no_environment_and_sets_with_its_own_source() {
    let sets = vec![
        ("log.filter".to_string(), "debug".to_string()),
        ("device.serial".to_string(), String::new()),
    ];
    let file = FileLayer::Text {
        path: FILE.into(),
        text: String::new(),
    };
    let loaded: Loaded<Config> = load(&Inputs {
        name: "app",
        file,
        env: None,
        sets: &sets,
        host: true,
    })
    .unwrap();
    assert_eq!(
        line(&loaded, "log.filter"),
        r#"log.filter = "debug" (host --set log.filter)"#
    );
    // `--set key=` sets an empty string.
    assert_eq!(loaded.config.device.serial.as_deref(), Some(""));
    assert_eq!(Source::Host("a".into()).to_string(), "host --set a");
}

#[test]
fn text_of_the_wrong_type_is_reported_with_its_variable() {
    let env = [
        ("APP_DEVICE__ENABLED", "yes"),
        ("APP_DEVICE__TAGS", "[unclosed"),
        ("APP_DEVICE__PORT", "70000"),
    ];
    let report = load_with(None, &env, &[("device..port", "1")]).unwrap_err();
    assert_eq!(
        problems(&report),
        [
            "invalid configuration: device..port (cli --set device..port): not a valid key",
            "invalid configuration: device.enabled (env APP_DEVICE__ENABLED): invalid type: string \"yes\", expected a boolean",
            "invalid configuration: device.port (env APP_DEVICE__PORT): invalid value: integer `70000`, expected u16",
            "invalid configuration: device.tags (env APP_DEVICE__TAGS): expected a TOML value such as [\"a\"] or { a = 1 }, got \"[unclosed\"",
        ]
    );
}

#[cfg(unix)]
#[test]
fn environment_values_must_be_utf8() {
    use std::os::unix::ffi::OsStringExt;
    let env = vec![(
        OsString::from("APP_DEVICE__SERIAL"),
        OsString::from_vec(vec![0x66, 0xff]),
    )];
    let inputs = Inputs {
        name: "app",
        file: FileLayer::None,
        env: Some(&env),
        sets: &[],
        host: false,
    };
    let report = load::<Config>(&inputs).unwrap_err();
    assert_eq!(
        problems(&report),
        ["invalid configuration: device.serial (env APP_DEVICE__SERIAL): not UTF-8 text"]
    );
}

#[test]
fn whole_file_problems_carry_the_position() {
    let report = load_with(Some("[device]\nport = = 1\n"), &[], &[]).unwrap_err();
    let text = problems(&report).join("\n");
    assert!(
        text.starts_with("invalid configuration: file /srv/app/config.toml: line 2, column 8: "),
        "{text}"
    );

    let dir = std::env::temp_dir().join(format!("rivium-config-{}", std::process::id()));
    let missing = dir.join("missing.toml");
    let path = |explicit| FileLayer::Path {
        path: missing.clone(),
        explicit,
    };
    let inputs = |file| Inputs {
        name: "app",
        file,
        env: None,
        sets: &[],
        host: false,
    };
    let loaded = load::<Config>(&inputs(path(false))).unwrap();
    assert_eq!(
        (loaded.file, loaded.missing_file),
        (None, Some(missing.clone()))
    );
    let report = load::<Config>(&inputs(path(true))).unwrap_err();
    let text = problems(&report).join("\n");
    assert!(
        text.starts_with(&format!(
            "invalid configuration: file {}: cannot be read: ",
            missing.display()
        )),
        "{text}"
    );
}

#[test]
fn the_service_must_not_define_rivium_sections() {
    #[derive(Debug, Default, Serialize, Deserialize)]
    struct Clash {
        log: BTreeMap<String, String>,
    }
    let inputs = Inputs {
        name: "app",
        file: FileLayer::None,
        env: None,
        sets: &[],
        host: false,
    };
    let report = load::<Clash>(&inputs).unwrap_err();
    assert_eq!(
        problems(&report),
        [
            "invalid configuration: log (default): is a section Rivium owns; the service's configuration must not define it"
        ]
    );
}

/// The least `log.file.max_total_size` for this many log files of 16MiB, as the problem states
/// it: with log export, its archive's share of 64MiB counts too.
fn least_total(sinks: u64) -> String {
    let (export, share) = match cfg!(feature = "log-export") {
        true => (" + log.export.max_size", 64),
        false => ("", 0),
    };
    let least = (sinks + 2) * 16 + share;
    format!(
        "must be at least {least}MiB for {sinks} log file(s): ({sinks} + 2) × log.file.max_file_size{export}"
    )
}

#[test]
fn rivium_sections_are_checked_and_listed() {
    let file = r#"
[log]
filter = ","
fiter = "x"

[log.file]
max_file_size = "16MiB"
max_total_size = "32MiB"

[[log.files]]
name = "access"
filter = "access=info"

[[log.files]]
name = "app"
filter = "nope=["

[lifecycle]
stop_timeout = "121s"
restart = "in-process"
"#;
    let report = load_with(Some(file), &[], &[]).unwrap_err();
    let source = "(file /srv/app/config.toml)";
    let mut found = problems(&report);
    let filter = found.remove(2);
    let prefix = format!("invalid configuration: log.files[1].filter {source}: not a log filter: ");
    assert!(filter.starts_with(&prefix), "{filter}");
    // The bad category is dropped while the rest is checked: two log files remain.
    assert_eq!(
        found,
        [
            format!(
                "invalid configuration: lifecycle.stop_timeout {source}: must be between 1s and 2m, got \"121s\""
            ),
            format!(
                "invalid configuration: log.file.max_total_size {source}: {}",
                least_total(2)
            ),
            format!(
                "invalid configuration: log.filter {source}: not a log filter: it has no directive"
            ),
            format!(
                "invalid configuration: log.fiter {source}: unknown key; did you mean `filter`?"
            ),
        ]
    );

    let fixed = file
        .replace("nope=[", "trace")
        .replace("121s", "10s")
        .replace("fiter = \"x\"\n", "");
    let fixed = fixed.replace("filter = \",\"", "filter = \"info\"");
    let report = load_with(Some(&fixed), &[], &[]).unwrap_err();
    assert_eq!(
        problems(&report),
        [
            format!(
                "invalid configuration: log.file.max_total_size {source}: {}",
                least_total(3)
            ),
            format!(
                "invalid configuration: log.files[1].name {source}: `app` is the name of another log file"
            ),
        ]
    );
    let fixed = fixed
        .replace("name = \"app\"", "name = \"debug\"")
        .replace("32MiB", "144MiB");
    let loaded = ok(load_with(Some(&fixed), &[], &[]));
    let lifecycle = &loaded.reserved.lifecycle;
    assert_eq!(
        (lifecycle.stop_timeout, lifecycle.restart),
        (Duration::from_secs(10), Restart::InProcess)
    );
    assert_eq!(
        line(&loaded, "log.files"),
        r#"log.files = [{ filter = "access=info", name = "access" }, { filter = "trace", name = "debug" }] (file /srv/app/config.toml)"#
    );
    assert_eq!(
        line(&loaded, "log.file.dir"),
        r#"log.file.dir = "logs/app" (default)"#
    );
    assert_eq!(
        line(&loaded, "lifecycle.startup_timeout"),
        r#"lifecycle.startup_timeout = "30s" (default)"#
    );
}

#[test]
fn the_default_configuration_round_trips() {
    let text =
        default_config::<Config>("app", &[("lifecycle.restart".into(), "in-process".into())])
            .unwrap();
    assert!(text.starts_with("[device]\n"), "{text}");
    assert!(text.contains("[log.file]\ndir = \"logs/app\"\n"), "{text}");
    assert!(text.contains("restart = \"in-process\""), "{text}");
    let loaded = ok(load_with(Some(&text), &[], &[]));
    assert_eq!(
        (loaded.config, loaded.reserved.lifecycle.restart),
        (Config::default(), Restart::InProcess)
    );
    let report = default_config::<Config>("app", &[("lifecycle.restart".into(), "never".into())])
        .unwrap_err();
    assert_eq!(
        problems(&report),
        [
            "invalid configuration: lifecycle.restart (cli --set lifecycle.restart): unknown variant `never`, expected `exit` or `in-process`"
        ]
    );
}

#[test]
fn paths_follow_the_command_line_then_the_environment_then_the_executable() {
    let base = std::env::temp_dir();
    let (srv, opt) = (base.join("srv"), base.join("opt"));
    let os = |path: &PathBuf| OsString::from(path.as_os_str());
    let env = vec![
        (OsString::from("APP_ROOT"), os(&opt)),
        (OsString::from("APP_CONFIG"), os(&opt.join("app.toml"))),
    ];
    let locate = |root: Option<&PathBuf>, config: Option<&PathBuf>, env| {
        Paths::locate(
            "app",
            root.cloned(),
            config.cloned(),
            env,
            "configs/default.toml",
        )
        .unwrap()
    };
    let (paths, explicit) = locate(
        Some(&srv),
        Some(&srv.join("app.toml")),
        Some(env.as_slice()),
    );
    assert_eq!(
        (paths.root(), paths.config_file(), explicit),
        (srv.as_path(), srv.join("app.toml").as_path(), true)
    );
    let (paths, explicit) = locate(None, None, Some(env.as_slice()));
    assert_eq!(
        (paths.root(), paths.config_file(), explicit),
        (opt.as_path(), opt.join("app.toml").as_path(), true)
    );
    assert_eq!(
        (paths.data_dir(), paths.resolve("logs")),
        (opt.join("data"), opt.join("logs"))
    );
    assert_eq!(paths.resolve(&srv), srv);

    let (paths, explicit) = locate(None, None, Some(&[]));
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert_eq!((paths.root(), explicit), (exe.parent().unwrap(), false));
    assert_eq!(
        paths.config_file(),
        exe.parent().unwrap().join("configs/default.toml")
    );
    let (paths, _) = locate(Some(&PathBuf::from("relative")), None, Some(&[]));
    assert_eq!(
        paths.root(),
        std::env::current_dir().unwrap().join("relative")
    );

    // The embedded host reads no environment and must pass the root.
    let error = Paths::locate("app", None, None, None, "config.toml").unwrap_err();
    assert_eq!(error.to_string(), "the embedded host must pass --root");
    let (paths, explicit) = locate(Some(&srv), None, None);
    assert_eq!(
        (paths.config_file(), explicit),
        (srv.join("configs/default.toml").as_path(), false)
    );
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Checked {
    #[serde(
        serialize_with = "crate::config::de::serialize_duration",
        deserialize_with = "crate::config::de::duration::<_, 0, 7200>"
    )]
    wait: Duration,
    #[serde(
        serialize_with = "crate::config::de::serialize_bytes",
        deserialize_with = "crate::config::de::bytes::<_, 1024, { 1 << 30 }>"
    )]
    size: u64,
    #[serde(deserialize_with = "crate::config::de::integer::<_, u16, 1, 65535>")]
    port: u16,
}

fn checked(text: &str) -> Result<Checked, String> {
    toml::from_str(text).map_err(|error| error.message().to_string())
}

#[test]
fn the_deserializers_check_form_and_range() {
    let read = checked("wait = \"90s\"\nsize = \"16MiB\"\nport = 161").unwrap();
    assert_eq!(
        read,
        Checked {
            wait: Duration::from_secs(90),
            size: 16 << 20,
            port: 161
        }
    );
    assert_eq!(
        checked("wait = \"250ms\"\nsize = 4096\nport = 1")
            .unwrap()
            .size,
        4096
    );
    assert_eq!(
        toml::to_string(&read).unwrap(),
        "wait = \"90s\"\nsize = \"16MiB\"\nport = 161\n"
    );
    let odd = Checked {
        wait: Duration::from_millis(1500),
        size: 1000,
        port: 1,
    };
    assert_eq!(
        toml::to_string(&odd).unwrap(),
        "wait = \"1500ms\"\nsize = 1000\nport = 1\n"
    );
    let hour = Checked {
        wait: Duration::from_secs(7200),
        size: 1 << 30,
        port: 1,
    };
    assert_eq!(
        toml::to_string(&hour).unwrap(),
        "wait = \"2h\"\nsize = \"1GiB\"\nport = 1\n"
    );

    let problem = |wait: &str, size: &str, port: &str| {
        checked(&format!("wait = {wait}\nsize = {size}\nport = {port}")).unwrap_err()
    };
    let expected = r#"a duration like "30s", "250ms", "5m" or "1h""#;
    for wait in ["\"5\"", "\"1.5s\"", "\"5 s\"", "\"-5s\"", "\"5sec\"", "5"] {
        let message = problem(wait, "1024", "1");
        assert!(
            message.starts_with("invalid ") && message.ends_with(expected),
            "{wait}: {message}"
        );
    }
    assert_eq!(
        problem("\"3h\"", "1024", "1"),
        r#"must be between 0s and 2h, got "3h""#
    );
    assert_eq!(
        problem("\"99999999999999999999h\"", "1024", "1"),
        r#"must be between 0s and 2h, got "99999999999999999999h""#
    );
    assert_eq!(
        problem("\"1s\"", "\"1MB\"", "1"),
        r#"invalid value: string "1MB", expected a size in bytes, or like "512KiB", "16MiB" or "1GiB""#
    );
    assert_eq!(
        problem("\"1s\"", "1023", "1"),
        "must be between 1KiB and 1GiB, got 1023"
    );
    assert_eq!(
        problem("\"1s\"", "\"2GiB\"", "1"),
        r#"must be between 1KiB and 1GiB, got "2GiB""#
    );
    assert_eq!(
        problem("\"1s\"", "1024", "0"),
        "must be between 1 and 65535, got 0"
    );
    assert_eq!(
        problem("\"1s\"", "1024", "\"x1\""),
        r#"invalid value: string "x1", expected an integer from 1 to 65535"#
    );
}

#[test]
fn the_deserializers_read_text() {
    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Wrapper {
        checked: Option<Checked>,
    }
    let env = [
        ("APP_CHECKED__WAIT", "5m"),
        ("APP_CHECKED__SIZE", "2048"),
        ("APP_CHECKED__PORT", "8080"),
    ];
    let env: Vec<(OsString, OsString)> = env.iter().map(|(k, v)| (k.into(), v.into())).collect();
    let inputs = Inputs {
        name: "app",
        file: FileLayer::None,
        env: Some(&env),
        sets: &[],
        host: false,
    };
    let loaded = load::<Wrapper>(&inputs).unwrap_or_else(|report| panic!("{report}"));
    let expected = Checked {
        wait: Duration::from_secs(300),
        size: 2048,
        port: 8080,
    };
    assert_eq!(loaded.config.checked, Some(expected));
}

#[test]
fn problems_stay_on_one_line() {
    let report = load_with(None, &[], &[("device.serial\nx", "1")]).unwrap_err();
    let text = report.to_string();
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains(r"device.serial\nx"), "{text}");
}
