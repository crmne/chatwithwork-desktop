#!/usr/bin/env python3
"""Build-time only: stage a pinned Node runtime and the unchanged locked engine.

No runtime downloader is shipped. Archives are verified before extracting only
their named executable and aggregate license. npm lifecycle scripts and bin links
are disabled. The installer copies the resulting private directory as a unit.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
import zipfile

SOURCE = Path(__file__).resolve().parents[1]
PLATFORMS = SOURCE / "runtime-platforms.json"
REQUIRED = ("entry.mjs", "engine.mjs", "package.json", "package-lock.json")


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def inventory(directory):
    files = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Symlinks are not allowed in the private engine: {path.name}")
        if path.is_file() and path != directory / "manifest.json":
            files[path.relative_to(directory).as_posix()] = digest(path)
    return files


def write_manifest(directory, app_version, target, config):
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", app_version):
        raise ValueError("Invalid app version")
    for name in REQUIRED:
        if not (directory / name).is_file():
            raise ValueError(f"Missing engine source: {name}")
    runtime = "node.exe" if "windows" in target else "node"
    if not (directory / runtime).is_file() or not (directory / "notices/Node-LICENSE").is_file():
        raise ValueError("Missing Node executable or aggregate license")
    package = json.loads((directory / "package.json").read_text())
    version = package["dependencies"]["@xmorse/playwright-core"]
    installed = json.loads((directory / "node_modules/@xmorse/playwright-core/package.json").read_text())
    if not re.fullmatch(r"\d+\.\d+\.\d+", version) or installed["version"] != version:
        raise ValueError("The engine dependency must match an exact registry version")
    manifest = dict(protocol=config["protocol"], app_version=app_version,
                    node_version=config["node_version"], engine_version=version,
                    target=target, node_support_ends=config["node_support_ends"],
                    files=inventory(directory))
    (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def resolve(directory):
    locator = directory / "current.json"
    if locator.is_symlink() or locator.stat().st_size > 1024:
        raise ValueError("Invalid engine locator")
    current = json.loads(locator.read_text())
    name = current.get("directory", "")
    if current.get("protocol") != 1 or not isinstance(name, str) or not re.fullmatch(r"[0-9a-f]{64}", name):
        raise ValueError("Invalid engine locator")
    payload = directory / name
    manifest = payload / "manifest.json"
    if payload.is_symlink() or manifest.is_symlink() or digest(manifest) != name:
        raise ValueError("Engine locator does not match its immutable manifest")
    return payload


def publish_locator(directory, payload):
    locator = directory / ".current.json.new"
    locator.write_text(json.dumps(dict(protocol=1, directory=payload.name)) + "\n")
    locator.replace(directory / "current.json")


def seal(directory, app_version, target, config):
    """Finalize an offline flat assembly into its immutable installation layout."""
    manifest = write_manifest(directory, app_version, target, config)
    entries = list(directory.iterdir())
    payload = directory / digest(directory / "manifest.json")
    payload.mkdir()
    for entry in entries:
        entry.replace(payload / entry.name)
    publish_locator(directory, payload)
    return manifest


def validate_payload(directory):
    manifest = json.loads((directory / "manifest.json").read_text())
    config = json.loads(PLATFORMS.read_text())
    if manifest.get("protocol") != config["protocol"] or manifest.get("node_version") != config["node_version"]:
        raise ValueError("Unexpected engine protocol or Node version")
    expected = set(REQUIRED) | {"node.exe" if "windows" in manifest["target"] else "node", "notices/Node-LICENSE"}
    if not expected.issubset(manifest["files"]):
        raise ValueError("Engine manifest is missing required files")
    for name in manifest["files"]:
        path = PurePosixPath(name)
        if path.is_absolute() or ".." in path.parts or "\\" in name:
            raise ValueError("Invalid manifest path")
    if manifest["files"] != inventory(directory):
        raise ValueError("Engine package differs from its manifest")
    return manifest


def validate(directory):
    return validate_payload(resolve(directory))


def refresh(directory):
    payload = resolve(directory)
    old = json.loads((payload / "manifest.json").read_text())
    write_manifest(payload, old["app_version"], old["target"], json.loads(PLATFORMS.read_text()))
    destination = directory / digest(payload / "manifest.json")
    if payload != destination:
        payload.replace(destination)
    publish_locator(directory, destination)


def fetch_runtime(spec, cache):
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / spec["file"]
    if not archive.exists():
        url = f"https://nodejs.org/dist/v{json.loads(PLATFORMS.read_text())['node_version']}/{spec['file']}"
        temporary = tempfile.NamedTemporaryFile(dir=cache, delete=False)
        partial = Path(temporary.name)
        try:
            with temporary:
                with urllib.request.urlopen(url, timeout=60) as response:
                    if not response.url.startswith("https://nodejs.org/dist/"):
                        raise ValueError("Unexpected runtime download origin")
                    shutil.copyfileobj(response, temporary)
                temporary.flush()
            # Windows cannot rename/unlink an open temporary download.
            if digest(partial) != spec["sha256"]:
                raise ValueError("Node archive checksum mismatch")
            partial.replace(archive)
        finally:
            partial.unlink(missing_ok=True)
    if digest(archive) != spec["sha256"]:
        raise ValueError("Cached Node archive checksum mismatch")
    return archive


def extract_runtime(archive, spec, executable, license_path):
    members = {f"{spec['directory']}/{spec['executable']}": executable,
               f"{spec['directory']}/LICENSE": license_path}
    # Do not extract an archive tree or follow archive-provided links.
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as source:
            for name, destination in members.items():
                info = source.getinfo(name)
                if info.is_dir() or (info.external_attr >> 16) & 0o170000 == 0o120000:
                    raise ValueError("Runtime member is not a regular file")
                with source.open(info) as stream, destination.open("wb") as output:
                    shutil.copyfileobj(stream, output)
    else:
        with tarfile.open(archive) as source:
            for name, destination in members.items():
                member = source.getmember(name)
                if not member.isfile():
                    raise ValueError("Runtime member is not a regular file")
                with source.extractfile(member) as stream, destination.open("wb") as output:
                    shutil.copyfileobj(stream, output)
    executable.chmod(0o755)
    license_path.chmod(0o644)


def stage(target, output, app_version, cache, replace=False):
    config = json.loads(PLATFORMS.read_text())
    if target in config["legacy_targets"]:
        raise ValueError(f"{target} retains the legacy browser; no official bundled runtime")
    targets = ["x86_64-apple-darwin", "aarch64-apple-darwin"] if target == "macos-universal" else [target]
    specs = [config["targets"][item] for item in targets]
    if output.exists() and (not replace or not (output / "current.json").is_file()):
        raise ValueError("Output already exists; replacement requires a previously staged engine")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".browser-engine-", dir=output.parent) as temporary:
        staged = Path(temporary) / "browser-engine"
        staged.mkdir()
        for name in REQUIRED:
            shutil.copyfile(SOURCE / name, staged / name)
        if (SOURCE / "notices").exists():
            shutil.copytree(SOURCE / "notices", staged / "notices")
        else:
            (staged / "notices").mkdir()
        node = "node.exe" if "windows" in target else "node"
        for index, spec in enumerate(specs):
            destination = staged / node if len(specs) == 1 else Path(temporary) / f"node-{index}"
            extract_runtime(fetch_runtime(spec, cache), spec, destination, staged / "notices/Node-LICENSE")
        if len(specs) == 2:
            subprocess.run(["lipo", "-create", str(Path(temporary) / "node-0"),
                            str(Path(temporary) / "node-1"), "-output", str(staged / node)], check=True)
            for architecture in ("x86_64", "arm64"):
                subprocess.run(["lipo", str(staged / node), "-verify_arch", architecture], check=True)
        npm = shutil.which("npm.cmd" if os.name == "nt" else "npm")
        if not npm:
            raise ValueError("npm is required at build time")
        env = {key: value for key, value in os.environ.items() if key not in ("NODE_OPTIONS", "NODE_PATH")}
        subprocess.run([npm, "ci", "--omit=dev", "--ignore-scripts", "--bin-links=false", "--no-audit", "--no-fund"],
                       cwd=staged, env=env, check=True)
        for path in staged.rglob("*"):
            if not path.is_symlink():
                path.chmod(0o755 if path.is_dir() or path == staged / node else 0o644)
        seal(staged, app_version, target, config)
        if output.exists():
            output.replace(Path(temporary) / "previous")
        staged.replace(output)
    print(f"Staged browser engine for {target}: {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("stage")
    build.add_argument("--target", required=True)
    build.add_argument("--output", type=Path, required=True)
    build.add_argument("--app-version", required=True)
    build.add_argument("--cache", type=Path, required=True)
    build.add_argument("--replace", action="store_true", help="Replace a previous staged engine only after new staging succeeds")
    refresh_command = commands.add_parser("refresh", help="Rehash after runtime signing, before sealing the app")
    refresh_command.add_argument("directory", type=Path)
    manifest = commands.add_parser("manifest", help="Hash an offline assembly, including Nix builds")
    manifest.add_argument("directory", type=Path)
    manifest.add_argument("--target", required=True)
    manifest.add_argument("--app-version", required=True)
    check = commands.add_parser("validate")
    check.add_argument("directory", type=Path)
    path = commands.add_parser("path", help="Print the resolved immutable payload for build tools")
    path.add_argument("directory", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "stage":
            stage(args.target, args.output.resolve(), args.app_version, args.cache.resolve(), args.replace)
        elif args.command == "refresh":
            refresh(args.directory)
        elif args.command == "manifest":
            seal(args.directory, args.app_version, args.target, json.loads(PLATFORMS.read_text()))
        elif args.command == "path":
            print(resolve(args.directory).resolve())
        else:
            validate(args.directory)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"browser-engine packaging: {error}\n")


if __name__ == "__main__":
    main()
