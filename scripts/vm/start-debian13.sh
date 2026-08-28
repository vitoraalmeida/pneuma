#!/usr/bin/env bash
#
# Raw-QEMU Debian 13 VM lifecycle: create-if-absent + start-if-stopped.
#
# Owns the full create/start path for a disposable (or, in the future,
# persistent) Debian 13 guest used by the portable E2E harness. It never
# destroys anything: stop/destroy are separate operations. The guest is
# reached only through a loopback-forwarded SSH port with a per-instance
# ephemeral key; no ~/.ssh/config mutation happens.
#
# Usage:
#   scripts/vm/start-debian13.sh
#
# Settings (all optional; see scripts/vm/instance.sh for the state layout):
#   PNEUMA_VM_STATE_DIR            runtime state directory
#   PNEUMA_VM_IMAGE_URL            Debian 13 genericcloud qcow2 URL
#   PNEUMA_VM_CPUS                 vCPUs (default 2)
#   PNEUMA_VM_MEMORY_MB            guest RAM in MiB (default 4096)
#   PNEUMA_VM_SSH_PORT             explicit forwarded SSH port (default: auto)
#   PNEUMA_VM_ACCEL                auto | kvm | tcg (default auto)
#   PNEUMA_VM_HOSTNAME             guest hostname (default pneuma-e2e)
#   PNEUMA_VM_SSH_TIMEOUT_SECONDS  bounded wait for guest SSH (default 900)
#
# Cloud-init only makes the fresh guest reachable and deterministic. Pneuma
# provisioning, fixture creation, and binary installation stay in the
# existing dev-vm scripts and are NOT performed here.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

IMAGE_URL="${PNEUMA_VM_IMAGE_URL:-https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2}"
IMAGE_NAME="${IMAGE_URL##*/}"
CPUS="${PNEUMA_VM_CPUS:-2}"
MEMORY_MB="${PNEUMA_VM_MEMORY_MB:-4096}"
ACCEL_SETTING="${PNEUMA_VM_ACCEL:-auto}"
GUEST_HOSTNAME="${PNEUMA_VM_HOSTNAME:-pneuma-e2e}"

STATE_DIR="$(vm_state_dir)"
BASE_DIR="$STATE_DIR/base"
INSTANCE_DIR="$STATE_DIR/instance"
BASE_IMAGE="$BASE_DIR/$IMAGE_NAME"
PROVENANCE="$BASE_DIR/provenance"

mkdir -p "$BASE_DIR" "$INSTANCE_DIR"

# --- Preflight --------------------------------------------------------------

cidata_tool=""
if command -v cloud-localds >/dev/null 2>&1; then
	cidata_tool="cloud-localds"
elif command -v genisoimage >/dev/null 2>&1; then
	cidata_tool="genisoimage"
elif command -v mkisofs >/dev/null 2>&1; then
	cidata_tool="mkisofs"
elif command -v xorriso >/dev/null 2>&1; then
	cidata_tool="xorriso"
fi
if [[ -z $cidata_tool ]]; then
	die "no cloud-init seed tool found; install cloud-image-utils (cloud-localds) or genisoimage/mkisofs/xorriso"
fi
vm_require_commands qemu-system-x86_64 qemu-img curl ssh ssh-keygen sha512sum ss

# --- Already running? -------------------------------------------------------

pid_rc=0
pid="$(vm_qemu_pid "$INSTANCE_DIR")" || pid_rc=$?
if [[ $pid_rc -eq 0 ]]; then
	die "instance is already running (QEMU PID $pid) in $STATE_DIR; use scripts/vm/stop.sh first"
elif [[ $pid_rc -eq 2 ]]; then
	die "stale PID file points at a live non-QEMU process in $INSTANCE_DIR/qemu.pid; resolve it manually, then destroy the instance"
fi
rm -f "$INSTANCE_DIR/qemu.pid"

# --- Immutable base image ---------------------------------------------------

