//! Configuration: one TOML file, the environment and `--set` overrides, loaded into typed
//! settings by the host before the services are built.
//!
//! # Layers
//!
//! Lowest priority first: the defaults (the service's `Default`, serialized), the configuration
//! file, the environment, then `--set KEY=VALUE`. In the environment, `<PREFIX>_<A>__<B>` sets
//! `a.b` (`PREFIX` is the service name in upper snake case; keys are lowercased) and `RUST_LOG`
//! sets `log.filter` below the prefixed variables. Empty variables count as unset, and prefixed
//! names without `__`, such as `<PREFIX>_ROOT`, are not keys. The embedded host reads no
//! environment.
//!
//! Values from the environment and `--set` are text, read as the field they land in asks:
//! integers, floats, `true`/`false`, strings, enum variant names; lists and tables are written
//! as TOML values, such as `["a", "b"]` or `{ a = 1 }`. Values in the file must have the TOML
//! type of their field.
//!
//! # Sections
//!
//! The top-level sections `log` and `lifecycle` belong to Rivium; every other top-level key
//! belongs to the service's configuration type. That type may use `Option` fields and maps with
//! keys of the configuration's choosing, but not `#[serde(flatten)]` or untagged enums: neither
//! the text conversion nor the detection of unknown keys can see through them. Fields can check
//! their form and range with the deserializers in [`de`].
//!
//! # Problems
//!
//! Every problem is reported at once, one line each:
//! `invalid configuration: <key> (<source>): <reason>`, where the [`Source`] is `default`,
//! `file <path>`, `env <NAME>`, `cli --set <key>` or `host --set <key>`. Unknown keys are
//! problems, with a hint when they look like a typo of a known one.

pub mod de;
pub(crate) mod load;
mod paths;
mod report;
#[cfg(test)]
mod tests;
mod tree;
mod value;

pub use paths::Paths;
pub use report::{Problem, Report, Source};
