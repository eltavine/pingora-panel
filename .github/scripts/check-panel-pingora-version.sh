#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ $# -gt 3 ]]; then
  printf 'usage: %s [panel-workspace-manifest] [adapter-source] [pingora-source-root]\n' \
    "${BASH_SOURCE[0]}" >&2
  exit 2
fi

manifest="${1:-$repo_root/panel/Cargo.toml}"
adapter_source="${2:-$repo_root/panel/gateway-pingora/src/lib.rs}"
pingora_source_root="${3:-$repo_root}"
for file in "$manifest" "$adapter_source"; do
  if [[ ! -f "$file" ]]; then
    printf 'required version contract file does not exist: %s\n' "$file" >&2
    exit 2
  fi
done
if [[ ! -d "$pingora_source_root" ]]; then
  printf 'Pingora source root does not exist: %s\n' "$pingora_source_root" >&2
  exit 2
fi
pingora_source_root="$(cd "$pingora_source_root" && pwd -P)"

for command in cargo jq sed; do
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
dependency_rows="$(
  jq -r '
    .packages[]
    | select(.name == "gateway-pingora")
    | .dependencies[]
    | select((.name == "pingora") or (.name | startswith("pingora-")))
    | [.name, .req, (.path // "")]
    | @tsv
  ' <<<"$metadata"
)"

if [[ -z "$dependency_rows" ]]; then
  printf 'gateway-pingora has no direct Pingora dependencies\n' >&2
  exit 1
fi

failed=0
declare -A dependency_versions=()
while IFS=$'\t' read -r dependency requirement dependency_path; do
  if [[ -z "$dependency_path" ]]; then
    printf 'version violation: %s must remain an audited local path dependency\n' \
      "$dependency" >&2
    failed=1
  else
    expected_dependency_path="$pingora_source_root/$dependency"
    if [[ ! -d "$expected_dependency_path" ]]; then
      printf 'version violation: audited source directory is missing for %s: %s\n' \
        "$dependency" "$expected_dependency_path" >&2
      failed=1
    elif [[ "$(cd "$dependency_path" && pwd -P)" \
      != "$(cd "$expected_dependency_path" && pwd -P)" ]]; then
      printf 'version violation: %s resolves outside its audited source directory: %s\n' \
        "$dependency" "$dependency_path" >&2
      failed=1
    fi
  fi
  if [[ ! "$requirement" =~ ^=([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
    printf 'version violation: %s must use an exact version, found %s\n' \
      "$dependency" "$requirement" >&2
    failed=1
    continue
  fi
  dependency_versions["${BASH_REMATCH[1]}"]=1
done <<<"$dependency_rows"

if [[ ${#dependency_versions[@]} -ne 1 ]]; then
  printf 'version violation: gateway-pingora dependencies do not share one version:\n%s\n' \
    "$dependency_rows" >&2
  failed=1
fi

constant_version="$(
  sed -nE \
    's/^pub const PINGORA_PACKAGE_VERSION: &str = "([0-9]+\.[0-9]+\.[0-9]+)";$/\1/p' \
    "$adapter_source"
)"
if [[ ! "$constant_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'version violation: PINGORA_PACKAGE_VERSION must be one literal semantic version\n' >&2
  failed=1
fi

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

dependency_version="${!dependency_versions[*]}"
if [[ "$constant_version" != "$dependency_version" ]]; then
  printf 'version violation: reported Pingora version %s does not match dependency version %s\n' \
    "$constant_version" "$dependency_version" >&2
  exit 1
fi

printf 'Pingora adapter version coherence verified: %s\n' "$dependency_version"
