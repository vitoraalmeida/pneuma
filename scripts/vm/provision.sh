#!/usr/bin/env bash
#
# Provision the disposable Debian 13 QEMU instance as a Pneuma host.
#
# The VM layer only delivers and invokes the existing provisioning path; it
# owns no runtime responsibility itself. Podman, Caddy, the pneuma user,
# subuid/subgid, linger, /var/lib/pneuma, Quadlet dirs, /etc/pneuma/environment,
# and the Caddy baseline are installed by:
#
#   scripts/dev-vm/provision-host.sh -> scripts/lib/provision-host.sh
#
# This script copies those files into the guest preserving the repository
# layout (dev-vm/ and lib/ siblings, as required by provision-host.sh), runs
# provisioning as root through the portable transport, and never sources the
# host library on the outer host. Output is streamed and captured to
# provision.log in the state directory; on failure the original exit code is
# preserved and diagnostics follow the log tail.
#
# Dependency boundary: the outer host needs only VM/SSH tooling plus this
# repository; all Pneuma runtime dependencies (Podman, Quadlet, Caddy, SQLite)
# are installed inside the Debian guest.
#
# Usage:
#   scripts/vm/provision.sh
#
# Settings (all optional):
#   PNEUMA_VM_STATE_DIR  runtime state directory

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

STATE_DIR="$(vm_state_dir)"
INSTANCE_DIR="$STATE_DIR/instance"
PROVISION_LOG="$STATE_DIR/provision.log"
GUEST_DIR="/tmp/pneuma-provision"

pid_rc=0
pid="$(vm_qemu_pid "$INSTANCE_DIR")" || pid_rc=$?
if [[ $pid_rc -eq 1 ]]; then
	die "no running instance in $STATE_DIR; run scripts/vm/start-debian13.sh first"
elif [[ $pid_rc -eq 2 ]]; then
	die "stale PID file points at a live non-QEMU process (PID $pid); resolve it manually"
fi

vm_require_commands ssh scp

vm_configure_transport "$INSTANCE_DIR"
remote_init ""

printf 'deploying provisioning files to the guest (%s layout)\n' "$GUEST_DIR"
remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" "rm -rf '$GUEST_DIR' && mkdir -p '$GUEST_DIR/dev-vm'"
remote_scp_to -q "$SCRIPT_DIR/../dev-vm/provision-host.sh" \
	"root@${REMOTE_HOST}:$GUEST_DIR/dev-vm/provision-host.sh"
remote_scp_to -q -r "$SCRIPT_DIR/../lib" "root@${REMOTE_HOST}:$GUEST_DIR/lib"

printf 'running scripts/dev-vm/provision-host.sh as root (log: %s)\n' "$PROVISION_LOG"
rc=0
remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" \
	"bash '$GUEST_DIR/dev-vm/provision-host.sh'" 2>&1 | tee "$PROVISION_LOG" || rc=$?

if ((rc != 0)); then
	printf '\nprovisioning failed (exit %s); last 40 log lines:\n' "$rc" >&2
	tail -n 40 "$PROVISION_LOG" >&2 || true
	printf '\nfull log: %s\n' "$PROVISION_LOG" >&2
	printf 'state diagnostics:\n' >&2
	"$SCRIPT_DIR/diagnostics.sh" >&2 || true
fi
exit "$rc"
