#!/usr/bin/env bash
# Backward-compatible wrapper: stage a persistent Firefox tree.
# Prefer: bash scripts/firefox-package.sh stage|lint|build
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
exec bash "$root/scripts/firefox-package.sh" stage "$@"
