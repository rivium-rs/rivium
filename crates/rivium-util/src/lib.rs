//! Independent tools. Enable only the capabilities your application consumes.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "config")]
#[cfg_attr(docsrs, doc(cfg(feature = "config")))]
pub mod config;
pub mod fs;
