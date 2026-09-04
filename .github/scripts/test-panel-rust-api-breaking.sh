#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script_dir="$repo_root/.github/scripts"

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
  local scenario="$1"
  if bash "$test_repo/.github/scripts/check-panel-rust-api-breaking.sh" "$baseline_ref" \
    >/dev/null 2>&1; then
    printf '%s was not rejected\n' "$scenario" >&2
    exit 1
  fi
}

test_root="$(mktemp -d "${TMPDIR:-/tmp}/pingora-panel-rust-api-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
test_repo="$test_root/repository"
mkdir -p \
  "$test_repo/.github/scripts" \
  "$test_repo/panel/panel-contracts/src" \
  "$test_repo/panel/stable-api/src"
cp "$script_dir/check-panel-rust-api-breaking.sh" "$test_repo/.github/scripts/"
cp "$script_dir/list-workspace-package-names.py" "$test_repo/.github/scripts/"

cat >"$test_repo/panel/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = ["panel-contracts", "stable-api"]
EOF

cat >"$test_repo/panel/panel-contracts/Cargo.toml" <<'EOF'
[package]
name = "panel-contracts"
version = "0.1.0"
edition = "2021"
publish = false
EOF

cat >"$test_repo/panel/panel-contracts/src/lib.rs" <<'EOF'
// Generated transport bindings are compatibility-checked by Buf, not Rust SemVer.
pub struct GeneratedContract {
    pub value: String,
}
EOF

cat >"$test_repo/panel/stable-api/Cargo.toml" <<'EOF'
[package]
name = "stable-api"
version = "0.1.0"
edition = "2021"
publish = false

[features]
default = []
extended = []
EOF

cat >"$test_repo/panel/stable-api/src/lib.rs" <<'EOF'
pub struct StableValue(u64);

impl StableValue {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn value(&self) -> u64 {
        self.0
    }
}

#[cfg(feature = "extended")]
pub fn feature_only_api() -> &'static str {
    "extended"
}
EOF

git -C "$test_repo" init --quiet
git -C "$test_repo" config user.name "Rust API Compatibility Test"
git -C "$test_repo" config user.email "rust-api-test@example.invalid"
cargo generate-lockfile --manifest-path "$test_repo/panel/Cargo.toml" --quiet
git -C "$test_repo" add panel
git -C "$test_repo" commit --quiet -m "test: establish Rust API baseline"
baseline_ref="$(git -C "$test_repo" rev-parse HEAD)"

bash "$test_repo/.github/scripts/check-panel-rust-api-breaking.sh" \
  0000000000000000000000000000000000000000 >/dev/null

set +e
bash "$test_repo/.github/scripts/check-panel-rust-api-breaking.sh" does-not-exist \
  >/dev/null 2>&1
invalid_ref_status=$?
set -e
assert_equal "2" "$invalid_ref_status" "invalid baseline ref"

cat >>"$test_repo/panel/stable-api/src/lib.rs" <<'EOF'

pub fn additive_api() -> &'static str {
    "additive"
}
EOF
bash "$test_repo/.github/scripts/check-panel-rust-api-breaking.sh" "$baseline_ref" >/dev/null

# A newly introduced package has no baseline API and is therefore an additive
# workspace bootstrap, not a breaking change in existing packages.
mkdir -p "$test_repo/panel/new-api/src"
cat >"$test_repo/panel/new-api/Cargo.toml" <<'EOF'
[package]
name = "new-api"
version = "0.1.0"
edition = "2021"
publish = false
EOF
cat >"$test_repo/panel/new-api/src/lib.rs" <<'EOF'
pub struct NewlyIntroducedValue;
EOF
sed -i.bak \
  's/members = \["panel-contracts", "stable-api"\]/members = ["panel-contracts", "stable-api", "new-api"]/' \
  "$test_repo/panel/Cargo.toml"
rm -f -- "$test_repo/panel/Cargo.toml.bak"
cargo generate-lockfile --manifest-path "$test_repo/panel/Cargo.toml" --quiet
bash "$test_repo/.github/scripts/check-panel-rust-api-breaking.sh" "$baseline_ref" >/dev/null

cat >"$test_repo/panel/panel-contracts/src/lib.rs" <<'EOF'
// Simulate a source-breaking generated change. Buf, not this Rust API guard,
// owns compatibility for the generated transport crate.
pub struct GeneratedContract;
EOF
bash "$test_repo/.github/scripts/check-panel-rust-api-breaking.sh" "$baseline_ref" >/dev/null

sed -i.bak '/#\[cfg(feature = "extended")\]/,/^}/d' \
  "$test_repo/panel/stable-api/src/lib.rs"
rm -f -- "$test_repo/panel/stable-api/src/lib.rs.bak"
assert_breaking_rejected "removed feature-gated public API"

git -C "$test_repo" show "$baseline_ref:panel/stable-api/src/lib.rs" \
  >"$test_repo/panel/stable-api/src/lib.rs"
sed -i.bak '/    pub fn value/,/    }/d' "$test_repo/panel/stable-api/src/lib.rs"
rm -f -- "$test_repo/panel/stable-api/src/lib.rs.bak"
assert_breaking_rejected "removed public method"

sed -i.bak \
  's/members = \["panel-contracts", "stable-api", "new-api"\]/members = ["panel-contracts", "new-api"]/' \
  "$test_repo/panel/Cargo.toml"
rm -f -- "$test_repo/panel/Cargo.toml.bak"
cargo generate-lockfile --manifest-path "$test_repo/panel/Cargo.toml" --quiet
assert_breaking_rejected "removed workspace package"

printf 'Panel Rust API compatibility guard self-test passed.\n'
