#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workflow_root="$repo_root/.github/workflows"
failed=0

while IFS=: read -r file line declaration; do
  action="${declaration#*uses:}"
  action="${action%%#*}"
  action="${action//[[:space:]]/}"
  if [[ "$action" == ./* ]]; then
    continue
  fi
  reference="${action##*@}"
  if [[ ! "$reference" =~ ^[0-9a-f]{40}$ ]]; then
    printf 'mutable GitHub Action reference: %s:%s: %s\n' "$file" "$line" "$action" >&2
    failed=1
  fi
done < <(rg --no-heading --line-number 'uses:[[:space:]]*[^[:space:]]+@' "$workflow_root")

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

printf 'All external GitHub Actions are pinned to immutable commit SHAs.\n'
