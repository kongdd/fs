#!/bin/sh
set -eu

PREFIX=${PREFIX:-/usr/local}
BINDIR="$PREFIX/bin"
DATADIR=${DATADIR:-/etc/nasfind}

if [ "$(uname -s)" != "Linux" ]; then
    echo "nasfind currently supports Linux only" >&2
    exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

for cmd in plocate updatedb plocate-build sort; do
    if [ ! -x "$SCRIPT_DIR/tools/bin/$cmd" ] && ! command -v "$cmd" >/dev/null 2>&1; then
        echo "missing dependency: $cmd (run python3 setup-tools.py, or install plocate and GNU sort)" >&2
        exit 1
    fi
done

BIN="$SCRIPT_DIR/nasfind"

# Binary release ZIPs place `nasfind` next to this script. In a source checkout,
# fall back to a local Cargo build so the same installer remains useful.
if [ ! -x "$BIN" ]; then
    if [ -f "$SCRIPT_DIR/Cargo.toml" ]; then
        SOURCE_ROOT="$SCRIPT_DIR"
    else
        SOURCE_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
    fi
    if [ -f "$SOURCE_ROOT/Cargo.toml" ]; then
        if ! command -v cargo >/dev/null 2>&1; then
            echo "no packaged binary found and cargo is not installed" >&2
            exit 1
        fi
        echo "building nasfind from source..."
        (cd "$SOURCE_ROOT" && cargo build --release)
        BIN="$SOURCE_ROOT/target/release/nasfind"
    else
        echo "missing packaged binary: $SCRIPT_DIR/nasfind" >&2
        exit 1
    fi
fi

install -d "$BINDIR" "$DATADIR"
install -m 0755 "$BIN" "$BINDIR/nasfind"
if [ -d "$SCRIPT_DIR/tools" ]; then
    TOOL_DIR="$PREFIX/lib/nasfind/tools"
    install -d "$TOOL_DIR"
    cp -R "$SCRIPT_DIR/tools/." "$TOOL_DIR/"
    chmod 0755 "$TOOL_DIR"
fi
if [ -f "$SCRIPT_DIR/config.toml.example" ]; then
    install -m 0644 "$SCRIPT_DIR/config.toml.example" "$DATADIR/config.toml.example"
elif [ -f "$SCRIPT_DIR/../examples/config.example.toml" ]; then
    install -m 0644 "$SCRIPT_DIR/../examples/config.example.toml" "$DATADIR/config.toml.example"
fi

echo "installed: $BINDIR/nasfind"
echo "example config: $DATADIR/config.toml.example"
echo "run: nasfind init"
