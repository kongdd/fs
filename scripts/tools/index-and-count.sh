#!/usr/bin/env bash
# Update configured indexes, then rank their directories.
set -euo pipefail

fs doctor
fs updatedb --no-progress
fs stats -n20 "$@"
