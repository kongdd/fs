#!/usr/bin/env python3
"""Package an already-built native executable."""
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
parser.add_argument("--out", type=pathlib.Path, default=root.parent / "dist-out")
args = parser.parse_args()

version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
arch = args.target.split("-")[0]
system = next((name for marker, name in [
    ("linux", "linux"), ("darwin", "macos"), ("windows", "windows"),
] if marker in args.target), None)
if system is None:
    parser.error("unsupported target")
name = f"nasfind-{version}-{system}-{arch}"
package = args.out / name
package.mkdir(parents=True, exist_ok=True)
binary = "nasfind.exe" if system == "windows" else "nasfind"
target_dir = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", root / "target"))
shutil.copy2(target_dir / args.target / "release" / binary, package / binary)
shutil.copy2(root / "INSTALL.md", package / "README.md")
shutil.copy2(root.parent / "LICENSE", package / "LICENSE")
example = "config.windows.toml" if system == "windows" else "config.example.toml"
shutil.copy2(root.parent / "examples" / example, package / "config.toml.example")

if system == "windows":
    archive = args.out / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
        for file in sorted(package.iterdir()):
            output.write(file, f"{name}/{file.name}")
else:
    (package / binary).chmod(0o755)
    archive = args.out / f"{name}.tar.gz"
    with tarfile.open(archive, "w:gz") as output:
        output.add(package, arcname=name)
print(archive)
