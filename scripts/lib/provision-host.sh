#!/usr/bin/env bash
#
# Shared Pneuma host provisioning invariants
#
# Concrete Bash functions used by both the production VPS bootstrap
# (scripts/bootstrap-vps.sh) and the development VM provisioning
# (scripts/dev-vm/provision-host.sh). Both callers must apply the same host
# invariants: runtime packages, Quadlet generator discovery, pneuma user,
# subordinate IDs, linger, host directories, host environment, the Caddy
# baseline, the user manager, and rootless Podman verification.
#
# This library is intentionally caller-agnostic:
#   - It never clones, resolves refs, builds, installs the Pneuma binary,
#     installs the CI key, or runs `pneuma doctor`; those operations belong to
#     the production bootstrap caller.
#   - It never sets up VM-only machinery; those operations belong to the VM
#     caller.
#
# Callers source this file by absolute path derived from ${BASH_SOURCE[0]} so
# it works regardless of the working directory, and provide the target user:
#
#   PNEUMA_USER=pneuma
#   PNEUMA_HOME=/home/pneuma
#
# Both variables default to those exact values when unset. Caller functions
# must run as root, in this order:
#
#   provision_runtime_packages   apt runtime packages (apt-get update)
#   require_quadlet_generator    fail fast when the Quadlet generator is absent
#   provision_pneuma_user        create the user when missing
#   provision_subordinate_ids    add missing subuid/subgid ranges for the user
#   provision_linger             lock the password and enable linger
#   provision_host_directories   runtime dirs, Quadlet dir, Caddy apps, /etc/pneuma
#   provision_host_environment   /etc/pneuma/environment plus ~/.profile exports
#   provision_caddy_baseline     baseline Caddyfile, validate and start Caddy
#   start_pneuma_user_manager    start user@UID.service
#   verify_rootless_podman       assert rootless Podman works for the user

set -euo pipefail

PNEUMA_USER="${PNEUMA_USER:-pneuma}"
PNEUMA_HOME="${PNEUMA_HOME:-/home/$PNEUMA_USER}"

# Installs the common runtime package set. Callers install their own extra
# packages (compiler toolchain, sqlite3) after this function returns.
provision_runtime_packages() {
    echo "==> Installing runtime packages..."
    apt-get update
    apt-get install -y \
        curl \
        git \
        podman \
        uidmap \
        fuse-overlayfs \
        caddy
}

# Discovers the Quadlet user generator on the Debian generator paths and fails
# fast with an actionable message when it is missing. Sets QUADLET_GENERATOR to
# the discovered executable path for diagnostics.
require_quadlet_generator() {
    local candidate
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
}

# Creates the pneuma user with home /home/pneuma and /bin/bash when it does not
# exist yet. Existing users are left untouched.
provision_pneuma_user() {
    if ! id "$PNEUMA_USER" >/dev/null 2>&1; then
        useradd \
            --create-home \
            --shell /bin/bash \
            "$PNEUMA_USER"
    fi
}

# Adds the standard subuid/subgid ranges when the user has no entry in
# /etc/subuid or /etc/subgid. Existing entries are left untouched.
provision_subordinate_ids() {
    if ! grep -q "^${PNEUMA_USER}:" /etc/subuid; then
        usermod --add-subuids 100000-165535 "$PNEUMA_USER"
    fi
    if ! grep -q "^${PNEUMA_USER}:" /etc/subgid; then
        usermod --add-subgids 100000-165535 "$PNEUMA_USER"
    fi
}

# Locks the password and enables linger for the pneuma user. Locking is
# idempotent on a rerun, so a locked password must not fail the script.
provision_linger() {
    passwd -l "$PNEUMA_USER" || true
    loginctl enable-linger "$PNEUMA_USER"
}

