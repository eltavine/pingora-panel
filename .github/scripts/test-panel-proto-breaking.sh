#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script_dir="$repo_root/.github/scripts"

if ! command -v buf >/dev/null 2>&1; then
  printf 'buf must be installed to run the Proto compatibility self-test\n' >&2
  exit 2
fi

assert_equal() {
  local expected="$1"
  local actual="$2"
  local scenario="$3"
  if [[ "$actual" != "$expected" ]]; then
    printf '%s: expected %s, got %s\n' "$scenario" "$expected" "$actual" >&2
    exit 1
  fi
}

assert_breaking_rejected() {
  local baseline_ref="$1"
  local scenario="$2"
  if bash "$test_repo/.github/scripts/check-panel-proto-breaking.sh" "$baseline_ref" \
    >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

restore_baseline_contract() {
  git -C "$test_repo" show \
    "$baseline_ref:panel/proto/common/v1/common.proto" \
    >"$test_repo/panel/proto/common/v1/common.proto"
}

test_root="$(mktemp -d "${TMPDIR:-/tmp}/pingora-panel-proto-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
test_repo="$test_root/repository"
mkdir -p "$test_repo/.github/scripts" "$test_repo/panel/proto/common/v1"
cp "$script_dir/check-panel-proto-breaking.sh" "$test_repo/.github/scripts/"

cat >"$test_repo/panel/buf.yaml" <<'EOF'
version: v2
modules:
  - path: proto
breaking:
  use:
    - FILE
EOF

cat >"$test_repo/panel/proto/common/v1/common.proto" <<'EOF'
syntax = "proto3";

package pingora.panel.common.v1;

message StableContract {
  string value = 1;
}
EOF

git -C "$test_repo" init --quiet
git -C "$test_repo" config user.name "Proto Compatibility Test"
git -C "$test_repo" config user.email "proto-test@example.invalid"
git -C "$test_repo" add panel
git -C "$test_repo" commit --quiet -m "test: establish proto baseline"
baseline_ref="$(git -C "$test_repo" rev-parse HEAD)"

bash "$test_repo/.github/scripts/check-panel-proto-breaking.sh" \
  0000000000000000000000000000000000000000 >/dev/null

set +e
bash "$test_repo/.github/scripts/check-panel-proto-breaking.sh" does-not-exist \
  >/dev/null 2>&1
invalid_ref_status=$?
set -e
assert_equal "2" "$invalid_ref_status" "invalid baseline ref"

cat >>"$test_repo/panel/proto/common/v1/common.proto" <<'EOF'

message AdditiveContract {
  string name = 1;
}
EOF
bash "$test_repo/.github/scripts/check-panel-proto-breaking.sh" "$baseline_ref"

restore_baseline_contract
sed -i.bak 's/string value = 1;/int64 value = 1;/' \
  "$test_repo/panel/proto/common/v1/common.proto"
rm -f -- "$test_repo/panel/proto/common/v1/common.proto.bak"
assert_breaking_rejected "$baseline_ref" "breaking field type change"

restore_baseline_contract
sed -i.bak '/string value = 1;/d' \
  "$test_repo/panel/proto/common/v1/common.proto"
rm -f -- "$test_repo/panel/proto/common/v1/common.proto.bak"
assert_breaking_rejected "$baseline_ref" "deleted field"

restore_baseline_contract
sed -i.bak 's/string value = 1;/string replacement = 1;/' \
  "$test_repo/panel/proto/common/v1/common.proto"
rm -f -- "$test_repo/panel/proto/common/v1/common.proto.bak"
assert_breaking_rejected "$baseline_ref" "reused field number"

printf 'Panel Proto compatibility guard self-test passed.\n'
