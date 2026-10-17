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

Benchmark output is intentionally local and is not part of the published source tree. Record
comparisons in an external or ignored evidence directory when a release review requires them.

An operation means:

- `scope/reserve_drop/N`: reserve and release the entire N-slot batch. Includes the
  admission mutex, cancellation check, allocation and atomic reservation release operations.
  This standalone scope has no subscribed waiters, shared supervisor notification or
  contending threads. It does not measure notification delivery to observers.
- `scope/reserve_track_ready/1`: reserve one slot, allocate and poll an immediately
  ready tracked future, then release it. A single `block_on` encloses each batch.
- `scope/closed_rejection`: attempt admission into an already closed scope.
- `manager/*/unchanged/128`: reconcile 128 unchanged cooperative tasks. Includes
  bounded desired-Vec construction, ordered comparison with current state, empty
  reservation and report handling.
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

## Task round latency

```sh
RIVIUM_LATENCY_RAW=/tmp/rivium-rounds.csv cargo bench -p rivium-benches --bench task_latency --locked
```

The matrix uses 128/512/1024 tasks, inline 1 KiB specs, and 0/1/10/100% changed keys
on current-thread and two-worker runtimes. The affected count is rounded up, so 1%
of 128 means two tasks. Keys and mixed actions rotate deterministically; input is
key-ordered. This does not characterize sorting costs for arbitrary input order.
Each case warms up 1,000 times and records 10,000 actual samples. Percentiles use
nearest rank: `ceil(percent * samples / 100)`, with one-based ranks. Run three
independent processes per revision and retain every process result and raw sample.

`replacement` measures one reconcile, including desired Vec/spec construction through
its return. Report validation and destruction are outside timing. These 24 cases
provide single-round p50/p95/p99. `mixed_recovery_episode` divides affected keys among
successful exits, failed exits and replacements. Exit commands and scope accounting
are prepared before timing. Scope release is not a JoinHandle completion barrier;
the measured episode includes repeated input construction, reconcile calls and yields
until all affected tasks restart. These episode percentiles are **not single-round
p99**. Summary and raw CSV record the number of reconcile calls. Both revisions use
1 ns backoff because the historical implementation rejects zero; the historical
retry decision can consequently require another round.

The same source runs against historical `aa48a03`: passing an explicit Vec adapts its
IntoIterator API without moving construction outside timing. No historical production
code is patched. `-- --test` runs three warmup and three measured samples per case
with the same assertions. These closed-loop samples are not a production arrival
distribution or failure-to-recovery SLO. The provisional +20% P-TASK trigger applies
to single-round replacement p99, not mixed recovery episode percentiles.

## Scope diagnostics

```sh
cargo bench -p rivium-benches --bench scope_diagnostics --locked
```

Six attribution probes use 21 batches of 100,000 operations after one warmup batch:
reserve/drop, pre-reserved ready, manually polled Pending-to-Ready, actual captured
resource destruction, subscribed waiter wake/rearm, and shared Supervisor notification
with a scheduler yield. `-- --test` checks one operation per case.

The pre-reserved probe excludes reservation setup and Vec destruction; only the final
slot reaches zero. Optimized assembly in this experiment elides its Box allocation,
so its result cannot stand in for complete tracked-future allocation cost. The
Pending/Ready probe excludes scheduling, while the Supervisor probe includes it.
Retain end-to-end task_lifecycle results when evaluating any scope optimization.

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
