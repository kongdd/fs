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
mkdir -p "$ROOT/research/nested/System Volume Information" "$ROOT/archive/System Volume Information"
touch "$ROOT/research/nested/System Volume Information/hidden_soil.nc" "$ROOT/archive/System Volume Information/hidden_soil.nc"

cat > "$ROOT/config.toml" <<EOF2
[filters]
exclude_dirs = ["node_modules", "#recycle", "System Volume Information"]
exclude_extensions = ["tmp"]

[[index]]
name = "research"
root = "$ROOT/research"
database = "$ROOT/research.db"
exclude_dirs = ["node_modules", "#recycle", "private", "System Volume Information"]
exclude_paths = ["cache with spaces"]
exclude_extensions = ["tmp", "pyc"]

[[index]]
name = "archive"
root = "$ROOT/archive"
database = "$ROOT/archive.db"
exclude_paths = ["cache with spaces"]
EOF2

BIN=${1:-target/release/nasfind}
"$BIN" --config "$ROOT/config.toml" doctor
"$BIN" --config "$ROOT/config.toml" index update --no-progress

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

# init leaves existing databases untouched; update also initializes new indexes.
existing = {name: (base / (name + '.db')).read_bytes() for name in ['research', 'archive']}
initialized = run('index', 'init', '--no-progress')
assert initialized.returncode == 0, initialized.stderr
assert b'already exist' in initialized.stderr
assert all((base / (name + '.db')).read_bytes() == data for name, data in existing.items())
assert run('index', 'init', 'missing').returncode != 0
assert run('index', 'init', '--folder', str(base / 'research')).returncode != 0
updated = run('index', 'update', 'research')
assert updated.returncode == 0, updated.stderr
assert b'[update]' in updated.stderr and b'estimated total' in updated.stderr

# A TTY gets live percentage/ETA updates even between bursts of stdout.
import pty, tempfile
cache = pathlib.Path.home() / '.cache'
cache.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix='nasfind-progress-', dir=cache) as temporary:
    fixture = pathlib.Path(temporary)
    locate = fixture / 'plocate'
    locate.write_text('#!/usr/bin/env python3\nprint(100)\n')
    locate.chmod(0o755)
    scanner = fixture / 'updatedb'
    scanner.write_text('#!/usr/bin/env python3\nimport time\nfor i in range(50): print("/data/file"+str(i), flush=True)\ntime.sleep(1)\nfor i in range(50,100): print("/data/file"+str(i), flush=True)\ntime.sleep(0.6)\nimport os\nos.close(1)\ntime.sleep(0.6)\n')
    scanner.chmod(0o755)
    database = fixture / 'progress.db'
    database.touch()
    progress_config = fixture / 'config.toml'
    progress_config.write_text('[tools]\nplocate=' + json.dumps(str(locate)) + '\nupdatedb=' + json.dumps(str(scanner)) + '\n[[index]]\nname="progress"\nroot=' + json.dumps(str(fixture)) + '\ndatabase=' + json.dumps(str(database)) + '\n')
    master, slave = pty.openpty()
    try:
        result = subprocess.run([binary, '--config', str(progress_config), 'index', 'update'], stdout=subprocess.PIPE, stderr=slave, timeout=10)
        os.close(slave)
        slave = None
        chunks = []
        while True:
            try:
                chunk = os.read(master, 65536)
            except OSError:
                break
            if not chunk:
                break
            chunks.append(chunk)
        display = b''.join(chunks)
        assert result.returncode == 0, display
        assert b'~50%' in display and b'ETA ~' in display, display
        assert b'past estimate; ETA unknown' in display, display
        assert b'finishing; ETA unknown' in display, display
        assert b'done progress: 100 entries' in display, display
        scanner.write_text('#!/usr/bin/env python3\nprint("/data/file", flush=True)\nraise SystemExit(3)\n')
        failed = subprocess.run([binary, '--config', str(progress_config), 'index', 'update'], capture_output=True, timeout=10)
        assert failed.returncode != 0 and b'updatedb failed' in failed.stderr, failed.stderr
    finally:
        os.close(master)
        if slave is not None:
            os.close(slave)

