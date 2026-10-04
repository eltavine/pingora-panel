#!/usr/bin/env bash
# Installs the Pingora Panel host agent (ADR 0030) as a systemd service with
# the capabilities named, directories alone by default. Run as root on the
# host, after building or pulling the panel's image, and start the Compose
# installation with compose.ops-agent.yaml so the agent gets its credentials:
#
#   sudo panel/deploy/ops-agent/install.sh directories listeners
#
# Each capability is a drop-in beside this script that grants its privilege:
# directories to measure the gateway's volumes, listeners to name the
# processes on TCP ports, containers to list and manage what runs on Docker
# and to start, stop and restart the gateway's container, which makes the
# agent root-equivalent.
#
# PINGORA_PANEL_IMAGE names the image holding the agent, CONTAINER_ENGINE
# the engine that has it and runs the installation, docker or podman, and
# PINGORA_PANEL_PROJECT the installation's Compose project.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
image=${PINGORA_PANEL_IMAGE:-localhost/pingora-panel:dev}
engine=${CONTAINER_ENGINE:-docker}
project=${PINGORA_PANEL_PROJECT:-pingora-panel}
capabilities=("$@")
if [ "${#capabilities[@]}" -eq 0 ]; then
  capabilities=(directories)
fi
for capability in "${capabilities[@]}"; do
  case "$capability" in
    directories | listeners | containers) ;;
    *)
      echo "unknown capability: $capability" >&2
      exit 1
      ;;
  esac
done

if [ "$(id -u)" -ne 0 ]; then
  echo "install.sh must run as root" >&2
  exit 1
fi

container=$("$engine" create "$image")
trap '"$engine" rm --force "$container" >/dev/null' EXIT
"$engine" cp "$container:/usr/local/bin/ops-agent" /usr/local/bin/.ops-agent.new
chmod 0755 /usr/local/bin/.ops-agent.new
mv /usr/local/bin/.ops-agent.new /usr/local/bin/ops-agent

install -D -m 0644 "$here/pingora-panel.sysusers" /etc/sysusers.d/pingora-panel.conf
install -D -m 0644 "$here/pingora-panel-ops-agent.tmpfiles" \
  /etc/tmpfiles.d/pingora-panel-ops-agent.conf
systemd-sysusers /etc/sysusers.d/pingora-panel.conf
systemd-tmpfiles --create /etc/tmpfiles.d/pingora-panel-ops-agent.conf

if [ ! -e /etc/pingora-panel/ops-agent.env ]; then
  install -D -m 0644 "$here/ops-agent.env" /etc/pingora-panel/ops-agent.env
fi
for unit in pingora-panel-ops-agent.service pingora-panel-ops-agent-credentials.service \
  pingora-panel-ops-agent-credentials.path; do
  install -m 0644 "$here/$unit" "/etc/systemd/system/$unit"
done
# Where the engine keeps one of the installation's volumes, creating it as
# Compose would when the installation has not started yet.
volume_path() {
  local volume=$1
  local name="${project}_${volume}"
  if ! "$engine" volume inspect "$name" >/dev/null 2>&1; then
    "$engine" volume create \
      --label "com.docker.compose.project=$project" \
      --label "com.docker.compose.volume=$volume" \
      "$name" >/dev/null
  fi
  "$engine" volume inspect --format '{{.Mountpoint}}' "$name"
}

drop_ins=/etc/systemd/system/pingora-panel-ops-agent.service.d
for capability in "${capabilities[@]}"; do
  install -D -m 0644 "$here/$capability.conf" "$drop_ins/$capability.conf"
  if [ "$capability" = directories ]; then
    for directory in CONFIGURATION:gateway-state LOGS:gateway-logs CERTIFICATES:gateway-secrets; do
      path=$(volume_path "${directory#*:}")
      printf 'BindReadOnlyPaths=-%s\nEnvironment=PINGORA_PANEL_OPS_%s_DIR=%s\n' \
        "$path" "${directory%%:*}" "$path" >>"$drop_ins/directories.conf"
    done
  fi
done

systemctl daemon-reload
systemctl enable --now pingora-panel-ops-agent-credentials.path
systemctl enable pingora-panel-ops-agent.service
# Skipped until the credentials exist; the path unit starts it then.
systemctl start pingora-panel-ops-agent.service
