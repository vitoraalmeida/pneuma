#!/usr/bin/env bash
#
# Overview of all Pneuma apps and infrastructure on the development VM
#
# Shows applications, per-app status, containers, systemd units, Caddy
# fragments and registry state in one place.
#
# Usage:
#   scripts/dev-vm/overview.sh [ssh-host]
#
# Default ssh-host: pneuma-dev

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"

echo "==> Applications:"
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app list"' 2>&1 | grep -v level=warning || true

echo
echo "==> Application status:"
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && for app in \$(pneuma app list | cut -f1); do echo \"--- \$app ---\"; pneuma app status \$app 2>&1 || true; done"' 2>&1 | grep -v level=warning || true

echo
echo "==> Containers:"
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && podman ps -a --format \"table {{.Names}}\t{{.Status}}\t{{.Ports}}\""' 2>&1 | grep -v level=warning || true

echo
echo "==> Systemd units:"
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && systemctl --user list-units --type=service --all | grep pneuma- || echo \"No Pneuma units\""' 2>&1 | grep -v level=warning || true

echo
echo "==> Caddy fragments:"
ssh "$SSH_HOST" 'ls -1 /etc/caddy/applications/ 2>/dev/null || echo "No fragments"' 2>&1 | grep -v level=warning || true

echo
echo "==> Registry catalog:"
ssh "$SSH_HOST" 'curl -s http://localhost:5000/v2/_catalog 2>/dev/null || echo "Registry not accessible"' 2>&1 | grep -v level=warning || true
