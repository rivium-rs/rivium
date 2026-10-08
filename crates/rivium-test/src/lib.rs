//! Test support for services built on Rivium: scripted services to drive the supervisor
//! ([`ScriptedService`]), log capture ([`capture_logs`]) and the dependency closure check
//! ([`deps`]). Use it as a dev-dependency only. It is pre-release: the process-level tools and
//! the lifecycle contract suites come next.
#![forbid(unsafe_code)]

pub mod deps;
mod logs;
mod services;

pub use logs::{LogCapture, capture_logs};
pub use services::{Journal, SCRIPTED, ScriptedService, Step};
