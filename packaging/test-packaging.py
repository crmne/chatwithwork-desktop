#!/usr/bin/env python3
"""Package layout and installer regressions, without touching the user's setup."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent
SCRATCH = ROOT / ".cache" / "packaging-tests"
spec = importlib.util.spec_from_file_location("engine_package", ROOT / "browser/engine/scripts/package.py")
engine_package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(engine_package)


def run(*args, **kwargs):
    return subprocess.run(args, text=True, capture_output=True, **kwargs)


def executable(path, contents):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents)
    path.chmod(0o755)


class PackagingTest(unittest.TestCase):
    def setUp(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.bin = self.work / "bin"
        self.bin.mkdir()
        self.cww = self.bin / "cww"
        executable(self.cww, '''#!/bin/sh
if [ "$1" = --version ]; then echo 'cww 9.8.7'; exit; fi
printf '%s\\n' "$*" >> "$TEST_CALLS"
exit "${TEST_DAEMON_EXIT:-0}"
''')
        executable(self.bin / "cww-app", "#!/bin/sh\necho 'Chat with Work 9.8.7'\n")
        self.env = dict(os.environ, TEST_CALLS=str(self.work / "calls"))
        self.engine = self.work / "browser-engine"
        self.engine.mkdir()
        for name in ("entry.mjs", "engine.mjs", "package-lock.json"):
            (self.engine / name).write_text("{}")
        (self.engine / "package.json").write_text(json.dumps({"dependencies": {"@xmorse/playwright-core": "1.59.13"}}))
        (self.engine / "node_modules/@xmorse/playwright-core").mkdir(parents=True)
        (self.engine / "node_modules/@xmorse/playwright-core/package.json").write_text('{"version":"1.59.13"}')
        (self.engine / "notices").mkdir()
        (self.engine / "notices/Node-LICENSE").write_text("Aggregate runtime license")
        executable(self.engine / "node", "#!/bin/sh\necho v22.23.4\n")
        engine_package.seal(self.engine, "9.8.7", "x86_64-unknown-linux-gnu", json.loads(engine_package.PLATFORMS.read_text()))

    def archive(self, target="x86_64-unknown-linux-gnu"):
        result = run("bash", str(ROOT / "packaging/linux/archive.sh"),
                     str(self.cww), str(self.bin / "cww-app"), "9.8.7", target,
                     str(self.work / "release"), str(self.engine))
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.work / "release" / f"cww-app-v9.8.7-{target}.tar.gz"

    def test_complete_archive_for_each_architecture(self):
        for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
            with self.subTest(target=target), tarfile.open(self.archive(target)) as archive:
                base = f"cww-app-v9.8.7-{target}/"
                for name in ("cww", "cww-app", "cww-app.desktop", "cww-app.svg",
                             "packaging/systemd/cww.service", "README.md",
                             "PROTOCOL.md", "SECURITY.md", "LICENSE-MIT", "LICENSE-APACHE",
                             f"browser-engine/{engine_package.resolve(self.engine).name}/node",
                             f"browser-engine/{engine_package.resolve(self.engine).name}/entry.mjs",
                             "browser-engine/current.json"):
                    self.assertTrue(archive.getmember(base + name).isfile(), name)
                for name in ("cww", "cww-app"):
                    self.assertEqual(archive.getmember(base + name).mode, 0o755)

    def test_static_cli_archive_includes_native_helper_and_identity(self):
        helper = self.bin / "cww-notify"
        executable(helper, "#!/bin/sh\necho 'cww-notify 9.8.7'\n")
        for target in ("x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"):
            with self.subTest(target=target):
                result = run("ruby", str(ROOT / "packaging/linux/cli-archive.rb"),
                             str(self.cww), str(helper), "9.8.7", target,
                             str(self.work / "release"),
                             *([str(self.engine)] if target.startswith("x86_64") else []))
                self.assertEqual(result.returncode, 0, result.stderr)
                name = f"cww-v9.8.7-{target}"
                with tarfile.open(self.work / "release" / f"{name}.tar.gz") as archive:
                    for binary in ("cww", "cww-notify"):
                        self.assertEqual(archive.getmember(f"{name}/{binary}").mode, 0o755)
                    self.assertTrue(archive.getmember(f"{name}/cww-app.svg").isfile())
                    desktop = archive.extractfile(f"{name}/cww-app.desktop").read().decode()
                    self.assertIn("Icon=cww-app\n", desktop)
                    self.assertIn("NoDisplay=true\n", desktop)
                    self.assertIn("Exec=cww\n", desktop)
                    names = archive.getnames()
                    self.assertEqual(f"{name}/browser-engine/current.json" in names, target.startswith("x86_64"))

    def test_cli_archive_requires_the_helper(self):
        result = run("ruby", str(ROOT / "packaging/linux/cli-archive.rb"),
                     str(self.cww), str(self.bin / "missing"), "9.8.7",
                     "x86_64-unknown-linux-musl", str(self.work / "release"))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.work / "release").exists())

    def test_arch_package_functions_install_gui_cli_service_and_launcher(self):
        archive = self.archive()
        source = self.work / "src"
        source.mkdir()
        with tarfile.open(archive) as tar:
            tar.extractall(source, filter="data")
        for variant in ("chatwithwork-desktop-bin", "chatwithwork-desktop",
                        "chatwithwork-desktop-git"):
            with self.subTest(variant=variant):
                recipe = ROOT / "packaging/arch" / variant / "PKGBUILD.in"
                tree = source / (variant if variant.endswith("-git") else f"{variant}-9.8.7")
                if not variant.endswith("-bin"):
                    shutil.copytree(source / "cww-app-v9.8.7-x86_64-unknown-linux-gnu", tree)
                    (tree / "target/release").mkdir(parents=True)
                    shutil.copy(self.cww, tree / "target/release/cww")
                    shutil.copy(self.bin / "cww-app", tree / "target/release/cww-app")
                    (tree / "packaging/linux").mkdir(parents=True)
                    shutil.copy(ROOT / "packaging/linux/cww-app.desktop", tree / "packaging/linux")
                    (tree / "app/assets").mkdir(parents=True)
                    shutil.copy(ROOT / "app/assets/mark.svg", tree / "app/assets")
                package = self.work / variant
                result = run("bash", "-ec", 'source "$1"; pkgver=9.8.7; package',
                             "test", str(recipe), env=dict(self.env, srcdir=str(source),
                             pkgdir=str(package), CARCH="x86_64"))
                self.assertEqual(result.returncode, 0, result.stderr)
                for name in ("usr/bin/cww", "usr/bin/cww-app",
                             "usr/lib/systemd/user/cww.service",
                             "usr/lib/chatwithwork/browser-engine/current.json",
                             "usr/share/applications/cww-app.desktop",
                             "usr/share/icons/hicolor/scalable/apps/cww-app.svg"):
                    self.assertTrue((package / name).is_file(), name)

    def installer(self, *, daemon_exit=0, uid="1000", corrupt=False, unusual_path=False):
        archive = self.archive()
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        (archive.parent / "checksums.txt").write_text(f"{checksum}  {archive.name}\n")
        if corrupt:
            archive.write_bytes(b"broken download")
        mocks = self.work / "mocks"
        executable(mocks / "curl", '''#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    https://*) name=${1##*/} ;;
    -o) shift; output=$1 ;;
  esac
  shift
done
cp "$TEST_RELEASE/$name" "$output"
''')
        executable(mocks / "uname", '#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
        executable(mocks / "id", f"#!/bin/sh\necho {uid}\n")
        install = self.work / ('bin spaces $cash `tick` % "quote" \\ slash' if unusual_path else "installed")
        data = self.work / "data"
        env = dict(self.env, PATH=f"{mocks}:{os.environ['PATH']}",
                   CWW_DOWNLOAD_BASE="https://example.invalid/release",
                   CWW_INSTALL_DIR=str(install), XDG_DATA_HOME=str(data),
                   XDG_CACHE_HOME=str(self.work / "cache"),
                   TEST_RELEASE=str(archive.parent), TEST_DAEMON_EXIT=str(daemon_exit))
        result = run("sh", str(ROOT / "packaging/install.sh"), env=env)
        return result, install, data

    def test_arch_git_version_ignores_newer_browser_release_tags(self):
        tree = self.work / "chatwithwork-desktop-git"
        tree.mkdir()

        def git(*args):
            result = run("git", "-c", "user.name=Packaging Test",
                         "-c", "user.email=packaging@example.invalid",
                         "-c", "commit.gpgsign=false", "-C", str(tree), *args)
            self.assertEqual(result.returncode, 0, result.stderr)

        git("init", "-q")
        git("commit", "--allow-empty", "-qm", "Desktop release")
        git("tag", "v1.2.3")
        git("commit", "--allow-empty", "-qm", "Browser release")
        git("tag", "browser-v0.99.0")
        recipe = ROOT / "packaging/arch/chatwithwork-desktop-git/PKGBUILD.in"
        result = run("bash", "-ec", 'source "$1"; pkgver', "test", str(recipe),
                     env=dict(self.env, srcdir=str(self.work)))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertRegex(result.stdout.strip(), r"^1\.2\.3\.r1\.[0-9a-f]{7}$")

    def test_installer_starts_daemon_and_installs_desktop_entry(self):
        result, install, data = self.installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(os.access(install / "cww-app", os.X_OK))
        self.assertTrue(os.access(install / "cww", os.X_OK))
        engine_package.validate(install / "browser-engine")
        self.assertEqual((self.work / "calls").read_text(), "daemon install\n")
        self.assertIn("will start at every login", result.stdout)
        entry = (data / "applications/cww-app.desktop").read_text()
        self.assertIn(f'Exec="{install}/cww-app"', entry)
        self.assertTrue((data / "icons/hicolor/scalable/apps/cww-app.svg").is_file())

    def test_installer_reports_failed_service_setup_without_failing_install(self):
        result, install, _ = self.installer(daemon_exit=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((install / "cww-app").is_file())
        self.assertIn("could not be started", result.stdout)
        self.assertIn("daemon install", result.stdout)
        self.assertIn("daemon run", result.stdout)

    def test_upgrade_replaces_engine_tree_without_retaining_removed_modules(self):
        (engine_package.resolve(self.engine) / "node_modules/obsolete.js").write_text("old code")
        engine_package.refresh(self.engine)
        result, install, _ = self.installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        previous = engine_package.resolve(install / "browser-engine")
        (engine_package.resolve(self.engine) / "node_modules/obsolete.js").unlink()
        engine_package.refresh(self.engine)
        result, _, _ = self.installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((engine_package.resolve(install / "browser-engine") / "node_modules/obsolete.js").exists())
        self.assertFalse(previous.exists())
        engine_package.validate(install / "browser-engine")

    def test_failed_restart_retains_previous_immutable_payload_without_mixing_files(self):
        result, install, _ = self.installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        previous = engine_package.resolve(install / "browser-engine")
        original = (previous / "entry.mjs").read_bytes()
        (engine_package.resolve(self.engine) / "entry.mjs").write_text("new code")
        engine_package.refresh(self.engine)
        result, _, _ = self.installer(daemon_exit=1)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotEqual(previous, engine_package.resolve(install / "browser-engine"))
        self.assertEqual((previous / "entry.mjs").read_bytes(), original)
        engine_package.validate_payload(previous)
        engine_package.validate(install / "browser-engine")

    def test_mac_installer_links_both_commands_into_the_same_bundle(self):
        name = "cww-v9.8.7-macos-universal"
        tree = self.work / name
        contents = tree / "Chat with Work.app/Contents"
        (contents / "MacOS").mkdir(parents=True)
        shutil.copy(self.cww, contents / "MacOS/cww")
        shutil.copy(self.bin / "cww-app", contents / "MacOS/cww-app")
        shutil.copytree(self.engine, contents / "Resources/browser-engine")
        (tree / "cww").symlink_to("Chat with Work.app/Contents/MacOS/cww")
        release = self.work / "release"
        release.mkdir()
        archive = release / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as out:
            out.add(tree, arcname=name)
        (release / "checksums.txt").write_text(f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
        mocks = self.work / "mocks"
        executable(mocks / "curl", '''#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in https://*) name=${1##*/};; -o) shift; output=$1;; esac
  shift
done
cp "$TEST_RELEASE/$name" "$output"
''')
        executable(mocks / "uname", '#!/bin/sh\necho Darwin\n')
        executable(mocks / "id", '#!/bin/sh\necho 1000\n')
        executable(mocks / "ditto", '#!/bin/sh\nmkdir -p "$2"\ncp -R "$1/." "$2/"\n')
        home = self.work / "home"
        install = home / "bin"
        result = run("sh", str(ROOT / "packaging/install.sh"), env=dict(self.env,
            PATH=f"{mocks}:{os.environ['PATH']}", HOME=str(home), CWW_INSTALL_DIR=str(install),
            XDG_CACHE_HOME=str(self.work / "cache"), TEST_RELEASE=str(release),
            CWW_DOWNLOAD_BASE="https://example.invalid/release"))
        self.assertEqual(result.returncode, 0, result.stderr)
        for binary in ("cww", "cww-app"):
            self.assertTrue((install / binary).is_symlink())
            self.assertEqual((install / binary).resolve(), home / "Applications/Chat with Work.app/Contents/MacOS" / binary)
        engine_package.validate(home / "Applications/Chat with Work.app/Contents/Resources/browser-engine")

    def test_windows_fragment_includes_every_file_and_stable_user_components(self):
        from xml.etree import ElementTree
        first, second = self.work / "first.wxs", self.work / "second.wxs"
        for output in (first, second):
            result = run("python3", str(ROOT / "packaging/windows/engine-fragment.py"), str(self.engine), str(output), str(output.with_suffix(".files")))
            self.assertEqual(result.returncode, 0, result.stderr)
        document = ElementTree.parse(first)
        again = ElementTree.parse(second)
        # Staging paths differ; component identities and installed names do not.
        for tree in (document, again):
            for item in tree.iter("{http://wixtoolset.org/schemas/v4/wxs}File"):
                item.set("Source", str(first.with_suffix(".files") / Path(item.get("Source")).name))
        self.assertEqual(ElementTree.tostring(document.getroot()), ElementTree.tostring(again.getroot()))
        ns = {"w": "http://wixtoolset.org/schemas/v4/wxs"}
        directory = document.find(".//w:DirectoryRef", ns)
        self.assertEqual(directory.get("Id"), "ProgramsFolder")
        self.assertEqual(directory.find("w:Directory", ns).get("Name"), "CwwBE")
        # The helper stays in the Known Folder even with INSTALLFOLDER=D:\Custom.
        self.assertNotIn("INSTALLFOLDER", first.read_text())
        components = document.findall(".//w:Component", ns)
        expected = set(engine_package.inventory(self.engine))
        reconstructed = self.work / "reconstructed"
        def install_files(node, destination):
            if node.tag == f"{{{ns['w']}}}Directory":
                destination /= node.get("Name")
            elif node.tag == f"{{{ns['w']}}}File":
                destination.mkdir(parents=True, exist_ok=True)
                source = Path(node.get("Source"))
                self.assertEqual(source.parent, first.with_suffix(".files"))
                self.assertLessEqual(len(str(source)), 259)
                shutil.copyfile(source, destination / node.get("Name"))
            for child in node:
                install_files(child, destination)
        install_files(directory, reconstructed)
        installed = reconstructed / "CwwBE"
        self.assertEqual(set(engine_package.inventory(installed)), expected)
        self.assertEqual(engine_package.inventory(installed), engine_package.inventory(self.engine))
        engine_package.validate(installed)
        for component in components:
            key = component.find("w:RegistryValue", ns)
            self.assertEqual((key.get("Root"), key.get("KeyPath")), ("HKCU", "yes"))

    def test_windows_packaging_refuses_paths_that_would_fail_in_long_profiles(self):
        payload = engine_package.resolve(self.engine)
        (payload / ("x" * 140)).write_text("a future long dependency")
        engine_package.refresh(self.engine)
        result = run("python3", str(ROOT / "packaging/windows/engine-fragment.py"), str(self.engine), str(self.work / "engine.wxs"), str(self.work / "engine.files"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("MSI path limits", result.stderr)

    def test_windows_cabinet_staging_flattens_long_build_paths_without_changing_sources(self):
        from xml.etree import ElementTree
        source = self.work / ("a" * 80) / ("b" * 80) / "browser-engine"
        shutil.copytree(self.engine, source)
        before = engine_package.inventory(source)
        self.assertTrue(any(len(str(path)) > 259 for path in source.rglob("*") if path.is_file()))
        output, cabinet = self.work / "engine.wxs", self.work / "flat"
        result = run("python3", str(ROOT / "packaging/windows/engine-fragment.py"), str(source), str(output), str(cabinet))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(engine_package.inventory(source), before)
        files = ElementTree.parse(output).findall(".//{http://wixtoolset.org/schemas/v4/wxs}File")
        self.assertEqual(len(files), len(before))
        for item in files:
            staged = Path(item.get("Source"))
            self.assertEqual(staged.parent, cabinet)
            self.assertLessEqual(len(str(staged)), 259)
            self.assertEqual(staged.name, item.get("Id"))
        self.assertCountEqual((hashlib.sha256(path.read_bytes()).hexdigest() for path in cabinet.iterdir()), before.values())

    def test_windows_cabinet_staging_refuses_overlong_temporary_directory(self):
        cabinet = self.work / ("x" * 200)
        result = run("python3", str(ROOT / "packaging/windows/engine-fragment.py"), str(self.engine), str(self.work / "engine.wxs"), str(cabinet))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cabinet source directory is too long", result.stderr)
        self.assertFalse(cabinet.exists())

    def test_root_installer_does_not_register_a_root_daemon(self):
        result, _, _ = self.installer(uid="0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.work / "calls").exists())
        self.assertIn("without sudo", result.stdout)

    def test_bad_checksum_does_not_install_or_start_anything(self):
        result, install, data = self.installer(corrupt=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertFalse(install.exists())
        self.assertFalse(data.exists())
        self.assertFalse((self.work / "calls").exists())

    def test_launcher_handles_spaces_and_desktop_reserved_characters(self):
        result, install, data = self.installer(unusual_path=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        entry = (data / "applications/cww-app.desktop").read_text()
        # First decode Desktop Entry string escapes, then quoted Exec escapes
        # and percent field codes. The result must be precisely the binary path.
        command = next(line[5:] for line in entry.splitlines() if line.startswith("Exec="))
        command = command.replace("\\\\", "\\")
        self.assertTrue(command.startswith('"') and command.endswith('"'))
        command = command[1:-1]
        for char in ('"', '`', '$', '\\'):
            command = command.replace("\\" + char, char)
        self.assertEqual(command.replace("%%", "%"), str(install / "cww-app"))

    def test_linux_postinstall_and_arch_upgrade_explain_per_user_startup(self):
        for command in (("sh", str(ROOT / "packaging/linux/postinstall.sh")),
                        ("bash", "-ec", 'source "$1"; post_upgrade', "test",
                         str(ROOT / "packaging/arch/cww.install"))):
            result = run(*command)
            self.assertEqual(result.returncode, 0, result.stderr)
            for text in ("applications menu", "without sudo", "cww daemon install",
                         "systemctl --user restart", "cww daemon run"):
                self.assertIn(text, result.stdout)


    def test_windows_install_instructions_work_before_winget_has_the_package(self):
        # ChatWithWork.Desktop is waiting for review in microsoft/winget-pkgs;
        # until packaging/winget/PUBLISHED exists, no install instruction
        # may offer `winget install`, and each names a download that works.
        if (ROOT / "packaging/winget/PUBLISHED").exists():
            self.skipTest("the WinGet package is published")
        for path in ("README.md", "packaging/release-notes/TEMPLATE.md"):
            text = (ROOT / path).read_text(encoding="utf-8")
            self.assertTrue([line for line in text.splitlines() if line.startswith("| Windows")], path)
            self.assertNotIn("winget install", text, path)
            self.assertNotIn("`winget upgrade`", text, path)
        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        row = next(line for line in readme.splitlines() if line.startswith("| Windows 10 and 11"))
        self.assertIn("releases/latest", row)
        self.assertIn("cww-installer.ps1", readme)
        self.assertIn("winget-pkgs/pull/460095", readme)

    def test_readme_names_the_windows_that_runs_commands(self):
        # Commands need a BaseContainer: Windows 11 24H2 with its August 2026
        # update or later (docs/commands.md). Every mention says so.
        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        mentions = [m for m in re.finditer(r"Windows 11 24H2", readme)]
        self.assertTrue(mentions)
        for mention in mentions:
            self.assertIn("August 2026 update", readme[mention.start():mention.start() + 80])

    def test_installer_source_is_well_formed_and_purges_only_on_uninstall(self):
        from xml.etree import ElementTree
        wxs = ElementTree.parse(ROOT / "packaging/windows/cww.wxs").getroot()
        ns = {"w": "http://wixtoolset.org/schemas/v4/wxs", "u": "http://wixtoolset.org/schemas/v4/wxs/util"}
        actions = {c.get("Action"): c.get("Condition") for c in wxs.iterfind(".//w:Custom", ns)}
        self.assertIn("NOT UPGRADINGPRODUCTCODE", actions["PurgeDaemon"])
        self.assertNotIn("NOT", actions["UnregisterDaemon"])
        close = wxs.find(".//u:CloseApplication", ns)
        self.assertEqual(close.get("Target"), "cww-app.exe")

if __name__ == "__main__":
    unittest.main()
