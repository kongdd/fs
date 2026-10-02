#!/bin/bash
# Update nasfind indexes, then rank directories from their databases.
# Usage: ./scripts/index-and-count.sh [-n20] [other dircount options]
set -euo pipefail

export PATH="$HOME/.local/bin:$PATH"
PROJECT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="${NASFIND_CONFIG:-$PROJECT/examples/config.toml}"

nasfind --config "$CONFIG" doctor
nasfind --config "$CONFIG" index update --no-progress
python3 "$PROJECT/scripts/dircount.py" --config "$CONFIG" -n20 "$@"
