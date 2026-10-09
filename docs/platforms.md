# Platform support

Each target is verified at one of three levels:

- **T1**: built natively in CI, and the full test suite runs natively.
- **T2**: cross-built in CI; the tests run under emulation (qemu-user, the Android bionic runner)
  or in a substitute environment (a desktop JVM for JNI). Release notes describe these targets as
  "verified under emulation".
- **T3**: builds, plus manual or external verification. Release notes describe a T3 target without
  a recorded verification as "unverified".

The minimum supported Rust version is 1.92, checked by `ci/msrv`.

| Target | Runs as | Artifact baseline | Level | How it is verified | CI jobs |
| --- | --- | --- | --- | --- | --- |
| `x86_64-unknown-linux-gnu` | process (systemd) | glibc ≤ 2.17 | T1 | native tests; glibc 2.17 symbol check and a run on CentOS 7; systemd hosting test | `ci/test-linux`, `cross/glibc`, `hosting/systemd` |
| `aarch64-unknown-linux-gnu` | process (systemd) | glibc ≤ 2.17 | T1 | native tests on an arm64 runner; glibc 2.17 symbol check | `ci/test-linux-arm64`, `cross/glibc` |
| `aarch64-apple-darwin` | process (LaunchAgent) | — | T1 | native tests; launchd hosting test | `ci/test-macos`, `hosting/launchd` |
| `x86_64-pc-windows-gnu` | process (WinSW service wrapper) | — | T1 | native tests with the GNU toolchain; WinSW hosting test | `ci/test-windows-gnu`, `hosting/winsw` |
| `armv7-unknown-linux-gnueabihf` | process (systemd) | glibc ≤ 2.17 | T2 | cross build; tests under qemu-user, including process-level tests; glibc 2.17 symbol check | `cross/armv7` |
| `loongarch64-unknown-linux-gnu` (new-world ABI) | process | glibc ≤ 2.36, Linux ≥ 5.19 | T2 | cross build; tests under qemu-user, including process-level tests; glibc 2.36 symbol check | `cross/loongarch64` |
| `aarch64-linux-android` (arm64-v8a, minSdk 30) | embedded (JNI) | 16 KB page alignment | T2 | NDK build and LOAD-segment alignment check; non-JNI tests under the bionic runner; JNI contract on a desktop JVM | `cross/android`, `ci/test-linux` |
| LoongArch old-world ABI 1.0 (vendor Rust 1.92 toolchain) | process | glibc ≤ 2.28, ELF flags `0x3`, interpreter `/lib64/ld.so.1` | T3 | built and tested on native hardware by an external, private validation project; `ci/msrv` is the public early warning | `ci/msrv` |
| Android devices; armv7 and LoongArch hardware | — | — | T3 | manual checklists | — |

## Current state

First edition, 2026-10-08 (CI baseline), updated as the crates are implemented. Every test job
runs the workspace tests: the T2 rows run them under qemu-user or the bionic runner, including
the embedded host's tests. The JNI rows build the `beacon-jni` example with `rivium_jni::export!`:
`ci/test-linux` runs rivium-jni's desktop JVM contract on it, `cross/android` checks its 16 KB
page alignment, and the glibc checks cover it. For the process form, the `udp-echo` example is
the artifact: the hosting jobs run it under systemd, launchd and WinSW as a deployment installs
it, the glibc checks read its builds, and the glibc 2.17 build runs and stops on CentOS 7. A
smoke program checks the TLS stack that services add on top of Rivium (rustls with the ring
provider) on every target. The jobs listed for the T1 and T2 rows pass; the T3 rows have no
record yet and are unverified. The table is finalised once the library and the validation slices
are complete.
