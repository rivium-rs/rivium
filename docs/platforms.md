# Platform support

Rivium runs as a standalone process or through an embedded host. Platform support depends on
both the target and the verification method: building a JNI library does not establish that
it runs on Android, and testing a process does not test its service manager.

The minimum supported Rust version is **1.92**, checked by `ci/msrv`.

- [Verification levels](#verification-levels)
- [Current verification scope](#current-verification-scope)
- [Release records](#release-records)
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

The verification toolchain is being rebuilt. The following describes workflows configured in
this checkout, not a completed release record. **Configuration is not a successful run.**
Record the candidate's actual results; do not carry the 0.1.0 results forward to a new version.

| Target or check | Workflow | Configured coverage and limits |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `ci`, `glibc`, `daily` | Native tests; glibc 2.17 linking/symbol checks in CI and old-userland runtime checks in daily |
| `aarch64-unknown-linux-gnu` | `ci` | Native tests; no renewed architecture-specific glibc baseline established by these jobs |
| `aarch64-apple-darwin`, `x86_64-apple-darwin` | `ci` | Native tests on both architectures; not launchd hosting verification |
| `x86_64-pc-windows-gnu` | `ci`, `daily` | Native tests and a separate Wine lane; not WinSW hosting verification |
| `armv7-unknown-linux-gnueabihf` | `daily` | Cross-build and library/process contracts under QEMU; not hardware verification |
| `loongarch64-unknown-linux-gnu` (new-world ABI) | `daily` | Cross-build and library/process contracts under QEMU; not hardware or vendor old-world ABI verification |
| `aarch64-linux-android` (arm64-v8a, API 30) | `daily` | NDK compilation/linking, JNI exports and 16 KB ELF LOAD alignment; no bionic, ART or device execution |
| JNI contracts on a desktop JVM | `ci` | Invocation and library-loading contracts; not an Android runtime substitute for device acceptance |
| Rust 1.92 | `ci/msrv` | Workspace build and tests on the MSRV; not a vendor-toolchain or hardware record |

See [development checks](development.md) for exact commands, prerequisites and test exclusions.
The `ci-pass` and `daily-pass` aggregates require every prerequisite job to succeed, but they
cannot establish coverage outside those jobs.

### Known gaps

- Real systemd, launchd and WinSW hosting are not established by the current jobs.
- `host-notify` is an ordinary process host, not an sd-notify implementation.
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
