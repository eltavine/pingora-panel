#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ $# -gt 1 ]]; then
  printf 'usage: %s [panel-workspace-manifest]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

manifest="${1:-$repo_root/panel/Cargo.toml}"
if [[ ! -f "$manifest" ]]; then
  printf 'Panel workspace manifest does not exist: %s\n' "$manifest" >&2
  exit 2
fi
for command in cargo jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 2
  fi
done

metadata="$(
  cargo metadata \
    --manifest-path "$manifest" \
    --format-version 1 \
    --no-deps \
    --locked
)"

publishable_packages=()
while IFS= read -r package; do
  publishable_packages+=("$package")
done < <(
  jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | select(.publish != [])
    | .name
  ' <<<"$metadata"
)

if [[ ${#publishable_packages[@]} -ne 0 ]]; then
  printf 'package policy violation: Panel workspace crates must set publish = false:\n' >&2
  printf '  %s\n' "${publishable_packages[@]}" >&2
  exit 1
fi

printf 'Panel package publication policy verified.\n'
