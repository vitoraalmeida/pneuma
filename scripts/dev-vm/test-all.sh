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
# with the VM provisioning key configured (~/.ssh/pneuma-dev).
# Transport settings (forwarded port, identity, known-hosts file) come from
# the PNEUMA_SSH_* environment described in scripts/lib/remote.sh. The
# restricted CI-dispatch phase uses the same endpoint with user `pneuma` and
# the explicitly supplied CI key.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"

remote_init "${1:-pneuma-dev}"
SSH_HOST="$REMOTE_HOST"
CI_KEY="${2:-$HOME/.ssh/pneuma-ci-test}"
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
	remote_remote_ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && $1'"
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

check_remote_rejected() {
	local expected="$1" description="$2" command="$3"
	local output rc
	set +e
	output=$(pneuma_cmd "$command" 2>&1)
	rc=$?
	set -e
	if [[ "$rc" -ne 0 ]] && printf '%s' "$output" | grep -qF -- "$expected"; then
		report ok "$description"
	else
		report fail "$description (exit $rc)"
		printf '        output: %s\n' "$(printf '%s' "$output" | tr '\n' ' ')"
	fi
}

ci_assert_ok() {
	local expected="$1" description="$2"
	shift 2
	local output rc
	set +e
	output=$(timeout 15 remote_ssh_as pneuma "$CI_KEY" -o BatchMode=yes "$@" 2>&1)
	rc=$?
	set -e
	if [[ "$rc" -eq 0 ]] && printf '%s' "$output" | grep -qF -- "$expected"; then
		report ok "$description"
	else
		report fail "$description (exit $rc)"
		printf '        output: %s\n' "$(printf '%s' "$output" | tr '\n' ' ')"
	fi
}

ci_assert_rejected() {
	local description="$1"
	shift
	local output rc before after
	before=$(pneuma_cmd "pneuma app deployments healthy-http" 2>&1 || true)
	set +e
	output=$(timeout 15 remote_ssh_as pneuma "$CI_KEY" -o BatchMode=yes "$@" 2>&1)
	rc=$?
	set -e
	after=$(pneuma_cmd "pneuma app deployments healthy-http" 2>&1 || true)
	if [[ "$rc" -ne 0 && "$before" == "$after" ]]; then
		report ok "$description"
	else
		report fail "$description (exit $rc or deployment history changed)"
		printf '        output: %s\n' "$(printf '%s' "$output" | tr '\n' ' ')"
	fi
}

healthy_http_port() {
	remote_ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc '\''cd $HOME && podman ps --format "{{.Ports}}" --filter name=pneuma-healthy-http'\''' |
		grep -v level=warning | cut -d: -f2 | cut -d- -f1 | head -1
}

check_http_body() {
	local expected="$1" description="$2"
	local port body
	port=$(healthy_http_port)
	if [[ -z "$port" ]]; then
		report fail "$description (no healthy-http container)"
		return
	fi
	body=$(remote_ssh "$SSH_HOST" "curl -s http://127.0.0.1:$port/")
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
if remote_ssh -o ConnectTimeout=5 "$SSH_HOST" 'true' 2>/dev/null; then
	report ok "SSH reachable ($SSH_HOST)"
else
	report fail "SSH unreachable ($SSH_HOST)"
	echo
	echo "$FAIL_COUNT check(s) failed, $PASS_COUNT passed, $SKIP_COUNT skipped."
	exit 1
fi

DEPLOY_USAGE=$(remote_ssh "$SSH_HOST" '/usr/local/bin/pneuma app deploy --help 2>&1' 2>/dev/null || true)
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
	report fail "CI key missing ($CI_KEY)"
	exit 1
fi

echo
echo "==> Phase 1: fixture cycle (e2e.sh)..."
if "$SCRIPT_DIR/e2e.sh" "$SSH_HOST" >"$LOG_DIR/e2e.log" 2>&1; then
	report ok "e2e.sh completed (reset, failed candidate, upgrade, real rollback, reboot)"
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
check_remote $'redirect-public\tRegistered\tDeployed' "redirect-public deployed with local HTTPS" "pneuma app list"
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
	ci_assert_ok "pneuma" "ci dispatch: version via restricted SSH key" version
	ci_assert_ok "Status: Succeeded" "ci dispatch: deploy healthy-http staging via restricted SSH key" "deploy healthy-http staging"
	check_http_body "healthy-http v2.0" "healthy-http serves v2.0 after CI deploy of staging"
	ci_assert_rejected "ci dispatch rejects id" id
	ci_assert_rejected "ci dispatch rejects podman" "podman ps"
	ci_assert_rejected "ci dispatch rejects file reads" "cat /etc/passwd"
	ci_assert_rejected "ci dispatch rejects branch injection" "deploy healthy-http 'staging;id'"
	ci_assert_rejected "ci dispatch rejects an empty command"
	ci_assert_rejected "ci dispatch rejects PTY allocation" -tt version
	ci_assert_rejected "ci dispatch rejects local forwarding" -N -L 127.0.0.1:18999:127.0.0.1:22
	ci_assert_rejected "ci dispatch rejects remote forwarding" -N -R 18999:127.0.0.1:22
	ci_assert_rejected "ci dispatch rejects agent forwarding" -A -N
	ci_assert_rejected "ci dispatch rejects X11 forwarding" -X -N
fi

echo
echo "==> Phase 5: administrative CLI and semantic restore..."
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
# The backup path is interpreted on the VM (pneuma_cmd runs remotely), so it
# must be valid there: a flat /tmp file exists on every fresh clone.
BACKUP_PATH="/tmp/pneuma-backup-$$.sqlite3"
check_remote "Created e2e-before-backup" "create pre-backup system" "pneuma system create e2e-before-backup --description restore-baseline"
check_remote "e2e-before-backup" "pre-backup system exists" "pneuma system list"
check_remote "Database backup:" "database backup" "pneuma database backup $BACKUP_PATH"
check_remote "Created e2e-after-backup" "create post-backup system" "pneuma system create e2e-after-backup --description restore-mutation"
check_remote "e2e-before-backup" "pre-backup system remains before restore" "pneuma system list"
check_remote "e2e-after-backup" "post-backup system exists before restore" "pneuma system list"
check_remote "Database restored from" "database restore" "pneuma database restore $BACKUP_PATH"
check_remote "System: e2e-before-backup" "restore keeps pre-backup system" "pneuma system show e2e-before-backup"
check_remote_rejected "was not found" "restore removes post-backup system" "pneuma system show e2e-after-backup"

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