# Creates the persistent host directories with the canonical owners and modes:
# the runtime data dirs under /var/lib/pneuma, the Quadlet user dirs under the
# user's home, the Caddy applications dir, and /etc/pneuma (group pneuma).
provision_host_directories() {
    install -d \
        -o "$PNEUMA_USER" \
        -g "$PNEUMA_USER" \
        -m 0750 \
        /var/lib/pneuma/database \
        /var/lib/pneuma/checkouts

    install -d -o "$PNEUMA_USER" -g "$PNEUMA_USER" -m 0750 "$PNEUMA_HOME/.config"
    install -d -o "$PNEUMA_USER" -g "$PNEUMA_USER" -m 0750 "$PNEUMA_HOME/.config/containers"
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

    # /etc/pneuma holds the canonical host environment shared by both callers,
    # so the pneuma group must exist even when the user was pre-created.
    if ! getent group "$PNEUMA_USER" >/dev/null 2>&1; then
        groupadd "$PNEUMA_USER"
        usermod -a -G "$PNEUMA_USER" "$PNEUMA_USER"
    fi
    install -d -o root -g "$PNEUMA_USER" -m 0750 /etc/pneuma
}

# Writes the canonical /etc/pneuma/environment and keeps the session exports in
# the user's ~/.profile present exactly once. The $(id -u) and $HOME fragments
# in the profile lines are literal on purpose.
provision_host_environment() {
    cat >/etc/pneuma/environment <<'EOF'
# Pneuma host environment configuration
# Loaded by pneuma binary at startup
PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3
PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts
PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications
PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile
PNEUMA_RUNTIME_PORT_RANGE=30000-39999
EOF

    chown root:"$PNEUMA_USER" /etc/pneuma/environment
    chmod 0640 /etc/pneuma/environment

    local profile="$PNEUMA_HOME/.profile" line
    touch "$profile"
    chown "$PNEUMA_USER:$PNEUMA_USER" "$profile"
    chmod 0644 "$profile"

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
        grep -qxF "$line" "$profile" || echo "$line" >>"$profile"
    done
}

# Ensures the baseline Caddyfile imports only Pneuma-managed fragments, keeps
# Caddy owned/readable correctly, and starts the service after validation.
# Backup semantics are preserved verbatim; atomic content-sensitive replacement
# is out of scope for this library.
provision_caddy_baseline() {
    if [[ -f /etc/caddy/Caddyfile ]]; then
        cp -a /etc/caddy/Caddyfile \
            "/etc/caddy/Caddyfile.backup.$(date +%Y%m%d%H%M%S)"
    fi

    cat >/etc/caddy/Caddyfile <<'EOF'
import /etc/caddy/applications/*.caddy
EOF

    chown root:caddy /etc/caddy/Caddyfile
    chmod 0644 /etc/caddy/Caddyfile

    systemctl enable --now caddy

    caddy validate \
        --config /etc/caddy/Caddyfile \
        --adapter caddyfile

    systemctl restart caddy
}

# Starts the pneuma user manager so the rootless runtime can be exercised.
start_pneuma_user_manager() {
    systemctl start "user@$(id -u "$PNEUMA_USER").service" || true
}

# Asserts the pneuma user can run rootless Podman. Runs from the user's home
# with the rootless session environment so root's working directory or missing
# session bus cannot break the assertion.
verify_rootless_podman() {
    local uid rootless_output
    uid="$(id -u "$PNEUMA_USER")"
    rootless_output="$(runuser -u "$PNEUMA_USER" -- \
        env HOME="$PNEUMA_HOME" XDG_RUNTIME_DIR="/run/user/$uid" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" \
        bash -c 'cd "$HOME" && podman info --format "{{.Host.Security.Rootless}}"' \
        2>/dev/null || true)"

    if [[ "$rootless_output" != "true" ]]; then
        echo
        echo "Rootless Podman is not usable by the $PNEUMA_USER user."
        echo "Expected {{.Host.Security.Rootless}} to be true; got: $rootless_output"
        echo "Check subuid/subgid, fuse-overlayfs and linger, then rerun the script."
        exit 1
    fi
}