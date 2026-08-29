#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
manifest="$repo_root/panel/Cargo.toml"
metadata="$(cargo metadata --manifest-path "$manifest" --format-version 1 --locked)"
failed=0

workspace_members="$(jq -r '.workspace_members[]' <<<"$metadata")"
for expected in panel-contracts panel-errors panel-domain panel-ir panel-engine gateway-pingora; do
  if ! grep -q "/panel/$expected#" <<<"$workspace_members"; then
    printf 'workspace violation: expected member %s is missing\n' "$expected" >&2
    failed=1
  fi
done

declare -A direct_dependencies
while IFS=$'\t' read -r package dependency; do
  direct_dependencies["$package"]+=" $dependency"
done < <(
  jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name as $package
    | .dependencies[]
    | [$package, .name]
    | @tsv
  ' <<<"$metadata"
)

report_path() {
  local package="$1"
  local dependency="$2"
  printf '  reverse dependency path: %s -> %s\n' "$package" "$dependency" >&2
}

for package in "${!direct_dependencies[@]}"; do
  for dependency in ${direct_dependencies[$package]}; do
    if [[ "$dependency" == pingora-* && "$package" != "gateway-pingora" ]]; then
      printf 'boundary violation: only gateway-pingora may depend on %s\n' "$dependency" >&2
      report_path "$package" "$dependency"
      failed=1
    fi
  done
done

protected=(panel-errors panel-domain panel-ir panel-engine)
for package in "${protected[@]}"; do
  for dependency in ${direct_dependencies[$package]-}; do
    case "$dependency" in
      pingora-*|tonic|tonic-prost|prost|axum|sqlx|async-nats|nats)
        printf 'boundary violation: %s must remain engine/transport/storage neutral\n' "$package" >&2
        report_path "$package" "$dependency"
        failed=1
        ;;
    esac
  done
done

for package in panel-domain panel-ir panel-engine; do
  if [[ " ${direct_dependencies[$package]-} " == *" panel-contracts "* ]]; then
    printf 'boundary violation: %s must not depend on generated Proto contracts\n' "$package" >&2
    report_path "$package" panel-contracts
    failed=1
  fi
done

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

printf 'Panel dependency boundaries verified.\n'
