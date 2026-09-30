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

Then edit `~/.config/nasfind/config.toml` and build the indexes:

```bash
nasfind index
```

## Configuration

```toml
[tools]
plocate = "plocate"
updatedb = "updatedb"

[[index]]
name = "research"
root = "/volume1/research"
database = "/var/lib/nasfind/research.db"
exclude_dirs = [".git", "node_modules", "target", "@eaDir", "#recycle"]
exclude_paths = ["/volume1/research/cache with spaces"]
exclude_extensions = ["tmp", "part", "pyc"]

[[index]]
name = "archive"
root = "/volume2/archive"
database = "/var/lib/nasfind/archive.db"
exclude_dirs = ["@eaDir", "#recycle"]
exclude_extensions = ["tmp"]
```

`exclude_dirs` is passed to plocate's `PRUNENAMES`, so entries must be plain directory names without
spaces or `/`. Use `exclude_paths` for exact paths (including paths containing spaces).

`exclude_extensions` is intentionally applied at query time. This keeps the implementation small and
lets `updatedb` remain completely stock. Directory exclusions, which usually remove the bulk of
unwanted files (`node_modules`, `.git`, caches), happen during indexing.

## Usage

```bash
# Update every configured DB
nasfind index

# Update selected DBs only
nasfind index research archive

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

## Development checks

```bash
make check  # formatting, Clippy and unit tests
make e2e    # release build and real plocate indexing/search tests
```

The end-to-end script uses a temporary directory and cleans it up automatically.
CI runs the same script. Indexing uses the configured exclusions, overriding
system `updatedb` pruning defaults that may otherwise skip NAS filesystems.

## License

MIT. `plocate` is a separate external dependency and keeps its own license.
