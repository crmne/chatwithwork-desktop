import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("runtime_smoke", Path(__file__).with_name("runtime_smoke.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class RuntimeSmokeStatusTest(unittest.TestCase):
    def status(self, sandbox="enforced"):
        return {"pid": 1234, "paired": False, "roots": [],
                "browser": {"enabled": True, "sites": [], "all_sites": False},
                "sandbox": {"state": sandbox}}

    def test_unix_runtime_ready_does_not_excuse_missing_confinement(self):
        for platform in ("linux", "darwin"):
            for state in ("off", "partial", "unavailable"):
                with self.subTest(platform=platform, state=state):
                    with self.assertRaisesRegex(smoke.SmokeFailure, "sandbox_not_enforced"):
                        smoke.check_status(self.status(state), platform)

    def test_windows_reports_actual_unconfined_status(self):
        self.assertEqual(smoke.check_status(self.status("off"), "win32"),
                         {"engine": "ready", "sandbox": "off", "platform": "win32"})

    def test_unexpected_user_state_is_not_a_valid_runtime_probe(self):
        for key, value in (("paired", True), ("roots", [{"local_path": "private-fixture"}])):
            status = self.status()
            status[key] = value
            with self.assertRaisesRegex(smoke.SmokeFailure, "unexpected_user_state"):
                smoke.check_status(status, "darwin")

    def test_success_never_returns_status_paths_or_identifiers(self):
        status = self.status()
        status.update(config_file="private-fixture", server="private-fixture", device_id="private-fixture")
        self.assertEqual(smoke.check_status(status, "linux"),
                         {"engine": "ready", "sandbox": "enforced", "platform": "linux"})


if __name__ == "__main__":
    unittest.main()
