#!/usr/bin/env bash
#
# Pneuma development VM host provisioning script
#
# Prepares a clean Debian 13 (trixie) VM as the standard Pneuma integration
# target. Reproduces the relevant properties of the production VPS without
# turning the VM into a second development station:
#   - rootless Podman + Quadlet user generator
#   - Caddy importing only Pneuma-managed fragments
#   - dedicated pneuma user with subuid/subgid and linger
#   - persistent Pneuma directories with VPS-like permissions
#   - PNEUMA_* environment variables on the pneuma user profile
#
# This script provisions the host only; it never builds Pneuma. The Pneuma
# binary is installed as a separate step after provisioning (see section 4 of
# docs/operations/dev-vm-tutorial.md).
#
# Prerequisites:
# - Debian 13 (trixie) VM — Debian 12 ships Podman 4.3.1 without the Quadlet
#   user generator, which Pneuma requires to supervise runtimes across reboots.
# - The VM must already have the provisioning SSH key installed (for example
#   root's authorized_keys) so this script can be run over SSH.
# - Run this script as root
# - Internet access
# - Caddy available from the configured APT repositories
#
# Usage:
#   bash provision-host.sh
#
# Example (from the development host, after ssh-copy-id):
#   ssh pneuma-dev 'sudo bash /tmp/provision-host.sh'

set -euo pipefail

PNEUMA_USER="pneuma"
PNEUMA_HOME="/home/$PNEUMA_USER"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "Run this script as root."
    exit 1
fi

apt-get update
apt-get install -y \
    curl \
    git \
    sqlite3 \
    podman \
    uidmap \
    fuse-overlayfs \
    caddy

QUADLET_GENERATOR=""
for candidate in \
    /usr/lib/systemd/user-generators/podman-user-generator \
    /lib/systemd/user-generators/podman-user-generator; do
    if [[ -x "$candidate" ]]; then
        QUADLET_GENERATOR="$candidate"
        break
    fi
done

if [[ -z "$QUADLET_GENERATOR" ]]; then
    echo
    echo "Podman Quadlet user generator not found (podman-user-generator)."
    echo "Pneuma supervises runtimes with Quadlet units, which require"
    echo "Podman >= 4.4. Debian 12 ships Podman 4.3.1 without it."
    echo "Install Debian 13 (trixie)."
    exit 1
fi

if ! id -u "$PNEUMA_USER" >/dev/null 2>&1; then
    useradd --create-home --shell /bin/bash "$PNEUMA_USER"
fi

PNEUMA_UID="$(id -u "$PNEUMA_USER")"

if ! id "$PNEUMA_USER" | grep -q "100000-165535"; then
    usermod --add-subuids 100000-165535 "$PNEUMA_USER"
    usermod --add-subgids 100000-165535 "$PNEUMA_USER"
fi

passwd -l "$PNEUMA_USER"
loginctl enable-linger "$PNEUMA_USER"

install -d -o "$PNEUMA_USER" -g "$PNEUMA_USER" -m 0750 \
    /var/lib/pneuma/database \
    /var/lib/pneuma/checkouts
install -d -o "$PNEUMA_USER" -g "$PNEUMA_USER" -m 0750 "$PNEUMA_HOME/.config"
install -d -o "$PNEUMA_USER" -g "$PNEUMA_USER" -m 0750 "$PNEUMA_HOME/.config/containers"
install -d -o "$PNEUMA_USER" -g "$PNEUMA_USER" -m 0750 "$PNEUMA_HOME/.config/containers/systemd"
install -d -o "$PNEUMA_USER" -g caddy -m 0750 /etc/caddy/applications

if [[ -f /etc/caddy/Caddyfile ]]; then
    cp -a /etc/caddy/Caddyfile \
        "/etc/caddy/Caddyfile.backup.$(date +%Y%m%d%H%M%S)"
fi

cat >/etc/caddy/Caddyfile <<'EOF'
import /etc/caddy/applications/*.caddy
EOF

chown root:caddy /etc/caddy/Caddyfile
chmod 0644 /etc/caddy/Caddyfile

PROFILE="$PNEUMA_HOME/.profile"

touch "$PROFILE"
chown "$PNEUMA_USER:$PNEUMA_USER" "$PROFILE"
chmod 0644 "$PROFILE"

for line in \
    'export XDG_RUNTIME_DIR="/run/user/$(id -u)"' \
    'export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"' \
    'export PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3' \
    'export PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts' \
    'export PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications' \
    'export PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile' \
    'export PNEUMA_RUNTIME_PORT_RANGE=30000-39999' \
    'export PNEUMA_QUADLET_DIR=$HOME/.config/containers/systemd'
do
    grep -qxF "$line" "$PROFILE" || echo "$line" >>"$PROFILE"
done

systemctl enable --now caddy

caddy validate \
    --config /etc/caddy/Caddyfile \
    --adapter caddyfile

systemctl restart caddy
systemctl start "user@$PNEUMA_UID.service" || true

ROOTLESS_OUTPUT="$(runuser -u "$PNEUMA_USER" -- \
    env HOME="$PNEUMA_HOME" XDG_RUNTIME_DIR="/run/user/$PNEUMA_UID" \
    bash -c 'cd "$HOME" && podman info --format "{{.Host.Security.Rootless}}"' \
    2>/dev/null || true)"

if [[ "$ROOTLESS_OUTPUT" != "true" ]]; then
    echo
    echo "Rootless Podman is not usable by the $PNEUMA_USER user."
    echo "Expected {{.Host.Security.Rootless}} to be true; got: $ROOTLESS_OUTPUT"
    echo "Check subuid/subgid, fuse-overlayfs and linger, then rerun the script."
    exit 1
fi

echo
echo "Provisioning complete."
echo "Open a Pneuma shell:"
echo "  sudo -iu $PNEUMA_USER"
echo
echo "Next steps:"
echo "  1. Install the Pneuma binary as a separate step (see section 4 of"
echo "     docs/operations/dev-vm-tutorial.md)."
echo "  2. Run: pneuma doctor"
