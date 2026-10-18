# Platform support

Rivium runs as a standalone process or through an embedded host. Platform support depends on
both the target and the verification method: building a JNI library does not establish that
it runs on Android, and testing a process does not test its service manager.

The minimum supported Rust version is **1.92**, checked by `ci/msrv`.

- [Verification levels](#verification-levels)
- [Current verification scope](#current-verification-scope)
- [Release records](#release-records)
- [0.2.0 candidate](#020-candidate)
- [0.1.1](#011)
- [0.1.0](#010)

## Verification levels

These levels describe recorded evidence, not a blanket guarantee for every environment:

| Level | Verification | Release description |
| --- | --- | --- |
| **T1** | Native CI build and native test suite | Natively verified, within the recorded scope |
| **T2** | Cross-build and tests under emulation or a substitute runtime, such as a desktop JVM | Verified under emulation; name the actual runtime |
| **T3** | Manual or external checks, including hardware and vendor toolchains | Unverified unless a matching record exists |

A compile-only check is not T2 runtime verification. Each release record states the relevant
ABI baseline, tests, omissions and source identity. No level implies real service-manager or
device testing unless the record explicitly includes it.

<a id="on-main"></a>

## Current verification scope

The automated verification toolchain is restored. The following describes its configured scope;
actual results belong to the version-specific records below. **Configuration is not a successful
run.** Do not carry the 0.1.0 results forward to a new version.

| Target or check | Workflow | Configured coverage and limits |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `ci`, `glibc`, `daily` | Native tests; glibc 2.17 linking/symbol checks in CI and old-userland runtime checks in daily |
| `aarch64-unknown-linux-gnu` | `ci` | Native tests; no renewed architecture-specific glibc baseline established by these jobs |
| `aarch64-apple-darwin`, `x86_64-apple-darwin` | `ci` | Native tests on both architectures; not launchd hosting verification |
| `x86_64-pc-windows-gnu` | `ci`, `daily` | Native tests and a separate Wine lane; not WinSW hosting verification |
| `armv7-unknown-linux-gnueabihf` | `daily` | Cross-build and library/process contracts under QEMU; not hardware verification |
| `loongarch64-unknown-linux-gnu` (new-world ABI) | `daily` | Cross-build and library/process contracts under QEMU; not hardware or vendor old-world ABI verification |
| `aarch64-linux-android` (arm64-v8a, API 30) | `daily` | NDK compilation/linking, JNI exports and 16 KB ELF LOAD alignment; no bionic, ART or device execution |
| JNI contracts on a desktop JVM | `ci` | Library-loading and signature contracts; not an Android runtime substitute for device acceptance |
| Rust 1.92 | `ci/msrv` | Workspace build and tests on the MSRV; not a vendor-toolchain or hardware record |

See [development checks](development.md) for exact commands, prerequisites and test exclusions.
The `ci-pass` and `daily-pass` aggregates require every prerequisite job to succeed, but they
cannot establish coverage outside those jobs.

### Known gaps

- Real systemd, launchd and WinSW hosting are not established by the current jobs.
- No systemd notification protocol is implemented by the workspace.
- Android bionic runtime tests, ART contracts and real-device checks are not run by the Android lane.
- Vendor old-world LoongArch, physical hardware tests and additional architecture-specific ABI
  baselines require matching verification records.

These gaps remain explicit until the corresponding checks have actual results. Neither a green
aggregate nor a desktop JVM result closes a missing hosting or device record.

## Release records

Every new release adds a `## X.Y.Z` section recording:

- The source revision or library identity and verification date.
- The target, ABI baseline and actual verification method.
- Results, including omissions, failures and unverified targets.
- Workflow evidence and any required manual acceptance.

The protected release reviewer checks that evidence before approving publication; a section's
presence is not proof of its claims. A T3 target without a matching record remains **unverified**.
See the [release guide](releasing.md) for the approval and publishing procedure. Historical
records retain their original scope and dates.

## 0.2.0 candidate

The latest local retention follow-up on **2026-10-18** uses frozen source
`99abbc88623d6387ee73b1f020a4fe25b26c23d3`, with production `crates` subtree
`b781250ef5aa86803f103e0c2cf89531540acde0`. The candidate record covers the latest native
validation and package preview; earlier platform/LaunchAgent results remain tied to
`b204cef` and HTTP/Supervisor measurements to `2e50d50`, with their older `cb3d3ec3` subtree;
they have not been rerun for this changed production source.

| Area | Latest evidence and limit |
| --- | --- |
| Local native and MSRV | Rust 1.96.0 workspace: 347 passed, 0 failed, 2 ignored. Rust 1.92 workspace/all-target/all-feature **check** and util **tests** passed (127 passed, 1 ignored). This is not a new full MSRV workspace-test run; other native targets are not renewed |
| JVM and cross-targets (retained earlier source) | At `b204cef`, OpenJDK 27 normal/fault JNI contracts passed; NDK r28b/API 30 compiled/linked Android artifacts with JNI exports and 16 KB LOAD alignment checked; all six Windows GNU libraries passed cross-check. These were not repeated for `99abbc8`; Android and Windows runtime remain unverified |
| Deployment (retained earlier source) | The `b204cef` temporary `test-host` LaunchAgent completed three readiness/SIGTERM/exit-0/restart rounds and cleanup. The newer test-host build is not a deployment rerun. Production HTTP hosting and systemd/WinSW remain unverified |
| Packages | A clean `99abbc8` snapshot completed source checks, six-package scanning, actual `cargo publish --dry-run` and unchanged-byte verification; no upload occurred. Earlier previews retain their own artifact hashes. Crate provisioning and Trusted Publishing remain separate prerequisites |
| HTTP and Supervisor (retained earlier source) | The `2e50d50` HTTP matrix retained 48 cases across three processes per revision: five qualifying normal cases met the provisional p99 trigger, 19 were unqualified, and the candidate mixed workload had 90,524 client errors and seven late cleanup observations. HTTP acceptance remains open. Supervisor ownership checks passed across 72,000 measured generations; timing regressions and the unevaluated steady-state gate remain |
| Sustained scale | File-count/byte retention repaired with boundary RED/GREEN evidence. Three load processes plus churn completed 17 phases and cleanup checks: 8,569 application rows, 1,700 OS samples without sampling errors, and file-count maximum 8,191 in each process. One original successful process was revalidated; three were fresh recovery runs. Withdrawal zeros are fixed post-cleanup placeholders, with separate OS observations. No universal RSS bound or full P-LOG/P-SCALE pass is claimed; old failed attempts remain retained |
| Required remote gates | No successful matching-source CI, daily or release workflow is included in this record. Earlier remote snapshots retain their own checked commit/time; local repair evidence does not renew them |

Release acceptance remains open. `release/ready` still requires successful `check`, `semver`,
`public`, `ci`, `daily` and `loom` results for the candidate, and the protected reviewer must
resolve required manual workload/deployment acceptance. The local package preview and finite
resource protocol do not replace those gates. T3 hardware/vendor targets, Android
bionic/ART/devices, systemd/WinSW and production hosting beyond the earlier LaunchAgent fixture
remain explicitly unverified. The heading will become exact `## 0.2.0` only after the required
acceptance is satisfied.

### Earlier candidate runs

Local verification on **2026-10-16** covers the unreleased six-package source rewrite on
`codex/rebuild-020-full`, based on `v0.1.1` / `abc50bc2887d9487799dd99dfb9f967315bc875b`.
The verified `crates` Git subtree is `792d22cecde7b20780299aefa7127db75f28b37e`;
this identifies the library sources, manifests and tests independently of later documentation.
These are local candidate results, not a release preview or a renewed native CI matrix.

| Target or check | Actual result and scope |
| --- | --- |
| `aarch64-apple-darwin`, Rust 1.96.0 | Workspace all-feature tests: 340 passed; two opt-in sustained stress cases are recorded separately. Examples and the process fixture built successfully |
| Rust 1.92, `aarch64-apple-darwin` | Same workspace suite: 340 passed, two opt-in stress cases excluded; examples and process fixture rebuilt with this toolchain |
| Feature boundaries | 16 independent capability test configurations and 36 cargo-hack build configurations passed; isolated consumer guards checked dependency closures |
| Desktop JVM, OpenJDK 27 | Rebuilt normal/fault JNI libraries passed loading, Unicode, signature rejection, restart and five-entry panic containment contracts with `-Xcheck:jni` |
| `aarch64-linux-android` | NDK r28b / API 30 built and linked workspace libraries, binaries and tests; JNI exports and 16 KB ELF LOAD alignment passed. Compile/link checks only |
| `x86_64-pc-windows-gnu` | All six production libraries passed cross-compilation checks with all features. No Windows or Wine test execution in this record |
| Concurrency and static checks | Six Loom models passed; all-target/all-feature Clippy and rustdoc rejected warnings; script guards, ShellCheck, actionlint and dependency policy checks passed |
| Sustained ownership tests | Seeded dynamic-task stress ran for 600 seconds; final host restart stress completed 153 blocked-and-released generations in 181 seconds. External watchdogs did not fire |
| Package contents | All six actual 0.2.0 archives passed file whitelists, source-byte comparison and private-content scanning. Packaging used `--no-verify`; the workspace and isolated consumers supplied compilation evidence, not a registry publish dry-run |

The historical 0.1.x Linux, macOS x86-64, Windows, QEMU, glibc and hardware results below
are not evidence for this rewrite. Native Linux/Windows, QEMU/Wine execution, service-manager
hosting, Android bionic/ART/devices and deployment workload acceptance remain unverified here.
Microbenchmarks and bounded synthetic stress do not establish production tail-latency or
RSS/FD capacity targets. Publishing still requires the candidate's complete release workflow.

Three sequential microbenchmark processes per revision also compared this candidate with the
earlier `aa48a03` implementation. The median batch mean for reserving/releasing 64 scope slots
rose from 329 ns to 1,473 ns; reconciling 128 unchanged tasks on one worker rose from 4.68 µs to
10.35 µs. That comparison did **not meet** the provisional performance regression target.
Logging without archives fell from 14.87 µs to 11.60 µs per event, and size rotation from 1.93 ms to 0.144 ms, including
flush. These are synthetic batch means on the same machine/toolchain, with each revision's
reviewed lockfile; they are neither per-operation percentiles nor deployment acceptance.

The **2026-10-17** performance follow-up verifies implementation `9541e28` (crates tree
`5f5c8dce3da030d84eab3ad023bb7087dfcf58d8`). Three sequential processes per version
compared the historical implementation, the unoptimized rewrite and the optimized rewrite.
Scope reserve/release of 64 slots fell from 1,422.4 ns to 270.8 ns (historical: 320.5 ns),
and unchanged reconciliation of 128 tasks on one worker fell from 10.04 µs to 2.90 µs
(historical: 4.57 µs). Immediately-ready tracking remains 10.6% above the historical
batch mean; single-task replacement on one worker remains 16.8% above it. **Performance
acceptance remains open**: these are batch means, and task p99 and the deployment workload
matrix are still unverified.

The follow-up passed workspace all-feature tests, the added native scope race, eight
production-code Loom models, strict Clippy, formatting, minimal-task feature tests and
Rust 1.92 checks. Task lifecycle stress completed 7,045,379 generations in 600 seconds;
host restart stress completed 154 blocked-and-released generations in 180.61 seconds.
Neither external watchdog fired. These checks supplement the local correctness evidence;
they do not renew the platform matrix or complete the performance acceptance work.

The second **2026-10-17** repair verifies implementation `8f976b8` (crates tree
`cb3d3ec3dd921a51288a789d3db5ebf6063c1f28`). It removes desired-tree construction and
redundant planning/specification comparisons. Unchanged reconciliation of 128 tasks
now takes 0.81 µs on both runtimes, against 2.88–2.90 µs in the first repair and
4.64–4.70 µs historically. All five standalone scope batch cases are within the
provisional +10% trigger; production scope code is unchanged from the first repair.

The new matrix measures 128/512/1024 tasks with 1 KiB specifications and 0/1/10/100%
changes on current-thread and two-worker runtimes. Each of three independent processes
records 10,000 samples per case after 1,000 warmups. All 24 single-reconcile replacement
cases meet the provisional +20% historical p99 trigger; the largest median-of-process-p99
regression is +13.9%. The 24 mixed recovery cases measure entire episodes and are
reported separately. Two-worker single-task replacement still has a +26.5% historical
batch-mean regression; this observation is not erased by passing the larger-N p99 matrix.
The detailed benchmark output remains outside the published source tree.

That `8f976b8` source passed 346 workspace tests (two opt-in stress cases excluded), strict
Clippy, formatting, minimal-task tests, Rust 1.92 checks and eight production scope
Loom models. The Send regression also covers Send-but-not-Sync specifications.
The fresh 600-second release task stress passed 6,425,290 generations, 123,504,483
resource lifecycles and 18,070,116 replacements. Host restart stress passed 154
blocked-and-released generations in 180.876 seconds. Both commands exited 0 and neither
external watchdog fired; these iteration counts are not throughput acceptance metrics.
This local record does not renew native Linux/Windows or Android runtime verification,
the Supervisor/HTTP/logging workload matrices, or deployment acceptance. The measured
scope/task subset passes its provisional triggers; complete 0.2.0 release acceptance
remains open.

## 0.1.1

Candidate verification recorded on **2026-10-13** at
[`448b47e`](https://github.com/rivium-rs/rivium/commit/448b47ede3abcf17e7f04c9ea4f9de72bda93f13).
All five public crates and internal dependency requirements are 0.1.1; MSRV remains Rust 1.92.
This records verified candidate behavior, not a claim that the version has been published.

Evidence:

- [PR CI](https://github.com/rivium-rs/rivium/actions/runs/38369209031): all native targets,
  MSRV, JVM, lint, supply-chain guards and glibc link checks succeeded, including `ci-pass`.
- [Release preview](https://github.com/rivium-rs/rivium/actions/runs/38369210903): same-revision
  CI, public-content checks, API compatibility, the complete daily matrix and `release/ready`
  succeeded. The event was manual dispatch; **`release/publish` was skipped**.
- That preview packaged and scanned the actual five **0.1.1** archives, completed a real
  `cargo publish --dry-run`, and checked archive/notes/metadata bytes. Artifact `11797758745`
  was downloaded and its archive digest and all seven file checksums verified. Both package
  and notes versions in `release.json` are 0.1.1; this is not the earlier 0.1.0-version preview.

The [release preparation PR](https://github.com/rivium-rs/rivium/pull/70) tracks checks on its
final revision as well. Documentation-only closeout does not waive a fresh candidate preview;
a future release tag must pass its own full workflow before publication.

| Target or check | Result and scope |
| --- | --- |
| `x86_64-unknown-linux-gnu` | Native suite passed; separate artifacts linked/checked against glibc 2.17, and the basic library/process suite passed in the pinned CentOS glibc 2.17 userland |
| `aarch64-unknown-linux-gnu` | Native suite passed; no renewed architecture-specific glibc baseline |
| `aarch64-apple-darwin`, `x86_64-apple-darwin` | Native suites passed on both architectures; not launchd hosting acceptance |
| `x86_64-pc-windows-gnu` | Native suite and the separate Wine 10.0 suite passed; not WinSW hosting acceptance |
| `armv7-unknown-linux-gnueabihf` | Library and target child-process contracts passed under QEMU 11.0; not hardware or systemd acceptance |
| `loongarch64-unknown-linux-gnu` | Library and target child-process contracts passed under QEMU 11.0; not hardware or vendor old-world ABI/toolchain acceptance |
| `aarch64-linux-android` | API 30 compilation/linking, JNI exports and 16 KB ELF LOAD checks passed; **compile-only**, not bionic/ART/device execution |
| Desktop JVM | Invocation, dynamic loading, signature rejection and panic-containment checks passed; not an Android device record |
| Rust 1.92 | Workspace build and tests passed on the MSRV |

The QEMU runs exclude exactly the five native signal/stop-deadline scenarios listed in the
[development guide](development.md#jvm-and-platform-boundaries); native CI still runs them. Wine
permits no ignored tests. The CentOS run excludes JVM Invocation and Cargo/tokei-dependent
build guards, which passed in native CI instead. Android compilation is not T2 runtime
verification, and the declared 16 KB ELF alignment is not a real-device compatibility result.

The following remain **unverified for 0.1.1**: real systemd/launchd/WinSW hosting and sd-notify;
Android bionic/ART runtime and devices; physical ARMv7 and LoongArch hardware, including the
vendor old-world ABI/toolchain; and renewed architecture-specific glibc baselines other than
x86_64. Historical T3 records below remain historical, not a claim of new candidate acceptance.
The protected release reviewer must confirm any required manual acceptance and publishing
configuration separately; this preview did not request OIDC credentials or upload packages.

## 0.1.0

The table below belongs to **0.1.0** and uses that release's CI job names. It is a historical
record, not the current workflow matrix.

| Target | Runs as | Artifact baseline | Level | How it is verified | CI jobs (0.1.0) |
| --- | --- | --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | process (systemd) | glibc ≤ 2.17 | T1 | native tests; glibc 2.17 symbol check and a run on CentOS 7; systemd hosting test | `ci/test-linux`, `cross/glibc`, `hosting/systemd` |
| `aarch64-unknown-linux-gnu` | process (systemd) | glibc ≤ 2.17 | T1 | native tests on an arm64 runner; glibc 2.17 symbol check | `ci/test-linux-arm64`, `cross/glibc` |
| `aarch64-apple-darwin` | process (LaunchAgent) | — | T1 | native tests; launchd hosting test | `ci/test-macos`, `hosting/launchd` |
| `x86_64-pc-windows-gnu` | process (WinSW service wrapper) | — | T1 | native tests with the GNU toolchain; WinSW hosting test | `ci/test-windows-gnu`, `hosting/winsw` |
| `armv7-unknown-linux-gnueabihf` | process (systemd) | glibc ≤ 2.17 | T2 | cross build; tests under qemu-user, including process-level tests; glibc 2.17 symbol check | `cross/armv7` |
| `loongarch64-unknown-linux-gnu` (new-world ABI) | process | glibc ≤ 2.36, Linux ≥ 5.19 | T2 | cross build; tests under qemu-user, including process-level tests; glibc 2.36 symbol check | `cross/loongarch64` |
| `aarch64-linux-android` (arm64-v8a, minSdk 30) | embedded (JNI) | 16 KB page alignment | T2 | NDK build and LOAD-segment alignment check; non-JNI tests under the bionic runner; JNI contract on a desktop JVM | `cross/android`, `ci/test-linux` |
| LoongArch old-world ABI 1.0 (vendor Rust 1.92 toolchain) | process | glibc ≤ 2.28, ELF flags `0x3`, interpreter `/lib64/ld.so.1` | T3 | built and tested natively from an offline bundle of the release candidate, and its builds checked against the baseline; recorded by hand before each release. `ci/msrv` is the public early warning | `ci/msrv` |
| armv7 hardware | process | glibc ≤ 2.17 | T3 | test binaries and release programs, cross-built against glibc 2.17, run on a board; recorded by hand | — |
| Android devices (arm64-v8a) | embedded (JNI) | 16 KB page alignment | T3 | device checklist: install, start, stop, three start-stop rounds, restart after the process is killed, logs in logcat, stop from `onDestroy` | — |
| LoongArch new-world hardware | process | glibc ≤ 2.36, Linux ≥ 5.19 | T3 | manual checklist | — |

First edition, 2026-10-08 (CI baseline); finalised on 2026-10-09 for the first release, 0.1.0, whose
release notes link to this table at its tag. Every test job ran the workspace tests: the T2 rows
ran them under qemu-user or the bionic runner, including the embedded host's tests. The JNI rows
built the `beacon-jni` example with `rivium_jni::export!`: `ci/test-linux` ran rivium-jni's desktop
JVM contract on it, `cross/android` checked its 16 KB page alignment, and the glibc checks covered
it. For the process form, the `udp-echo` example was the artifact: the hosting jobs ran it under
systemd, launchd and WinSW as a deployment installs it, the glibc checks read its builds, and the
glibc 2.17 build ran and stopped on CentOS 7. A smoke program checked the TLS stack that services add
on top of Rivium (rustls with the ring provider) on every target. The jobs listed for the T1 and T2
rows passed. The T3 rows hold for the library code they were recorded on; a release whose library
code differs is recorded again. The records:

- **LoongArch old-world ABI**: recorded on 2026-10-10. On native hardware with glibc 2.28 and the
  vendor toolchain, the workspace tests pass offline, and three release programs, `udp-echo` among
  them, have ELF flags `0x3`, the interpreter `/lib64/ld.so.1` and no symbol newer than
  `GLIBC_2.28`; each runs and stops with 0 on SIGTERM.
- **armv7 hardware**: recorded on 2026-10-10. On a board with glibc 2.35, the test binaries pass,
  except the few that need cargo or a JVM, and the same three programs need no symbol newer than
  `GLIBC_2.17`, run, and stop with 0 on SIGTERM.
- **Android devices** and **LoongArch new-world hardware**: no record yet; unverified.

Two validation services exercised Rivium as an SNMP agent run both as
a program and through JNI on a desktop JVM, and as an edge collector under the three hosts. They ran
in CI on validation branches
([#45](https://github.com/rivium-rs/rivium/pull/45),
[#46](https://github.com/rivium-rs/rivium/pull/46),
[#47](https://github.com/rivium-rs/rivium/pull/47)) and are not part of `main`; the programs of the
hardware records other than `udp-echo` are theirs.
