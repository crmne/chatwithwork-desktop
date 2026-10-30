#!/usr/bin/env python3
"""Prove installed runtime discovery and startup inside the daemon's sandbox.

Uses an unpaired, temporary CWW_HOME, with no folders, sites or browser profile.
Only fixed readiness/status codes leave this check, never raw daemon output.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time


class SmokeFailure(Exception):
    pass


def check_status(status, platform):
    if not isinstance(status, dict) or type(status.get("pid")) is not int or status["pid"] <= 0:
        raise SmokeFailure("daemon_not_running")
    if status.get("paired") is not False or status.get("roots") != []:
        raise SmokeFailure("unexpected_user_state")
    browser = status.get("browser", {})
    if not isinstance(browser, dict):
        raise SmokeFailure("invalid_status")
    if browser.get("enabled") is not True or browser.get("sites") != [] or browser.get("all_sites"):
        raise SmokeFailure("unexpected_browser_policy")
    sandbox = status.get("sandbox", {})
    if not isinstance(sandbox, dict):
        raise SmokeFailure("invalid_status")
    expected = "enforced" if platform in ("linux", "darwin") else "off"
    if sandbox.get("state") != expected:
        raise SmokeFailure("sandbox_not_enforced" if expected == "enforced" else "unexpected_sandbox_status")
    return {"engine": "ready", "sandbox": sandbox["state"], "platform": platform}


def smoke(binary, timeout=60):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("CWW_") and key not in ("NODE_OPTIONS", "NODE_PATH")}
    with tempfile.TemporaryDirectory(prefix="cww-engine-smoke-") as temporary:
        home = Path(temporary)
        (home / "config").mkdir()
        (home / "config/config.toml").write_text(
            'secret_store = "file"\ncrash_reports = false\n[browser]\nenabled = true\n')
        env.update(CWW_HOME=str(home), CWW_CRASH_REPORTS="0",
                   CWW_LOG="warn,cww::browsing::engine=info")
        ready = threading.Event()
        unavailable = threading.Event()
        process = subprocess.Popen([str(binary), "daemon", "run"], env=env,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.PIPE)

        def read_codes():
            # A rolling bounded buffer detects split messages without keeping
            # or printing logs, paths, command output or exception details.
            buffer = b""
            while True:
                try:
                    chunk = process.stderr.read1(4096)
                except (OSError, ValueError):
                    return
                if not chunk:
                    return
                buffer = (buffer + chunk)[-8192:]
                if b"bundled browser engine is ready" in buffer:
                    ready.set()
                if b"bundled browser runtime did not become ready" in buffer:
                    unavailable.set()

        reader = threading.Thread(target=read_codes, daemon=True)
        reader.start()
        try:
            deadline = time.monotonic() + timeout
            while not ready.wait(min(0.1, max(0, deadline - time.monotonic()))):
                if process.poll() is not None:
                    raise SmokeFailure("daemon_exited_before_ready")
                if unavailable.is_set():
                    raise SmokeFailure("runtime_unavailable")
                if time.monotonic() >= deadline:
                    raise SmokeFailure("runtime_ready_timeout")
            result = subprocess.run([str(binary), "status", "--json"], env=env,
                                    capture_output=True, timeout=10, check=False)
            if result.returncode != 0 or len(result.stdout) > 1024 * 1024:
                raise SmokeFailure("status_failed")
            try:
                status = json.loads(result.stdout)
            except (ValueError, UnicodeDecodeError):
                raise SmokeFailure("invalid_status") from None
            return check_status(status, sys.platform)
        finally:
            # The temporary CWW_HOME binds this request to our daemon alone.
            # Its Unix supervisor forwards shutdown to its confined worker.
            if process.poll() is None:
                try:
                    subprocess.run([str(binary), "daemon", "stop"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   timeout=10, check=False)
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=10)
            reader.join(timeout=2)
            process.stderr.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cww", required=True, type=Path, help="Installed cww executable, including custom installer paths")
    args = parser.parse_args()
    try:
        result = smoke(args.cww.absolute())
    except SmokeFailure as error:
        result = {"error": str(error)}
    except (OSError, subprocess.SubprocessError):
        result = {"error": "runtime_smoke_failed"}
    print(json.dumps(result))
    return 1 if "error" in result else 0


if __name__ == "__main__":
    sys.exit(main())
