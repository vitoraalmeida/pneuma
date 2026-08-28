#!/usr/bin/env bash
#
# Bounded diagnostics for the Debian 13 QEMU instance.
#
# Exposes acceleration/resources, QEMU process state, the serial console log
# tail, SSH reachability, guest OS release and boot ID, and state-directory
# disk usage. Never prints private key material or key contents.
#
# Usage:
#   scripts/vm/diagnostics.sh
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

section() {
	printf '\n== %s ==\n' "$1"
}

section "state"
printf 'state dir: %s\n' "$STATE_DIR"
printf 'instance dir: %s\n' "$INSTANCE_DIR"
if [[ -s $INSTANCE_DIR/env ]]; then
	vm_load_instance_env "$INSTANCE_DIR"
	printf 'instance id: %s\n' "${PNEUMA_VM_INSTANCE_ID:-<missing>}"
	printf 'guest hostname: %s\n' "${PNEUMA_VM_GUEST_HOSTNAME:-<missing>}"
	printf 'ssh endpoint: root@%s port %s\n' "${PNEUMA_VM_SSH_HOST:-127.0.0.1}" "${PNEUMA_VM_SSH_PORT:-<missing>}"
else
	printf 'instance metadata: absent\n'
fi
if [[ -d $STATE_DIR/base ]]; then
	printf 'base cache: %s\n' "$STATE_DIR/base"
	if [[ -f $STATE_DIR/base/provenance ]]; then
		cat "$STATE_DIR/base/provenance"
	fi
fi

section "qemu process"
pid_rc=0
pid="$(vm_qemu_pid "$INSTANCE_DIR")" || pid_rc=$?
case $pid_rc in
0)
	printf 'qemu pid: %s (running)\n' "$pid"
	ps -o pid,etime,%cpu,%mem,args -p "$pid" || true
	;;
1)
	printf 'qemu pid: not running\n'
	;;
2)
	printf 'qemu pid: %s is a live NON-QEMU process; stale pid file\n' "$pid"
	;;
esac

section "disk usage"
if [[ -d $STATE_DIR ]]; then
	du -sh "$STATE_DIR" "$STATE_DIR/base" "$INSTANCE_DIR" 2>/dev/null || true
	df -h "$STATE_DIR" || true
fi

section "serial log tail"
if [[ -f $INSTANCE_DIR/serial.log ]]; then
	tail -n 40 "$INSTANCE_DIR/serial.log"
else
	printf 'serial log: absent\n'
fi

section "guest ssh"
if [[ ! -s $INSTANCE_DIR/env ]]; then
	printf 'skipped: no instance metadata\n'
	exit 0
fi
vm_configure_transport "$INSTANCE_DIR"
remote_init ""
if vm_ssh_probe; then
	printf 'ssh: reachable\n'
	remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" 'cat /etc/os-release; printf "boot_id: "; cat /proc/sys/kernel/random/boot_id' || true
else
	printf 'ssh: unreachable\n'
fi