assert json.loads(search("--json", "nothing_matches_123")) == []
assert run("-i", "SOIL").stdout == search("-i", "SOIL")
implicit = run("--json", "-d", "research", "soil")
assert implicit.returncode == 0 and json.loads(implicit.stdout) == json.loads(search("--json", "-d", "research", "soil"))
assert search("ignore") == b""
assert search("local_visibility") == (root + "/archive/private/local_visibility.txt\n").encode()
rows = json.loads(search("--json", "soil"))
assert {r["path"] for r in rows} == {root + "/research/soil_moisture.nc", root + "/archive/soil_archive.txt"}
assert len(json.loads(search("--json", "-l", "1", "soil"))) == 1
assert run("search", "--json", "--null", "soil").returncode != 0
assert run("search", "-l", "0", "soil").returncode != 0

# Folder selection updates additions/deletions in one DB, leaving others untouched.
archive_before = (base / "archive.db").read_bytes()
(base / "research/soil_moisture.nc").unlink()
(base / "research/folder_added.nc").touch()
(base / "archive/folder_pending.nc").touch()
selected = run("index", "--folder", str(base / "research"), "--folder", str(base / "research/./"), "--no-progress")
assert selected.returncode == 0, selected.stderr
assert selected.stderr.count(b"indexing research:") == 1
assert (base / "archive.db").read_bytes() == archive_before
assert search("folder_added") == (root + "/research/folder_added.nc\n").encode()
assert search("folder_pending") == b""
assert search("soil_moisture") == b""
assert run("index", "research", "--folder", str(base / "research")).returncode != 0
assert run("index", "--folder", root).returncode != 0
assert run("index", "--folder", str(base / "research/folder_added.nc")).returncode != 0
assert run("index", "--folder", str(base / "research"), "--folder", root).returncode != 0
relative = subprocess.run([str(pathlib.Path(binary).resolve()), "--config", config, "index", "--folder", "archive", "--no-progress"], cwd=base, capture_output=True, timeout=10)
assert relative.returncode == 0, relative.stderr
assert search("folder_pending") == (root + "/archive/folder_pending.nc\n").encode()
(base / "research/soil_moisture.nc").touch()

# Nested roots select the most specific DB without updating its parent DB.
nested = base / "research/project"
nested.mkdir()
nested_config = base / "nested.toml"
nested_config.write_text((base / "config.toml").read_text() + '\n[[index]]\nname="project"\nroot=' + json.dumps(str(nested)) + '\ndatabase=' + json.dumps(str(base / 'project.db')) + '\n')
parent_before = (base / "research.db").read_bytes()
(nested / "nested_added.nc").touch()
result = subprocess.run([binary, "--config", str(nested_config), "index", "--folder", str(nested), "--no-progress"], capture_output=True, timeout=10)
assert result.returncode == 0, result.stderr
assert b"indexing project:" in result.stderr and b"indexing research:" not in result.stderr
assert (base / "project.db").is_file()
assert (base / "research.db").read_bytes() == parent_before
(base / 'project.db').unlink()
initialized = subprocess.run([binary, '--config', str(nested_config), 'index', 'init', 'project'], capture_output=True, timeout=10)
assert initialized.returncode == 0 and b'[init]' in initialized.stderr, initialized.stderr
assert b'total unknown' in initialized.stderr
assert (base / 'research.db').read_bytes() == parent_before
(base / 'project.db').unlink()
updated = subprocess.run([binary, '--config', str(nested_config), 'index', 'update', 'project', '--no-progress'], capture_output=True, timeout=10)
assert updated.returncode == 0 and b'[init]' in updated.stderr, updated.stderr
assert (base / 'project.db').is_file()

