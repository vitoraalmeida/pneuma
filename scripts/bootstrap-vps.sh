#!/usr/bin/env bash
#
# Pneuma VPS bootstrap script
#
# Prerequisites:
# - Debian 13 (trixie) VPS — Debian 12 ships Podman 4.3.1 without the Quadlet
#   user generator, which Pneuma requires to supervise runtimes across reboots.
# - Run this script as root
# - Internet access
# - Caddy available from the configured APT repositories
# - DNS A/AAAA records point to this VPS
# - TCP ports 80 and 443 are open
# - Nginx or another service does not own ports 80/443
#
# See docs/operations/vps-bootstrap.md for the full guide.
#
# Git prerequisites:
# - Public HTTPS repositories do not need an SSH key.
# - Private SSH repositories require a deploy key for the pneuma user.
# - On the first run, this script creates the key and prints the public key.
# - Add that key to the Git provider, then run the script again.
#
# Usage:
#   bash bootstrap-vps.sh <pneuma-source-url> [deploy-application-repository-url]
#
# Example:
#   bash bootstrap-vps.sh \
#     git@github.com:USER/pneuma.git \
#     git@github.com:USER/vitoralmeida.tech.git
#

set -euo pipefail

PNEUMA_SOURCE_URL="${1:-}"
APPLICATION_SOURCE_URL="${2:-}"

PNEUMA_USER="pneuma"
PNEUMA_HOME="/home/$PNEUMA_USER"
PNEUMA_SOURCE_PATH="$PNEUMA_HOME/pneuma"
APPLICATION_PATH="/var/lib/pneuma/checkouts/vitoralmeida.tech"
SSH_DIR="$PNEUMA_HOME/.ssh"
SSH_KEY="$SSH_DIR/id_ed25519"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "Run this script as root."
    exit 1
fi

if [[ -z "$PNEUMA_SOURCE_URL" ]]; then
    echo "Missing Pneuma source repository URL."
    exit 1
fi

apt-get update
apt-get install -y \
    build-essential \
    curl \
    git \
    pkg-config \
    libssl-dev \
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
    echo "Use Debian 13 (trixie) or newer, then rerun this script."
    exit 1
fi

if ! id "$PNEUMA_USER" >/dev/null 2>&1; then
    useradd \
        --create-home \
        --shell /bin/bash \
        "$PNEUMA_USER"
fi

PNEUMA_UID="$(id -u "$PNEUMA_USER")"

if ! grep -q "^${PNEUMA_USER}:" /etc/subuid; then
    usermod --add-subuids 100000-165535 "$PNEUMA_USER"
fi

if ! grep -q "^${PNEUMA_USER}:" /etc/subgid; then
    usermod --add-subgids 100000-165535 "$PNEUMA_USER"
fi

passwd -l "$PNEUMA_USER" || true
loginctl enable-linger "$PNEUMA_USER"

install -d \
    -o "$PNEUMA_USER" \
    -g "$PNEUMA_USER" \
    -m 0700 \
    "$SSH_DIR"

install -d \
    -o "$PNEUMA_USER" \
    -g "$PNEUMA_USER" \
    -m 0750 \
    /var/lib/pneuma/database \
    /var/lib/pneuma/checkouts \
    "$APPLICATION_PATH"

install -d \
    -o "$PNEUMA_USER" \
    -g "$PNEUMA_USER" \
    -m 0750 \
    "$PNEUMA_HOME/.config/containers/systemd"

install -d \
    -o "$PNEUMA_USER" \
    -g caddy \
    -m 0750 \
    /etc/caddy/applications

SSH_REPOSITORY=false

