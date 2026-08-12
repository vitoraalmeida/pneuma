#!/usr/bin/env bash
#
# Pneuma VPS Delivery 7 verification script
#
# Validates the operational capabilities shipped in the "Operabilidade final"
# iteration (Quadlet runtime supervision, fixed loopback ports, doctor, and
# database backup/restore) on a configured VPS.
#
# Run as the pneuma user (sudo -iu pneuma or su - pneuma) on a host prepared by
# scripts/bootstrap-vps.sh. The script sources the user's ~/.profile so the
# PNEUMA_* environment variables apply even in a non-login shell. Reboot
# survival is not exercised here: reboot the host and re-run this script (or
# `pneuma app status`) to confirm the runtime comes back.
#
# Usage:
#   bash scripts/verify-vps.sh [application-name]
#
# Example:
#   bash scripts/verify-vps.sh vitoralmeida-tech-prod
#

set -u

PNEUMA_USER_APP="${1:-vitoralmeida-tech-prod}"

if [[ -f "$HOME/.profile" ]]; then
	# shellcheck source=/dev/null
	source "$HOME/.profile"
fi

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
	echo "$FAIL_COUNT check(s) failed, $PASS_COUNT passed, $SKIP_COUNT skipped."
	exit 1
fi
report ok "pneuma binary is on PATH ($(command -v pneuma))"

MISSING_ENVS=0
for variable in \
	PNEUMA_DATABASE_PATH \
	PNEUMA_WORKSPACE_PATH \
	PNEUMA_CADDY_MANAGED_PATH \
	PNEUMA_CADDYFILE_PATH \
	PNEUMA_RUNTIME_PORT_RANGE \
	PNEUMA_QUADLET_DIR; do
	if [[ -n "${!variable:-}" ]]; then
		report ok "$variable is set (${!variable})"
	else
		report fail "$variable is not set"
		MISSING_ENVS=1
	fi
done

if [[ "$MISSING_ENVS" -eq 1 ]]; then
	echo
	echo "PNEUMA_* environment variables are missing. The bootstrap script writes"
	echo "them to /home/<pneuma-user>/.profile, which is only sourced in login shells."
	echo "Fix options:"
	echo "  (1) Re-run:  sudo bash scripts/bootstrap-vps.sh <pneuma-source-url>"
	echo "  (2) Or append the exports manually to ~/.profile, then re-login:"
	echo "        export PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3"
	echo "        export PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts"
	echo "        export PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications"
	echo "        export PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile"
	echo "        export PNEUMA_RUNTIME_PORT_RANGE=30000-39999"
	echo "        export PNEUMA_QUADLET_DIR=\$HOME/.config/containers/systemd"
	echo
fi

if [[ -d "${PNEUMA_QUADLET_DIR:-}" && -w "${PNEUMA_QUADLET_DIR:-}" ]]; then
	report ok "Quadlet directory exists and is writable ($PNEUMA_QUADLET_DIR)"
else
	report fail "Quadlet directory is missing or not writable (${PNEUMA_QUADLET_DIR:-unset})"
fi

if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
	report ok "XDG_RUNTIME_DIR is set ($XDG_RUNTIME_DIR)"
else
	report fail "XDG_RUNTIME_DIR is not set (rootless Podman needs it; export XDG_RUNTIME_DIR=/run/user/\$(id -u))"
fi

if [[ "$(podman info --format '{{.Host.Security.Rootless}}' 2>/dev/null)" == "true" ]]; then
	report ok "Podman is rootless"
else
	report fail "Podman is not rootless (see XDG_RUNTIME_DIR check above)"
fi

check_command_success "pneuma doctor" "$LOG_DIR/doctor.log" pneuma doctor

APPLICATION_PRESENT=false
if pneuma app list >"$LOG_DIR/app-list.log" 2>&1; then
	if grep -q "$PNEUMA_USER_APP" "$LOG_DIR/app-list.log"; then
		report ok "application $PNEUMA_USER_APP is registered"
		APPLICATION_PRESENT=true
	else
		report fail "application $PNEUMA_USER_APP is not registered in the catalog"
	fi
else
	report fail "pneuma app list failed"
fi

if [[ "$APPLICATION_PRESENT" == false ]]; then
	echo
	echo "Skipping runtime checks: the application must first be imported and"
	echo "deployed. Run the GHCR flow once, for example:"
	echo "  pneuma app import https://github.com/owner/my-app --manifest deploy/staging/pneuma.toml"
	echo "  pneuma app deploy $PNEUMA_USER_APP --image ghcr.io/owner/image@sha256:<digest>"
	echo
else
	if pneuma app status "$PNEUMA_USER_APP" >"$LOG_DIR/status.log" 2>&1; then
		report ok "pneuma app status succeeded"
		if grep -q "Observed state: Running" "$LOG_DIR/status.log"; then
			report ok "runtime is observed as Running"
		else
			report fail "runtime is not observed as Running"
		fi
	else
		report fail "pneuma app status failed (see $LOG_DIR/status.log)"
	fi

	if compgen -G "${PNEUMA_QUADLET_DIR:-/nonexistent}/pneuma-$PNEUMA_USER_APP-*.container" >/dev/null; then
		report ok "a Quadlet unit exists for the application"
	else
		report fail "no Quadlet unit found (redeploy with the current binary to generate it)"
	fi

	GENERATOR_WANTS_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/systemd/generator/default.target.wants"
	if compgen -G "$GENERATOR_WANTS_DIR/pneuma-$PNEUMA_USER_APP-*.service" >/dev/null; then
		report ok "Quadlet unit is boot-enabled (generator WantedBy default.target)"
	else
		report fail "Quadlet unit is not boot-enabled (generator symlink missing in $GENERATOR_WANTS_DIR)"
	fi

	if systemctl --user is-active pneuma-$PNEUMA_USER_APP-*.service >/dev/null 2>&1; then
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

	if pneuma app deployments "$PNEUMA_USER_APP" >"$LOG_DIR/deployments.log" 2>&1; then
		report ok "pneuma app deployments succeeded"
	else
		report fail "pneuma app deployments failed"
	fi
fi

echo "============================================================"
echo "$PASS_COUNT check(s) passed, $FAIL_COUNT failed, $SKIP_COUNT skipped."
echo "Logs: $LOG_DIR"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
	exit 1
fi
