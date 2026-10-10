//! Where a service's files are.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rivium_error::{Error, OrErr, kinds};

use super::load::env_prefix;
use crate::Result;

/// The root directory of a service and its configuration file. Relative paths in the
/// configuration are relative to the root, never to the working directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
    config_file: PathBuf,
}

impl Paths {
    /// The root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The configuration file, whether or not it exists.
    #[must_use]
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// The directory for the service's data: `<root>/data`.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    /// A path from the configuration: relative paths are relative to the root.
    #[must_use]
    pub fn resolve(&self, path: impl AsRef<Path>) -> PathBuf {
        self.root.join(path)
    }

    /// Finds the root and the configuration file of the service `name`, and whether the file was
    /// named explicitly.
    ///
    /// The root is `root` (`--root` or `WorkDir=`), else `<PREFIX>_ROOT`, else the directory of
    /// the executable (symbolic links resolved). The file is `config` (`--config`), else
    /// `<PREFIX>_CONFIG`, else `config_file` below the root. Relative paths given on the command
    /// line or in the environment are relative to the working directory. `env` is `None` for the
    /// embedded host, which reads no environment and must pass the root.
    pub(crate) fn locate(
        name: &str,
        root: Option<PathBuf>,
        config: Option<PathBuf>,
        env: Option<&[(OsString, OsString)]>,
        config_file: &str,
    ) -> Result<(Paths, bool)> {
        let prefix = env_prefix(name);
        let var = |suffix: &str| {
            let name = format!("{prefix}_{suffix}");
            let mut vars = env.unwrap_or_default().iter();
            vars.find(|(key, value)| *key == *name && !value.is_empty())
                .map(|(_, value)| PathBuf::from(value))
        };
        let absolute = |path: PathBuf| {
            std::path::absolute(&path).or_err_with(kinds::INVALID_INPUT, || {
                format!("the path {}", path.display())
            })
        };
        let root = match root.or_else(|| var("ROOT")) {
            Some(root) => absolute(root)?,
            None if env.is_none() => {
                return Error::e_explain(
                    kinds::INVALID_INPUT,
                    "the embedded host must pass --root",
                );
            }
            None => (std::env::current_exe().and_then(|exe| exe.canonicalize()))
                .ok()
                .and_then(|exe| exe.parent().map(without_verbatim))
                .ok_or_else(|| {
                    let reason = "the directory of the executable is unknown; pass --root";
                    Error::explain(kinds::INVALID_INPUT, reason)
                })?,
        };
        let (config_file, explicit) = match config.or_else(|| var("CONFIG")) {
            Some(file) => (absolute(file)?, true),
            None => (root.join(config_file), false),
        };
        Ok((Paths { root, config_file }, explicit))
    }
}

/// `path` without the verbatim prefix that `canonicalize` adds on Windows: `\\?\C:\dir` becomes
/// `C:\dir`, and `\\?\UNC\server\share` becomes `\\server\share`. Logs show the usual form, and
/// other programs given the root expect it; std adds the prefix itself where a long path needs
/// it. Other paths are returned as they are.
pub(super) fn without_verbatim(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}
