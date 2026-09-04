#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [configuration-root]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
exec python3 "$repo_root/.github/scripts/check-ci-tools-pinned.py" "$@"
