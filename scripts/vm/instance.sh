#!/usr/bin/env bash
#
# Shared instance-state handling for the raw-QEMU Debian 13 VM lifecycle.
#
# This is a source-only library (like scripts/lib/remote.sh). It owns the
# runtime state-directory layout, instance metadata loading, live QEMU PID
# validation, and SSH transport wiring for the per-instance provisioning
# identity. It performs no VM effects by itself.
#
# State directory layout (all mutable state, untracked):
#
#   base/                     immutable downloaded Debian 13 base image cache
#   instance/                 per-instance mutable state
#     disk.qcow2              qcow2 overlay backed by the base image
#     seed.iso                NoCloud cloud-init seed
#     user-data, meta-data    cloud-init inputs kept for provenance
#     root-key, root-key.pub  per-instance ephemeral provisioning keypair
#     known_hosts             disposable guest host-key file
#     qemu.pid                live QEMU PID (transient)
#     serial.log              guest serial console log
#     env                     persisted connection metadata (sourced by shell)
#
# The mutable disk and instance metadata are per-instance, not per-run:
# stop/start cycles reuse them; only destroy removes them.

# shellcheck disable=SC2034
VM_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

vm_repo_root() {
	git -C "$VM_LIB_DIR" rev-parse --show-toplevel 2>/dev/null || (
		cd "$VM_LIB_DIR/../.." && pwd
	)
}

vm_default_state_dir() {
	printf '%s/.tmp/pneuma-vm' "$(vm_repo_root)"
}

# Resolve the state directory. PNEUMA_VM_STATE_DIR wins; a relative value is
# interpreted relative to the caller's working directory, the default is
# anchored at the repository root so behavior does not depend on the CWD.
vm_state_dir() {
	local dir="${PNEUMA_VM_STATE_DIR:-$(vm_default_state_dir)}"
	if [[ ! $dir == /* ]]; then
		dir="$PWD/$dir"
	fi
	printf '%s' "$dir"
}

die() {
	printf 'error: %s\n' "$*" >&2
	exit 1
}

# Fail fast listing every missing outer-host command. Never installs anything.
vm_require_commands() {
	local missing=() cmd
	for cmd in "$@"; do
		command -v "$cmd" >/dev/null 2>&1 || missing+=("$cmd")
	done
	if ((${#missing[@]} > 0)); then
		die "required commands are missing on the outer host: ${missing[*]}; install them manually (this harness never installs packages)"
	fi
}

vm_load_instance_env() {
	local instance_dir="$1"
	# shellcheck source=/dev/null
	source "$instance_dir/env"
}

# Print the live QEMU PID for the state directory owning instance_dir.
# Returns 1 when the instance is not running (missing, stale, or dead PID).
# Returns 2 when the PID file points at a live foreign process; callers must
# never kill that process.
vm_qemu_pid() {
	local instance_dir="$1"
	local pid_file="$instance_dir/qemu.pid" pid comm argv0
	[[ -s $pid_file ]] || return 1
	read -r pid <"$pid_file"
	[[ $pid =~ ^[0-9]+$ ]] || return 1
	[[ -d /proc/$pid ]] || return 1
	# comm is truncated by the kernel to 15 characters ("qemu-system-x86"),
	# so the exact launcher identity is the basename of argv[0] in cmdline.
	comm="$(cat "/proc/$pid/comm" 2>/dev/null)" || comm=""
	argv0="$(tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | head -n 1)"
	argv0="${argv0##*/}"
	if [[ $comm != "qemu-system-x86"* || $argv0 != "qemu-system-x86_64" ]]; then
		printf '%s\n' "$pid"
		return 2
	fi
	printf '%s\n' "$pid"
}

# Wire the portable transport (scripts/lib/remote.sh) to the instance:
# loopback host, persisted forwarded port, per-instance provisioning identity,
# and the disposable per-instance known-hosts file. The caller still calls
# remote_init afterwards.
vm_configure_transport() {
	local instance_dir="$1"
	[[ -s $instance_dir/env ]] || die "no instance metadata in $instance_dir; the instance must be created by start-debian13.sh"
	vm_load_instance_env "$instance_dir"
	if [[ -z "${PNEUMA_VM_SSH_PORT:-}" ]]; then
		die "instance metadata is missing PNEUMA_VM_SSH_PORT; destroy and recreate the instance"
	fi
	PNEUMA_SSH_HOST="${PNEUMA_VM_SSH_HOST:-127.0.0.1}"
	PNEUMA_SSH_PORT="$PNEUMA_VM_SSH_PORT"
	PNEUMA_SSH_IDENTITY="$instance_dir/root-key"
	PNEUMA_SSH_KNOWN_HOSTS_FILE="$instance_dir/known_hosts"
	PNEUMA_SSH_STRICT_HOST_KEY_CHECKING="accept-new"
}

# Single SSH reachability probe of the guest. Requires vm_configure_transport
# plus remote_init to have run. BatchMode ensures key-only authentication and
# a bounded connect timeout.
vm_ssh_probe() {
	remote_ssh -o BatchMode=yes -o ConnectTimeout=5 "root@${REMOTE_HOST}" true
}
