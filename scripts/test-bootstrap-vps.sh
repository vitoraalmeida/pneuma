#!/usr/bin/env bash
#
# Test the bootstrap-vps.sh script on a clean VM
#
# Validates that the VPS bootstrap script correctly sets up a production-ready
# Pneuma host from a clean Debian 13 base. Covers package installation, user
# creation, rootless Podman, Caddy configuration, binary compilation, the
# restricted CI deploy key, a real deploy through the CI dispatcher, and
# idempotent re-runs.
#
# Phases:
#   1. Preflight checks
#   2. Bootstrap execution
#   3. Post-bootstrap validation
#   4. Pneuma functionality
#   5. CI deploy key + restricted SSH dispatcher
#   6. Application import + deploy pushed through the CI dispatcher
#   7. Bootstrap idempotency (installed state survives a re-run)
#
# Prerequisites:
# - Clean Debian 13 (trixie) VM with SSH root access
# - Internet access on the VM (also needed for the fixture registry)
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

CI_KEY="$LOG_DIR/ci-test-key"
CI_KEY_PUB="$CI_KEY.pub"

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
    local output rc
    output=$(ssh "$SSH_HOST" "$command" 2>&1)
    rc=$?
    if [[ -z "$expected" && $rc -eq 0 ]]; then
        report ok "$description"
    elif [[ -n "$expected" ]] && printf '%s' "$output" | grep -qF -- "$expected"; then
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

# Phase 5: CI deploy key + restricted SSH dispatcher
echo
echo "==> Phase 5: CI deploy key + restricted SSH dispatcher..."

if [[ -f "$CI_KEY" ]]; then
    echo "  (reusing local CI key from $CI_KEY)"
else
    ssh-keygen -q -t ed25519 -N "" -C "pneuma-bootstrap-test" -f "$CI_KEY"
    echo "  (test CI key generated at $CI_KEY)"
fi
scp -q "$CI_KEY_PUB" "$SSH_HOST":/tmp/pneuma-ci-test.pub

if ssh "$SSH_HOST" 'bash /tmp/bootstrap-vps.sh '"$SOURCE_URL --ci-public-key /tmp/pneuma-ci-test.pub" >"$LOG_DIR/bootstrap-ci.log" 2>&1; then
    report ok "bootstrap re-run with --ci-public-key completed"
else
    report fail "bootstrap re-run failed (see $LOG_DIR/bootstrap-ci.log)"
    exit 1
fi

check 'restrict,command="/usr/local/bin/pneuma ci dispatch"' \
    "CI key installed with restricted + forced command" \
    "cat /home/pneuma/.ssh/authorized_keys"

if VERSION_OUT=$(ssh -i "$CI_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
    "pneuma@$SSH_HOST" "version" 2>&1); then
    report ok "CI dispatcher responds to version"
else
    report fail "CI dispatcher version failed: $VERSION_OUT"
fi

if ssh -i "$CI_KEY" -o BatchMode=yes "pneuma@$SSH_HOST" "id" 2>/dev/null; then
    report fail "CI dispatcher allowed an arbitrary command (id)"
else
    report ok "CI dispatcher rejects arbitrary commands"
fi

# Phase 6: Application import + deploy through the CI dispatcher
echo
echo "==> Phase 6: Application import + deploy via CI dispatcher..."

echo "  -> ensuring local registry and insecure registry config..."
ssh "$SSH_HOST" 'mkdir -p /etc/containers/registries.conf.d
printf "[[registry]]\nlocation = \"localhost:5000\"\ninsecure = true\n" \
    > /etc/containers/registries.conf.d/pneuma-test.conf' 2>&1
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "podman start pneuma-registry 2>/dev/null || podman run -d --name pneuma-registry -p 5000:5000 docker.io/library/registry:2"' \
    2>&1 | grep -v level=warning || true

echo "  -> copying fixture source and preparing Git repository..."
scp -rq "$SCRIPT_DIR/dev-vm/fixtures" "$SSH_HOST":/var/lib/pneuma/checkouts/
ssh "$SSH_HOST" 'chown -R pneuma:pneuma /var/lib/pneuma/checkouts/fixtures'

