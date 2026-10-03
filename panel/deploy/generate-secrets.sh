#!/usr/bin/env sh
# Creates the passwords, the one-time bootstrap token and the password
# pepper Compose mounts as secrets, keeping existing ones.
set -eu

directory=${1:-"$(dirname "$0")/secrets"}
umask 077
mkdir -p "$directory"
chmod 700 "$directory"
for name in postgres-admin-password identity-database-password config-database-password \
    automation-database-password observability-database-password audit-database-password \
    bootstrap-token password-pepper; do
    file="$directory/$name"
    if [ ! -s "$file" ]; then
        head -c 48 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-40 >"$file"
        # Containers run as other users; the directory keeps other host users out.
        chmod 644 "$file"
    fi
done
echo "secrets in $directory"
