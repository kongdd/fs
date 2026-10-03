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
    for mode in [[], ["--locate"], ["--regex"]]:
        assert search(*mode, "soil") == paths[3:]
        assert search(*mode, "--offset", "1", "-l", "1", "soil") == paths[4:]
        assert search(*mode, "-l", "1", "soil") == paths[3:4]
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
    for mode in [[], ["--locate"], ["--regex"]]:
        assert search(*mode, "--dirs", "soil") == directories
        assert search(*mode, "--files", "soil") == files
        assert search(*mode, "--dirs", "--offset", "1", "-l", "1", "soil") == directories[1:2]
        assert search(*mode, "--files", "-l", "1", "soil") == files[:1]
    assert search("--files", "<soil | rain>") == files
    # Mount mapping happens only at output time, after matching and filtering.
    mount_paths = ["/volume1/CMIP6/soil.nc", "/volume1/CMIP6-other/soil.nc",
                   "/volume2/GitHub/repo/soil.nc", "/unmapped/soil.nc",
                   "/volume1/Researches/project/soil.nc", "/volume1/CUG-hydro/soil.nc"]
    mapped = ["/mnt/z/soil.nc", mount_paths[1], "/mnt/x/repo/soil.nc", mount_paths[3],
              "/mnt/y/project/soil.nc", "/mnt/o/soil.nc"]
    config.write_text(f'[[index]]\nname = "test"\nroot = "/"\ndatabase = "{db}"\n')
    source.write_text("\n".join(mount_paths) + "\n")
    subprocess.run(["plocate-build", "-p", "-l", "no", str(source), str(db)], check=True)
    for mode in [[], ["--locate"], ["--regex"]]:
        assert search(*mode, "soil") == mount_paths
        assert search(*mode, "--mnt", "soil") == mapped
        assert run("search", *mode, "--mnt", "soil").splitlines() == mapped
        assert run("search", *mode, "--mnt", "-0", "soil").split("\0") == mapped + [""]
        assert search(*mode, "--mnt", "--path", "/volume1/CMIP6", "soil") == mapped[:1]
    assert search("--mnt", "-p", "/volume2/GitHub") == mapped[2:3]
    run("ignore", "add", "repo")
    assert search("--mnt", "soil") == mapped[:2] + mapped[3:]
print("ignore, type-filter and mount-mapping integration tests passed")
