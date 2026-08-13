#!/usr/bin/env bash
#
# Test the bootstrap-vps.sh script on a clean VM
#
# Validates that the VPS bootstrap script correctly sets up a production-ready
# Pneuma host from a clean Debian 13 base. Covers package installation, user
# creation, rootless Podman, Caddy configuration, binary compilation, the
# restricted CI deploy key and immutable re-runs. Functional E2E (fixtures,
# deploy by digest/branch, rollback, reboot) lives in scripts/dev-vm/test-all.sh;
# this script validates a clean host, bootstrap, rerun and the basic CI
# dispatcher only.
#
# Phases:
#   0. Argument validation (local, no host change)
#   1. Preflight checks
#   2. Bootstrap execution
#   3. Post-bootstrap host invariants
#   3b. Immutable --ref evidence (when --ref is passed)
#   3c. Immutable --ref rejections (branch, missing tag, unresolvable SHA)
#   4. Pneuma functionality
#   5. CI deploy key rerun + restricted SSH dispatcher
#   6. Final bootstrap idempotency (singular state survives a re-run)
#
# Prerequisites:
# - Clean Debian 13 (trixie) VM with SSH root access
# - Internet access on the VM
# - Public Git repository URL with Pneuma source
#
# Usage:
#   scripts/test-bootstrap-vps.sh <ssh-host> <pneuma-source-url> [--ref <ref>]
#
# Example:
#   scripts/test-bootstrap-vps.sh my-vps https://github.com/user/pneuma.git
#   scripts/test-bootstrap-vps.sh my-vps \
#     https://github.com/user/pneuma.git --ref 0123456789abcdef0123456789abcdef01234567
#
# The script copies scripts/bootstrap-vps.sh and scripts/lib/provision-host.sh
# to the VM; bootstrap-vps.sh sources the library by self-derived path.
#

set -euo pipefail

SSH_HOST="${1:-}"
SOURCE_URL="${2:-}"
REF=""

if [[ -z "$SSH_HOST" || -z "$SOURCE_URL" ]]; then
	echo "Usage: $0 <ssh-host> <pneuma-source-url> [--ref <ref>]"
	exit 1
fi

