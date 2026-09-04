#!/usr/bin/env bash
set -euo pipefail

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [gitleaks-binary]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

gitleaks="${1:-gitleaks}"
if [[ ! -x "$gitleaks" ]] && ! command -v "$gitleaks" >/dev/null 2>&1; then
  printf 'Gitleaks executable is unavailable: %s\n' "$gitleaks" >&2
  exit 2
fi

test_root="$(mktemp -d "${TMPDIR:-/tmp}/gitleaks-secret-scan.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
repository="$test_root/repository"
mkdir -p "$repository"
git -C "$repository" init --quiet
git -C "$repository" config user.name "Secret Scan Test"
git -C "$repository" config user.email "secret-scan@example.invalid"

printf 'api_token = "not-a-secret"\n' >"$repository/config.txt"
git -C "$repository" add config.txt
git -C "$repository" commit --quiet -m "test: add clean fixture"
"$gitleaks" git --no-banner --redact --exit-code 1 "$repository" >/dev/null

readonly fake_secret="Aq7mZ9pL2vX8cN4bK6sT1wY5dF3hJ0rQ"
printf 'api_key = "%s"\n' "$fake_secret" >"$repository/config.txt"
git -C "$repository" add config.txt
git -C "$repository" commit --quiet -m "test: add synthetic leak"

set +e
"$gitleaks" git --no-banner --redact --exit-code 23 "$repository" \
  >"$test_root/output" 2>&1
scan_status=$?
set -e
if [[ "$scan_status" -ne 23 ]]; then
  printf 'synthetic secret was not rejected with the configured exit code\n' >&2
  exit 1
fi
if rg --fixed-strings --quiet "$fake_secret" "$test_root/output"; then
  printf 'redacted scanner output exposed the synthetic secret\n' >&2
  exit 1
fi

printf 'Gitleaks secret scanning self-test passed.\n'
