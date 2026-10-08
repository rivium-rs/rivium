//! The command line: a small fixed syntax, parsed with std only.
//!
//! ```text
//! <bin> [run] [--root DIR | WorkDir=DIR] [--config FILE] [--set KEY=VALUE]...
//! <bin> check-config [--root DIR | WorkDir=DIR] [--config FILE] [--set KEY=VALUE]...
//! <bin> default-config [--set KEY=VALUE]...
//! <bin> --version | -V | --help | -h
//! ```

use std::ffi::OsString;
use std::path::PathBuf;

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Command {
    Run(Options),
    CheckConfig(Options),
    DefaultConfig(Vec<(String, String)>),
    Version,
    Help,
}

/// Where the configuration comes from: the options of `run`, which are also the arguments of
/// the embedded host's start.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) root: Option<PathBuf>,
    pub(crate) config: Option<PathBuf>,
    pub(crate) sets: Vec<(String, String)>,
}

/// Parses the arguments after the program name; the error says what is wrong.
pub(super) fn parse(args: &[OsString]) -> Result<Command, String> {
    if let [only] = args {
        match only.to_str() {
            Some("--version" | "-V") => return Ok(Command::Version),
            Some("--help" | "-h") => return Ok(Command::Help),
            _ => {}
        }
    }
    let (command, rest) = match args.first().and_then(|arg| arg.to_str()) {
        Some(command @ ("run" | "check-config" | "default-config")) => (command, &args[1..]),
        _ => ("run", args),
    };
    let options = options(rest, command == "default-config")?;
    Ok(match command {
        "check-config" => Command::CheckConfig(options),
        "default-config" => Command::DefaultConfig(options.sets),
        _ => Command::Run(options),
    })
}

/// Parses options; with `sets_only`, as for `default-config`, only `--set`.
pub(crate) fn options(args: &[OsString], sets_only: bool) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |option: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{option} needs a value"))
        };
        match arg.to_str() {
            Some("--set") => {
                let pair = value("--set")?;
                let pair = pair.to_str().ok_or("--set needs UTF-8 text")?;
                let (key, value) = (pair.split_once('='))
                    .filter(|(key, _)| !key.is_empty())
                    .ok_or_else(|| format!("--set needs KEY=VALUE, not `{pair}`"))?;
                options.sets.push((key.into(), value.into()));
            }
            Some("--root") if !sets_only => once(&mut options.root, value("--root")?)?,
            Some("--config") if !sets_only => {
                let file = value("--config")?;
                if options.config.replace(file.into()).is_some() {
                    return Err("--config is given twice".into());
                }
            }
            // The installer's form: `WorkDir=DIR`, with the key in any case.
            Some(text) if !sets_only && work_dir(text).is_some() => {
                let dir = work_dir(text).unwrap_or_default();
                if dir.is_empty() {
                    return Err("WorkDir= needs a directory".into());
                }
                once(&mut options.root, dir.into())?;
            }
            _ => return Err(format!("unknown argument `{}`", arg.to_string_lossy())),
        }
    }
    Ok(options)
}

fn work_dir(arg: &str) -> Option<&str> {
    let (key, dir) = arg.split_once('=')?;
    key.eq_ignore_ascii_case("workdir").then_some(dir)
}

fn once(root: &mut Option<PathBuf>, dir: OsString) -> Result<(), String> {
    match root.replace(dir.into()) {
        Some(_) => Err("the root is given twice (--root or WorkDir=)".into()),
        None => Ok(()),
    }
}

