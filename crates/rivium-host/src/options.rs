use rivium::{Error, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Run,
    CheckConfig,
    DefaultConfig,
    Version,
    Help,
}
#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
    config: PathBuf,
    explicit_config: bool,
}
impl Paths {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn config_file(&self) -> &Path {
        &self.config
    }
    pub(crate) fn explicit_config(&self) -> bool {
        self.explicit_config
    }
}
#[derive(Clone, Debug)]
pub struct Options {
    pub action: Action,
    pub paths: Paths,
    pub sets: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub prefix: String,
}
impl Options {
    /// Parse arguments excluding `argv[0]`. Only process callers enable environment.
    pub fn parse(
        name: &str,
        args: impl IntoIterator<Item = OsString>,
        read_env: bool,
    ) -> Result<Self> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(Error::invalid("invalid application name"));
        }
        let prefix = name.to_ascii_uppercase().replace('-', "_");
        let mut root = None;
        let mut config = None;
        let mut sets = Vec::new();
        let mut action = Action::Run;
        let mut selected = false;
        let mut arguments = args.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--root") => {
                    root = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| Error::invalid("--root requires a path"))?,
                    ))
                }
                Some("--config") => {
                    config = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| Error::invalid("--config requires a path"))?,
                    ))
                }
                Some("--set") => sets.push(
                    arguments
                        .next()
                        .and_then(|v| v.into_string().ok())
                        .ok_or_else(|| Error::invalid("--set requires UTF-8 key=value"))?,
                ),
                Some(
                    value @ ("run" | "check-config" | "default-config" | "--version" | "-V"
                    | "--help" | "-h"),
                ) if !selected => {
                    selected = true;
                    action = match value {
                        "run" => Action::Run,
                        "check-config" => Action::CheckConfig,
                        "default-config" => Action::DefaultConfig,
                        "--version" | "-V" => Action::Version,
                        _ => Action::Help,
                    };
                }
                _ => return Err(Error::invalid("unknown argument or repeated command")),
            }
        }
        let mut environment = Vec::new();
        if read_env {
            for (key, value) in std::env::vars_os() {
                let Some(key) = key.to_str() else { continue };
                if key == format!("{prefix}_ROOT") {
                    if root.is_none() {
                        root = Some(value.into());
                    }
                    continue;
                }
                if key == format!("{prefix}_CONFIG") {
                    if config.is_none() {
                        config = Some(value.into());
                    }
                    continue;
                }
                if key.starts_with(&format!("{prefix}_")) || key == "RUST_LOG" {
                    environment.push((
                        key.to_owned(),
                        value.into_string().map_err(|_| {
                            Error::invalid("configuration environment value is not UTF-8")
                        })?,
                    ));
                }
            }
        }
        if !read_env && root.is_none() && matches!(action, Action::Run | Action::CheckConfig) {
            return Err(Error::invalid("embedded host requires --root"));
        }
        let root = match root {
            Some(root) => {
                if root.is_absolute() {
                    root
                } else {
                    std::env::current_dir().map_err(Error::from)?.join(root)
                }
            }
            None => std::env::current_exe()
                .map_err(Error::from)?
                .parent()
                .unwrap_or(Path::new("."))
                .to_owned(),
        };
        let explicit_config = config.is_some();
        let config = config.unwrap_or_else(|| PathBuf::from(format!("{name}.toml")));
        let config = crate::config::resolve(&root, &config);
        Ok(Self {
            action,
            paths: Paths {
                root,
                config,
                explicit_config,
            },
            sets,
            environment,
            prefix,
        })
    }
}
