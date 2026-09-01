#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="$repo_root/.github/scripts/check-cargo-machete-workspace.sh"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/cargo-machete-workspace.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
test_root="$(cd "$test_root" && pwd -P)"

workspace_root="$test_root/workspace with spaces"
outer_package="$workspace_root/outer-package"
nested_workspace="$workspace_root/nested"
nested_package="$nested_workspace/nested-package"
mkdir -p \
  "$outer_package/src" \
  "$nested_package/src" \
  "$test_root/bin"

cat >"$workspace_root/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = ["outer-package"]
exclude = ["nested"]
EOF
cat >"$outer_package/Cargo.toml" <<'EOF'
[package]
name = "outer-package"
version = "0.1.0"
edition = "2021"
EOF
printf '// outer fixture\n' >"$outer_package/src/lib.rs"

cat >"$nested_workspace/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = ["nested-package"]
EOF
cat >"$nested_package/Cargo.toml" <<'EOF'
[package]
name = "nested-package"
version = "0.1.0"
edition = "2021"
EOF
printf '// nested fixture\n' >"$nested_package/src/lib.rs"

cargo generate-lockfile --manifest-path "$workspace_root/Cargo.toml" --offline >/dev/null
cargo generate-lockfile --manifest-path "$nested_workspace/Cargo.toml" --offline >/dev/null

cat >"$test_root/bin/cargo-machete" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${MACHETE_CAPTURE:?MACHETE_CAPTURE must be set}"
printf '%s\n' "$@" >"$MACHETE_CAPTURE"
EOF
chmod +x "$test_root/bin/cargo-machete"

assert_status() {
  local expected="$1"
  local scenario="$2"
  shift 2

  set +e
  "$@" >/dev/null 2>&1
  local actual=$?
  set -e
  if [[ "$actual" -ne "$expected" ]]; then
    printf '%s: expected exit %s, got %s\n' "$scenario" "$expected" "$actual" >&2
    exit 1
  fi
}

assert_arguments() {
  local capture="$1"
  local scenario="$2"
  shift 2
  local -a expected=("$@")
  local -a actual=()
  mapfile -t actual <"$capture"

  if [[ ${#actual[@]} -ne ${#expected[@]} ]]; then
    printf '%s: expected %s arguments, got %s\n' \
      "$scenario" "${#expected[@]}" "${#actual[@]}" >&2
    exit 1
  fi

  local index
  for ((index = 0; index < ${#expected[@]}; index++)); do
    if [[ "${actual[$index]}" != "${expected[$index]}" ]]; then
      printf '%s: argument %s expected %s, got %s\n' \
        "$scenario" "$index" "${expected[$index]}" "${actual[$index]}" >&2
      exit 1
    fi
  done
}

empty_workspace="$test_root/empty"
mkdir -p "$empty_workspace"
cat >"$empty_workspace/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = []
EOF
cargo generate-lockfile --manifest-path "$empty_workspace/Cargo.toml" --offline >/dev/null
assert_status \
  2 \
  "empty workspace" \
  env PATH="$test_root/bin:$PATH" MACHETE_CAPTURE="$test_root/empty-arguments" \
  bash "$checker" "$empty_workspace/Cargo.toml"

outer_capture="$test_root/outer-arguments"
PATH="$test_root/bin:$PATH" MACHETE_CAPTURE="$outer_capture" \
  bash "$checker" "$workspace_root/Cargo.toml"
assert_arguments \
  "$outer_capture" "outer workspace" machete --skip-target-dir "$outer_package"

nested_capture="$test_root/nested-arguments"
PATH="$test_root/bin:$PATH" MACHETE_CAPTURE="$nested_capture" \
  bash "$checker" "$nested_workspace/Cargo.toml"
assert_arguments \
  "$nested_capture" "nested workspace" machete --skip-target-dir "$nested_package"

assert_status 2 "missing manifest argument" bash "$checker"
assert_status 2 "missing manifest file" bash "$checker" "$test_root/missing.toml"

printf 'Workspace cargo-machete wrapper self-test passed.\n'
