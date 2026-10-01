#!/usr/bin/env python3
"""Download official Alpine Linux binaries into a private, relocatable tools directory."""
import argparse
import gzip
import io
import json
import os
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path

BRANCH = "v3.22"
MIRROR = "https://dl-cdn.alpinelinux.org/alpine"


def download(url):
    print("Download: " + url, flush=True)
    with urllib.request.urlopen(url, timeout=120) as response:
        return response.read()


def catalog(arch):
    packages, providers = {}, {}
    for repository in ("main", "community"):
        url = "{}/{}/{}/{}/APKINDEX.tar.gz".format(MIRROR, BRANCH, repository, arch)
        with tarfile.open(fileobj=io.BytesIO(download(url)), mode="r:gz") as archive:
            text = archive.extractfile("APKINDEX").read().decode()
        for paragraph in text.split("\n\n"):
            fields = dict(line.split(":", 1) for line in paragraph.splitlines() if ":" in line)
            if "P" not in fields:
                continue
            fields["repository"] = repository
            packages[fields["P"]] = fields
            for provided in fields.get("p", "").split():
                providers[re.split(r"[<>=~]", provided, maxsplit=1)[0]] = fields["P"]
    return packages, providers


def dependencies(packages, providers):
    selected = {}
    pending = ["plocate", "coreutils-sort" if "coreutils-sort" in packages else "coreutils"]
    while pending:
        dependency = pending.pop()
        if dependency.startswith("!") or dependency == "/bin/sh":
            continue
        name = re.split(r"[<>=~]", dependency, maxsplit=1)[0]
        name = name if name in packages else providers.get(name)
        if not name or name not in packages:
            raise RuntimeError("Cannot resolve dependency: " + dependency)
        if name in selected:
            continue
        selected[name] = packages[name]
        pending.extend(packages[name].get("D", "").split())
    return selected.values()


def unpack(data, root):
    # APK v2 consists of concatenated gzip/tar members, including control files.
    with tarfile.open(fileobj=io.BytesIO(gzip.decompress(data)), mode="r:", ignore_zeros=True) as archive:
        for member in archive:
            relative = Path(member.name)
            if relative.is_absolute() or ".." in relative.parts:
                raise RuntimeError("Unsafe archive path: " + member.name)
            if not relative.parts or relative.parts[0] not in ("lib", "usr", "bin", "sbin"):
                continue
            destination = root / relative
            if not str(destination.resolve()).startswith(str(root.resolve()) + os.sep):
                raise RuntimeError("Archive path escapes tools directory")
            if member.isdir():
                destination.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                destination.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(member) as source, destination.open("wb") as output:
                    shutil.copyfileobj(source, output)
                destination.chmod(member.mode & 0o755)
            elif member.issym():
                target = Path(member.linkname)
                target = root / str(target).lstrip("/") if target.is_absolute() else destination.parent / target
                if not str(target.resolve()).startswith(str(root.resolve()) + os.sep):
                    raise RuntimeError("Archive link escapes tools directory")
                destination.parent.mkdir(parents=True, exist_ok=True)
                if destination.is_symlink():
                    destination.unlink()
                destination.symlink_to(os.path.relpath(target, destination.parent))
            elif member.islnk():
                target = root / member.linkname
                if not str(target.resolve()).startswith(str(root.resolve()) + os.sep):
                    raise RuntimeError("Archive hardlink escapes tools directory")
                destination.parent.mkdir(parents=True, exist_ok=True)
                os.link(target, destination)


def install(destination, arch):
    packages, providers = catalog(arch)
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".nasfind-tools-", dir=destination.parent))
    try:
        root = stage / "root"
        root.mkdir()
        installed = []
        for package in dependencies(packages, providers):
            filename = "{}-{}.apk".format(package["P"], package["V"])
            url = "{}/{}/{}/{}/{}".format(MIRROR, BRANCH, package["repository"], arch, filename)
            unpack(download(url), root)
            installed.append({"package": package["P"], "version": package["V"], "url": url})
        (stage / "bin").mkdir()
        loader = "lib/ld-musl-{}.so.1".format(arch)
        if not (root / loader).is_file():
            raise RuntimeError("Missing musl loader: " + loader)
        for name in ("plocate", "updatedb", "plocate-build", "sort"):
            program = next((root / directory / name for directory in ("usr/bin", "usr/sbin", "bin", "sbin")
                            if (root / directory / name).is_file()), None)
            if program is None:
                raise RuntimeError("Missing tool: " + name)
            wrapper = stage / "bin" / name
            wrapper.write_text('#!/bin/sh\nset -eu\nroot=$(CDPATH= cd -- "$(dirname -- "$0")/../root" && pwd)\n'
                               'exec "$root/{}" --library-path "$root/lib:$root/usr/lib" "$root/{}" "$@"\n'.format(
                                   loader, program.relative_to(root)))
            wrapper.chmod(0o755)
            subprocess.run([str(wrapper), "--version"], check=True)
        (stage / "PACKAGES.json").write_text(json.dumps(installed, indent=2) + "\n")
        # Do not replace a directory another setup process has installed.
        if destination.exists():
            raise RuntimeError("Tools already exist: " + str(destination))
        stage.rename(destination)
        print("Ready: " + str(destination))
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path(__file__).resolve().parent / "tools")
    args = parser.parse_args()
    arch = platform.machine()
    if arch not in ("x86_64", "aarch64"):
        parser.error("supported architectures: x86_64 and aarch64; found " + arch)
    destination = args.directory.resolve()
    if destination.exists():
        parser.error("tools already exist; use a fresh directory to install another copy")
    install(destination, arch)


if __name__ == "__main__":
    main()
