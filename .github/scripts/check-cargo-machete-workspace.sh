#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  printf 'usage: %s <workspace-manifest>\n' "$0" >&2
  exit 2
fi

workspace_manifest="$1"
if [[ ! -f "$workspace_manifest" ]]; then
  printf 'workspace manifest does not exist: %s\n' "$workspace_manifest" >&2
  exit 2
fi

for command in cargo cargo-machete dirname jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 2
  fi
done

workspace_metadata="$(
  cargo metadata \
    --manifest-path "$workspace_manifest" \
    --format-version 1 \
    --no-deps \
    --locked
)"
package_manifests="$(jq -er '.packages[].manifest_path' <<<"$workspace_metadata")"

declare -a package_directories=()
while IFS= read -r package_manifest; do
  package_directories+=("$(dirname "$package_manifest")")
done <<<"$package_manifests"

if [[ ${#package_directories[@]} -eq 0 ]]; then
  printf 'workspace has no package members: %s\n' "$workspace_manifest" >&2
  exit 2
fi

cargo machete --skip-target-dir "${package_directories[@]}"
