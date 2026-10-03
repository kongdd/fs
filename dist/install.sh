#!/bin/sh
set -eu
PREFIX=${PREFIX:-"$HOME/.local"}
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BIN="$SCRIPT_DIR/fs"

if [ ! -x "$BIN" ]; then
    SOURCE_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
    (cd "$SOURCE_ROOT" && cargo build --release --bin fs)
    BIN="${CARGO_TARGET_DIR:-$SOURCE_ROOT/target}/release/fs"
fi

install -d "$PREFIX/bin"
install -m 0755 "$BIN" "$PREFIX/bin/fs"
echo "installed: $PREFIX/bin/fs"
echo "run: fs init"
