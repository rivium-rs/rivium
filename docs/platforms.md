# Platform support

Each target is verified at one of three levels:

- **T1**: built natively in CI, and the full test suite runs natively.
- **T2**: cross-built in CI; the tests run under emulation (qemu-user, the Android bionic runner)
  or in a substitute environment (a desktop JVM for JNI). Release notes describe these targets as
  "verified under emulation".
- **T3**: builds, plus manual or external verification. Release notes describe a T3 target without
  a recorded verification as "unverified".

The minimum supported Rust version is 1.92, checked by `ci/msrv`. The table records how each
target was verified for 0.1.0, with that release's CI jobs; [On main](#on-main) says what runs
now.

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

## On main

Since 0.1.0 the verification is being rebuilt. CI on `main` runs the workspace tests natively on
`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin` and `x86_64-pc-windows-gnu` (`ci/test`) and on
Rust 1.92 (`ci/msrv`). The other jobs of the table, the hosting, glibc, Android and JVM checks,
return with the rebuild; until then the targets they cover are verified as of 0.1.0 only.

## 0.1.0

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
