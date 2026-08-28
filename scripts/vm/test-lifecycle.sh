#!/usr/bin/env bash
#
# Lifecycle acceptance for the raw-QEMU Debian 13 harness.
#
# Automates the iteration-2 acceptance procedure against a throwaway state
# directory: boot a fresh Debian 13 guest, prove Debian 13 + systemd PID 1,
# reboot it (boot_id change, same QEMU process), stop/resume the same
# instance with a guest marker preserved, destroy it, and prove the next
# start is a fresh instance from the immutable base.
#
# This is a real-VM test: it downloads/caches the Debian base image and boots
# a guest. It never provisions Pneuma and never touches libvirt VMs.
#
# Usage:
#   scripts/vm/test-lifecycle.sh
#
# Settings: the PNEUMA_VM_* variables honored by start-debian13.sh
# (PNEUMA_VM_ACCEL, PNEUMA_VM_CPUS, PNEUMA_VM_MEMORY_MB, ...). The state
# directory is always a fresh temporary directory.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

STATE_DIR="$(mktemp -d /tmp/pneuma-vm-lifecycle.XXXXXX)"
export PNEUMA_VM_STATE_DIR="$STATE_DIR"
INSTANCE_DIR="$STATE_DIR/instance"

cleanup() {
	"$SCRIPT_DIR/destroy.sh" >/dev/null 2>&1 || true
	rm -rf "$STATE_DIR"
}
trap cleanup EXIT

failures=0
assert() {
	local description="$1" condition="$2"
	if eval "$condition"; then
		printf 'ok: %s\n' "$description"
	else
		printf 'FAIL: %s\n' "$description" >&2
		failures=$((failures + 1))
	fi
}

guest_boot_id() {
	remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" 'cat /proc/sys/kernel/random/boot_id'
}

printf 'lifecycle test state dir: %s\n' "$STATE_DIR"

# 1. Fresh instance boots, Debian 13 verified inside, systemd PID 1.
"$SCRIPT_DIR/start-debian13.sh"
vm_configure_transport "$INSTANCE_DIR"
remote_init ""
# Values consumed inside assert() condition strings, which shellcheck cannot
# trace through eval.
# shellcheck disable=SC2034
BOOT_ID_1="$(guest_boot_id)"
# shellcheck disable=SC2034
QEMU_PID_1="$(cat "$INSTANCE_DIR/qemu.pid")"

# 2. Guest reboot: SSH drops, returns, boot_id changes, QEMU process survives.
remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" 'systemctl reboot' || true
REBOOT_DEADLINE=$((SECONDS + 120))
until ! vm_ssh_probe; do
	if ((SECONDS >= REBOOT_DEADLINE)); then
		die "guest SSH never went away after reboot"
	fi
	sleep 3
done
printf 'guest SSH went away; waiting for it to return\n'
"$SCRIPT_DIR/wait-for-ssh.sh"
# shellcheck disable=SC2034
BOOT_ID_2="$(guest_boot_id)"
# shellcheck disable=SC2034
QEMU_PID_2="$(cat "$INSTANCE_DIR/qemu.pid")"
assert "boot id changed across reboot" '[[ $BOOT_ID_1 != "$BOOT_ID_2" ]]'
assert "same QEMU process survived the guest reboot" '[[ $QEMU_PID_1 == "$QEMU_PID_2" ]]'

# 3. stop preserves the instance; restart resumes the SAME instance.
remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" 'echo lifecycle-marker > /root/pneuma-lifecycle-marker'
"$SCRIPT_DIR/stop.sh"
assert "stop removed the live PID file" '[[ ! -e $INSTANCE_DIR/qemu.pid ]]'
assert "stop preserved the mutable overlay" '[[ -s $INSTANCE_DIR/disk.qcow2 ]]'
assert "stop preserved instance metadata" '[[ -s $INSTANCE_DIR/env ]]'

"$SCRIPT_DIR/start-debian13.sh"
# shellcheck disable=SC2034
MARKER="$(remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" 'cat /root/pneuma-lifecycle-marker')"
assert "restarted instance preserved the guest marker" '[[ $MARKER == "lifecycle-marker" ]]'

# 4. destroy removes per-instance state but keeps the base cache; the next
#    start is a fresh instance.
"$SCRIPT_DIR/destroy.sh"
assert "destroy removed per-instance state" '[[ ! -e $INSTANCE_DIR ]]'
assert "destroy retained the base-image cache" '[[ -n "$(find "$STATE_DIR/base" -name "*.qcow2" -print -quit)" ]]'

"$SCRIPT_DIR/start-debian13.sh"
vm_configure_transport "$INSTANCE_DIR"
remote_init ""
assert "fresh instance after destroy has no old marker" \
	'! remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" "test -e /root/pneuma-lifecycle-marker"'

"$SCRIPT_DIR/destroy.sh"

if ((failures > 0)); then
	printf '\n%d lifecycle assertion(s) failed\n' "$failures" >&2
	exit 1
fi
printf '\nlifecycle acceptance passed\n'
