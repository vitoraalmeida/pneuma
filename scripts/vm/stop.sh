#!/usr/bin/env bash
#
# Stop the Debian 13 QEMU instance without destroying it.
#
# stop != destroy: this preserves the mutable qcow2 overlay, the cloud-init
# seed and instance metadata, the per-instance keypair and known-hosts file,
# and the persisted connection configuration, so a later start-debian13.sh
# with the same state directory resumes the SAME instance. Only transient
# live-process state (a stale qemu.pid) is removed.
#
# The shutdown is graceful when the guest is reachable, bounded, and only
# falls back to terminating QEMU after the timeout. A PID file pointing at a
# live non-QEMU process is never killed.
#
# Usage:
#   scripts/vm/stop.sh
#
# Settings (all optional):
#   PNEUMA_VM_STATE_DIR             runtime state directory
#   PNEUMA_VM_STOP_TIMEOUT_SECONDS  bounded wait for graceful poweroff (180)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

STATE_DIR="$(vm_state_dir)"
INSTANCE_DIR="$STATE_DIR/instance"

pid_rc=0
pid="$(vm_qemu_pid "$INSTANCE_DIR")" || pid_rc=$?
if [[ $pid_rc -eq 1 ]]; then
	rm -f "$INSTANCE_DIR/qemu.pid"
	printf 'instance is not running (nothing to stop)\n'
	exit 0
elif [[ $pid_rc -eq 2 ]]; then
	die "stale PID file points at a live non-QEMU process (PID $pid); refusing to stop"
fi

# Graceful guest poweroff when reachable; ignored when the guest is already
# going down or unreachable.
if [[ -s $INSTANCE_DIR/env ]]; then
	vm_configure_transport "$INSTANCE_DIR"
	remote_init ""
	if vm_ssh_probe; then
		printf 'requesting guest poweroff\n'
		remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" 'systemctl poweroff' || true
	else
		printf 'guest SSH unreachable; falling back to QEMU termination\n'
	fi
fi

TIMEOUT_SECONDS="${PNEUMA_VM_STOP_TIMEOUT_SECONDS:-180}"
DEADLINE=$((SECONDS + TIMEOUT_SECONDS))
printf 'waiting for QEMU (PID %s) to exit (up to %ss)\n' "$pid" "$TIMEOUT_SECONDS"
while [[ -d /proc/$pid ]]; do
	if ((SECONDS >= DEADLINE)); then
		printf 'graceful shutdown timed out; terminating QEMU\n' >&2
		kill -TERM "$pid" 2>/dev/null || true
		grace_deadline=$((SECONDS + 30))
		while [[ -d /proc/$pid ]] && ((SECONDS < grace_deadline)); do
			sleep 1
		done
		if [[ -d /proc/$pid ]]; then
			kill -KILL "$pid" 2>/dev/null || true
		fi
		break
	fi
	sleep 3
done

rm -f "$INSTANCE_DIR/qemu.pid"
if [[ ! -s $INSTANCE_DIR/disk.qcow2 ]]; then
	die "instance overlay is missing after stop; unexpected state in $INSTANCE_DIR"
fi
printf 'instance stopped; overlay and metadata preserved for restart\n'
