#!/usr/bin/env bash
#
# Reset all fixtures and Pneuma state on the development VM
#
# Stops all Pneuma apps, removes Quadlet units and containers, removes Caddy
# fragments, removes checkouts, and recreates the database from scratch.
# Designed for a clean slate before a fresh deployment cycle.
#
# Usage:
#   scripts/dev-vm/reset-fixtures.sh [ssh-host]
#
# Default ssh-host: pneuma-dev
# The SSH target must be root because this script resets Caddy and Pneuma state.

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"

echo "==> Stopping all Pneuma apps..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && for app in \$(pneuma app list | cut -f1); do echo \"  -> stop \$app\"; pneuma app stop \$app >/dev/null 2>&1 || true; done"' 2>&1 | grep -v level=warning || true

echo "==> Removing Quadlet units..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && systemctl --user stop \"pneuma-*.service\" 2>/dev/null || true; systemctl --user disable \"pneuma-*.service\" 2>/dev/null || true; rm -f ~/.config/containers/systemd/pneuma-*.container; systemctl --user daemon-reload"' 2>&1 | grep -v level=warning || true

echo "==> Removing containers..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && podman rm -f \$(podman ps -aq --filter name=pneuma- 2>/dev/null) 2>/dev/null || true"' 2>&1 | grep -v level=warning || true

echo "==> Removing Caddy fragments..."
ssh "$SSH_HOST" 'rm -f /etc/caddy/applications/*.caddy; systemctl reload caddy 2>/dev/null || caddy reload --config /etc/caddy/Caddyfile 2>/dev/null || true' 2>&1 | grep -v level=warning || true

echo "==> Removing checkouts..."
ssh "$SSH_HOST" 'rm -rf /var/lib/pneuma/checkouts/*' 2>&1

echo "==> Resetting database..."
ssh "$SSH_HOST" 'rm -f /var/lib/pneuma/database/pneuma.sqlite3*; chown pneuma:pneuma /var/lib/pneuma/database' 2>&1

echo "==> Running pneuma doctor..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma doctor"' 2>&1 | grep -v level=warning || true

echo
echo "==> Reset complete. Run rebuild-fixtures.sh and deploy-all-fixtures.sh."
