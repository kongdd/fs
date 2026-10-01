# nasfind

`nasfind` is a deliberately small Rust wrapper around **plocate** for fast NAS file-name search.
It does not reimplement a search engine. `updatedb` builds compact trigram/posting-list databases and
`plocate` performs the queries; `nasfind` adds multiple named databases, per-index exclusions, progress,
and a cleaner CLI.

## Features

- Multiple independent databases (`research`, `data`, `archive`, ...)
- Search all databases or selected databases
- Directory exclusion using plocate `PRUNENAMES` / `PRUNEPATHS`
- File-extension exclusion as a cheap result filter
- `updatedb -v` based indexing progress (entries, rate, elapsed time, current path)
- Unambiguous NUL-safe communication with plocate
- JSON output
- Implicit search: `nasfind soil moisture`
- No daemon and no custom database format

## Dependency

Linux with `plocate` installed. On Debian/Ubuntu:

```bash
sudo apt install plocate
```

`nasfind` deliberately treats plocate as a system dependency rather than vendoring it.

## Install

From a release ZIP:

```bash
unzip nasfind-*.zip
cd nasfind-*
sudo ./install.sh
nasfind init
```

Release binaries are statically linked for x86_64 and aarch64. Synology users
should follow [the NAS setup guide](docs/synology.md); no compilation is needed.

Then edit `~/.config/nasfind/config.toml` and build the indexes:

```bash
nasfind index
```

## Configuration

```toml
[filters]
# Installed environments, dependency trees, caches and NAS/system metadata.
# Keep project directories and source files; add custom installation paths below.
exclude_dirs = [
  ".git", "node_modules", "target",
  ".julia", ".conda", "anaconda3", "miniconda3", "miniforge3", "mambaforge",
  ".venv", "venv", "site-packages", "__pycache__", ".tox", ".nox",
  "renv", "site-library", "win-library", "x86_64-pc-linux-gnu-library", "R.framework",
  ".cache", ".bun", ".npm", ".pnpm-store", ".node-gyp", ".cargo", ".rustup",
  ".pytest_cache", ".mypy_cache", ".ruff_cache", ".ipynb_checkpoints",
  "@eaDir", "#recycle", "$RECYCLE.BIN", ".Trash", ".Trashes", ".Spotlight-V100", ".fseventsd"
]
# Relative paths apply under every index root; absolute paths affect that location only.
exclude_paths = ["System Volume Information"]
exclude_extensions = ["tmp", "part", "pyc", "pyo", "rlib", "rmeta"]
# Exact basenames, matched without ASCII case sensitivity.
exclude_files = [".DS_Store", "Thumbs.db", "desktop.ini", ".directory"]

[[index]]
name = "research"
root = "/volume1/research"
database = "/var/lib/nasfind/research.db"

[[index]]
name = "archive"
root = "/volume2/archive"
database = "/var/lib/nasfind/archive.db"
```

Tool paths are detected beside the executable (`tools/bin`) or in the installation
prefix (`lib/nasfind/tools/bin`), falling back to commands on PATH. Optional `[tools]`
keys `plocate`, `updatedb`, `plocate_build` and `sort` override detection.

Rules in `[filters]` are the defaults for every database. Each `[[index]]` may
replace individual `exclude_dirs`, `exclude_paths`, `exclude_extensions`, or
`exclude_files` lists. An omitted list inherits the global list; an explicit
empty list disables that type of exclusion for the index. Local lists replace,
rather than extend, global lists. Relative excluded paths
are resolved against each index root; absolute paths are used as written.

`exclude_dirs` is passed to plocate's `PRUNENAMES`, so entries must be plain directory names without
spaces or `/`. Use `exclude_paths` for exact paths (including paths containing spaces).

The shared example rules skip Synology's `#recycle` and the listed recycle/trash
directories at any depth during indexing. Add these rules to existing configs
and run `nasfind index` again to remove previously indexed recycle-bin entries.

`exclude_files` excludes exact basenames (ASCII case-insensitive), including hidden
files such as `.DS_Store`. It is applied at query time, like extension exclusions.
The example skips common language environments, dependencies, caches and system
metadata. It keeps `.py`, `.jl`, `.R`, `.rs`, datasets and documents. Custom installation
directories need explicit exclusions. Review any personal work stored inside an
excluded environment directory, such as `.julia/dev`, before using the defaults.

