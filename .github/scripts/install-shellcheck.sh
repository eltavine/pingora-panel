#!/usr/bin/env bash
set -euo pipefail

readonly shellcheck_version="0.11.0"
readonly shellcheck_sha256="8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198"
readonly archive_name="shellcheck-v${shellcheck_version}.linux.x86_64.tar.xz"
readonly download_url="https://github.com/koalaman/shellcheck/releases/download/v${shellcheck_version}/${archive_name}"

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [install-directory]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
  printf 'ShellCheck installer only supports the pinned Linux x86_64 CI runner\n' >&2
  exit 2
fi

install_directory="${1:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/shellcheck-bin}"
temporary_directory="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/shellcheck-install.XXXXXX")"
trap 'rm -rf -- "$temporary_directory"' EXIT

curl --fail --location --retry 3 --retry-all-errors --connect-timeout 30 \
  --proto '=https' --tlsv1.2 \
  --output "$temporary_directory/$archive_name" \
  "$download_url"
printf '%s  %s\n' "$shellcheck_sha256" "$temporary_directory/$archive_name" \
  | sha256sum --check --status
tar --extract --xz --file "$temporary_directory/$archive_name" \
  --directory "$temporary_directory"
mkdir -p "$install_directory"
install -m 0755 \
  "$temporary_directory/shellcheck-v${shellcheck_version}/shellcheck" \
  "$install_directory/shellcheck"

installed_version="$("$install_directory/shellcheck" --version \
  | awk '$1 == "version:" { print $2 }')"
if [[ "$installed_version" != "$shellcheck_version" ]]; then
  printf 'installed ShellCheck version mismatch: expected %s, got %s\n' \
    "$shellcheck_version" "${installed_version:-unknown}" >&2
  exit 1
fi
