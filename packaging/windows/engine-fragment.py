#!/usr/bin/env python3
"""Emit stable, per-user WiX components for the complete private engine tree."""
import hashlib
import importlib.util
from pathlib import Path
import shutil
import sys
import uuid
from xml.etree import ElementTree as ET

SCRIPT = Path(__file__).resolve().parents[2] / "browser/engine/scripts/package.py"
spec = importlib.util.spec_from_file_location("engine_package", SCRIPT)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)
NS = "http://wixtoolset.org/schemas/v4/wxs"
ET.register_namespace("", NS)


def element(parent, tag, **attributes):
    return ET.SubElement(parent, f"{{{NS}}}{tag}", attributes)


def identifier(prefix, path):
    return prefix + hashlib.sha256(path.encode()).hexdigest()[:24]


def fragment(source, output, cabinet_source):
    package.validate(source)
    # MSI still needs legacy path lengths even if Node supports long paths.
    # Keep the complete dependency tree and full digest; fail before release
    # if a future dependency outgrows a representative long profile path.
    prefix = "C:/Users/Firstname Lastname Long Employee Name/AppData/Local/Programs/CwwBE/"
    if any(len(prefix + path.relative_to(source).as_posix()) > 259 for path in source.rglob("*") if path.is_file()):
        raise ValueError("Browser engine exceeds MSI path limits for a long user profile")
    # WiX 5's native cabinet builder also has MAX_PATH limits on *sources*.
    # A temp directory plus the manifest digest and npm paths can exceed those
    # even when every installed path fits. Flatten only the cabinet's inputs;
    # Directory/File Name below retain the complete immutable installation.
    cabinet_source = cabinet_source.resolve()
    if len(str(cabinet_source / ("BE_F" + "0" * 24))) > 259:
        raise ValueError("MSI cabinet source directory is too long")
    cabinet_source.mkdir()
    root = ET.Element(f"{{{NS}}}Wix")
    body = element(root, "Fragment")
    # A short sibling avoids MAX_PATH with unchanged npm paths + full SHA256.
    install = element(body, "DirectoryRef", Id="ProgramsFolder")
    directories = {".": element(install, "Directory", Id="BrowserEngineDir", Name="CwwBE")}
    group = element(body, "ComponentGroup", Id="BrowserEngine")
    for path in sorted(source.rglob("*")):
        relative = path.relative_to(source).as_posix()
        parent = directories[path.parent.relative_to(source).as_posix()]
        if path.is_dir():
            directories[relative] = element(parent, "Directory", Id=identifier("BE_D", relative), Name=path.name)
        elif path.is_file():
            file_id = identifier("BE_F", relative)
            cabinet_file = cabinet_source / file_id
            shutil.copyfile(path, cabinet_file)
            component_id = identifier("BE_C", relative)
            component = element(parent, "Component", Id=component_id,
                                Guid=str(uuid.uuid5(uuid.NAMESPACE_URL, "https://chatwithwork.com/desktop/browser-engine/" + relative)))
            element(component, "File", Id=file_id, Name=path.name,
                    Source=str(cabinet_file), KeyPath="no")
            element(component, "RegistryValue", Root="HKCU", Key="Software\\Chat with Work\\Desktop\\BrowserEngine",
                    Name=component_id, Type="integer", Value="1", KeyPath="yes")
            element(component, "RemoveFolder", Id=identifier("BE_R", relative), On="uninstall")
            element(group, "ComponentRef", Id=component_id)
    ET.indent(root)
    ET.ElementTree(root).write(output, encoding="utf-8", xml_declaration=True)


if __name__ == "__main__":
    fragment(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]))