# A partial update replaces only the selected subtree in the main DB.
project = base / "research/project"
(project / "obsolete.nc").touch()
sibling = base / "research/project-other"
sibling.mkdir()
(sibling / "kept_sibling.nc").touch()
assert run("index", "research", "--no-progress").returncode == 0
(project / "obsolete.nc").unlink()
(project / "partial_added.nc").touch()
(base / "research/unscanned_pending.nc").touch()
(project / "node_modules").mkdir()
(project / "node_modules/discard_dependency.nc").touch()
permissions = (base / "research.db").stat().st_mode
partial = run("index", "--folder", str(project), "--folder", str(project), "--no-progress")
assert partial.returncode == 0, partial.stderr
assert partial.stderr.count(b"merged ") == 1
assert search("obsolete") == b""
assert search("partial_added") == (str(project / "partial_added.nc") + "\n").encode()
assert search("kept_sibling") == (str(sibling / "kept_sibling.nc") + "\n").encode()
assert search("unscanned_pending") == b""
assert search("discard_dependency") == b""
assert (base / "research.db").stat().st_mode == permissions
assert not list(base.glob(".nasfind-*"))

# Unsupported newline names and failed builders cannot replace the old main DB.
original = (base / "research.db").read_bytes()
newline = project / "partial_line\nbreak.nc"
newline.touch()
failed = run("index", "--folder", str(project), "--no-progress")
assert failed.returncode != 0 and b"newlines" in failed.stderr
assert (base / "research.db").read_bytes() == original
newline.unlink()
builder = base / "failing-builder"
builder.write_text("#!/bin/sh\nexit 2\n")
builder.chmod(0o755)
builder_config = base / "builder-config.toml"
builder_config.write_text('[tools]\nplocate_build=' + json.dumps(str(builder)) + '\n' + (base / "config.toml").read_text())
failed = subprocess.run([binary, "--config", str(builder_config), "index", "--folder", str(project), "--no-progress"], capture_output=True, timeout=10)
assert failed.returncode != 0
assert (base / "research.db").read_bytes() == original

# The native lock prevents overlapping writers; corrupt input cannot be merged.
import fcntl
with (base / "research.db.lock").open("r+") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    assert run("index", "--folder", str(project), "--no-progress").returncode != 0
assert (base / "research.db").read_bytes() == original
(base / "research.db").write_bytes(b"corrupt-main")
assert run("index", "--folder", str(project), "--no-progress").returncode != 0
assert (base / "research.db").read_bytes() == b"corrupt-main"
(base / "research.db").write_bytes(original)
assert run("index", "--folder", str(base / "research/private"), "--no-progress").returncode != 0

# NAS recycle bins are pruned from each DB, including nested share folders.
for index in ["research", "archive"]:
    recycle = base / index / "share/#recycle/nested"
    recycle.mkdir(parents=True)
    (recycle / "discard_recycled.nc").touch()
assert run("index", "--no-progress").returncode == 0
assert search("discard_recycled") == b""
for index in ["research", "archive"]:
    raw = subprocess.run(["plocate", "-d", str(base / (index + ".db")), "discard_recycled"], capture_output=True, timeout=10)
    assert raw.returncode == 1 and raw.stdout == b"", raw.stderr

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

# Default rules discard installed packages/system clutter, not personal source/data.
default_config = base / "defaults.toml"
init = subprocess.run([binary, "init", str(default_config)], capture_output=True, timeout=10)
assert init.returncode == 0, init.stderr
own = base / "personal"
own.mkdir()
for folder in [".julia", "miniconda3", ".venv", "renv", "node_modules", "target", ".cargo", ".rustup", ".bun", ".npm", ".pnpm-store", ".node-gyp", ".cache", "#recycle", "$RECYCLE.BIN", ".Trash", ".Trashes"]:
    (own / folder).mkdir()
    (own / folder / "discard_package.nc").touch()
for name in [".DS_Store", "Thumbs.db", "DESKTOP.INI", ".directory", "discard.tmp", "discard.rlib", "discard.rmeta"]:
    (own / name).touch()
kept = ["my_model.py", "my_model.jl", "my_model.R", "my_model.rs", "my_data.nc", "my_results.db", "my_notes.pdf"]
for name in kept:
    (own / name).touch()
