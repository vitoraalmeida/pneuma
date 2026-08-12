#!/usr/bin/env bash
#
# Pneuma VPS bootstrap script
#
# Verifies the host prerequisites and provisions a production-ready Pneuma host:
# packages, the pneuma user (no sudo), rootless Podman, Caddy and the compiled
# binary. After this script succeeds, the host is ready to import applications
# and deploy them; CI can reach it via the restricted SSH key (--ci-public-key).
#
# The script fails fast on: Debian < 13, no internet/DNS, insufficient disk
# space (< 3 GiB on /, < 1 GiB on /var) or memory (< 2 GiB), conflicting web
# servers (nginx/apache), occupied ports 80/443, and (at the end) any state
# that would break the flow.
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
# Git prerequisites:
# - Public HTTPS repositories do not need an SSH key.
# - Private SSH repositories require a deploy key for the pneuma user.
# - On the first run, this script creates the key and prints the public key.
# - Add that key to the Git provider, then run the script again.
#
# GitHub Actions deploy:
# - Pass the public key of the CI deploy key with --ci-public-key.
#   The script installs it in the pneuma user's authorized_keys (restricted
#   + forced command), so any repository in the account can SSH as pneuma
#   (no root) to run `pneuma ci dispatch`.
# - Generate the key pair on a trusted machine, not on the VPS: store the
#   private key as an account-level secret and pass the public key here.
#
# Usage:
#   bash bootstrap-vps.sh <pneuma-source-url> \
#     [--ci-public-key <path>] [--ref <ref>]
#
# --ref pins the source to a full commit SHA ([0-9a-f]{40}) or an existing Git
# tag. Branches and abbreviated SHAs are rejected. Every run (including re-runs)
# forces a detached checkout of the resolved commit before building. Without
# --ref, the remote default branch is built, as before.
#
# Example:
#   bash bootstrap-vps.sh \
#     git@github.com:USER/pneuma.git \
#     --ci-public-key ~/.ssh/pneuma-ci.pub \
#     --ref v0.3.0
#
#   bash bootstrap-vps.sh \
#     https://github.com/USER/pneuma.git \
#     --ref 0123456789abcdef0123456789abcdef01234567
#

set -euo pipefail

PNEUMA_SOURCE_URL=""
PNEUMA_REF=""
CI_PUBLIC_KEY_FILE=""

usage() {
    cat >&2 <<EOF
Usage: $0 <pneuma-source-url> [--ci-public-key <path>] [--ref <ref>]

  --ci-public-key <path>  Path to the CI deploy public key (installed with a
                          restricted, forced command for 'pneuma ci dispatch').
  --ref <ref>             Pin the source to a full commit SHA ([0-9a-f]{40}) or
                          an existing Git tag. Branches and abbreviated SHAs
                          are rejected; every run reinstalls the same commit.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --ci-public-key)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "ERROR: --ci-public-key requires a value." >&2
                usage
                exit 1
            fi
            CI_PUBLIC_KEY_FILE="$2"
            shift 2
            ;;
        --ref)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "ERROR: --ref requires a value." >&2
                usage
                exit 1
            fi
            PNEUMA_REF="$2"
            shift 2
            ;;
        --help | -h)
            usage
            exit 0
            ;;
        -*)
            echo "ERROR: Unknown option: $1" >&2
            usage
            exit 1
            ;;
        *)
            if [[ -n "$PNEUMA_SOURCE_URL" ]]; then
                echo "ERROR: Unexpected argument: $1" >&2
                usage
                exit 1
            fi
            PNEUMA_SOURCE_URL="$1"
            shift
            ;;
    esac
done

if [[ -z "$PNEUMA_SOURCE_URL" ]]; then
    echo "ERROR: Missing Pneuma source repository URL." >&2
    usage
    exit 1
fi

if [[ -n "$CI_PUBLIC_KEY_FILE" && ! -f "$CI_PUBLIC_KEY_FILE" ]]; then
    echo "ERROR: CI public key file not found: $CI_PUBLIC_KEY_FILE" >&2
    exit 1
fi

