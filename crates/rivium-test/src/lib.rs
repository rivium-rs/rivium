//! Test support for services built on Rivium: scripted services to drive the supervisor
//! ([`ScriptedService`]), log capture ([`capture_logs`]), tools to test a service program as a
//! process, the lifecycle contract every program keeps as a process ([`process`]) and when
//! embedded ([`embedded`]), and the dependency closure check ([`deps`]). Use it as a
//! dev-dependency only.
#![forbid(unsafe_code)]

pub mod deps;
pub mod embedded;
mod logs;
pub mod process;
mod services;

pub use logs::{LogCapture, capture_logs};
pub use services::{Journal, SCRIPTED, ScriptedService, Step};

/// The platform this crate was compiled for, which is the platform of the calling test.
const TARGET: &str = env!("RIVIUM_TEST_TARGET");
