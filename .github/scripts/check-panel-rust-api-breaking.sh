#!/usr/bin/env bash
set -euo pipefail

readonly expected_tool_version="0.50.0"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
baseline_ref="${1:-}"
semver_checks="${CARGO_SEMVER_CHECKS_BIN:-cargo-semver-checks}"
package_lister="$repo_root/.github/scripts/list-workspace-package-names.py"

if [[ -z "$baseline_ref" ]]; then
  printf 'usage: %s <git-baseline-ref>\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

if [[ "$baseline_ref" =~ ^0+$ ]]; then
  printf 'No parent commit exists; skipping the Rust API bootstrap comparison.\n'
  exit 0
fi

if ! git -C "$repo_root" cat-file -e "${baseline_ref}^{commit}" 2>/dev/null; then
  printf 'Rust API baseline ref does not resolve to a commit: %s\n' "$baseline_ref" >&2
  exit 2
fi

if ! git -C "$repo_root" cat-file -e "${baseline_ref}:panel/Cargo.toml" 2>/dev/null; then
  printf 'No Panel workspace exists at %s; treating this as the bootstrap comparison.\n' \
    "$baseline_ref"
  exit 0
fi

actual_tool_version="$("$semver_checks" --version 2>/dev/null || true)"
if [[ "$actual_tool_version" != "cargo-semver-checks $expected_tool_version" ]]; then
  printf 'cargo-semver-checks %s is required, found: %s\n' \
    "$expected_tool_version" "${actual_tool_version:-not installed}" >&2
  exit 2
fi

for command in cargo comm git python3 tar; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'required Rust API compatibility command is unavailable: %s\n' "$command" >&2
    exit 2
  fi
done
if [[ ! -f "$package_lister" ]]; then
  printf 'workspace package discovery helper does not exist: %s\n' "$package_lister" >&2
  exit 2
fi

semver_operation_root="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pingora-panel-semver.XXXXXX")"
trap 'rm -rf -- "$semver_operation_root"' EXIT
baseline_source="$semver_operation_root/baseline"
semver_target_root="$semver_operation_root/target"
mkdir -p "$baseline_source" "$semver_target_root"
git -C "$repo_root" archive "$baseline_ref" | tar -x -C "$baseline_source"

current_packages="$semver_operation_root/current-packages"
baseline_packages="$semver_operation_root/baseline-packages"
python3 "$package_lister" "$repo_root/panel/Cargo.toml" >"$current_packages"
python3 "$package_lister" "$baseline_source/panel/Cargo.toml" >"$baseline_packages"

removed_packages="$(LC_ALL=C comm -23 "$baseline_packages" "$current_packages")"
if [[ -n "$removed_packages" ]]; then
  printf 'Rust API packages were removed from the Panel workspace:\n%s\n' \
    "$removed_packages" >&2
  exit 1
fi

declare -a packages_to_check=()
while IFS= read -r package; do
  [[ -n "$package" ]] || continue
  if [[ "$package" != panel-contracts ]]; then
    packages_to_check+=("$package")
  fi
done < <(LC_ALL=C comm -12 "$baseline_packages" "$current_packages")

while IFS= read -r package; do
  [[ -n "$package" ]] || continue
  if [[ "$package" == panel-contracts ]]; then
    continue
  fi
  printf 'New Rust API package has no baseline and is treated as additive: %s\n' "$package"
done < <(LC_ALL=C comm -13 "$baseline_packages" "$current_packages")

# Additive Proto evolution necessarily adds fields to generated Rust structs.
# Buf owns that wire contract; this guard owns every hand-written public API.
for package in "${packages_to_check[@]}"; do
  # cargo-semver-checks deliberately skips `publish = false` crates selected
  # through --workspace. Every Panel package is private, so select each one
  # explicitly or this guard would clone the baseline and check zero APIs.
  CARGO_TARGET_DIR="$semver_target_root" "$semver_checks" check-release \
    --manifest-path "$repo_root/panel/Cargo.toml" \
    --package "$package" \
    --baseline-rev "$baseline_ref" \
    --release-type minor \
    --all-features
done
