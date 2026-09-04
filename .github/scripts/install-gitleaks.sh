#!/usr/bin/env bash
set -euo pipefail

readonly gitleaks_version="8.30.0"

if [[ $# -gt 1 ]]; then
  printf 'usage: %s [install-directory]\n' "${BASH_SOURCE[0]}" >&2
  exit 2
fi

case "$(uname -s):$(uname -m)" in
  Linux:x86_64)
    platform="linux_x64"
    sha256="79a3ab579b53f71efd634f3aaf7e04a0fa0cf206b7ed434638d1547a2470a66e"
    ;;
  Linux:aarch64|Linux:arm64)
    platform="linux_arm64"
    sha256="b4cbbb6ddf7d1b2a603088cd03a4e3f7ce48ee7fd449b51f7de6ee2906f5fa2f"
    ;;
  Darwin:arm64)
    platform="darwin_arm64"
    sha256="b251ab2bcd4cd8ba9e56ff37698c033ebf38582b477d21ebd86586d927cf87e7"
    ;;
  Darwin:x86_64)
    platform="darwin_x64"
    sha256="ca221d012d247080c2f6f61f4b7a83bffa2453806b0c195c795bbe9a8c775ed5"
    ;;
  *)
    printf 'unsupported Gitleaks installer platform: %s %s\n' \
      "$(uname -s)" "$(uname -m)" >&2
    exit 2
    ;;
esac

archive_name="gitleaks_${gitleaks_version}_${platform}.tar.gz"
download_url="https://github.com/gitleaks/gitleaks/releases/download/v${gitleaks_version}/${archive_name}"
install_directory="${1:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/gitleaks-bin}"
temporary_directory="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/gitleaks-install.XXXXXX")"
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
install -m 0755 "$temporary_directory/gitleaks" "$install_directory/gitleaks"

installed_version="$("$install_directory/gitleaks" version)"
if [[ "$installed_version" != "$gitleaks_version" ]]; then
  printf 'installed Gitleaks version mismatch: expected %s, got %s\n' \
    "$gitleaks_version" "${installed_version:-unknown}" >&2
  exit 1
fi
