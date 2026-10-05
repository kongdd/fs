#!/usr/bin/env python3
"""Package an already-built fs executable (Python 3.11+)."""
import argparse
import os
import pathlib
import shutil
import tarfile
import tomllib
import zipfile

root = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("target")
parser.add_argument("--out", type=pathlib.Path, default=root / "dist-out")
args = parser.parse_args()

version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
arch = args.target.split("-")[0]
system = next((name for marker, name in [
    ("linux", "linux"), ("darwin", "macos"), ("windows", "windows"),
] if marker in args.target), None)
if system is None:
    parser.error("unsupported target")
name = f"fs-{version}-{system}-{arch}"
package = args.out / name
package.mkdir(parents=True, exist_ok=True)
binary = "fs.exe" if system == "windows" else "fs"
target_dir = root / os.environ.get("CARGO_TARGET_DIR", "target")
shutil.copy2(target_dir / args.target / "release" / binary, package / binary)
shutil.copy2(root / "INSTALL.md", package / "README.md")
shutil.copy2(root / "LICENSE", package / "LICENSE")
example = "updatedb_win.toml" if system == "windows" else "updatedb_nas.toml"
shutil.copy2(root / "config" / example, package / "config.toml.example")

if system == "windows":
    archive = args.out / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
        for file in sorted(package.iterdir()):
            output.write(file, f"{name}/{file.name}")
else:
    (package / binary).chmod(0o755)
    with tarfile.open(args.out / f"{name}.tar.gz", "w:gz") as output:
        output.add(package, arcname=name)
    archive = args.out / f"{name}.tar.gz"
print(archive)
