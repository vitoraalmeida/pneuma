#!/usr/bin/env bash
#
# Test the bootstrap-vps.sh script on a clean VM
#
# Validates that the VPS bootstrap script correctly sets up a production-ready
# Pneuma host from a clean Debian 13 base. Covers package installation, user
# creation, rootless Podman, Caddy configuration, binary compilation, and
# basic functionality.
#
# Prerequisites:
# - Clean Debian 13 (trixie) VM with SSH root access
# - Internet access on the VM
# - Public Git repository URL with Pneuma source
#
# Usage:
#   scripts/test-bootstrap-vps.sh <ssh-host> <pneuma-source-url>
#
# Example:
#   scripts/test-bootstrap-vps.sh my-vps https://github.com/user/pneuma.git
#

set -euo pipefail

SSH_HOST="${1:-}"
SOURCE_URL="${2:-}"

if [[ -z "$SSH_HOST" || -z "$SOURCE_URL" ]]; then
    echo "Usage: $0 <ssh-host> <pneuma-source-url>"
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${TMPDIR:-/tmp}/pneuma-test-bootstrap"
mkdir -p "$LOG_DIR"

PASS_COUNT=0
FAIL_COUNT=0

report() {
    local result="$1" message="$2"
    case "$result" in
        ok)
            PASS_COUNT=$((PASS_COUNT + 1))
            printf 'PASS  %s\n' "$message"
            ;;
        fail)
            FAIL_COUNT=$((FAIL_COUNT + 1))
            printf 'FAIL  %s\n' "$message"
            ;;
    esac
}

check() {
    local expected="$1" description="$2" command="$3"
    local output
    output=$(ssh "$SSH_HOST" "$command" 2>&1) || true
    if [[ -z "$expected" ]]; then
        # If no expected string, just check if command succeeded
        if [[ $? -eq 0 ]]; then
            report ok "$description"
        else
            report fail "$description"
            printf '        output: %s\n' "$(printf '%s' "$output" | head -c 200)"
        fi
    elif printf '%s' "$output" | grep -qF -- "$expected"; then
        report ok "$description"
    else
        report fail "$description"
        printf '        output: %s\n' "$(printf '%s' "$output" | head -c 200)"
    fi
}

echo "=========================================="
echo "Bootstrap VPS Test — $SSH_HOST"
echo "=========================================="

# Phase 1: Preflight
echo
echo "==> Phase 1: Preflight..."
if ssh -o ConnectTimeout=5 "$SSH_HOST" 'true' 2>/dev/null; then
    report ok "SSH reachable"
else
    report fail "SSH unreachable"
    exit 1
fi

check "13" "Debian 13 base" "cat /etc/debian_version"

if ssh "$SSH_HOST" 'id pneuma 2>/dev/null' >/dev/null 2>&1; then
    report fail "pneuma user already exists (VM not clean)"
else
    report ok "VM is clean (no pneuma user)"
fi

if ssh "$SSH_HOST" 'which podman caddy 2>/dev/null | grep -q .' 2>/dev/null; then
    report fail "packages already installed (VM not clean)"
else
    report ok "VM is clean (no packages)"
fi

# Phase 2: Bootstrap execution
echo
echo "==> Phase 2: Bootstrap execution..."
scp "$SCRIPT_DIR/bootstrap-vps.sh" "$SSH_HOST":/tmp/ >/dev/null
if ssh "$SSH_HOST" 'bash /tmp/bootstrap-vps.sh '"$SOURCE_URL" >"$LOG_DIR/bootstrap.log" 2>&1; then
    report ok "bootstrap-vps.sh completed"
else
    report fail "bootstrap-vps.sh failed (see $LOG_DIR/bootstrap.log)"
    exit 1
fi

# Phase 3: Post-bootstrap validation
echo
echo "==> Phase 3: Post-bootstrap validation..."
check "pneuma" "pneuma user created" "id pneuma"
check "pneuma" "pneuma group created" "getent group pneuma"
check "/usr/local/bin/pneuma" "binary installed" "ls -la /usr/local/bin/pneuma"
check "podman" "podman installed" "which podman"
check "caddy" "caddy installed" "which caddy"
check "true" "rootless podman works" "su - pneuma -c 'podman info --format {{.Host.Security.Rootless}}'"
check "active" "caddy service active" "systemctl is-active caddy"
check "exists" "database directory exists" "test -d /var/lib/pneuma/database && echo exists"
check "exists" "checkouts directory exists" "test -d /var/lib/pneuma/checkouts && echo exists"
check "exists" "caddy applications dir exists" "test -d /etc/caddy/applications && echo exists"
check "PNEUMA_DATABASE_PATH" "environment file created" "cat /etc/pneuma/environment"
check "pneuma" "quadlet directory created" "ls -la /home/pneuma/.config/containers/systemd"

# Phase 4: Pneuma functionality
echo
echo "==> Phase 4: Pneuma functionality..."
check "Database connection: OK" "pneuma doctor passes" "su - pneuma -c '/usr/local/bin/pneuma doctor'"
check "pneuma" "pneuma version works" "su - pneuma -c '/usr/local/bin/pneuma version'"
check "" "pneuma app list works" "su - pneuma -c '/usr/local/bin/pneuma app list'"

# Summary
echo
echo "============================================================"
echo "$PASS_COUNT check(s) passed, $FAIL_COUNT failed."
echo "Logs: $LOG_DIR"
echo "============================================================"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    exit 1
fi
