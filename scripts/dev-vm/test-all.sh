#!/usr/bin/env bash
#
# Full end-to-end test battery for the development VM
#
# Orchestrates every functional area of Pneuma against the development VM,
# from a clean slate: the fixture cycle (e2e.sh: reset, rebuild, deploy by
# digest, health, upgrade, rollback, reboot, recovery), the branch-based Git
# flow (test-branch-deploy.sh), the restricted-key CI dispatcher, the
# administrative CLI (systems, visibility, lifecycle, deployments, database
# backup/restore) and a final smoke. Each check is reported as PASS/FAIL/SKIP
# with a summary and non-zero exit code on failure.
#
# Prerequisites:
#   scripts/dev-vm/provision-host.sh   # host provisioned (user pneuma, Podman)
#   scripts/dev-vm/sync-binary.sh      # binary must know --branch and ci dispatch
#   CI key installed on the VM:        # ~/.ssh/pneuma-ci-test.pub in
#                                      #   /home/pneuma/.ssh/authorized_keys with
#                                      #   restrict,command="pneuma ci dispatch"
#   scripts/dev-vm/rebuild-fixtures.sh # registry + fixture images built
#
# Usage:
#   scripts/dev-vm/test-all.sh [ssh-host] [ci-key]
#
# Defaults: ssh-host = pneuma-dev, ci-key = ~/.ssh/pneuma-ci-test
#
# The reboot inside e2e.sh requires root over SSH; run this script from a host
# with the VM provisioning key configured (docs/operations/e2e-testing.md).

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"
CI_KEY="${2:-$HOME/.ssh/pneuma-ci-test}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${TMPDIR:-/tmp}/pneuma-test-all"
mkdir -p "$LOG_DIR"

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

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
        skip)
            SKIP_COUNT=$((SKIP_COUNT + 1))
            printf 'SKIP  %s\n' "$message"
            ;;
    esac
}

pneuma_cmd() {
    ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && $1'"
}

check_remote() {
    local expected="$1" description="$2" command="$3"
    local output
    output=$(pneuma_cmd "$command" 2>&1 | grep -v level=warning || true)
    if printf '%s' "$output" | grep -qF -- "$expected"; then
        report ok "$description"
    else
        report fail "$description"
        printf '        output: %s\n' "$(printf '%s' "$output" | tr '\n' ' ')"
    fi
}

check_ci() {
    local expected="$1" description="$2" command="$3"
    local output
    output=$(ssh -i "$CI_KEY" pneuma@"$SSH_HOST" "$command" 2>&1 | grep -v level=warning || true)
    if printf '%s' "$output" | grep -qF -- "$expected"; then
        report ok "$description"
    else
        report fail "$description"
        printf '        output: %s\n' "$(printf '%s' "$output" | tr '\n' ' ')"
    fi
}

healthy_http_port() {
    ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc '\''cd $HOME && podman ps --format "{{.Ports}}" --filter name=pneuma-healthy-http'\''' \
        | grep -v level=warning | cut -d: -f2 | cut -d- -f1 | head -1
}

check_http_body() {
    local expected="$1" description="$2"
    local port body
    port=$(healthy_http_port)
    if [[ -z "$port" ]]; then
        report fail "$description (no healthy-http container)"
        return
    fi
    body=$(ssh "$SSH_HOST" "curl -s http://127.0.0.1:$port/")
    if [[ "$body" == "$expected" ]]; then
        report ok "$description"
    else
        report fail "$description (expected '$expected', got: '$body')"
    fi
}

echo "=========================================="
echo "Pneuma Full E2E Battery — $SSH_HOST"
echo "=========================================="

echo
echo "==> Preflight..."
if ssh -o ConnectTimeout=5 "$SSH_HOST" 'true' 2>/dev/null; then
    report ok "SSH reachable ($SSH_HOST)"
else
    report fail "SSH unreachable ($SSH_HOST)"
    echo
    echo "$FAIL_COUNT check(s) failed, $PASS_COUNT passed, $SKIP_COUNT skipped."
    exit 1
fi

DEPLOY_USAGE=$(ssh "$SSH_HOST" '/usr/local/bin/pneuma app deploy 2>&1' 2>/dev/null || true)
if printf '%s' "$DEPLOY_USAGE" | grep -qF -- '--branch'; then
    report ok "installed binary supports --branch"
else
    report fail "installed binary lacks --branch; run scripts/dev-vm/sync-binary.sh first"
    echo
    echo "$FAIL_COUNT check(s) failed, $PASS_COUNT passed, $SKIP_COUNT skipped."
    exit 1
fi

if [[ -f "$CI_KEY" ]]; then
    report ok "CI key present ($CI_KEY)"