if [[ -n "$PNEUMA_REF" && ! "$PNEUMA_REF" =~ ^[0-9a-f]{40}$ ]]; then
    if [[ "$PNEUMA_REF" =~ ^[0-9a-f]+$ ]]; then
        echo "ERROR: --ref must not be an abbreviated SHA: '$PNEUMA_REF'." >&2
        echo "Use a full 40-character SHA or a Git tag name." >&2
        exit 1
    fi
    if [[ "$PNEUMA_REF" == *..* || "$PNEUMA_REF" == -* ]]; then
        echo "ERROR: invalid --ref value: '$PNEUMA_REF'." >&2
        exit 1
    fi
fi

PNEUMA_USER="pneuma"
PNEUMA_HOME="/home/$PNEUMA_USER"
PNEUMA_SOURCE_PATH="$PNEUMA_HOME/pneuma"
SSH_DIR="$PNEUMA_HOME/.ssh"
SSH_KEY="$SSH_DIR/id_ed25519"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "ERROR: Run this script as root." >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Pre-flight checks: fail fast with actionable messages before touching the
# system, so a misconfigured VPS does not leave a half-installed host behind.
# ---------------------------------------------------------------------------

echo "==> Checking prerequisites..."

DEBIAN_VERSION="$(cat /etc/debian_version)"
if ! [[ "$DEBIAN_VERSION" =~ ^13(\.|$) ]]; then
    echo "ERROR: this script requires Debian 13 (trixie), found '$DEBIAN_VERSION'."
    echo "Debian 12 ships Podman 4.3.1 without the Quadlet user generator,"
    echo "which Pneuma needs to supervise runtimes across reboots."
    exit 1
fi

if ! timeout 5 bash -c 'echo > /dev/tcp/deb.debian.org/80' 2>/dev/null; then
    echo "ERROR: no internet access to deb.debian.org."
    echo "Check the network configuration and firewall, then rerun this script."
    exit 1
fi

if ! getent hosts deb.debian.org >/dev/null 2>&1; then
    echo "ERROR: DNS resolution failed (cannot resolve deb.debian.org)."
    echo "Check /etc/resolv.conf, then rerun this script."
    exit 1
fi

AVAILABLE_ROOT_KB="$(df -Pk / 2>/dev/null | awk 'NR==2 {print $4}')"
REQUIRED_ROOT_KB=$((3 * 1024 * 1024))
if [[ -n "$AVAILABLE_ROOT_KB" && "$AVAILABLE_ROOT_KB" -lt "$REQUIRED_ROOT_KB" ]]; then
    echo "ERROR: insufficient disk space on / (root filesystem):"
    echo "  available: $((AVAILABLE_ROOT_KB / 1024 / 1024)) GiB"
    echo "  required:  3 GiB"
    echo "The bootstrap installs packages and compiles Pneuma under /."
    exit 1
fi

AVAILABLE_VAR_KB="$(df -Pk /var 2>/dev/null | awk 'NR==2 {print $4}')"
REQUIRED_VAR_KB=$((1 * 1024 * 1024))
if [[ -n "$AVAILABLE_VAR_KB" && "$AVAILABLE_VAR_KB" -lt "$REQUIRED_VAR_KB" ]]; then
    echo "ERROR: insufficient disk space on /var:"
    echo "  available: $((AVAILABLE_VAR_KB / 1024 / 1024)) GiB"
    echo "  required:  1 GiB"
    echo "Pneuma keeps its database and checkouts under /var/lib/pneuma."
    echo "(Rootless Podman stores containers under the pneuma user's home.)"
    exit 1
fi

AVAILABLE_MEM_KB="$(free -k | awk '/^Mem:/ {print $2}')"
REQUIRED_MEM_KB=$((2 * 1024 * 1024))
if [[ -n "$AVAILABLE_MEM_KB" && "$AVAILABLE_MEM_KB" -lt "$REQUIRED_MEM_KB" ]]; then
    echo "ERROR: insufficient memory to compile Pneuma:"
    echo "  available: $((AVAILABLE_MEM_KB / 1024 / 1024)) GiB"
    echo "  required:  2 GiB"
    exit 1
