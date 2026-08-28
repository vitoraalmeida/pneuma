#!/usr/bin/env bash
#
# Destroy the Debian 13 QEMU instance (the destructive lifecycle operation).
#
# Removes ALL per-instance generated state: the mutable qcow2 overlay, the
# cloud-init seed, the ephemeral root keypair, the disposable known-hosts
# file, and the instance env/connection metadata. The downloaded immutable
# Debian base-image cache is retained. Idempotent when the instance is
# already absent.
#
# Only this script removes the mutable guest disk during normal lifecycle
# operation. E2E orchestration may call it because E2E instances are
# disposable; the launcher itself stays lifecycle-neutral.
#
# Usage:
#   scripts/vm/destroy.sh
#
# Settings (all optional):
#   PNEUMA_VM_STATE_DIR  runtime state directory

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

STATE_DIR="$(vm_state_dir)"
INSTANCE_DIR="$STATE_DIR/instance"

if [[ ! -d $STATE_DIR ]]; then
	printf 'no state directory at %s (nothing to destroy)\n' "$STATE_DIR"
	exit 0
fi
if [[ ! -d $INSTANCE_DIR && ! -d $STATE_DIR/base ]]; then
	die "refusing to destroy $STATE_DIR: it does not look like a VM state directory (no instance/ or base/)"
fi

if [[ -d $INSTANCE_DIR ]]; then
	pid_rc=0
	pid="$(vm_qemu_pid "$INSTANCE_DIR")" || pid_rc=$?
	if [[ $pid_rc -eq 0 ]]; then
		printf 'instance is running (QEMU PID %s); stopping before destroy\n' "$pid"
		"$SCRIPT_DIR/stop.sh"
	elif [[ $pid_rc -eq 2 ]]; then
		die "stale PID file points at a live non-QEMU process (PID $pid); refusing to destroy"
	fi
	rm -rf "$INSTANCE_DIR"
	printf 'per-instance state removed from %s\n' "$INSTANCE_DIR"
else
	printf 'instance already absent\n'
fi

if [[ -d $STATE_DIR/base ]]; then
	printf 'base-image cache retained: %s\n' "$STATE_DIR/base"
fi