if [[ $# -gt 2 ]]; then
	if [[ $# -eq 4 && "$3" == "--ref" && -n "$4" ]]; then
		REF="$4"
	else
		echo "Usage: $0 <ssh-host> <pneuma-source-url> [--ref <ref>]"
		exit 1
	fi
fi

CI_SSH_HOST="${SSH_HOST#*@}"

REF_ARGS=""
if [[ -n "$REF" ]]; then
	REF_ARGS=" --ref $REF"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${TMPDIR:-/tmp}/pneuma-test-bootstrap"
mkdir -p "$LOG_DIR"
REMOTE_RUN_INDEX=0

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

# Runs a remote command, captures stdout+stderr to a log file, preserves the
# ssh exit status without `|| true` masking. Failure is reported and mirrored
# in the FAIL counter; the caller never sees a false PASS.
remote_assert() {
	local expected="$1" description="$2" command="$3"
	local log rc
	REMOTE_RUN_INDEX=$((REMOTE_RUN_INDEX + 1))
	log="$LOG_DIR/remote-$REMOTE_RUN_INDEX.log"
	set +e
	ssh "$SSH_HOST" "$command" >"$log" 2>&1
	rc=$?
	set -e
	if [[ "$rc" -ne 0 ]]; then
		report fail "$description (remote exit $rc)"
		printf '        output: %s\n' "$(head -c 200 "$log")"
		return 0
	fi
	if [[ -n "$expected" ]] && ! grep -qF -- "$expected" "$log"; then
		report fail "$description (missing '$expected')"
		printf '        output: %s\n' "$(head -c 200 "$log")"
		return 0
	fi
	report ok "$description"
	return 0
}

# Asserts a remote command is REJECTED: ssh must fail and (when an expected text
# is given) the failure output must contain it. Used for the "refuses" cases,
# never for success assertions.
remote_assert_rejected() {
	local expected="$1" description="$2" command="$3"
	local log rc
	REMOTE_RUN_INDEX=$((REMOTE_RUN_INDEX + 1))
	log="$LOG_DIR/remote-$REMOTE_RUN_INDEX.log"
	set +e
	ssh "$SSH_HOST" "$command" >"$log" 2>&1
	rc=$?
	set -e
	if [[ "$rc" -eq 0 ]]; then
		report fail "$description (remote command unexpectedly succeeded)"
		return 0
	fi
	if [[ -n "$expected" ]] && ! grep -qF -- "$expected" "$log"; then
		report fail "$description (missing rejection '$expected')"
		printf '        output: %s\n' "$(head -c 200 "$log")"
		return 0
	fi
	report ok "$description"
	return 0
}

# Run bootstrap-vps.sh locally with arguments that must be rejected before any
# host change, and assert the failure message. Also supports expected success.
assert_bootstrap() {
	local expected_rc="$1" expected_msg="$2"
	shift 2
	local output rc
	set +e
	output=$(bash "$SCRIPT_DIR/bootstrap-vps.sh" "$@" 2>&1)
	rc=$?
	set -e
	if [[ $rc -ne "$expected_rc" ]]; then
		report fail "unexpected exit code $rc (expected $expected_rc) for: $*"
		printf '        output: %s\n' "$(printf '%s' "$output" | head -c 300)"
		return
	fi
	if [[ -n "$expected_msg" ]] && ! printf '%s' "$output" | grep -qF -- "$expected_msg"; then
		report fail "missing expected message '$expected_msg' for: $*"
		printf '        output: %s\n' "$(printf '%s' "$output" | head -c 300)"
		return
	fi
	report ok "$*"
}

echo "=========================================="
echo "Bootstrap VPS Test — $SSH_HOST"
echo "=========================================="

# Phase 0: Argument validation (fails before any host change)
# Only arg-parsing rejections run locally as non-root. Git-resolution rejections
# (branch, missing tag, unresolvable SHA) run on the VM in Phase 3b.
echo
echo "==> Phase 0: Argument validation..."
assert_bootstrap 1 "Missing Pneuma source repository URL"
assert_bootstrap 1 "Unknown option: --bogus" \
	--bogus "$SOURCE_URL"
assert_bootstrap 1 "--ci-public-key requires a value" \
	"$SOURCE_URL" --ci-public-key
assert_bootstrap 1 "CI public key file not found" \
	"$SOURCE_URL" --ci-public-key /nonexistent/key.pub
assert_bootstrap 1 "--ref requires a value" \
	"$SOURCE_URL" --ref
assert_bootstrap 1 "--ref must not be an abbreviated SHA" \
	"$SOURCE_URL" --ref abcdef1
assert_bootstrap 1 "invalid --ref value" \
	"$SOURCE_URL" --ref v1..v2

# Phase 1: Preflight
echo
echo "==> Phase 1: Preflight..."
if ssh -o ConnectTimeout=5 "$SSH_HOST" 'true' 2>/dev/null; then
	report ok "SSH reachable"
else
	report fail "SSH unreachable"
	exit 1
fi

remote_assert "13" "Debian 13 base" "cat /etc/debian_version"

if [[ "${PNEUMA_BOOTSTRAP_TEST_FORCE_FALSE_ASSERTION:-}" == "1" ]]; then
	remote_assert "not-present" "forced false remote assertion" "true"
	echo
	echo "============================================================"
	echo "$PASS_COUNT check(s) passed, $FAIL_COUNT failed."
	echo "Logs: $LOG_DIR"
	echo "============================================================"
	exit 1
fi

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
scp -r "$SCRIPT_DIR/lib" "$SSH_HOST":/tmp/ >/dev/null
if ssh "$SSH_HOST" 'bash /tmp/bootstrap-vps.sh '"$SOURCE_URL$REF_ARGS" >"$LOG_DIR/bootstrap.log" 2>&1; then
	report ok "bootstrap-vps.sh completed"
else
	report fail "bootstrap-vps.sh failed (see $LOG_DIR/bootstrap.log)"
	exit 1
fi

# Phase 3: Post-bootstrap validation
echo
echo "==> Phase 3: Post-bootstrap validation..."
remote_assert "pneuma" "pneuma user created" "id pneuma"
remote_assert "pneuma" "pneuma group created" "getent group pneuma"
remote_assert "/usr/local/bin/pneuma" "binary installed" "ls -la /usr/local/bin/pneuma"
remote_assert "podman" "podman installed" "which podman"
remote_assert "caddy" "caddy installed" "which caddy"
remote_assert "true" "rootless podman works" "runuser -u pneuma -- env HOME=/home/pneuma XDG_RUNTIME_DIR=/run/user/\$(id -u pneuma) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/\$(id -u pneuma)/bus bash -c 'cd /home/pneuma && podman info --format \"{{.Host.Security.Rootless}}\"'"
remote_assert "active" "caddy service active" "systemctl is-active caddy"

# Explicit user invariants: UID/home/shell, locked password, no sudo group.
remote_assert "" "pneuma has a numeric UID" "id -u pneuma | grep -Eq '^[0-9]+$'"
remote_assert "/home/pneuma" "pneuma home is /home/pneuma" "getent passwd pneuma | cut -d: -f6"
remote_assert "/bin/bash" "pneuma shell is /bin/bash" "getent passwd pneuma | cut -d: -f7"
remote_assert "L" "pneuma password is locked" "passwd -S pneuma | awk '{print \$2}'"
remote_assert "" "pneuma is not in the sudo group" "! id -Gn pneuma | grep -qw sudo"
remote_assert "" "subuids have one safe pneuma allocation" "awk -F: '\$0 ~ /^[[:space:]]*(#|\$)/ { next } \$2 !~ /^[0-9]+$/ || \$3 !~ /^[0-9]+$/ || \$2 == 0 || \$3 == 0 { exit 1 } \$1 == \"pneuma\" { count++; start=\$2 + 0; end=start + \$3; if (\$3 < 65536) exit 1; next } { other_count++; other_start[other_count]=\$2 + 0; other_end[other_count]=other_start[other_count] + \$3 } END { if (count != 1) exit 1; for (i=1; i<=other_count; i++) if (start < other_end[i] && other_start[i] < end) exit 1 }' /etc/subuid"
remote_assert "" "subgids have one safe pneuma allocation" "awk -F: '\$0 ~ /^[[:space:]]*(#|\$)/ { next } \$2 !~ /^[0-9]+$/ || \$3 !~ /^[0-9]+$/ || \$2 == 0 || \$3 == 0 { exit 1 } \$1 == \"pneuma\" { count++; start=\$2 + 0; end=start + \$3; if (\$3 < 65536) exit 1; next } { other_count++; other_start[other_count]=\$2 + 0; other_end[other_count]=other_start[other_count] + \$3 } END { if (count != 1) exit 1; for (i=1; i<=other_count; i++) if (start < other_end[i] && other_start[i] < end) exit 1 }' /etc/subgid"
if ! SUBUID_ENTRY="$(ssh "$SSH_HOST" "grep '^pneuma:' /etc/subuid")"; then
	report fail "could not record subuid range for rerun comparison"
	SUBUID_ENTRY=""
fi
if ! SUBGID_ENTRY="$(ssh "$SSH_HOST" "grep '^pneuma:' /etc/subgid")"; then
	report fail "could not record subgid range for rerun comparison"
	SUBGID_ENTRY=""
fi

remote_assert "yes" "linger enabled for pneuma" "loginctl show-user pneuma -p Linger --value"

# Directory ownership and modes.
remote_assert "pneuma pneuma" ".ssh owner:group" "stat -c '%U %G' /home/pneuma/.ssh"
remote_assert "700" ".ssh mode 0700" "stat -c '%a' /home/pneuma/.ssh"
remote_assert "pneuma pneuma" "database dir owner:group" "stat -c '%U %G' /var/lib/pneuma/database"
remote_assert "750" "database dir mode 0750" "stat -c '%a' /var/lib/pneuma/database"
remote_assert "pneuma pneuma" "checkouts dir owner:group" "stat -c '%U %G' /var/lib/pneuma/checkouts"
remote_assert "750" "checkouts dir mode 0750" "stat -c '%a' /var/lib/pneuma/checkouts"
remote_assert "pneuma pneuma 750" ".config owner:group:mode" "stat -c '%U %G %a' /home/pneuma/.config"
remote_assert "pneuma pneuma 750" "containers config owner:group:mode" "stat -c '%U %G %a' /home/pneuma/.config/containers"
remote_assert "pneuma pneuma 750" "Quadlet dir owner:group:mode" "stat -c '%U %G %a' /home/pneuma/.config/containers/systemd"
remote_assert "pneuma caddy" "caddy applications dir owner:group" "stat -c '%U %G' /etc/caddy/applications"
remote_assert "750" "caddy applications dir mode 0750" "stat -c '%a' /etc/caddy/applications"
remote_assert "root pneuma" "/etc/pneuma owner:group" "stat -c '%U %G' /etc/pneuma"
remote_assert "750" "/etc/pneuma mode 0750" "stat -c '%a' /etc/pneuma"
remote_assert "root pneuma" "environment file owner:group" "stat -c '%U %G' /etc/pneuma/environment"
remote_assert "640" "environment file mode 0640" "stat -c '%a' /etc/pneuma/environment"
remote_assert "root root" "binary owner:group" "stat -c '%U %G' /usr/local/bin/pneuma"
remote_assert "755" "binary mode 0755" "stat -c '%a' /usr/local/bin/pneuma"
remote_assert "root caddy" "Caddyfile owner:group" "stat -c '%U %G' /etc/caddy/Caddyfile"
remote_assert "644" "Caddyfile mode 0644" "stat -c '%a' /etc/caddy/Caddyfile"

# Canonical environment: /etc/pneuma/environment is the source of truth.
remote_assert "PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3" "environment database path" "cat /etc/pneuma/environment"
remote_assert "PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts" "environment workspace path" "cat /etc/pneuma/environment"
remote_assert "PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications" "environment caddy managed path" "cat /etc/pneuma/environment"
remote_assert "PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile" "environment caddyfile path" "cat /etc/pneuma/environment"
remote_assert "PNEUMA_RUNTIME_PORT_RANGE=30000-39999" "environment runtime port range" "cat /etc/pneuma/environment"

# Caddy valid and Quadlet generator present.
remote_assert "Valid configuration" "caddy validates its Caddyfile" "caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile"
remote_assert 'http:// {' "Caddyfile has the unmatched HTTP fallback" "cat /etc/caddy/Caddyfile"
remote_assert 'respond "Not Found" 404' "Caddyfile returns generic HTTP 404" "cat /etc/caddy/Caddyfile"
remote_assert "" "quadlet generator discovered" "test -x /usr/lib/systemd/user-generators/podman-user-generator || test -x /lib/systemd/user-generators/podman-user-generator"
remote_assert "pneuma" "quadlet directory created" "ls -la /home/pneuma/.config/containers/systemd"

# Phase 3b: Immutable --ref evidence (only when a ref was requested)
check_vm_sha() {
	local expected="$1" actual
	if ! actual=$(ssh "$SSH_HOST" "runuser -u pneuma -- git -C /home/pneuma/pneuma rev-parse HEAD" 2>/dev/null); then
		return 1
	fi
	[[ "$actual" == "$expected" ]]
}
if [[ -n "$REF" ]]; then
	echo
	echo "==> Phase 3b: Immutable --ref evidence..."
	RESOLVED_SHA="$(grep '^    SHA: ' "$LOG_DIR/bootstrap.log" | head -1 | awk '{print $2}')"
	if [[ -z "$RESOLVED_SHA" ]]; then
		report fail "bootstrap log records no resolved SHA (see $LOG_DIR/bootstrap.log)"
	else
		report ok "bootstrap log records resolved SHA $RESOLVED_SHA"
		if check_vm_sha "$RESOLVED_SHA"; then
			report ok "source checkout detached at $RESOLVED_SHA"
		else
			report fail "source checkout not pinned at $RESOLVED_SHA"
		fi
	fi
fi

# Phase 3c: Immutable --ref rejections (resolved after clone on the VM)
echo
echo "==> Phase 3c: Immutable --ref rejections..."
if ! DEFAULT_BRANCH="$(ssh "$SSH_HOST" \
	'git -C /home/pneuma/pneuma symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed "s#refs/remotes/origin/##"' 2>/dev/null)"; then
	DEFAULT_BRANCH=""
fi
DEFAULT_BRANCH="${DEFAULT_BRANCH:-main}"
remote_assert_rejected "--ref names a branch, not a tag: '$DEFAULT_BRANCH'" \
	"branch passed to --ref is rejected" \
	"bash /tmp/bootstrap-vps.sh $SOURCE_URL --ref $DEFAULT_BRANCH"
remote_assert_rejected "Git tag not found" \
	"missing tag passed to --ref is rejected" \
	"bash /tmp/bootstrap-vps.sh $SOURCE_URL --ref no-such-pneuma-tag"
remote_assert_rejected "--ref SHA does not resolve to a commit" \
	"unresolvable SHA passed to --ref is rejected" \
	"bash /tmp/bootstrap-vps.sh $SOURCE_URL --ref 0123456789abcdef0123456789abcdef01234567"

# Phase 4: Pneuma functionality
echo
echo "==> Phase 4: Pneuma functionality..."
remote_assert "Database connection: OK" "pneuma doctor passes" "runuser -u pneuma -- env HOME=/home/pneuma XDG_RUNTIME_DIR=/run/user/\$(id -u pneuma) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/\$(id -u pneuma)/bus bash -c 'cd /home/pneuma && /usr/local/bin/pneuma doctor'"
remote_assert "pneuma" "pneuma version works" "runuser -u pneuma -- env HOME=/home/pneuma /usr/local/bin/pneuma version"
remote_assert "" "pneuma app list works" "runuser -u pneuma -- env HOME=/home/pneuma XDG_RUNTIME_DIR=/run/user/\$(id -u pneuma) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/\$(id -u pneuma)/bus /usr/local/bin/pneuma app list"

# Phase 5: CI deploy key rerun + restricted SSH dispatcher
echo
echo "==> Phase 5: CI deploy key + restricted SSH dispatcher..."

if [[ -f "$CI_KEY" ]]; then
	echo "  (reusing local CI key from $CI_KEY)"
else
	ssh-keygen -q -t ed25519 -N "" -C "pneuma-bootstrap-test" -f "$CI_KEY"
	echo "  (test CI key generated at $CI_KEY)"
fi
scp -q "$CI_KEY_PUB" "$SSH_HOST":/tmp/pneuma-ci-test.pub

if ssh "$SSH_HOST" 'bash /tmp/bootstrap-vps.sh '"$SOURCE_URL --ci-public-key /tmp/pneuma-ci-test.pub$REF_ARGS" >"$LOG_DIR/bootstrap-ci.log" 2>&1; then
	report ok "bootstrap re-run with --ci-public-key completed"
else
	report fail "bootstrap re-run failed (see $LOG_DIR/bootstrap-ci.log)"
	exit 1
fi

remote_assert 'restrict,command="/usr/local/bin/pneuma ci dispatch"' \
	"CI key installed with restricted + forced command" \
	"cat /home/pneuma/.ssh/authorized_keys"

CI_PUBLIC="$(grep -v '^#' "$CI_KEY_PUB")"
remote_assert "1" "CI key appears exactly once in authorized_keys" \
	"count=\$(grep -cF '$CI_PUBLIC' /home/pneuma/.ssh/authorized_keys); printf '%s\\n' \"\$count\"; test \"\$count\" -eq 1"

ci_assert_ok() {
	local expected="$1" description="$2" command="$3"
	local log rc
	REMOTE_RUN_INDEX=$((REMOTE_RUN_INDEX + 1))
	log="$LOG_DIR/ci-$REMOTE_RUN_INDEX.log"
	set +e
	ssh -i "$CI_KEY" -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
		"pneuma@$CI_SSH_HOST" "$command" >"$log" 2>&1
	rc=$?
	set -e
	if [[ "$rc" -ne 0 ]]; then
		report fail "$description (remote exit $rc)"
		printf '        output: %s\n' "$(head -c 200 "$log")"
		return 0
	fi
	if [[ -n "$expected" ]] && ! grep -qF -- "$expected" "$log"; then
		report fail "$description (missing '$expected')"
		printf '        output: %s\n' "$(head -c 200 "$log")"
		return 0
	fi
	report ok "$description"
	return 0
}

ci_assert_rejected() {
	local expected="$1" description="$2" command="$3"
	local log rc
	REMOTE_RUN_INDEX=$((REMOTE_RUN_INDEX + 1))
	log="$LOG_DIR/ci-$REMOTE_RUN_INDEX.log"
	set +e
	ssh -i "$CI_KEY" -o BatchMode=yes \
		"pneuma@$CI_SSH_HOST" "$command" >"$log" 2>&1
	rc=$?
	set -e
	if [[ "$rc" -eq 0 ]]; then
		report fail "$description (command executed)"
		printf '        output: %s\n' "$(head -c 200 "$log")"
		return 0
	fi
	if ! grep -qF -- "$expected" "$log"; then
		report fail "$description (missing rejection '$expected')"
		printf '        output: %s\n' "$(head -c 200 "$log")"
		return 0
	fi
	report ok "$description"
	return 0
}

profile_assert_single() {
	local line="$1" description="$2"
	remote_assert "1" "$description" \
		"count=\$(grep -cxF '$line' /home/pneuma/.profile); printf '%s\\n' \"\$count\"; test \"\$count\" -eq 1"
}

ci_assert_ok "pneuma" "CI dispatcher responds to version" "version"
ci_assert_rejected "unknown command: id" \
	"CI dispatcher rejects arbitrary command (id)" "id"
ci_assert_rejected "unknown command: podman" \
	"CI dispatcher rejects arbitrary command (podman ps)" "podman ps"

# Phase 6: Final bootstrap idempotency (singular state survives a re-run)
echo
echo "==> Phase 6: Bootstrap idempotency..."
CADDY_BACKUP_COUNT="$(ssh "$SSH_HOST" "find /etc/caddy -maxdepth 1 -name 'Caddyfile.backup.*' -type f | wc -l")"
if ssh "$SSH_HOST" 'bash /tmp/bootstrap-vps.sh '"$SOURCE_URL --ci-public-key /tmp/pneuma-ci-test.pub$REF_ARGS" >"$LOG_DIR/bootstrap-idempotent.log" 2>&1; then
	report ok "final bootstrap re-run after deploy completed"
else
	report fail "final bootstrap re-run failed (see $LOG_DIR/bootstrap-idempotent.log)"
	exit 1
fi
if grep -q "CI key already installed" "$LOG_DIR/bootstrap-idempotent.log"; then
	report ok "CI key idempotent (skip install on re-run)"
else
	report fail "CI key not skipped on re-run (see $LOG_DIR/bootstrap-idempotent.log)"
fi
if [[ -n "$REF" && -n "${RESOLVED_SHA:-}" ]]; then
	if check_vm_sha "$RESOLVED_SHA"; then
		report ok "rerun reinstalls the same pinned commit ($RESOLVED_SHA)"
	else
		report fail "rerun moved the source checkout off $RESOLVED_SHA"
	fi
fi

remote_assert "pneuma" "pneuma user survives re-run" "id pneuma"
remote_assert "active" "caddy still active" "systemctl is-active caddy"
remote_assert "$CADDY_BACKUP_COUNT" "unchanged Caddyfile creates no backup on re-run" \
	"count=\$(find /etc/caddy -maxdepth 1 -name 'Caddyfile.backup.*' -type f | wc -l); printf '%s\\n' \"\$count\"; test \"\$count\" -eq '$CADDY_BACKUP_COUNT'"
remote_assert "" "invalid Caddy candidate preserves active configuration" \
	"before=\$(sha256sum /etc/caddy/Caddyfile); printf 'invalid {\\n' >/etc/caddy/applications/pneuma-invalid-test.caddy; source /tmp/lib/provision-host.sh; if provision_caddy_baseline; then rm -f /etc/caddy/applications/pneuma-invalid-test.caddy; exit 1; fi; rm -f /etc/caddy/applications/pneuma-invalid-test.caddy; test \"\$before\" = \"\$(sha256sum /etc/caddy/Caddyfile)\"; systemctl is-active --quiet caddy"

remote_assert "1" "single CI key after re-run" \
	"count=\$(grep -cF '$CI_PUBLIC' /home/pneuma/.ssh/authorized_keys); printf '%s\\n' \"\$count\"; test \"\$count\" -eq 1"
profile_assert_single 'export XDG_RUNTIME_DIR="/run/user/$(id -u)"' \
	"profile has one XDG_RUNTIME_DIR line after re-run"
profile_assert_single 'export DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u)/bus"' \
	"profile has one DBUS_SESSION_BUS_ADDRESS line after re-run"
profile_assert_single 'export PNEUMA_DATABASE_PATH=/var/lib/pneuma/database/pneuma.sqlite3' \
	"profile has one database path line after re-run"
profile_assert_single 'export PNEUMA_WORKSPACE_PATH=/var/lib/pneuma/checkouts' \
	"profile has one workspace path line after re-run"
profile_assert_single 'export PNEUMA_CADDY_MANAGED_PATH=/etc/caddy/applications' \
	"profile has one caddy managed path line after re-run"
profile_assert_single 'export PNEUMA_CADDYFILE_PATH=/etc/caddy/Caddyfile' \
	"profile has one Caddyfile path line after re-run"
profile_assert_single 'export PNEUMA_RUNTIME_PORT_RANGE=30000-39999' \
	"profile has one runtime port range line after re-run"
profile_assert_single 'export PNEUMA_QUADLET_DIR=$HOME/.config/containers/systemd' \
	"profile has one Quadlet path line after re-run"
if [[ -n "$SUBUID_ENTRY" ]]; then
	remote_assert "$SUBUID_ENTRY" "stable subuid range after re-run" "grep '^pneuma:' /etc/subuid"
fi
if [[ -n "$SUBGID_ENTRY" ]]; then
	remote_assert "$SUBGID_ENTRY" "stable subgid range after re-run" "grep '^pneuma:' /etc/subgid"
fi

remote_assert "Database connection: OK" "pneuma doctor passes after re-run" \
	"runuser -u pneuma -- env HOME=/home/pneuma XDG_RUNTIME_DIR=/run/user/\$(id -u pneuma) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/\$(id -u pneuma)/bus bash -c 'cd /home/pneuma && /usr/local/bin/pneuma doctor'"

# Summary
echo
echo "============================================================"
echo "$PASS_COUNT check(s) passed, $FAIL_COUNT failed."
echo "Logs: $LOG_DIR"
echo "============================================================"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
	exit 1
fi
