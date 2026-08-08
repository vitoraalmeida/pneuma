#!/usr/bin/env bash
#
# Pneuma VPS Delivery 7 verification script
#
# Validates the operational capabilities shipped in the "Operabilidade final"
# iteration (Quadlet runtime supervision, fixed loopback ports, doctor, and
# database backup/restore) on a configured VPS.
#
# Run as the pneuma user (sudo -iu pneuma) on a host prepared by
# scripts/bootstrap-vps.sh. Reboot survival is not exercised here: reboot the
# host and re-run this script (or `pneuma app status`) to confirm the runtime
# comes back.
#
# Usage:
#   bash scripts/verify-vps.sh [application-name]
#
# Example:
#   bash scripts/verify-vps.sh vitoralmeida-tech-prod
#

set -u

PNEUMA_USER_APP="${1:-vitoralmeida-tech-prod}"

PASS_COUNT=0
FAIL_COUNT=0

report() {
    local result="$1" message="$2"
    if [[ "$result" == "ok" ]]; then
        PASS_COUNT=$((PASS_COUNT + 1))
        printf 'PASS  %s\n' "$message"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        printf 'FAIL  %s\n' "$message"
    fi
}

check_env() {
    local variable="$1"
    if [[ -n "${!variable:-}" ]]; then
        report ok "$variable is set (${!variable})"
    else
        report fail "$variable is not set"
    fi
}

check_command_success() {
    local description="$1" log_file="$2"
    shift 2
    if "$@" >"$log_file" 2>&1; then
        report ok "$description"
    else
        report fail "$description (see $log_file)"
    fi
}

LOG_DIR="${TMPDIR:-/tmp}/pneuma-verify"
mkdir -p "$LOG_DIR"

echo "Pneuma VPS verification for application: $PNEUMA_USER_APP"
echo "============================================================"

if ! command -v pneuma >/dev/null 2>&1; then
    report fail "pneuma binary not found on PATH"
    echo
    echo "$FAIL_COUNT check(s) failed."
    exit 1
fi
report ok "pneuma binary is on PATH ($(command -v pneuma))"

check_env PNEUMA_DATABASE_PATH
check_env PNEUMA_WORKSPACE_PATH
check_env PNEUMA_CADDY_MANAGED_PATH
check_env PNEUMA_CADDYFILE_PATH
check_env PNEUMA_RUNTIME_PORT_RANGE
check_env PNEUMA_QUADLET_DIR

if [[ -d "${PNEUMA_QUADLET_DIR:-}" && -w "${PNEUMA_QUADLET_DIR:-}" ]]; then
    report ok "Quadlet directory exists and is writable ($PNEUMA_QUADLET_DIR)"
else
    report fail "Quadlet directory is missing or not writable (${PNEUMA_QUADLET_DIR:-unset})"
fi

if [[ "$(podman info --format '{{.Host.Security.Rootless}}' 2>/dev/null)" == "true" ]]; then
    report ok "Podman is rootless"
else
    report fail "Podman is not rootless"
fi

check_command_success "pneuma doctor" "$LOG_DIR/doctor.log" pneuma doctor

if pneuma app status "$PNEUMA_USER_APP" >"$LOG_DIR/status.log" 2>&1; then
    report ok "pneuma app status succeeded"
    if grep -q "Observed state: Running" "$LOG_DIR/status.log"; then
        report ok "runtime is observed as Running"
    else
        report fail "runtime is not observed as Running"
    fi
else
    report fail "pneuma app status failed"
fi

UNIT_PATTERN="pneuma-$PNEUMA_USER_APP-*.service"
if compgen -G "${PNEUMA_QUADLET_DIR:-/nonexistent}/pneuma-$PNEUMA_USER_APP-*.container" >/dev/null; then
    report ok "a Quadlet unit exists for the application"
else
    report fail "no Quadlet unit found for the application"
fi

if systemctl --user is-enabled $UNIT_PATTERN >/dev/null 2>&1; then
    report ok "Quadlet unit is enabled"
else
    report fail "Quadlet unit is not enabled"
fi

if systemctl --user is-active $UNIT_PATTERN >/dev/null 2>&1; then
    report ok "Quadlet unit is active"
else
    report fail "Quadlet unit is not active"
fi

if pneuma app stop "$PNEUMA_USER_APP" >"$LOG_DIR/stop.log" 2>&1; then
    report ok "pneuma app stop succeeded"
else
    report fail "pneuma app stop failed"
fi

if pneuma app stop "$PNEUMA_USER_APP" >/dev/null 2>&1; then
    report ok "repeated app stop is idempotent"
else
    report fail "repeated app stop failed"
fi

if pneuma app start "$PNEUMA_USER_APP" >"$LOG_DIR/start.log" 2>&1; then
    report ok "pneuma app start succeeded"
else
    report fail "pneuma app start failed"
fi

if pneuma app start "$PNEUMA_USER_APP" >/dev/null 2>&1; then
    report ok "repeated app start is idempotent"
else
    report fail "repeated app start failed"
fi

BACKUP_PATH="$LOG_DIR/pneuma-backup.sqlite3"
if pneuma database backup "$BACKUP_PATH" >"$LOG_DIR/backup.log" 2>&1; then
    report ok "pneuma database backup succeeded"
else
    report fail "pneuma database backup failed"
fi

if pneuma database restore "$BACKUP_PATH" >"$LOG_DIR/restore.log" 2>&1; then
    report ok "pneuma database restore succeeded"
else
    report fail "pneuma database restore failed"
fi

if pneuma deployment list "$PNEUMA_USER_APP" >"$LOG_DIR/deployments.log" 2>&1; then
    report ok "pneuma deployment list succeeded"
else
    report fail "pneuma deployment list failed"
fi

echo "============================================================"
echo "$PASS_COUNT check(s) passed, $FAIL_COUNT check(s) failed."
echo "Logs: $LOG_DIR"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
    exit 1
fi