/// The text of `--help`.
pub(super) fn help(name: &str, version: &str, config_file: &str, prefix: &str) -> String {
    format!(
        "{name} {version}

Usage:
  {name} [run] [--root DIR | WorkDir=DIR] [--config FILE] [--set KEY=VALUE]...
  {name} check-config [--root DIR | WorkDir=DIR] [--config FILE] [--set KEY=VALUE]...
  {name} default-config [--set KEY=VALUE]...
  {name} --version | --help

Commands:
  run             Runs the service until it is asked to stop (the default)
  check-config    Checks the configuration and prints every key with its value and source
  default-config  Prints the default configuration, with the --set values

Options:
  --root DIR, WorkDir=DIR  The root directory; default: {prefix}_ROOT, else the executable's directory
  --config FILE            The configuration file; default: {prefix}_CONFIG, else <root>/{config_file}
  --set KEY=VALUE          Sets a key of the configuration, such as log.filter=debug

Environment:
  {prefix}_<SECTION>__<KEY>  Sets a key of the configuration, such as {prefix}_LOG__FILTER=debug
  RUST_LOG                 Sets log.filter, below {prefix}_LOG__FILTER
"
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{Command, Options, parse};

    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn run(root: Option<&str>, config: Option<&str>, sets: &[(&str, &str)]) -> Command {
        Command::Run(Options {
            root: root.map(PathBuf::from),
            config: config.map(PathBuf::from),
            sets: sets
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    #[test]
    fn the_command_line_syntax() {
        let cases: Vec<(&[&str], Command)> = vec![
            (&[], run(None, None, &[])),
            (&["run"], run(None, None, &[])),
            (&["WorkDir=/srv/edge"], run(Some("/srv/edge"), None, &[])),
            (&["workdir=/srv/edge"], run(Some("/srv/edge"), None, &[])),
            (
                &[
                    "--root",
                    "/srv",
                    "--config",
                    "a.toml",
                    "--set",
                    "log.filter=debug",
                    "--set",
                    "a.b=",
                ],
                run(
                    Some("/srv"),
                    Some("a.toml"),
                    &[("log.filter", "debug"), ("a.b", "")],
                ),
            ),
            (
                &["--set", "url=http://h/?a=b"],
                run(None, None, &[("url", "http://h/?a=b")]),
            ),
            (
                &["check-config", "--root", "/srv"],
                Command::CheckConfig(Options {
                    root: Some("/srv".into()),
                    ..Options::default()
                }),
            ),
            (
                &["default-config", "--set", "lifecycle.restart=in-process"],
                Command::DefaultConfig(vec![("lifecycle.restart".into(), "in-process".into())]),
            ),
            (&["--version"], Command::Version),
            (&["-V"], Command::Version),
            (&["--help"], Command::Help),
            (&["-h"], Command::Help),
        ];
        for (given, expected) in cases {
            assert_eq!(parse(&args(given)), Ok(expected), "{given:?}");
        }
    }

    #[test]
    fn anything_else_is_a_usage_error() {
        let cases: [(&[&str], &str); 10] = [
            (&["--verbose"], "unknown argument `--verbose`"),
            (&["serve"], "unknown argument `serve`"),
            (&["run", "--version"], "unknown argument `--version`"),
            (&["--root"], "--root needs a value"),
            (
                &["--root", "/a", "WorkDir=/b"],
                "the root is given twice (--root or WorkDir=)",
            ),
            (
                &["--config", "a", "--config", "b"],
                "--config is given twice",
            ),
            (&["--set", "nokey"], "--set needs KEY=VALUE, not `nokey`"),
            (&["--set", "=value"], "--set needs KEY=VALUE, not `=value`"),
            (&["WorkDir="], "WorkDir= needs a directory"),
            (
                &["default-config", "--root", "/srv"],
                "unknown argument `--root`",
            ),
        ];
        for (given, problem) in cases {
            assert_eq!(parse(&args(given)), Err(problem.to_string()), "{given:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn paths_need_not_be_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let root = OsString::from_vec(b"/srv/\xff".to_vec());
        let given = vec![OsString::from("--root"), root.clone()];
        let Ok(Command::Run(options)) = parse(&given) else {
            panic!("not a run");
        };
        assert_eq!(options.root, Some(PathBuf::from(root)));
    }
}
