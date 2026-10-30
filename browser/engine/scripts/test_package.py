import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location("engine_package", Path(__file__).with_name("package.py"))
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class RuntimePackageTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.config = json.loads(package.PLATFORMS.read_text())

    def fixture(self):
        root = self.root / "engine"
        root.mkdir()
        for name in ("entry.mjs", "engine.mjs", "node", "package-lock.json"):
            (root / name).write_text("fixture")
        (root / "notices").mkdir()
        (root / "notices/Node-LICENSE").write_text("Node aggregate notices")
        dependency = root / "node_modules/@xmorse/playwright-core"
        dependency.mkdir(parents=True)
        (dependency / "package.json").write_text('{"version":"1.59.13"}')
        (root / "package.json").write_text('{"dependencies":{"@xmorse/playwright-core":"1.59.13"}}')
        return root

    def test_manifest_detects_changed_added_and_removed_files(self):
        for mutation in ("change", "add", "remove"):
            with self.subTest(mutation=mutation):
                root = self.fixture()
                package.write_manifest(root, "0.9.1", "x86_64-unknown-linux-gnu", self.config)
                package.validate_payload(root)
                if mutation == "change":
                    (root / "entry.mjs").write_text("changed")
                elif mutation == "add":
                    (root / "unexpected.mjs").write_text("extra")
                else:
                    (root / "engine.mjs").unlink()
                with self.assertRaisesRegex(ValueError, "differs"):
                    package.validate_payload(root)
                import shutil
                shutil.rmtree(root)

    @unittest.skipIf(os.name == "nt", "symlink creation needs a Windows privilege")
    def test_manifest_refuses_symlink_escape(self):
        root = self.fixture()
        (root / "escape").symlink_to(self.root)
        with self.assertRaisesRegex(ValueError, "Symlinks"):
            package.write_manifest(root, "0.9.1", "x86_64-unknown-linux-gnu", self.config)

    def test_exact_engine_version_and_node_license_required(self):
        root = self.fixture()
        (root / "package.json").write_text('{"dependencies":{"@xmorse/playwright-core":"^1.59.13"}}')
        with self.assertRaisesRegex(ValueError, "exact registry"):
            package.write_manifest(root, "0.9.1", "x86_64-unknown-linux-gnu", self.config)
        (root / "notices/Node-LICENSE").unlink()
        with self.assertRaisesRegex(ValueError, "aggregate license"):
            package.write_manifest(root, "0.9.1", "x86_64-unknown-linux-gnu", self.config)

    def test_corrupt_cached_runtime_is_never_used_or_silently_redownloaded(self):
        archive = self.root / "node.tar.xz"
        archive.write_bytes(b"untrusted")
        spec = dict(file=archive.name, sha256=hashlib.sha256(b"trusted").hexdigest())
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            package.fetch_runtime(spec, self.root)

    def test_failed_replacement_keeps_the_previous_complete_package(self):
        root = self.fixture()
        before = package.seal(root, "0.9.1", "x86_64-unknown-linux-gnu", self.config)
        with patch.object(package, "SOURCE", package.resolve(root)), patch.object(package, "fetch_runtime", side_effect=ValueError("checksum mismatch")):
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                package.stage("x86_64-unknown-linux-gnu", root, "0.9.2", self.root / "cache", replace=True)
        self.assertEqual(package.validate(root), before)

    def test_locator_binds_exact_manifest_and_refresh_renames_changed_payload(self):
        root = self.fixture()
        package.seal(root, "0.9.1", "x86_64-unknown-linux-gnu", self.config)
        previous = package.resolve(root)
        self.assertEqual(previous.name, package.digest(previous / "manifest.json"))
        (previous / "node").write_text("signed runtime")
        with self.assertRaisesRegex(ValueError, "differs"):
            package.validate(root)
        package.refresh(root)
        self.assertNotEqual(previous, package.resolve(root))
        self.assertFalse(previous.exists())
        package.validate(root)
        locator = root / "current.json"
        locator.write_text('{"protocol":1,"directory":"../escape"}')
        with self.assertRaisesRegex(ValueError, "locator"):
            package.validate(root)

    def test_replacement_refuses_an_unrelated_directory(self):
        root = self.fixture()
        with self.assertRaisesRegex(ValueError, "previously staged"):
            package.stage("x86_64-unknown-linux-gnu", root, "0.9.1", self.root / "cache", replace=True)
        self.assertTrue((root / "entry.mjs").exists())

    def test_extracts_exact_root_license_and_only_named_files(self):
        spec = dict(directory="node-v1-linux-x64", executable="bin/node")
        archive = self.root / "node.tar.xz"
        with tarfile.open(archive, "w:xz") as out:
            for name, content in {"lib/npm/LICENSE": b"wrong license", "LICENSE": b"Node notices", "bin/node": b"runtime", "../outside": b"ignored"}.items():
                info = tarfile.TarInfo(spec["directory"] + "/" + name)
                info.size = len(content)
                out.addfile(info, io.BytesIO(content))
        package.extract_runtime(archive, spec, self.root / "node", self.root / "LICENSE")
        self.assertEqual((self.root / "LICENSE").read_bytes(), b"Node notices")
        self.assertEqual((self.root / "node").read_bytes(), b"runtime")
        self.assertFalse((self.root / "outside").exists())

    def test_windows_archive_uses_exact_executable_and_notices(self):
        spec = dict(directory="node-v1-win-arm64", executable="node.exe")
        archive = self.root / "node.zip"
        with zipfile.ZipFile(archive, "w") as out:
            out.writestr(spec["directory"] + "/node.exe", b"PE")
            out.writestr(spec["directory"] + "/LICENSE", b"Node notices")
        package.extract_runtime(archive, spec, self.root / "node.exe", self.root / "LICENSE")
        self.assertEqual((self.root / "node.exe").read_bytes(), b"PE")

    def test_arm64_musl_is_explicitly_legacy_and_creates_nothing(self):
        output = self.root / "output"
        with self.assertRaisesRegex(ValueError, "retains the legacy browser"):
            package.stage("aarch64-unknown-linux-musl", output, "0.9.1", self.root / "cache")
        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