else
    report fail "CI key missing ($CI_KEY); see docs/operations/e2e-testing.md"
fi

echo
echo "==> Phase 1: fixture cycle (e2e.sh)..."
if "$SCRIPT_DIR/e2e.sh" "$SSH_HOST" >"$LOG_DIR/e2e.log" 2>&1; then
    report ok "e2e.sh completed (reset, rebuild, deploy, upgrade, rollback, reboot, recovery)"
else
    report fail "e2e.sh failed (see $LOG_DIR/e2e.log)"
    echo
    echo "$FAIL_COUNT check(s) failed, $PASS_COUNT passed, $SKIP_COUNT skipped."
    exit 1
fi

echo
echo "==> Phase 2: fixture outcome asserts..."
check_remote $'healthy-http\tRegistered\tDeployed' "healthy-http is registered and deployed" "pneuma app list"
for fixture in unhealthy-http slow-start bad-port; do
    check_remote "$(printf '%s\tRegistered\tNot deployed' "$fixture")" "$fixture is registered and not deployed (expected health/config failure)" "pneuma app list"
done
APP_LIST=$(pneuma_cmd "pneuma app list" 2>&1 | grep -v level=warning || true)
if printf '%s' "$APP_LIST" | grep -qF $'redirect-public\tRegistered\tDeployed'; then
    check_remote "Observed state: Running" "redirect-public deployed (Caddy local_certs enabled)" "pneuma app status redirect-public"
else
    report skip "redirect-public public exposure (requires Caddy local_certs; see dev-vm-tutorial.md section 7)"
fi
check_remote "Observed state: Running" "healthy-http is Running after reboot" "pneuma app status healthy-http"
check_http_body "healthy-http v1.0" "healthy-http serves v1.0 after rollback and reboot"

echo
echo "==> Phase 3: branch-based Git flow (test-branch-deploy.sh)..."
if "$SCRIPT_DIR/test-branch-deploy.sh" "$SSH_HOST" >"$LOG_DIR/branch-deploy.log" 2>&1; then
    report ok "test-branch-deploy.sh completed (import by Git URL, deploy --branch main/staging)"
else
    report fail "test-branch-deploy.sh failed (see $LOG_DIR/branch-deploy.log)"
    echo
    echo "$FAIL_COUNT check(s) failed, $PASS_COUNT passed, $SKIP_COUNT skipped."
    exit 1
fi

echo
echo "==> Phase 4: restricted-key CI dispatcher..."
if [[ -f "$CI_KEY" ]]; then
    check_ci "pneuma" "ci dispatch: version via restricted SSH key" "version"
    check_ci "Status: Succeeded" "ci dispatch: deploy healthy-http staging via restricted SSH key" "deploy healthy-http staging"
    check_http_body "healthy-http v2.0" "healthy-http serves v2.0 after CI deploy of staging"
fi

echo
echo "==> Phase 5: administrative CLI..."
check_remote "Created e2e-cli-test" "system create" "pneuma system create e2e-cli-test --description e2e-battery"
check_remote "e2e-cli-test" "system list contains created system" "pneuma system list"
check_remote "System: fixtures-test" "system show resolves manifest system" "pneuma system show fixtures-test"
check_remote "Visibility for healthy-http: Internal" "app visibility set (idempotent to internal)" "pneuma app visibility set healthy-http internal"
check_remote "Stopped healthy-http" "app stop" "pneuma app stop healthy-http"
check_remote "Stopped healthy-http" "app stop is idempotent" "pneuma app stop healthy-http"
check_remote "Started healthy-http" "app start" "pneuma app start healthy-http"
check_remote "Started healthy-http" "app start is idempotent" "pneuma app start healthy-http"
check_remote "Observed state: Running" "app status reflects Running" "pneuma app status healthy-http"
check_remote "Deployments for healthy-http:" "app deployments lists history" "pneuma app deployments healthy-http"
BACKUP_PATH="$LOG_DIR/pneuma-backup-$$.sqlite3"
check_remote "Database backup:" "database backup" "pneuma database backup $BACKUP_PATH"
check_remote "Database restored from" "database restore" "pneuma database restore $BACKUP_PATH"

echo
echo "==> Phase 6: smoke..."
if "$SCRIPT_DIR/smoke.sh" "$SSH_HOST" >"$LOG_DIR/smoke.log" 2>&1; then
    report ok "smoke.sh passed (version, doctor, app list)"
else
    report fail "smoke.sh failed (see $LOG_DIR/smoke.log)"
fi

echo
echo "============================================================"
echo "$PASS_COUNT check(s) passed, $FAIL_COUNT failed, $SKIP_COUNT skipped."
echo "Logs: $LOG_DIR"
echo "============================================================"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    exit 1
fi
