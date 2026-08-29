#!/usr/bin/env bash
#
# Full local E2E against a disposable Debian 13 QEMU instance.
#
# One command takes a fresh Debian 13 cloud image to a fully validated Pneuma
# host by orchestrating prerequisites and lifecycle only; the assertions stay
# in the existing scripts/dev-vm/ suite and test-all.sh remains the full-suite
# authority:
#
#   preflight -> start-debian13 -> provision -> sync-binary
#   -> ephemeral restricted CI key -> scripts/dev-vm/test-all.sh
#   -> summary/diagnostics on failure -> destroy
#
# test-all.sh exercises failed-candidate preservation, OCI digest deployment,
# upgrade, rollback, the REAL guest reboot with post-reboot recovery of the
# user systemd/Quadlet runtime, branch deployment, restricted CI-dispatch
# allow/deny, app lifecycle/visibility, database backup/restore, and smoke.
#
# The CI key is generated per run, installed for the pneuma user with
# restrict + forced command (never the root/provisioning key), and destroyed
# with the instance because it lives in the instance directory.
#
# Failure cleanup keeps the original exit code, runs diagnostics (state +
# serial console + test log tails), and only then destroys the instance, so
# cleanup can never convert a failure into success.
#
# Dependency boundary: the outer host needs only VM/SSH tooling, cargo, and
# this repository; every Pneuma runtime dependency lives in the guest.
#
# Usage:
#   scripts/vm/run-e2e.sh
#
# Settings: the PNEUMA_VM_* variables honored by start-debian13.sh, plus:
#   PNEUMA_VM_KEEP  set to 1 to preserve the instance after the run so the
#                   same overlay can be started again for debugging; a
#                   pre-existing instance is then refused, never destroyed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

cd "$(vm_repo_root)"

KEEP="${PNEUMA_VM_KEEP:-0}"
case $KEEP in
0 | 1) ;;
*)
	die "invalid PNEUMA_VM_KEEP=$KEEP (expected unset, 0, or 1)"
	;;
esac

STATE_DIR="$(vm_state_dir)"
INSTANCE_DIR="$STATE_DIR/instance"
LOG_DIR="${TMPDIR:-/tmp}/pneuma-test-all"
mkdir -p "$LOG_DIR"

if [[ -e $INSTANCE_DIR ]]; then
	if [[ $KEEP == 1 ]]; then
		die "PNEUMA_VM_KEEP=1 but an instance already exists in $INSTANCE_DIR; run scripts/vm/destroy.sh or unset PNEUMA_VM_KEEP"
	fi
	printf 'removing previous instance for a fresh E2E guarantee\n'
	"$SCRIPT_DIR/destroy.sh"
fi

report_failure_diagnostics() {
	printf '\nE2E failed; collecting diagnostics\n' >&2
	"$SCRIPT_DIR/diagnostics.sh" >&2 || true
	local log
	shopt -s nullglob
	for log in "$LOG_DIR"/*.log; do
		printf '\n== tail of %s ==\n' "$log" >&2
		tail -n 40 "$log" >&2 || true
	done
	shopt -u nullglob
}

cleanup() {
	local rc=$?
	if ((rc != 0)); then
		report_failure_diagnostics
	fi
	if [[ $KEEP == 1 ]]; then
		printf 'PNEUMA_VM_KEEP=1: instance preserved for debugging\n'
		printf '  state dir:   %s\n' "$STATE_DIR"
		printf '  ssh:         root@%s port %s (identity: %s)\n' \
			"${PNEUMA_SSH_HOST:-127.0.0.1}" "${PNEUMA_SSH_PORT:-<unknown>}" \
			"$INSTANCE_DIR/root-key"
		printf '  ci key:      %s\n' "$INSTANCE_DIR/ci-key"
		printf '  restart:     scripts/vm/start-debian13.sh (resumes this instance)\n'
	else
		"$SCRIPT_DIR/destroy.sh" >/dev/null 2>&1 || true
	fi
}
trap cleanup EXIT

vm_require_commands cargo ssh scp ssh-keygen

printf '== starting fresh Debian 13 instance ==\n'
"$SCRIPT_DIR/start-debian13.sh"

printf '\n== provisioning guest through existing dev-VM path ==\n'
"$SCRIPT_DIR/provision.sh"

printf '\n== building and syncing the Pneuma binary ==\n'
vm_configure_transport "$INSTANCE_DIR"
export PNEUMA_SSH_HOST PNEUMA_SSH_PORT PNEUMA_SSH_IDENTITY \
	PNEUMA_SSH_KNOWN_HOSTS_FILE PNEUMA_SSH_STRICT_HOST_KEY_CHECKING
GUEST_TARGET="root@${PNEUMA_SSH_HOST}"
"$SCRIPT_DIR/../dev-vm/sync-binary.sh" "$GUEST_TARGET"

printf '\n== installing the ephemeral restricted CI key ==\n'
CI_KEY="$INSTANCE_DIR/ci-key"
ssh-keygen -q -t ed25519 -N '' -C pneuma-e2e-ci -f "$CI_KEY"
CI_PUB="$(cat "$CI_KEY.pub")"
remote_init "$GUEST_TARGET"
remote_ssh -o BatchMode=yes "$REMOTE_HOST" 'bash -s' <<REMOTE
set -euo pipefail
install -d -m 0700 -o pneuma -g pneuma /home/pneuma/.ssh
touch /home/pneuma/.ssh/authorized_keys
line="restrict,command=\"/usr/local/bin/pneuma ci dispatch\" $CI_PUB"
if ! grep -qxF -- "\$line" /home/pneuma/.ssh/authorized_keys; then
	printf '%s\n' "\$line" >> /home/pneuma/.ssh/authorized_keys
fi
chown pneuma:pneuma /home/pneuma/.ssh/authorized_keys
chmod 0600 /home/pneuma/.ssh/authorized_keys
REMOTE
printf 'restricted CI key installed for the pneuma user (restrict + forced command)\n'

printf '\n== running the full test-all battery ==\n'
"$SCRIPT_DIR/../dev-vm/test-all.sh" "$GUEST_TARGET" "$CI_KEY"

printf '\nfull E2E on the disposable Debian host passed\n'
