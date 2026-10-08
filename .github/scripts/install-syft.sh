#!/usr/bin/env bash
set -euo pipefail

readonly syft_version="1.54.1"

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [install-directory]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

case "$(uname -s):$(uname -m)" in
  Linux:x86_64)
    platform="linux_amd64"
    sha256="c069905b391cc4c20a5ba65ad5c10be2a7ba074f8ea6ad203e24d14e303dad47"
    ;;
  Linux:aarch64|Linux:arm64)
    platform="linux_arm64"
    sha256="dfdf0537610113edbefe1f1fc6548bc957b2d77439636ec824fcf0e10d46d054"
    ;;
  Darwin:arm64)
    platform="darwin_arm64"
    sha256="b4319c3abaa87a0170ab76ee83ea2260ca34b53aecfa3ab0dd5428d2319d744f"
    ;;
  Darwin:x86_64)
    platform="darwin_amd64"
    sha256="2956322838b2f64e470eea474495f0cd96be4f222b1ac037258cdc47d965064e"
    ;;
  *)
    printf 'unsupported Syft installer platform: %s %s\n' \
      "$(uname -s)" "$(uname -m)" >&2
    exit 2
    ;;
esac

archive_name="syft_${syft_version}_${platform}.tar.gz"
download_url="https://github.com/anchore/syft/releases/download/v${syft_version}/${archive_name}"
install_directory="${1:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/syft-bin}"
temporary_directory="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/syft-install.XXXXXX")"
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
install -m 0755 "$temporary_directory/syft" "$install_directory/syft"

installed_version="$("$install_directory/syft" --version)"
if [[ "$installed_version" != "syft 1.54.1" ]]; then
  printf 'installed Syft version mismatch: expected %s, got %s\n' \
    "syft 1.54.1" "${installed_version:-unknown}" >&2
  exit 1
fi
