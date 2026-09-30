#!/bin/sh
set -eu
PREFIX=${PREFIX:-/usr/local}
rm -f "$PREFIX/bin/nasfind"
echo "removed $PREFIX/bin/nasfind"
echo "configuration and databases were left untouched"
