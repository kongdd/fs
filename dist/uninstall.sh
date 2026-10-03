#!/bin/sh
set -eu
PREFIX=${PREFIX:-"$HOME/.local"}
rm -f "$PREFIX/bin/fs"
echo "removed $PREFIX/bin/fs"
echo "configuration and databases were left untouched"
