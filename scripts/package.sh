#!/usr/bin/env bash
set -euo pipefail

TARGET=${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}
VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
ARCH=$(uname -m)
NAME="nasfind-${VERSION}-linux-${ARCH}"
OUT=${OUT:-dist-out}

cargo build --release --target "$TARGET"
rm -rf "$OUT/$NAME"
mkdir -p "$OUT/$NAME"
cp "target/$TARGET/release/nasfind" "$OUT/$NAME/nasfind"
cp dist/install.sh dist/uninstall.sh "$OUT/$NAME/"
chmod 0755 "$OUT/$NAME/install.sh" "$OUT/$NAME/uninstall.sh"
cp examples/config.example.toml "$OUT/$NAME/config.toml.example"
cp dist/nasfind-update.service dist/nasfind-update.timer "$OUT/$NAME/"
cp README.md LICENSE "$OUT/$NAME/"
cp scripts/benchmark.py "$OUT/$NAME/benchmark.py"
cp docs/benchmark.md "$OUT/$NAME/BENCHMARK.md"
cp scripts/setup-tools.py "$OUT/$NAME/setup-tools.py"
cp docs/synology.md "$OUT/$NAME/SYNOLOGY.md"
cp -R docs "$OUT/$NAME/docs"
(
  cd "$OUT"
  rm -f "$NAME.zip" "$NAME.zip.sha256"
  zip -9 -r "$NAME.zip" "$NAME"
)
printf '%s\n' "$OUT/$NAME.zip"
