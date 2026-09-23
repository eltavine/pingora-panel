#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
baseline_ref="${1:-}"
spec_path=panel/panel-api/tests/fixtures/openapi.json

if [[ -z "$baseline_ref" ]]; then
  printf 'usage: %s <git-baseline-ref>\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi
if [[ ! -f "$repo_root/$spec_path" ]]; then
  printf 'Current OpenAPI fixture is missing: %s\n' "$spec_path" >&2
  exit 2
fi
if [[ "$baseline_ref" =~ ^0+$ ]]; then
  printf 'No parent commit exists; skipping the OpenAPI bootstrap comparison.\n'
  exit 0
fi
if ! git -C "$repo_root" cat-file -e "${baseline_ref}^{commit}" 2>/dev/null; then
  printf 'OpenAPI baseline ref does not resolve to a commit: %s\n' "$baseline_ref" >&2
  exit 2
fi
if ! git -C "$repo_root" cat-file -e "${baseline_ref}:${spec_path}" 2>/dev/null; then
  printf 'No OpenAPI fixture exists at %s; treating this as the bootstrap comparison.\n' "$baseline_ref"
  exit 0
fi
if ! command -v oasdiff >/dev/null 2>&1; then
  printf 'oasdiff must be installed to compare OpenAPI contracts\n' >&2
  exit 2
fi

baseline_spec="$(mktemp "${TMPDIR:-/tmp}/pingora-panel-openapi.XXXXXX.json")"
trap 'rm -f -- "$baseline_spec"' EXIT
git -C "$repo_root" show "${baseline_ref}:${spec_path}" >"$baseline_spec"
oasdiff breaking --allow-external-refs=false --fail-on ERR \
  "$baseline_spec" "$repo_root/$spec_path"
