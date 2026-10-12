#!/usr/bin/env python3
"""I1 process verification only. No platform installation, release, or task framework."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
ENTRIES = {"helper_environment", "host_boundaries", "process_contract"}
# Only known build/runtime inputs; never forward tokens, application overrides, or local.env.
ENV_KEYS = {
    "PATH", "HOME", "USERPROFILE", "SYSTEMROOT", "SystemRoot", "WINDIR", "COMSPEC",
    "PATHEXT", "TEMP", "TMP", "TMPDIR", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN",
    "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "MSYSTEM", "MINGW_PREFIX",
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def suite_names(output):
    return {line.removesuffix(": test") for line in output.splitlines() if line.endswith(": test")}


def require_suite(output):
    if suite_names(output) != ENTRIES:
        raise RuntimeError("process suite missing, empty, or filtered; expected all three entries")


def require_execution(output):
    if "3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out" not in output:
        raise RuntimeError("process suite did not execute all three entries")


def require_contract_args(trace, subject):
    calls = [parts.split("\0") for parts in trace.split("\0\0") if parts]
    rooted = [args for args in calls if args[0] == str(subject) and "--root" in args
              and args.index("--root") + 1 < len(args)
              and Path(args[args.index("--root") + 1]).name.startswith("rivium-contract-")]
    if len(rooted) != 4:
        raise RuntimeError("contract runner trace must include all four configuration paths")
    for args in rooted:
        if "verify.case=idle" not in args:
            raise RuntimeError("contract args missing from a configuration path")
        if "lifecycle.stop_timeout=0s" in args:
            if args.index("verify.case=idle") > args.index("lifecycle.stop_timeout=0s"):
                raise RuntimeError("contract settings must follow consumer args")


def artifact(output, package, kind, package_id):
    found = []
    for line in output.splitlines():
        if not line.startswith("{"):
            continue
        item = json.loads(line)
        if item.get("reason") != "compiler-artifact":
            continue
        if (item["package_id"] == package_id and item["target"]["name"] == package
                and kind in item["target"]["kind"]):
            if item.get("executable"):
                found.append(Path(item["executable"]))
    if len(found) != 1 or not found[0].is_absolute() or not found[0].is_file():
        raise RuntimeError("ordinary subject artifact must resolve to one absolute file")
    return found[0]


def run(command, env, report, label, receipt, timeout=3600):
    started = time.monotonic()
    process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace", start_new_session=True)
    output = ""
    try:
        output, _ = process.communicate(timeout=timeout)
    except (subprocess.TimeoutExpired, KeyboardInterrupt):
        if os.name == "nt":
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(process.pid)], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            os.killpg(process.pid, signal.SIGKILL)
        output, _ = process.communicate(timeout=60)
        raise RuntimeError(f"{label}: watchdog/cancellation; process tree terminated") from None
    finally:
        (report / f"{label}.log").write_text(output, encoding="utf-8")
        receipt["commands"].append({"command": command, "label": label,
                                    "seconds": round(time.monotonic() - started, 3),
                                    "exit": process.returncode})
    if os.name != "nt":
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            pass
        else:
            os.killpg(process.pid, signal.SIGKILL)
            raise RuntimeError(f"{label}: leaked process group; killed, not a passing cleanup")
    if process.returncode:
        print(output, file=sys.stderr)
        raise RuntimeError(f"{label}: exit {process.returncode}")
    return output


def main():
    if sys.version_info < (3, 11) or len(sys.argv) != 2:
        raise RuntimeError("usage: test-hosts.sh <target>; Python >=3.11; no test filters")
    target = sys.argv[1]
    if not target or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789-_." for c in target):
        raise RuntimeError("invalid target triple")
    key = target.upper().replace("-", "_").replace(".", "_")
    env = {name: os.environ[name] for name in ENV_KEYS if name in os.environ}
    for name in (f"CARGO_TARGET_{key}_RUNNER", f"CARGO_TARGET_{key}_LINKER"):
        if name in os.environ:
            env[name] = os.environ[name]
    env.update(CARGO_TERM_COLOR="never", CARGO_INCREMENTAL="0")
    report = ROOT / "target/verify/reports" / target / f"process-{time.time_ns()}"
    report.mkdir(parents=True)
    receipt = {"target": target, "features": "default", "commands": [], "result": "failed"}
    try:
        receipt["revision"] = run(["git", "rev-parse", "HEAD"], env, report, "revision", receipt).strip()
        receipt["rustc"] = run(["rustc", "-vV"], env, report, "rustc", receipt).strip()
        receipt["cargo"] = run(["cargo", "-V"], env, report, "cargo", receipt).strip()
        receipt["host"] = next(line.removeprefix("host: ") for line in receipt["rustc"].splitlines() if line.startswith("host: "))
        locks = [ROOT / "Cargo.lock", ROOT / "tests/Cargo.lock"]
        receipt["locks"] = {str(path.relative_to(ROOT)): digest(path) for path in locks}
        common = ["--manifest-path", "tests/Cargo.toml", "--locked", "--target", target]
        # Paths are separated by toolchain as well as target/profile/feature.
        version = receipt["rustc"].splitlines()[0].split()[1]
        subjects = ROOT / "target/verify/subjects" / target / "debug/default" / version
        driver = ROOT / "target/verify/drivers" / target / "debug/default" / version
        built = run(["cargo", "build", *common, "-p", "rivium-verify-process",
                     "--target-dir", str(subjects), "--message-format=json"],
                    env, report, "subject-build", receipt)
        package_id = "path+" + (ROOT / "tests/host-bin").as_uri() + "#rivium-verify-process@0.0.0"
        subject = artifact(built, "rivium-verify-process", "bin", package_id)
        for line in built.splitlines():
            if not line.startswith("{"):
                continue
            item = json.loads(line)
            if item.get("reason") == "compiler-artifact":
                if item["target"]["name"] == "rivium_test" or (
                    item["target"]["name"] == "tokio" and "test-util" in item["features"]
                ):
                    raise RuntimeError("ordinary subject contaminated with test dependencies/features")
        receipt["subject"] = {"package_id": package_id, "path": str(subject.relative_to(ROOT)), "sha256": digest(subject)}
        env.update(RIVIUM_VERIFY_PROCESS=str(subject), RUST_LOG="off",
                   RIVIUM_VERIFY_VERIFY__CASE="invalid-case-requires-suite-args")
        tests = ["cargo", "test", *common, "-p", "rivium-verify-app", "--test", "process",
                 "--target-dir", str(driver)]
        listed = run([*tests, "--", "--list"], env, report, "suite-list", receipt)
        require_suite(listed)
        # All entries, no ignored tests/filters; 600s is an outer suite watchdog, not a stop budget.
        output = run([*tests, "--", "--test-threads=1", "--nocapture"],
                     env, report, "process-suite", receipt, timeout=600)
        require_execution(output)
        if receipt["host"] == target and os.name != "nt":
            # A native protocol probe, not a claim about any emulator or cross image.
            runner = report / "runner.sh"
            trace = report / "runner-args.bin"
            runner.write_text(f'#!/bin/sh\nprintf \'%s\\0\' "$@" >> "{trace}"\nprintf \'\\0\' >> "{trace}"\nexec "$@"\n')
            runner.chmod(0o755)
            env[f"CARGO_TARGET_{key}_RUNNER"] = str(runner)
            env["RIVIUM_VERIFY_NATIVE_RUNNER"] = "1"
            run([*tests, "--", "--test-threads=1", "--nocapture"],
                env, report, "native-runner-suite", receipt, timeout=600)
            require_contract_args(trace.read_bytes().decode("utf-8"), subject)
            trace.unlink()
            runner.unlink()
            receipt["runner"] = "native exec/PID/argument probe passed; T2 runner deferred"
        else:
            receipt["runner"] = "native exec probe not applicable; actual target runner still requires platform validation"
        if {str(path.relative_to(ROOT)): digest(path) for path in locks} != receipt["locks"]:
            raise RuntimeError("input lock changed during process verification")
        if digest(subject) != receipt["subject"]["sha256"]:
            raise RuntimeError("subject changed while tests were running")
        receipt["result"] = "passed"
    finally:
        (report / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(f"process verification: {receipt['result']}; receipt {report.relative_to(ROOT)}/receipt.json")
        # Retain a neutral CI summary without uploading raw logs or absolute checkout paths.
        summary = json.dumps(receipt, ensure_ascii=True).replace(str(ROOT), "<checkout>")
        print("process verification receipt: " + summary)


if __name__ == "__main__":
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except (RuntimeError, OSError, ValueError) as error:
        print(f"test-hosts: {error}", file=sys.stderr)
        sys.exit(1)
