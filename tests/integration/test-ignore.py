#!/usr/bin/env python3
"""Query-time ignore regression test; requires plocate and plocate-build."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/fs").resolve())
with tempfile.TemporaryDirectory(prefix="fs-ignore-") as temp:
    base = Path(temp)
    root = base / "data"
    ignored = root / "cache with spaces"
    paths = [str(ignored / "soil_a.nc"), str(ignored / "nested/soil_b.nc"),
             str(root / "nested/cache with spaces/soil_nested.nc"),
             str(root / "cache with spaces-other/soil_c.nc"), str(root / "soil_d.nc")]
    source = base / "paths.txt"
    source.write_text("\n".join(paths) + "\n")
    db = base / "index.db"
    subprocess.run(["plocate-build", "-p", "-l", "no", str(source), str(db)], check=True)
    config = base / "config.toml"
    original = f'[[index]]\nname = "test"\nroot = "{root}"\ndatabase = "{db}"\n'
    config.write_text(original)

    def run(*args):
        return subprocess.run([binary, "--config", str(config), *args],
                              check=True, capture_output=True, text=True, timeout=10).stdout

    def search(*args):
        return [row["path"] for row in json.loads(run("search", "--json", *args))]

    assert search("soil") == paths
    # Names match any depth without accessing the filesystem; duplicate adds are idempotent.
    run("ignore", "add", ignored.name, "node_modules", ignored.name)
    run("ignore", "add", ignored.name)
    assert run("ignore", "list").splitlines() == [ignored.name, "node_modules"]
    for mode in [[], ["-p"], ["--regex"]]:
        assert search(*mode, "soil") == paths[3:]
        assert search(*mode, "--offset", "1", "-n", "1", "soil") == paths[4:]
        assert search(*mode, "-n", "1", "soil") == paths[3:4]
    assert search("<soil | rain> ext:nc") == paths[3:]
    assert config.read_text() == original
    run("ignore", "rm", ignored.name, "node_modules", ignored.name, "not-present")
    assert run("ignore", "list") == ""
    assert search("soil") == paths

    # Type inference is metadata-free and precedes offset/limit in every query mode.
    directories = [str(root / "soil"), str(root / "soil."), str(root / ".soil")]
    files = [str(root / "soil.nc"), str(root / "soil.tar.gz")]
    source.write_text("\n".join(directories + files) + "\n")
    subprocess.run(["plocate-build", "-p", "-l", "no", str(source), str(db)], check=True)
    for mode in [[], ["-p"], ["--regex"]]:
        assert search(*mode, "--dirs", "soil") == directories
        assert search(*mode, "--files", "soil") == files
        assert search(*mode, "--dirs", "--offset", "1", "-n", "1", "soil") == directories[1:2]
        assert search(*mode, "--files", "-n", "1", "soil") == files[:1]
    assert search("--files", "<soil | rain>") == files
    # Plocate stores absolute paths; output must not apply hard-coded mount mappings.
    nas_paths = ["/volume1/CMIP6/soil.nc", "/volume1/CMIP6-other/soil.nc",
                 "/volume2/GitHub/repo/soil.nc", "/unmapped/soil.nc",
                 "/volume1/Researches/project/soil.nc", "/volume1/CUG-hydro/soil.nc"]
    config.write_text(f'[[index]]\nname = "test"\nroot = "/"\ndatabase = "{db}"\n')
    source.write_text("\n".join(nas_paths) + "\n")
    subprocess.run(["plocate-build", "-p", "-l", "no", str(source), str(db)], check=True)
    for mode in [[], ["-p"], ["--regex"]]:
        assert search(*mode, "soil") == nas_paths
        assert run("search", *mode, "soil").splitlines() == nas_paths
        assert run("search", *mode, "-0", "soil").split("\0") == nas_paths + [""]
        assert search(*mode, "--path", "/volume1/CMIP6", "soil") == nas_paths[:1]
    assert search("-p", "/volume2/GitHub") == nas_paths[2:3]
    run("ignore", "add", "repo")
    assert search("soil") == nas_paths[:2] + nas_paths[3:]
print("ignore, type-filter and byte-safe output integration tests passed")
