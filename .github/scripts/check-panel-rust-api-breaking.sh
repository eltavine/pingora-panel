#!/usr/bin/env bash
set -euo pipefail

readonly expected_tool_version="0.50.0"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
baseline_ref="${1:-}"
semver_checks="${CARGO_SEMVER_CHECKS_BIN:-cargo-semver-checks}"

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

semver_target_root="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pingora-panel-semver.XXXXXX")"
trap 'rm -rf -- "$semver_target_root"' EXIT

# Additive Proto evolution necessarily adds fields to generated Rust structs.
# Buf owns that wire contract; this guard owns every hand-written public API.
CARGO_TARGET_DIR="$semver_target_root" "$semver_checks" check-release \
  --manifest-path "$repo_root/panel/Cargo.toml" \
  --workspace \
  --exclude panel-contracts \
  --baseline-rev "$baseline_ref" \
  --release-type minor \
  --all-features
