# rivium-util

Independent capability features; defaults are empty. Atomic file replacement is always available.

- `config`: typed defaults/TOML/explicit-environment/override loading, provenance, 1 MiB default
  input limit and single-writer immutable snapshots. Publication is not consumer acknowledgement.
- `log`: scoped tracing, 1024 queued file events, 64 KiB whole-event limit, counted overload,
  disk budgets, text/JSON output, category filters, rotation/gzip and Android logcat.
- `log-export`: bounded single-job ZIP generation, cancellation/expiry and directory ownership.
- `task`: persistent desired-set planning, handle ownership, confirmed replacement and bounded
  records. The driver reports source failures and rejected input while preserving existing work.
- `full`: all of the above. `task` alone does not enable the Supervisor or logging subscriber.

Manager capacity includes backoff/exhausted records, not only live handles. Closed admission
rejects new rounds; earlier reservations remain valid. Stops share one deadline plus a 100 ms
abort-confirmation budget. Unconfirmed work keeps its slot and ownership; Drop cannot await it.
A borrowed service scope remains available to sibling managers when one manager shuts down.

Logger close is permanent and retryable after timeout. Its workers retain the directory lease
until they really finish. Instance counters distinguish queue-full, oversized and IO failures;
process-cumulative counters are separate. Diagnostic logs do not guarantee lossless delivery.

AtomicWriteError distinguishes failure before replacement from failure after replacement but
before durability confirmation. Paths are trusted caller inputs, not a filesystem sandbox.

Requires Rust 1.92. Licensed under MIT or Apache-2.0.
