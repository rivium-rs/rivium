//! One service library, two programs: `station` samples a meter and answers queries for its
//! latest reading over UDP; `collector` asks stations for their readings periodically and logs
//! them. Each program is a composition root in [`apps`] over the same business modules, and its
//! file in `src/bin/` only hands it to the process host.
//!
//! ```toml
//! # station
//! [sampler]
//! every = "1s"
//!
//! [query]
//! addr = "127.0.0.1:0"   # any free port; the `listening` event says which
//! read_timeout = "1s"    # each blocking read waits at most this long
//!
//! # collector
//! [collect]
//! stations = ["127.0.0.1:7400"]
//! every = "10s"
//! timeout = "1s"
//! ```

pub mod apps;
pub mod collect;
pub mod meter;
pub mod query;