fetch_base_image() {
	local url_dir="${IMAGE_URL%/*}"
	local checksum_url="$url_dir/SHA512SUMS" tmp_file expected actual
	tmp_file="$BASE_DIR/.download.$$"
	trap 'rm -f "$tmp_file"' RETURN
	printf 'downloading base image: %s\n' "$IMAGE_URL"
	curl -fsSL --retry 3 -o "$tmp_file" "$IMAGE_URL"
	printf 'verifying checksum: %s\n' "$checksum_url"
	expected="$(curl -fsSL --retry 3 "$checksum_url" | awk -v name="$IMAGE_NAME" '$2 == name || $2 == "*"name {print $1}')"
	if [[ -z $expected ]]; then
		rm -f "$tmp_file"
		die "no checksum entry for $IMAGE_NAME in $checksum_url; check PNEUMA_VM_IMAGE_URL"
	fi
	actual="$(sha512sum "$tmp_file" | awk '{print $1}')"
	if [[ $actual != "$expected" ]]; then
		rm -f "$tmp_file"
		die "base image checksum mismatch: expected $expected, got $actual"
	fi
	mv "$tmp_file" "$BASE_IMAGE"
	printf '%s\nurl=%s\nsha512=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$IMAGE_URL" "$expected" >"$PROVENANCE"
}

if [[ -s $BASE_IMAGE ]]; then
	printf 'reusing cached immutable base image: %s\n' "$BASE_IMAGE"
else
	fetch_base_image
fi

# --- Acceleration -----------------------------------------------------------

# KVM is an optimization, never a correctness dependency; TCG is the fallback.
select_accel() {
	case $ACCEL_SETTING in
	auto)
		if [[ -c /dev/kvm && -w /dev/kvm ]]; then
			printf 'kvm'
		else
			printf 'tcg'
		fi
		;;
	kvm)
		if [[ ! -c /dev/kvm || ! -w /dev/kvm ]]; then
			die "PNEUMA_VM_ACCEL=kvm but /dev/kvm is not usable; grant access or use PNEUMA_VM_ACCEL=auto"
		fi
		printf 'kvm'
		;;
	tcg)
		printf 'tcg'
		;;
	*)
		die "invalid PNEUMA_VM_ACCEL=$ACCEL_SETTING (expected auto, kvm, or tcg)"
		;;
	esac
}
ACCEL_MODE="$(select_accel)"
if [[ $ACCEL_MODE == kvm ]]; then
	CPU_MODEL="host"
else
	CPU_MODEL="max"
fi
printf 'acceleration: %s (cpu=%s, cpus=%s, memory=%sMiB)\n' "$ACCEL_MODE" "$CPU_MODEL" "$CPUS" "$MEMORY_MB"

# --- Instance creation (only when absent) -----------------------------------

if [[ -s $INSTANCE_DIR/disk.qcow2 && -s $INSTANCE_DIR/env ]]; then
	printf 'reusing existing stopped instance in %s\n' "$INSTANCE_DIR"
	vm_load_instance_env "$INSTANCE_DIR"
	SSH_PORT="$PNEUMA_VM_SSH_PORT"
elif [[ -e $INSTANCE_DIR/disk.qcow2 || -e $INSTANCE_DIR/env ]]; then
	die "instance state in $INSTANCE_DIR is incomplete (disk/env mismatch); destroy the instance and start again"
else
	printf 'creating fresh instance in %s\n' "$INSTANCE_DIR"
	INSTANCE_ID="pneuma-${GUEST_HOSTNAME}-$(date -u +%Y%m%dT%H%M%SZ)-$RANDOM"
	ssh-keygen -q -t ed25519 -N '' -f "$INSTANCE_DIR/root-key" -C "$INSTANCE_ID"
	chmod 600 "$INSTANCE_DIR/root-key"
	cat >"$INSTANCE_DIR/user-data" <<EOF
#cloud-config
hostname: $GUEST_HOSTNAME
manage_etc_hosts: true
disable_root: false
ssh_pwauth: false
users:
  - name: root
    ssh_authorized_keys:
      - $(cat "$INSTANCE_DIR/root-key.pub")
EOF
	cat >"$INSTANCE_DIR/meta-data" <<EOF
