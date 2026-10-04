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
# processes on TCP ports, gateway-unit to start, stop and restart the
# gateway's systemd unit with the polkit rule that allows it, containers to
# list and manage what runs on Docker, which makes the agent root-equivalent.
#
# PINGORA_PANEL_IMAGE names the image holding the agent, and
# CONTAINER_ENGINE the engine that has it, docker or podman.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
image=${PINGORA_PANEL_IMAGE:-localhost/pingora-panel:dev}
engine=${CONTAINER_ENGINE:-docker}
capabilities=("$@")
if [ "${#capabilities[@]}" -eq 0 ]; then
  capabilities=(directories)
fi
for capability in "${capabilities[@]}"; do
  case "$capability" in
    directories | listeners | gateway-unit | containers) ;;
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
for capability in "${capabilities[@]}"; do
  install -D -m 0644 "$here/$capability.conf" \
    "/etc/systemd/system/pingora-panel-ops-agent.service.d/$capability.conf"
  if [ "$capability" = gateway-unit ]; then
    install -D -m 0644 "$here/pingora-panel-ops-agent.rules" \
      /etc/polkit-1/rules.d/50-pingora-panel-ops-agent.rules
  fi
done

systemctl daemon-reload
systemctl enable --now pingora-panel-ops-agent-credentials.path
systemctl enable pingora-panel-ops-agent.service
# Skipped until the credentials exist; the path unit starts it then.
systemctl start pingora-panel-ops-agent.service
