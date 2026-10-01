#!/usr/bin/env bash
set -euo pipefail
ROOT=$(mktemp -d "${TMPDIR:-/tmp}/nasfind-e2e.XXXXXX")
trap 'rm -rf "$ROOT"' EXIT
mkdir -p "$ROOT/research/node_modules" "$ROOT/research/cache with spaces" "$ROOT/archive/node_modules" "$ROOT/archive/cache with spaces" "$ROOT/research/private" "$ROOT/archive/private"
touch "$ROOT/research/soil_moisture.nc"
touch "$ROOT/research/ignore.tmp"
touch "$ROOT/research/node_modules/hidden_soil.js"
touch "$ROOT/research/cache with spaces/hidden_soil.nc"
touch "$ROOT/archive/soil_archive.txt"
touch "$ROOT/archive/node_modules/hidden_soil.js"
touch "$ROOT/archive/cache with spaces/hidden_soil.nc"
touch "$ROOT/archive/ignore.TMP"
touch "$ROOT/research/private/local_visibility.txt"
touch "$ROOT/archive/private/local_visibility.txt"
touch "$ROOT/research/ignore.pyc"

cat > "$ROOT/config.toml" <<EOF2
[filters]
exclude_dirs = ["node_modules"]
exclude_paths = ["cache with spaces"]
exclude_extensions = ["tmp"]

[[index]]
name = "research"
root = "$ROOT/research"
database = "$ROOT/research.db"
exclude_dirs = ["private"]
exclude_extensions = ["pyc"]

[[index]]
name = "archive"
root = "$ROOT/archive"
database = "$ROOT/archive.db"
EOF2

BIN=${1:-target/release/nasfind}
"$BIN" --config "$ROOT/config.toml" doctor
"$BIN" --config "$ROOT/config.toml" index --no-progress

out=$("$BIN" --config "$ROOT/config.toml" soil)
grep -F "$ROOT/research/soil_moisture.nc" <<<"$out"
grep -F "$ROOT/archive/soil_archive.txt" <<<"$out"
if grep -F "hidden_soil" <<<"$out"; then exit 1; fi

out=$("$BIN" --config "$ROOT/config.toml" search -d research ignore)
test -z "$out"

out=$("$BIN" --config "$ROOT/config.toml" search -d research soil)
grep -F "soil_moisture.nc" <<<"$out"
if grep -F "soil_archive.txt" <<<"$out"; then exit 1; fi

# Rebuilding a selected DB also exercises the verbose progress reader.
"$BIN" --config "$ROOT/config.toml" index research
test -n "$("$BIN" --config "$ROOT/config.toml" search -d research soil)"
# Byte-safe filenames, streaming JSON, limits and real backend errors.
python3 - "$BIN" "$ROOT" <<'PYTEST'
import json, os, pathlib, shutil, subprocess, sys
binary, root = sys.argv[1:]
base = pathlib.Path(root)
config = str(base / "config.toml")
def run(*args, env=None):
    return subprocess.run([binary, "--config", config, *args], capture_output=True, timeout=10, env=env)

def search(*args):
    result = run("search", *args)
    assert result.returncode == 0, result.stderr
    return result.stdout

assert json.loads(search("--json", "nothing_matches_123")) == []
assert search("ignore") == b""
assert search("local_visibility") == (root + "/archive/private/local_visibility.txt\n").encode()
rows = json.loads(search("--json", "soil"))
assert {r["path"] for r in rows} == {root + "/research/soil_moisture.nc", root + "/archive/soil_archive.txt"}
assert len(json.loads(search("--json", "-l", "1", "soil"))) == 1
assert run("search", "--json", "--null", "soil").returncode != 0
assert run("search", "-l", "0", "soil").returncode != 0

names = [b"byte_soil_\xff.nc", b"byte_soil_line\nbreak.nc", b"byte_soil_\xff.TMP"]
for name in names:
    with open(os.fsencode(root + "/research/") + name, "wb"):
        pass
assert run("index", "research").returncode == 0
expected = {os.fsencode(root + "/research/") + name for name in names[:2]}
assert set(search("-0", "byte_soil").split(b"\0")[:-1]) == expected
assert len(json.loads(search("--json", "byte_soil"))) == 2
assert b"\xef\xbf\xbd" not in search("byte_soil")

# Explicit DB selection must ignore ambient LOCATE_PATH.
ambient = run("search", "-d", "research", "soil", env={**os.environ, "LOCATE_PATH": root + "/archive.db"})
assert ambient.returncode == 0, ambient.stderr
assert (root + "/archive/soil_archive.txt").encode() not in ambient.stdout

# A pattern beginning with '-' must remain a pattern, not a backend option.
(base / "archive/--version").touch()
assert run("index", "archive", "--no-progress").returncode == 0
assert search("-d", "archive", "--", "--version") == (root + "/archive/--version\n").encode()

# plocate uses ':' as a DB-list separator and '\\' as an escape character.
odd_db = base / "colon:back\\slash.db"
shutil.copyfile(base / "archive.db", odd_db)
odd_config = base / "odd-config.toml"
odd_config.write_text('[[index]]\nname="archive"\nroot=' + json.dumps(root + '/archive') + '\ndatabase=' + json.dumps(str(odd_db)) + '\n')
odd = subprocess.run([binary, "--config", str(odd_config), "search", "soil"], capture_output=True, timeout=10)
assert odd.returncode == 0, odd.stderr
assert odd.stdout == (root + "/archive/soil_archive.txt\n").encode()

# A readable but corrupt DB must not look like an empty successful search.
(base / "research.db").write_bytes(b"not a plocate database")
assert run("search", "-d", "research", "soil").returncode != 0

# Diagnostics larger than a pipe buffer must also drain without a deadlock.
mock = base / "failing-plocate"
mock.write_text("#!/usr/bin/env python3\nimport sys\nsys.stderr.write('failure' * 20000)\nsys.exit(1)\n")
mock.chmod(0o755)
(base / "config.toml").write_text('[tools]\nplocate = ' + json.dumps(str(mock)) + '\n[[index]]\nname="research"\nroot=' + json.dumps(root + '/research') + '\ndatabase=' + json.dumps(root + '/research.db') + '\n')
assert run("search", "soil").returncode != 0

# Reaching a filtered limit must not suppress an already reported backend error.
mock.write_text("#!/usr/bin/env python3\nimport sys\nsys.stderr.write('backend failure\\n')\nsys.stderr.flush()\nsys.stdout.buffer.write(" + repr((root + '/research/soil.nc').encode() + b'\0') + ")\nsys.stdout.flush()\nsys.exit(1)\n")
with (base / "config.toml").open("a") as f:
    f.write('exclude_extensions=["tmp"]\n')
assert run("search", "-l", "1", "soil").returncode != 0

PYTEST
echo "End-to-end plocate tests passed"