instance-id: $INSTANCE_ID
local-hostname: $GUEST_HOSTNAME
EOF
	case $cidata_tool in
	cloud-localds)
		cloud-localds "$INSTANCE_DIR/seed.iso" "$INSTANCE_DIR/user-data" "$INSTANCE_DIR/meta-data"
		;;
	*)
		# cloud-localds builds a "cidata"-labeled ISO; reproduce that exactly.
		"$cidata_tool" -quiet -output "$INSTANCE_DIR/seed.iso" -volid cidata -joliet -rock \
			"$INSTANCE_DIR/user-data" "$INSTANCE_DIR/meta-data"
		;;
	esac

	allocate_ssh_port() {
		local port
		if [[ -n ${PNEUMA_VM_SSH_PORT:-} ]]; then
			printf '%s\n' "$PNEUMA_VM_SSH_PORT"
			return
		fi
		for _ in $(seq 1 50); do
			port=$((RANDOM % 10000 + 20000))
			if ! ss -ltn | awk '{print $4}' | grep -q ":$port\$"; then
				printf '%s\n' "$port"
				return
			fi
		done
		die "no free loopback port found for SSH forwarding"
	}
	SSH_PORT="$(allocate_ssh_port)"

	cat >"$INSTANCE_DIR/env" <<EOF
# Instance connection metadata (generated by start-debian13.sh).
PNEUMA_VM_SSH_HOST=127.0.0.1
PNEUMA_VM_SSH_PORT=$SSH_PORT
PNEUMA_VM_INSTANCE_ID=$INSTANCE_ID
PNEUMA_VM_GUEST_HOSTNAME=$GUEST_HOSTNAME
EOF

	qemu-img create -f qcow2 -F qcow2 -b "$BASE_IMAGE" "$INSTANCE_DIR/disk.qcow2" >/dev/null
fi

printf 'ssh forwarding: 127.0.0.1:%s -> guest:22\n' "$SSH_PORT"

# --- Launch ------------------------------------------------------------------

# Forwarded SSH binds to loopback only. The guest reboots inside this QEMU
# process; -daemonize returns once the VM is up and the PID file is written.
qemu-system-x86_64 \
	-name "pneuma-vm-instance" \
	-machine q35 \
	-accel "$ACCEL_MODE" \
	-cpu "$CPU_MODEL" \
	-smp "$CPUS" \
	-m "$MEMORY_MB" \
	-drive "file=$INSTANCE_DIR/disk.qcow2,if=virtio,format=qcow2" \
	-drive "file=$INSTANCE_DIR/seed.iso,if=virtio,media=cdrom,format=raw,readonly=on" \
	-netdev "user,id=pneuma0,hostfwd=tcp:127.0.0.1:${SSH_PORT}-:22" \
	-device virtio-net-pci,netdev=pneuma0 \
	-display none \
	-serial "file:$INSTANCE_DIR/serial.log" \
	-pidfile "$INSTANCE_DIR/qemu.pid" \
	-daemonize

printf 'QEMU started (PID %s); waiting for guest SSH\n' "$(cat "$INSTANCE_DIR/qemu.pid")"
"$SCRIPT_DIR/wait-for-ssh.sh"

# --- Guest identity verification ---------------------------------------------

vm_configure_transport "$INSTANCE_DIR"
remote_init ""
OS_ID="$(remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" '. /etc/os-release; printf "%s" "$ID"')"
OS_VERSION="$(remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" '. /etc/os-release; printf "%s" "$VERSION_ID"')"
PID1_COMM="$(remote_ssh -o BatchMode=yes "root@${REMOTE_HOST}" 'ps -p 1 -o comm=')"
if [[ $OS_ID != "debian" || $OS_VERSION != "13" ]]; then
	die "guest is not Debian 13 (ID=$OS_ID VERSION_ID=$OS_VERSION)"
fi
if [[ $PID1_COMM != "systemd" ]]; then
	die "guest PID 1 is '$PID1_COMM', expected systemd"
fi

cat <<EOF

Debian 13 instance is up.
  state dir:   $STATE_DIR
  ssh:         root@127.0.0.1 port $SSH_PORT (identity: $INSTANCE_DIR/root-key)
  stop:        scripts/vm/stop.sh
  destroy:     scripts/vm/destroy.sh
  diagnostics: scripts/vm/diagnostics.sh
EOF