fi

CPU_CORES="$(nproc)"
if [[ "$CPU_CORES" -lt 2 ]]; then
    echo "ERROR: insufficient CPU cores: $CPU_CORES available, at least 2 are required."
    exit 1
fi

echo "==> Checking for conflicting services..."
for service in nginx apache2 httpd; do
    if systemctl is-active --quiet "$service" 2>/dev/null; then
        echo "ERROR: conflicting service '$service' is active."
        echo "Pneuma routes public traffic through Caddy on ports 80/443."
        echo "Stop and disable it, then rerun this script:"
        echo "  systemctl stop $service"
        echo "  systemctl disable $service"
        exit 1
    fi
done

echo "==> Checking port availability..."
# Ports must be free before Caddy is installed. On re-runs the already-managed
# Caddy legitimately listens on 80/443 and is accepted; any other owner is
# blocked. /proc/net/tcp{,6} rows: local_address(hex ip:port) st(0A=LISTEN) inode.
listening_inodes() {
    local port="$1"
    local hex_port
    hex_port="$(printf '%04X' "$port")"
    awk -v p=":$hex_port" '$4 == "0A" && $2 ~ p"$" { print $12 }' \
        /proc/net/tcp /proc/net/tcp6 2>/dev/null
}

listener_owners() {
    local inode="$1" pid fd name
    for pid in /proc/[0-9]*; do
        pid="${pid#/proc/}"
        for fd in /proc/"$pid"/fd/*; do
            if [[ "$(readlink "$fd" 2>/dev/null)" == "socket:[$inode]" ]]; then
                name="$(basename "$(readlink "/proc/$pid/exe" 2>/dev/null)" 2>/dev/null)"
                printf '%s\n' "$name"
                break
            fi
        done
    done
}

for port in 80 443; do
    inodes="$(listening_inodes "$port")"
    if [[ -z "$inodes" ]]; then
        continue
    fi

    owners=""
    while read -r inode; do
        if [[ -z "$inode" ]]; then
            continue
        fi
        owners+="$(listener_owners "$inode")"
    done <<<"$inodes"

    # shellcheck disable=SC2086 # process names carry no glob chars; intentional word split
    owners_unique="$(printf '%s\n' $owners | sort -u)"

    all_caddy=true
    while read -r owner; do
        if [[ -z "$owner" ]]; then
            continue
        fi
        if [[ "$owner" != caddy ]]; then
            all_caddy=false
        fi
    done <<<"$owners_unique"

    if [[ -n "$owners_unique" ]] && [[ "$all_caddy" == true ]] \
        && systemctl is-active --quiet caddy 2>/dev/null; then
        echo "    port $port is owned by the active managed Caddy (accepted on re-run)."
    else
        echo "ERROR: port $port is already in use by:"
        # shellcheck disable=SC2086 # intentional word split on a controlled set
        printf '%s\n' $owners_unique | sed 's/^/    /'
        echo "Stop the owning service, then rerun this script."
        echo "On a re-run only the active managed Caddy may own ports 80/443."
        exit 1
    fi
done

apt-get update
apt-get install -y \
    build-essential \
    curl \
    git \
    iproute2 \
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

SSH_REPOSITORY=false

if [[ "$PNEUMA_SOURCE_URL" == git@* ||
      "$PNEUMA_SOURCE_URL" == ssh://* ]]; then
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
    host="$(extract_ssh_host "$PNEUMA_SOURCE_URL")"
    if [[ -n "$host" ]] && ! grep -qF "$host" "$SSH_DIR/known_hosts" 2>/dev/null; then
        runuser -u "$PNEUMA_USER" -- \
            ssh-keyscan -H "$host" >>"$SSH_DIR/known_hosts"
    fi

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

RESOLVED_SHA=""
if [[ -n "$PNEUMA_REF" ]]; then
    if ! runuser -u "$PNEUMA_USER" -- \
        env HOME="$PNEUMA_HOME" GIT_TERMINAL_PROMPT=0 \
        git -C "$PNEUMA_SOURCE_PATH" fetch --prune --tags --force \
        origin '+refs/heads/*:refs/remotes/origin/*' >/dev/null 2>&1; then
        echo "ERROR: failed to fetch the Pneuma source repository." >&2
        exit 1
    fi

    if [[ "$PNEUMA_REF" =~ ^[0-9a-f]{40}$ ]]; then
        if ! runuser -u "$PNEUMA_USER" -- \
            env HOME="$PNEUMA_HOME" \
            git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify --quiet \
            "$PNEUMA_REF^{commit}" >/dev/null 2>&1; then
            runuser -u "$PNEUMA_USER" -- \
                env HOME="$PNEUMA_HOME" GIT_TERMINAL_PROMPT=0 \
                git -C "$PNEUMA_SOURCE_PATH" fetch --prune origin "$PNEUMA_REF" \
                >/dev/null 2>&1 || true
        fi
        if ! RESOLVED_SHA="$(runuser -u "$PNEUMA_USER" -- \
            env HOME="$PNEUMA_HOME" \
            git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify --quiet \
            "$PNEUMA_REF^{commit}")"; then
            echo "ERROR: --ref SHA does not resolve to a commit: $PNEUMA_REF" >&2
            exit 1
        fi
    else
        if ! runuser -u "$PNEUMA_USER" -- \
            env HOME="$PNEUMA_HOME" \
            git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify --quiet \
            "refs/tags/$PNEUMA_REF^{commit}" >/dev/null 2>&1; then
            if runuser -u "$PNEUMA_USER" -- \
                env HOME="$PNEUMA_HOME" \
                git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify --quiet \
                "refs/remotes/origin/$PNEUMA_REF" >/dev/null 2>&1; then
                echo "ERROR: --ref names a branch, not a tag: '$PNEUMA_REF'." >&2
            else
                echo "ERROR: Git tag not found: '$PNEUMA_REF'." >&2
            fi
            exit 1
        fi
        RESOLVED_SHA="$(runuser -u "$PNEUMA_USER" -- \
            env HOME="$PNEUMA_HOME" \
            git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify \
            "refs/tags/$PNEUMA_REF^{commit}")"
    fi

    runuser -u "$PNEUMA_USER" -- \
        env HOME="$PNEUMA_HOME" \
        git -C "$PNEUMA_SOURCE_PATH" checkout --force --detach "$RESOLVED_SHA"
else
    RESOLVED_SHA="$(runuser -u "$PNEUMA_USER" -- \
        env HOME="$PNEUMA_HOME" \
        git -C "$PNEUMA_SOURCE_PATH" rev-parse HEAD)"
fi

echo "==> Building Pneuma from:"
echo "    source URL: $PNEUMA_SOURCE_URL"
echo "    ref:        ${PNEUMA_REF:-remote default branch}"
echo "    SHA:        $RESOLVED_SHA"

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

mkdir -p /etc/pneuma
if ! getent group pneuma >/dev/null 2>&1; then
    groupadd pneuma
    usermod -a -G pneuma "$PNEUMA_USER"
fi
chown root:pneuma /etc/pneuma
chmod 0750 /etc/pneuma

cat >/etc/pneuma/environment <<'EOF'
# Pneuma host environment configuration
# Loaded by pneuma binary at startup
PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3
PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts
PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications
PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile
PNEUMA_RUNTIME_PORT_RANGE=30000-39999
EOF

chown root:pneuma /etc/pneuma/environment
chmod 0640 /etc/pneuma/environment

if [[ -n "$CI_PUBLIC_KEY_FILE" ]]; then
    if [[ ! -f "$CI_PUBLIC_KEY_FILE" ]]; then
        echo "CI public key file not found: $CI_PUBLIC_KEY_FILE"
        exit 1
    fi

    CI_PUBLIC_KEY="$(cat "$CI_PUBLIC_KEY_FILE")"

    if [[ ! "$CI_PUBLIC_KEY" =~ ^(ssh-ed25519|ssh-rsa|ecdsa-sha2-nistp[0-9]+)\ + ]]; then
        echo "Invalid SSH public key format in $CI_PUBLIC_KEY_FILE"
        exit 1
    fi

    AUTHORIZED_KEYS="$SSH_DIR/authorized_keys"
    touch "$AUTHORIZED_KEYS"

    if grep -qF "$CI_PUBLIC_KEY" "$AUTHORIZED_KEYS"; then
        echo "CI key already installed (skipping)."
    else
        printf 'restrict,command="/usr/local/bin/pneuma ci dispatch" %s\n' "$CI_PUBLIC_KEY" >>"$AUTHORIZED_KEYS"
        chown "$PNEUMA_USER:$PNEUMA_USER" "$AUTHORIZED_KEYS"
        chmod 0600 "$AUTHORIZED_KEYS"
        echo "CI deploy key installed for the $PNEUMA_USER user (restricted + forced command)."
    fi
fi

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
    bash -c 'cd "$HOME" && podman info --format "{{.Host.Security.Rootless}}" 2>/dev/null' || true)"

if [[ "$ROOTLESS_OUTPUT" != "true" ]]; then
    echo
    echo "Rootless Podman is not usable by the $PNEUMA_USER user."
    echo "Expected {{.Host.Security.Rootless}} to be true; got: $ROOTLESS_OUTPUT"
    echo "Check subuid/subgid, fuse-overlayfs and linger, then rerun the script."
    exit 1
fi

if [[ ! -x /usr/local/bin/pneuma ]]; then
    echo
    echo "ERROR: Pneuma binary not found at /usr/local/bin/pneuma"
    echo "The cargo build may have failed. Check the output above."
    exit 1
fi

echo
echo "Running pneuma doctor..."
if ! runuser -u "$PNEUMA_USER" -- \
    bash -c "cd '$PNEUMA_HOME' && exec env HOME='$PNEUMA_HOME' \
        XDG_RUNTIME_DIR='/run/user/$PNEUMA_UID' \
        DBUS_SESSION_BUS_ADDRESS='unix:path=/run/user/$PNEUMA_UID/bus' \
        /usr/local/bin/pneuma doctor"; then
    echo
    echo "pneuma doctor failed. Review the output above."
    exit 1
fi

echo
echo "==> Verifying final state..."
if ! systemctl is-active --quiet caddy; then
    echo "ERROR: caddy service is not active."
    echo "Review the caddy configuration and logs: journalctl -u caddy"
    exit 1
fi
if ! loginctl show-user "$PNEUMA_USER" 2>/dev/null | grep -q '^Linger=yes'; then
    echo "ERROR: linger is not enabled for the $PNEUMA_USER user."
    echo "Re-enable it with: loginctl enable-linger $PNEUMA_USER"
    exit 1
fi
if ! grep -q "^${PNEUMA_USER}:" /etc/subuid || ! grep -q "^${PNEUMA_USER}:" /etc/subgid; then
    echo "ERROR: subuid/subgid ranges are missing for the $PNEUMA_USER user."
    echo "Check /etc/subuid and /etc/subgid, then rerun this script."
    exit 1
fi
echo "✓ Final state verified"

echo
echo "Pneuma host setup completed."
echo
echo "Open a Pneuma shell:"
echo "  sudo -iu pneuma"
echo
echo "Rootless Podman is working for the pneuma user."
echo
echo "Import applications with:"
echo "  pneuma app import <git-url> --manifest <manifest-path>"
echo
echo "Example:"
echo "  pneuma app import https://github.com/owner/app --manifest deploy/staging/pneuma.toml"
echo

if [[ -n "$CI_PUBLIC_KEY_FILE" ]]; then
    echo "CI deployment identity configured for the pneuma user."
    echo
    echo "Store the private key as an account-level secret (DEPLOY_SSH_KEY)"
    echo "so all repositories in the account can deploy."
    echo
    echo "Example workflow command:"
    echo '  ssh -i <private-key> pneuma@<host> "deploy <application> <branch>"'
    echo
    echo "Test the key from a trusted machine:"
    echo '  ssh -i <private-key> pneuma@<host> "version"'
fi