(own / "python").mkdir()
(own / "python/my_source.py").touch()
for directory in ["target/debug/deps", "target/release", "target/debug/incremental"]:
    (own / directory).mkdir(parents=True, exist_ok=True)
    (own / directory / "discard_binary").touch()
prefix = default_config.read_text().split("[[index]]", 1)[0]
default_config.write_text(prefix + '[[index]]\nname="personal"\nroot=' + json.dumps(str(own)) + '\ndatabase=' + json.dumps(str(base / 'personal.db')) + '\n')
def personal(*args):
    result = subprocess.run([binary, "--config", str(default_config), *args], capture_output=True, timeout=10)
    assert result.returncode == 0, result.stderr
    return result.stdout
personal("index", "--no-progress")
for pattern in ["discard", ".DS_Store", "Thumbs.db", "DESKTOP.INI", ".directory"]:
    assert personal("search", pattern) == b"", pattern
assert {row["path"] for row in json.loads(personal("search", "--json", "my_"))} == {str(own / name) for name in kept} | {str(own / "python/my_source.py")}

# The packaged benchmark records normal updates, subtree merges and query latency.
benchmark = base / "benchmark.json"
measured = subprocess.run([sys.executable, "scripts/benchmark.py", "--nasfind", binary, "--config", str(default_config), "--index", "personal", "--query", "my_", "--runs", "3", "--limit", "0", "--update", "--folder", str(own / "python"), "--output", str(benchmark)], capture_output=True, timeout=30)
assert measured.returncode == 0, measured.stderr
report = json.loads(benchmark.read_text())
assert len(report["updates"]) == 3
assert report["queries"][0]["samples"][-1]["matches"] == len(kept) + 1
assert report["queries"][0]["repeated_p95_ms"] >= 0

# Local [] disables that field's global filter; omitted fields inherit.
local_config = base / "local-config.toml"
local_config.write_text('[filters]\nexclude_extensions=["tmp"]\n[[index]]\nname="research"\nroot=' + json.dumps(root + '/research') + '\ndatabase=' + json.dumps(root + '/research.db') + '\nexclude_extensions=[]\n')
local = subprocess.run([binary, "--config", str(local_config), "search", "-0", "ignore.tmp"], capture_output=True, timeout=10)
assert local.returncode == 0 and local.stdout == (root + '/research/ignore.tmp').encode() + b'\0'

# dircount reads DB records, including deleted paths, and escapes subtree globs.
rank_root = base / r'archive/rank[*?]\back'
(rank_root / 'left').mkdir(parents=True)
(rank_root / 'right').mkdir()
for name in ['left/a.nc', 'left/deleted.nc', 'left/ignored.tmp', 'right/c.nc']:
    (rank_root / name).touch()
assert run('index', 'archive', '--no-progress').returncode == 0
(rank_root / 'left/deleted.nc').unlink()
ranked = subprocess.run([sys.executable, 'scripts/dircount.py', '--nasfind', binary,
                         '--config', config, '-d', 'archive', '-n3', str(rank_root)],
                        capture_output=True, timeout=10)
assert ranked.returncode == 0, ranked.stderr
lines = ranked.stdout.decode().splitlines()[1:]
assert [line.split(None, 1) for line in lines] == [
    ['5', str(rank_root)], ['2', str(rank_root / 'left')], ['1', str(rank_root / 'right')]]
assert b'Total: 5 unique indexed paths' in ranked.stderr
# A standalone/package copy inherits nasfind's config lookup without --config.
rank_script = base / 'dircount.py'
shutil.copyfile('scripts/dircount.py', rank_script)
inherited = subprocess.run([sys.executable, str(rank_script), '--nasfind', binary,
                            '-d', 'archive', '-n3', str(rank_root)], capture_output=True,
                           env={**os.environ, 'NASFIND_CONFIG': config}, timeout=10)
assert inherited.returncode == 0, inherited.stderr
assert inherited.stdout == ranked.stdout

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
