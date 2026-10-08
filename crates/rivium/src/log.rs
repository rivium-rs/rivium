//! Logging: output layers, files with a disk budget, and the flush barrier.

mod budget;
mod files;
mod settings;

pub(crate) use settings::LogSettings;
