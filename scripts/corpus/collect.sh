#!/usr/bin/env bash
# Compatibility entry point. Collection requires an explicit pinned target file.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
exec node "$ROOT/scripts/corpus/collect.mjs" "$@"
