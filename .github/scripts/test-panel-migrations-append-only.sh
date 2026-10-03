#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/pingora-panel-migrations-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
test_repo="$test_root/repository"
migrations="$test_repo/panel/service/migrations"
mkdir -p "$test_repo/.github/scripts" "$migrations" "$test_repo/panel/other/migrations"
cp "$script_dir/check-panel-migrations-append-only.sh" "$test_repo/.github/scripts/"

git -C "$test_repo" init --quiet
git -C "$test_repo" config user.name "Migration Test"
git -C "$test_repo" config user.email "migration-test@example.invalid"
git -C "$test_repo" commit --quiet --allow-empty -m 'test: before migrations'
bootstrap_ref="$(git -C "$test_repo" rev-parse HEAD)"
printf 'CREATE TABLE a (id int);\n' >"$migrations/0100_a.sql"
printf 'CREATE TABLE b (id int);\n' >"$migrations/0200_b.sql"
printf 'CREATE TABLE c (id int);\n' >"$test_repo/panel/other/migrations/0001_c.sql"
git -C "$test_repo" add panel
git -C "$test_repo" commit --quiet -m 'test: publish migrations'
baseline_ref="$(git -C "$test_repo" rev-parse HEAD)"

check() {
  bash "$test_repo/.github/scripts/check-panel-migrations-append-only.sh" "$1"
}

expect_failure() {
  local case="$1" message="$2"
  if check "$baseline_ref" >"$test_root/result" 2>&1; then
    printf '%s was accepted\n' "$case" >&2
    exit 1
  fi
  if ! grep -q "$message" "$test_root/result"; then
    printf '%s did not report %s:\n' "$case" "$message" >&2
    cat "$test_root/result" >&2
    exit 1
  fi
}

restore() {
  git -C "$test_repo" checkout --quiet "$baseline_ref" -- panel
  git -C "$test_repo" clean --quiet -fd -- panel
}

check 0000000000000000000000000000000000000000 >/dev/null
check "$bootstrap_ref" >/dev/null
check "$baseline_ref" >/dev/null
if check does-not-exist >"$test_root/result" 2>&1; then
  printf 'Invalid baseline was accepted\n' >&2
  exit 1
fi
grep -q 'does not resolve' "$test_root/result"

printf 'CREATE TABLE d (id int);\n' >"$migrations/0300_d.sql"
printf 'CREATE TABLE e (id int);\n' >"$test_repo/panel/other/migrations/0002_e.sql"
check "$baseline_ref" >/dev/null
restore

printf -- '-- edited\n' >>"$migrations/0100_a.sql"
expect_failure 'An edited migration' 'was changed'
restore

rm "$migrations/0200_b.sql"
expect_failure 'A removed migration' 'removed or renamed'
restore

git -C "$test_repo" mv "$migrations/0200_b.sql" "$migrations/0200_renamed.sql"
expect_failure 'A renamed migration' 'removed or renamed'
restore

printf 'CREATE TABLE f (id int);\n' >"$migrations/0150_f.sql"
expect_failure 'A migration inserted before a published one' 'sorts before'
restore

printf 'Migration guard self-test passed.\n'
