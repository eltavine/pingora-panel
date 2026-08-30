#!/usr/bin/env bash
set -euo pipefail

workflow_root="${1:-.github/workflows}"
status=0

while IFS= read -r match; do
  command="${match#*:*:}"
  if [[ ! "$command" =~ --version([= 	]+)[0-9]+\.[0-9]+\.[0-9]+([^0-9.]|$) ]]; then
    echo "cargo install must use an exact --version: $match" >&2
    status=1
  fi
  if [[ ! "$command" =~ (^|[[:space:]])--locked($|[[:space:]]) ]]; then
    echo "cargo install must use --locked: $match" >&2
    status=1
  fi
done < <(grep -RInE 'cargo[[:space:]]+install' "$workflow_root" || true)

exit "$status"