if [[ "$PNEUMA_SOURCE_URL" == git@* ||
      "$PNEUMA_SOURCE_URL" == ssh://* ||
      "$APPLICATION_SOURCE_URL" == git@* ||
      "$APPLICATION_SOURCE_URL" == ssh://* ]]; then
    SSH_REPOSITORY=true
fi

if [[ "$SSH_REPOSITORY" == true && ! -f "$SSH_KEY" ]]; then
    runuser -u "$PNEUMA_USER" -- \
        ssh-keygen \
        -t ed25519 \
        -f "$SSH_KEY" \
        -N "" \
        -C "pneuma@$(hostname)"

    chown "$PNEUMA_USER:$PNEUMA_USER" "$SSH_KEY" "$SSH_KEY.pub"
    chmod 0600 "$SSH_KEY"
    chmod 0644 "$SSH_KEY.pub"

    echo
    echo "An SSH key was created for the pneuma user."
    echo
    echo "Add this public key as a read-only deploy key:"
    echo
    cat "$SSH_KEY.pub"
    echo
    echo "After adding the key, run this script again."
    exit 0
fi

extract_ssh_host() {
    local url="$1" remainder host
    if [[ "$url" == ssh://* ]]; then
        remainder="${url#ssh://}"
        remainder="${remainder#*@}"
        remainder="${remainder%%/*}"
        host="${remainder%%:*}"
    elif [[ "$url" == git@* ]]; then
        host="${url#git@}"
        host="${host%%:*}"
    else
        host=""
    fi
    printf '%s' "$host"
}

if [[ "$SSH_REPOSITORY" == true ]]; then
    for host in "$(extract_ssh_host "$PNEUMA_SOURCE_URL")" "$(extract_ssh_host "$APPLICATION_SOURCE_URL")"; do
        [[ -n "$host" ]] || continue
        if ! grep -qF "$host" "$SSH_DIR/known_hosts" 2>/dev/null; then
            runuser -u "$PNEUMA_USER" -- \
                ssh-keyscan -H "$host" >>"$SSH_DIR/known_hosts"
        fi
    done

    chown "$PNEUMA_USER:$PNEUMA_USER" "$SSH_DIR/known_hosts" 2>/dev/null || true
    chmod 0600 "$SSH_DIR/known_hosts" 2>/dev/null || true
fi

if [[ ! -d "$PNEUMA_SOURCE_PATH/.git" ]]; then
    runuser -u "$PNEUMA_USER" -- \
        env \
            HOME="$PNEUMA_HOME" \
            XDG_RUNTIME_DIR="/run/user/$PNEUMA_UID" \
        git clone "$PNEUMA_SOURCE_URL" "$PNEUMA_SOURCE_PATH"
fi

if ! command -v rustup >/dev/null 2>&1; then
    runuser -u "$PNEUMA_USER" -- \
        env HOME="$PNEUMA_HOME" \
        sh -c \
        'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'
fi

runuser -u "$PNEUMA_USER" -- bash -lc "
    source '$PNEUMA_HOME/.cargo/env'
    cd '$PNEUMA_SOURCE_PATH'
    cargo build --release
"

install \
    -o root \
    -g root \
    -m 0755 \
    "$PNEUMA_SOURCE_PATH/target/release/pneuma" \
    /usr/local/bin/pneuma

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
    'export PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3' \
    'export PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts' \
    'export PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications' \
    'export PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile' \
    'export PNEUMA_RUNTIME_PORT_RANGE=30000-39999' \
    'export PNEUMA_QUADLET_DIR=$HOME/.config/containers/systemd'
do
    grep -qxF "$line" "$PROFILE" || echo "$line" >>"$PROFILE"
done

if [[ -n "$APPLICATION_SOURCE_URL" &&
      ! -e "$APPLICATION_PATH/.git" ]]; then
    runuser -u "$PNEUMA_USER" -- \
        env \
            HOME="$PNEUMA_HOME" \
            XDG_RUNTIME_DIR="/run/user/$PNEUMA_UID" \
        git clone "$APPLICATION_SOURCE_URL" "$APPLICATION_PATH"
fi

systemctl enable --now caddy

caddy validate \
    --config /etc/caddy/Caddyfile \
    --adapter caddyfile

systemctl restart caddy
systemctl start "user@$PNEUMA_UID.service" || true

ROOTLESS_OUTPUT="$(runuser -u "$PNEUMA_USER" -- \
    env HOME="$PNEUMA_HOME" XDG_RUNTIME_DIR="/run/user/$PNEUMA_UID" \
    podman info --format '{{.Host.Security.Rootless}}' 2>&1 || true)"

if [[ "$ROOTLESS_OUTPUT" != "true" ]]; then
    echo
    echo "Rootless Podman is not usable by the $PNEUMA_USER user."
    echo "Expected {{.Host.Security.Rootless}} to be true; got: $ROOTLESS_OUTPUT"
    echo "Check subuid/subgid, fuse-overlayfs and linger, then rerun the script."
    exit 1
fi

echo
echo "VPS setup completed."
echo
echo "Open a Pneuma shell:"
echo "  sudo -iu pneuma"
echo
echo "Rootless Podman is working for the pneuma user."
echo
echo "Import and deploy the application. The application name is the"
echo "[application] name declared in the pneuma.toml of the checkout:"
echo "  pneuma app import $APPLICATION_PATH"
echo "  pneuma app list"
echo "  pneuma app deploy <application-name> --image ghcr.io/owner/image@sha256:<digest>"