SHA_OUT=$(ssh "$SSH_HOST" 'runuser -u pneuma -- bash -l -s' <<'REMOTE'
set -euo pipefail
cd "$HOME"
REPOS_ROOT="/var/lib/pneuma/repos"
REGISTRY="localhost:5000"
FIXTURE="/var/lib/pneuma/checkouts/fixtures/healthy-http"

rm -rf "$REPOS_ROOT"/*
mkdir -p "$REPOS_ROOT"

git init --quiet --initial-branch=main "$REPOS_ROOT/work"
cd "$REPOS_ROOT/work"
git config user.name "Pneuma Tests"
git config user.email "pneuma@example.invalid"
cp "$FIXTURE"/* .
git add -A
git commit --quiet -m "healthy-http v1.0"
SHA_MAIN=$(git rev-parse HEAD)

git checkout --quiet -b staging
sed -i 's/healthy-http v1.0/healthy-http v2.0/' server.py
git add server.py
git commit --quiet -m "healthy-http v2.0"
SHA_STAGING=$(git rev-parse HEAD)
git checkout --quiet main

git init --quiet --bare "$REPOS_ROOT/healthy-http.git"
git push --quiet "$REPOS_ROOT/healthy-http.git" main staging
git --git-dir="$REPOS_ROOT/healthy-http.git" symbolic-ref HEAD refs/heads/main

for branch in main staging; do
    git checkout --quiet "$branch"
    sha=$(git rev-parse HEAD)
    podman build --quiet --tag "$REGISTRY/healthy-http:$sha" . >/dev/null 2>&1
    podman push --tls-verify=false "$REGISTRY/healthy-http:$sha" >/dev/null 2>&1
done

echo "SHA_MAIN=$SHA_MAIN"
echo "SHA_STAGING=$SHA_STAGING"
REMOTE
)
SHA_MAIN=$(echo "$SHA_OUT" | grep '^SHA_MAIN=' | cut -d= -f2)
SHA_STAGING=$(echo "$SHA_OUT" | grep '^SHA_STAGING=' | cut -d= -f2)
echo "  main=$SHA_MAIN staging=$SHA_STAGING"

if ssh "$SSH_HOST" 'su - pneuma -c "pneuma app import file:///var/lib/pneuma/repos/healthy-http.git"' \
    >/dev/null 2>&1; then
    report ok "app imported from Git URL"
else
    report fail "app import failed"
    exit 1
fi
check "healthy-http" "app registered" \
    "su - pneuma -c 'pneuma app list'"

echo "  -> deploying via CI dispatcher (deploy healthy-http staging)..."
DEPLOY_OUT=$(ssh -i "$CI_KEY" -o BatchMode=yes "pneuma@$SSH_HOST" "deploy healthy-http staging" 2>&1) || true
if printf '%s' "$DEPLOY_OUT" | grep -q "Status: Succeeded"; then
    report ok "CI dispatcher deploy succeeded"
else
    report fail "CI dispatcher deploy failed"
    printf '        output: %s\n' "$(printf '%s' "$DEPLOY_OUT" | head -c 400)"
fi
printf '%s\n' "$DEPLOY_OUT" | grep -v level=warning | sed 's/^/        /' || true

check "Running" "application is running" \
    "su - pneuma -c 'pneuma app status healthy-http'"
check "pneuma-healthy-http.container" "quadlet unit created" \
    "ls /home/pneuma/.config/containers/systemd/"

BODY_CHECK=$(ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc '\''PORT=$(podman ps --format "{{.Ports}}" --filter name=pneuma-healthy-http | cut -d: -f2 | cut -d- -f1); curl -s "http://127.0.0.1:$PORT/"'\''' 2>&1) || true
if [[ "$BODY_CHECK" == "healthy-http v2.0" ]]; then
    report ok "staging revision served ($BODY_CHECK)"
else
    report fail "staging revision not served: $BODY_CHECK"
fi

# Phase 7: Bootstrap idempotency (installed state survives a re-run)
echo
echo "==> Phase 7: Bootstrap idempotency..."
if ssh "$SSH_HOST" 'bash /tmp/bootstrap-vps.sh '"$SOURCE_URL --ci-public-key /tmp/pneuma-ci-test.pub" >"$LOG_DIR/bootstrap-idempotent.log" 2>&1; then
    report ok "bootstrap re-run after deploy completed"
else
    report fail "bootstrap re-run failed (see $LOG_DIR/bootstrap-idempotent.log)"
    exit 1
fi
if grep -q "CI key already installed" "$LOG_DIR/bootstrap-idempotent.log"; then
    report ok "CI key idempotent (skip install on re-run)"
else
    report fail "CI key not skipped on re-run (see $LOG_DIR/bootstrap-idempotent.log)"
fi

check "pneuma" "pneuma user survives re-run" "id pneuma"
check "active" "caddy still active" "systemctl is-active caddy"
check "Running" "application survives re-run" \
    "su - pneuma -c 'pneuma app status healthy-http'"
check "Database connection: OK" "pneuma doctor passes after re-run" \
    "su - pneuma -c '/usr/local/bin/pneuma doctor'"

# Summary
echo
echo "============================================================"
echo "$PASS_COUNT check(s) passed, $FAIL_COUNT failed."
echo "Logs: $LOG_DIR"
echo "============================================================"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    exit 1
fi
