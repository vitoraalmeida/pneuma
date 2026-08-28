#!/usr/bin/env bash
#
# Full smoke path for the disposable Debian 13 QEMU instance.
#
# One command turns a fresh Debian 13 cloud image into a validated Pneuma
# host using only the existing dev-VM provisioning path:
#
#   start-debian13 -> provision -> sync-binary -> scripts/dev-vm/smoke.sh
#
# The run is ephemeral by default: any previous instance is destroyed before
# the run (fresh-guest guarantee), and the trap destroys the instance again
# on exit, success or failure. Set PNEUMA_VM_KEEP=1 to keep the instance
# after the run for debugging; in that mode a pre-existing instance is
# refused instead of destroyed.
#
# Dependency boundary: the outer host needs only VM/SSH tooling, cargo, and
# this repository (the binary is built here and copied in); the Debian guest
# receives all Pneuma runtime dependencies (Podman, Quadlet, Caddy, SQLite)
# exclusively through scripts/dev-vm/provision-host.sh. The guest never
# builds Pneuma, clones the repository, or sees a Rust toolchain.
#
# Usage:
#   scripts/vm/smoke.sh
#
# Settings: the PNEUMA_VM_* variables honored by start-debian13.sh
# (PNEUMA_VM_STATE_DIR, PNEUMA_VM_ACCEL, PNEUMA_VM_CPUS, ...), plus:
#   PNEUMA_VM_KEEP  set to 1 to preserve the instance after the run

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

cd "$(vm_repo_root)"

STATE_DIR="$(vm_state_dir)"
INSTANCE_DIR="$STATE_DIR/instance"
KEEP="${PNEUMA_VM_KEEP:-0}"
case $KEEP in
0 | 1) ;;
*)
	die "invalid PNEUMA_VM_KEEP=$KEEP (expected unset, 0, or 1)"
	;;
esac

if [[ -e $INSTANCE_DIR ]]; then
	if [[ $KEEP == 1 ]]; then
		die "PNEUMA_VM_KEEP=1 but an instance already exists in $INSTANCE_DIR; run scripts/vm/destroy.sh or unset PNEUMA_VM_KEEP"
	fi
	printf 'removing previous instance for a fresh guest guarantee\n'
	"$SCRIPT_DIR/destroy.sh"
fi

cleanup() {
	if [[ $KEEP == 1 ]]; then
		printf 'PNEUMA_VM_KEEP=1: instance kept for debugging in %s\n' "$STATE_DIR"
	else
		"$SCRIPT_DIR/destroy.sh" >/dev/null 2>&1 || true
	fi
}
trap cleanup EXIT

vm_require_commands cargo ssh scp

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

printf '\n== running existing dev-VM smoke ==\n'
"$SCRIPT_DIR/../dev-vm/smoke.sh" "$GUEST_TARGET"

printf '\nsmoke path passed\n'
