"""Fail-closed input checks for the ordinary process build/driver entry."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import os
import sys

spec = importlib.util.spec_from_file_location("hosts", Path(__file__).parents[1] / "test-hosts.py")
hosts = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hosts)


class ProcessInputs(unittest.TestCase):
    def test_watchdog_is_failure_and_reaps_child(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory)
            receipt = {"commands": []}
            env = {name: os.environ[name] for name in hosts.ENV_KEYS if name in os.environ}
            with self.assertRaisesRegex(RuntimeError, "watchdog/cancellation"):
                hosts.run([sys.executable, "-c", "import time; time.sleep(30)"],
                          env, report, "watchdog", receipt, timeout=0.1)
            self.assertIsNotNone(receipt["commands"][0]["exit"])
            self.assertTrue((report / "watchdog.log").is_file())

    def test_missing_or_filtered_suite_fails(self):
        valid = "\n".join(f"{name}: test" for name in hosts.ENTRIES)
        hosts.require_suite(valid)
        for invalid in ("", "0 tests", "host_boundaries: test", valid.replace("process_contract: test", "process_contract: benchmark")):
            with self.assertRaisesRegex(RuntimeError, "suite missing, empty, or filtered"):
                hosts.require_suite(invalid)

    def test_ignored_or_filtered_execution_fails(self):
        hosts.require_execution("3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out")
        for invalid in ("", "2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out",
                        "0 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out"):
            with self.assertRaisesRegex(RuntimeError, "did not execute all three"):
                hosts.require_execution(invalid)

    def test_contract_args_reach_every_configuration_path(self):
        subject = Path("/subject")
        calls = [[str(subject), "--root", "/tmp/rivium-contract-1", "--set", "verify.case=idle"] for _ in range(4)]
        calls[0] += ["--set", "lifecycle.stop_timeout=0s"]
        def trace(items):
            return "\0\0".join("\0".join(args) for args in items) + "\0\0"
        hosts.require_contract_args(trace(calls), subject)
        for index in range(4):
            changed = [list(args) for args in calls]
            changed[index].remove("verify.case=idle")
            with self.assertRaisesRegex(RuntimeError, "args missing"):
                hosts.require_contract_args(trace(changed), subject)
        with self.assertRaisesRegex(RuntimeError, "four configuration paths"):
            hosts.require_contract_args(trace(calls[:3]), subject)

    def test_artifact_is_current_unique_and_absolute(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "subject"
            binary.write_text("current artifact")
            event = {"reason": "compiler-artifact", "package_id": "current-package", "target": {"name": "rivium-verify-process", "kind": ["bin"]}, "executable": str(binary)}
            output = json.dumps(event)
            self.assertEqual(hosts.artifact(output, "rivium-verify-process", "bin", "current-package"), binary)
            for invalid in ("", output + "\n" + output, output.replace(str(binary), "relative"), output.replace('"bin"', '"lib"'), output.replace("current-package", "stale-registry-package")):
                with self.assertRaisesRegex(RuntimeError, "artifact must resolve"):
                    hosts.artifact(invalid, "rivium-verify-process", "bin", "current-package")
            binary.unlink()
            with self.assertRaisesRegex(RuntimeError, "artifact must resolve"):
                hosts.artifact(output, "rivium-verify-process", "bin", "current-package")


if __name__ == "__main__":
    unittest.main()
