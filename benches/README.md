# Rivium benchmarks

This private workspace package benchmarks public APIs across Rivium crates using normal
production dependencies. It shares the workspace lockfile and is never published.

Run with a quiet machine and the same toolchain, allocator, features and CPU power
settings when comparing revisions:

```sh
cargo bench -p rivium-benches --bench task_lifecycle --locked
rustc --version --verbose
```

The dependency-free harness uses the optimized bench profile, one full untimed
batch to warm up each case, then 21 equally sized measured batches. CSV output
reports the median, minimum and maximum **batch mean** in nanoseconds per operation;
these are not request latency percentiles or confidence intervals. Iteration counts
are printed with each result. No random inputs or seed are used. Run at least three
times to assess noise. Do not run concurrent tests, builds or soak workloads while
collecting a comparison baseline. There is no absolute performance gate in CI.

An operation means:

- `scope/reserve_drop/N`: reserve and release the entire N-slot batch. Includes the
  admission mutex, cancellation check, allocation and reservation release operations.
  This standalone scope has no generation parent and no contending threads.
- `scope/reserve_track_ready/1`: reserve one slot, allocate and poll an immediately
  ready tracked future, then release it. A single `block_on` encloses each batch.
- `scope/closed_rejection`: attempt admission into an already closed scope.
- `manager/*/unchanged/128`: reconcile 128 unchanged cooperative tasks. Includes
  bounded desired-Vec construction, planning over borrowed current state, empty reservation and report handling.
- `manager/*/replace/N`: replace all N tasks, including cancellation, joining old
  tasks, allocating and spawning replacements, and report validation. Startup and
  final shutdown are outside timing; replacement tasks may first be polled during
  the following iteration. One `block_on` encloses each batch. Both current-thread
  and two-worker Tokio runtimes are exercised; replacement measurements include
  scheduler costs. Reports must contain all expected replacements and no unconfirmed
  tasks, and final shutdown must leave zero tracked tasks.

`cargo bench -p rivium-benches --bench task_lifecycle -- --test`
executes one iteration per case as a functional smoke check without collecting
timing. `cargo test --all-targets` also selects this smoke mode.

These are synthetic baselines, not Edge workload measurements. They do not measure
blocked work, slow resource destructors, I/O,
shutdown timeout tails or memory usage. Performance results alone do not establish
concurrent correctness; use the model checks and stress suites for that purpose.

## Log directory throughput

```sh
cargo bench -p rivium-benches --bench log_directory --locked
```

`log_directory` measures 500 public tracing events plus the writer flush barrier per
sample. Each case has one untimed warm-up and seven measured samples; the reported
median/minimum/maximum are batch means in nanoseconds per event. Directory creation,
fixture writes, `Logger::new`, shutdown and deletion are outside the measurement.
Each sample uses a fresh logger and asserts zero dropped events and a directory size
within its configured budget. Run three times on a quiet machine for comparisons.

The cases cover no archives, 1,000 valid pre-existing gzip archives, and size rotation
under a 4 KiB total budget (1 KiB file and export reservations). The archive fixtures
are real gzip streams, not files filled with arbitrary bytes. The two non-rotating
cases use a 1 MiB file limit and 64 MiB total budget. The measured flush includes
its directory reconciliation and `sync_data`; results include filesystem/cache noise
and are not per-event latency percentiles or claims about durable disk throughput.
No exports run during timing. Setup/recovery cost, steady export contention,
incompressible workloads and category fanout need separate measurements.

`cargo bench -p rivium-benches --bench log_directory -- --test` performs a functional
smoke run without collecting samples. For revision comparisons, copy this exact
harness into both revisions, build both first, then run their standalone optimized
executables sequentially with no concurrent tests or compilation.
Use separate target directories for different source snapshots; sharing build outputs
can reuse stale path dependencies when archived source timestamps precede existing artifacts.
