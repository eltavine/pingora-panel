#!/usr/bin/env bash
set -euo pipefail

readonly cargo_about_version="0.9.2"

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [install-directory]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

case "$(uname -s):$(uname -m)" in
  Linux:x86_64)
    target="x86_64-unknown-linux-musl"
    sha256="9099a59e820c38a68b9d65f300662a567d56562f9a10f6aa4c7e86c17c2566af"
    ;;
  Linux:aarch64|Linux:arm64)
    target="aarch64-unknown-linux-musl"
    sha256="af5169282fb6f84e13471493f405437e43ac517744c9ae12fbe2cdf0a6f0e5a8"
    ;;
  Darwin:arm64)
    target="aarch64-apple-darwin"
    sha256="ae72f0df0c399a1e96336f696fa55b1b28679fd725632eba8cf8e4568467cc3e"
    ;;
  *)
    printf 'unsupported cargo-about installer platform: %s %s\n' \
      "$(uname -s)" "$(uname -m)" >&2
    exit 2
    ;;
esac

archive_name="cargo-about-${cargo_about_version}-${target}.tar.gz"
download_url="https://github.com/EmbarkStudios/cargo-about/releases/download/${cargo_about_version}/${archive_name}"
install_directory="${1:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cargo-about-bin}"
temporary_directory="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/cargo-about-install.XXXXXX")"
trap 'rm -rf -- "$temporary_directory"' EXIT

curl --fail --location --retry 3 --retry-all-errors --connect-timeout 30 \
  --proto '=https' --tlsv1.2 \
  --output "$temporary_directory/$archive_name" \
  "$download_url"

if [[ "$(uname -s)" == Linux ]] && command -v sha256sum >/dev/null 2>&1; then
  printf '%s  %s\n' "$sha256" "$temporary_directory/$archive_name" \
    | sha256sum -c >/dev/null
elif command -v shasum >/dev/null 2>&1; then
  printf '%s  %s\n' "$sha256" "$temporary_directory/$archive_name" \
    | shasum -a 256 -c >/dev/null
else
  printf 'neither sha256sum nor shasum is available\n' >&2
  exit 2
fi

tar --extract --gzip --file "$temporary_directory/$archive_name" \
  --directory "$temporary_directory"
mkdir -p "$install_directory"
install -m 0755 "$temporary_directory/cargo-about-${cargo_about_version}-${target}/cargo-about" \
  "$install_directory/cargo-about"

installed_version="$("$install_directory/cargo-about" --version)"
if [[ "$installed_version" != "cargo-about ${cargo_about_version}" ]]; then
  printf 'installed cargo-about version mismatch: expected %s, got %s\n' \
    "cargo-about ${cargo_about_version}" "${installed_version:-unknown}" >&2
  exit 1
fi