Rust's default `target` directory (including debug/release, dependencies and
incremental caches) is excluded during indexing. Loose `.rlib` and `.rmeta` files
are filtered from results. If Cargo uses a custom `CARGO_TARGET_DIR`, add that
location to `exclude_paths`; source files and Cargo manifests remain searchable.

`exclude_extensions` is intentionally applied at query time. This keeps the implementation small and
lets `updatedb` remain completely stock. Directory exclusions, which usually remove the bulk of
unwanted files (`node_modules`, `.git`, caches), happen during indexing.

## Usage

```bash
# Update every configured DB
nasfind index

# Update selected DBs only
nasfind index research archive

# Scan only this folder and merge its paths into the main DB
nasfind index --folder /volume1/research/project

# Maximum indexing throughput, no entry counter
nasfind index research --no-progress

# Search every DB (implicit `search`)
nasfind soil moisture

# Explicit search
nasfind search soil moisture

# Search only selected DBs
nasfind search -d research -d archive Richards

# Case-insensitive, basename-only, limit 50
nasfind search -i -b -l 50 ERA5

# JSON
nasfind search --json soil

# Verify config and dependencies
nasfind doctor
```

Multiple patterns are passed directly to plocate and therefore use AND semantics.

`index --folder PATH` scans only the specified directory with stock `updatedb`,
then replaces that subtree's old paths in the containing DB using `plocate-build`.
Deleted and renamed files disappear; other folders keep their existing records.
Absolute and relative paths are supported. The most specific configured root owns
nested folders. Repeat `--folder` for multiple folders; it cannot be combined with
index names. A folder equal to an index root uses a normal `updatedb` update.

Build the main DB first. Folders outside configured roots or excluded by directory/
path rules are rejected. Merging reads and rebuilds the entire main DB locally,
but does not scan other NAS directories. It requires `plocate-build` (included in
plocate) and GNU `sort`. Temporary files stay beside the DB; allow space for the
path lists and replacement DB. Writers take an OS lock and the main DB is replaced
atomically only after the merge succeeds.

The merged DB contains the filename index but loses `updatedb`'s directory reuse
metadata, so the next full update must reread directories. Plaintext import cannot
represent filenames containing newlines: partial updates reject those names without
replacing the old DB; normal full updates still support them.

Text and `--null` output preserve filename bytes; use `--null` for names containing
newlines. JSON is streamed as a compact array with bounded buffering. Since JSON
requires Unicode, invalid UTF-8 filename bytes are replaced in JSON output.

## NAS benchmarks

See [the NAS benchmark guide](docs/benchmark.md). Release ZIPs include
`benchmark.py`, `BENCHMARK.md`, `setup-tools.py` and `SYNOLOGY.md`.

## Why this design?

The goal is minimum custom code and maximum reuse of a mature high-performance index. `plocate` uses
an inverted index over trigrams and is designed for very large filename databases. `updatedb` can
reuse information from an existing database to avoid rereading unchanged directories.

`nasfind` only provides orchestration and policy around those tools.

## Scheduling

Examples are included in `dist/`:

```bash
sudo cp dist/nasfind-update.service /etc/systemd/system/
sudo cp dist/nasfind-update.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now nasfind-update.timer
```

The provided service expects `/etc/nasfind/config.toml`; copy and edit `examples/config.toml` first.
The timer first runs five minutes after boot, then every two days (48 hours,
with up to 30 seconds of randomized delay). It uses a monotonic interval;
reboots restart the boot schedule. After replacing an installed timer, run
`sudo systemctl daemon-reload` and `sudo systemctl restart nasfind-update.timer`.

## Development checks

```bash
make check  # formatting, Clippy and unit tests
make e2e    # release build and real plocate indexing/search tests
```

The end-to-end script requires plocate and Python 3, uses a temporary directory,
and cleans it up automatically.
CI runs the same script. Indexing uses the configured exclusions, overriding
system `updatedb` pruning defaults that may otherwise skip NAS filesystems.

## License

MIT. `plocate` is a separate external dependency and keeps its own license.
