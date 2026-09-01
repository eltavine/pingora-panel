#!/usr/bin/env bash
set -euo pipefail

workflow_root="${1:-.github/workflows}"
status=0

while IFS= read -r match; do
  command="${match#*:*:}"
  command="${command%%#*}"
  if [[ ! "$command" =~ --version([= 	]+)[0-9]+\.[0-9]+\.[0-9]+([^0-9.]|$) ]]; then
    echo "cargo install must use an exact --version: $match" >&2
    status=1
  fi
  if [[ ! "$command" =~ (^|[[:space:]])--locked($|[[:space:]]) ]]; then
    echo "cargo install must use --locked: $match" >&2
    status=1
  fi
done < <(grep -RInE 'cargo[[:space:]]+install' "$workflow_root" || true)

while IFS= read -r match; do
  command="${match#*:*:}"
  command="${command%%#*}"
  if [[ ! "$command" =~ (^|[[:space:]])go[[:space:]]+install[[:space:]]+([^[:space:]]+)[[:space:]]*$ ]]; then
    echo "go install must install exactly one versioned module: $match" >&2
    status=1
    continue
  fi
  module="${BASH_REMATCH[2]}"
  if [[ ! "$module" =~ ^[^@[:space:]]+@v[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]; then
    echo "go install must use an exact module version: $match" >&2
    status=1
  fi
done < <(grep -RInE '(^|[[:space:]])go[[:space:]]+install' "$workflow_root" || true)

exit "$status"
