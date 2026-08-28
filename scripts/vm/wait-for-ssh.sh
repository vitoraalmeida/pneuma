#!/usr/bin/env bash
#
# Wait, bounded, until the Debian 13 guest answers SSH.
#
# Reads the per-instance connection metadata (loopback host, forwarded port,
# provisioning identity, disposable known-hosts file) from the state
# directory. Host-key verification uses StrictHostKeyChecking=accept-new
# against the dedicated per-instance file only.
#
# Usage:
#   scripts/vm/wait-for-ssh.sh
#
# Settings (all optional):
#   PNEUMA_VM_STATE_DIR            runtime state directory
#   PNEUMA_VM_SSH_TIMEOUT_SECONDS  bounded wait (default 900; TCG is slow)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"
# shellcheck source=instance.sh
source "$SCRIPT_DIR/instance.sh"

STATE_DIR="$(vm_state_dir)"
INSTANCE_DIR="$STATE_DIR/instance"
[[ -s $INSTANCE_DIR/env ]] || die "no instance metadata in $INSTANCE_DIR; run scripts/vm/start-debian13.sh first"

vm_configure_transport "$INSTANCE_DIR"
remote_init ""

TIMEOUT_SECONDS="${PNEUMA_VM_SSH_TIMEOUT_SECONDS:-900}"
DEADLINE=$((SECONDS + TIMEOUT_SECONDS))
printf 'waiting for SSH on 127.0.0.1:%s (up to %ss)\n' "$PNEUMA_VM_SSH_PORT" "$TIMEOUT_SECONDS"

attempts=0
until vm_ssh_probe; do
	attempts=$((attempts + 1))
	if ((SECONDS >= DEADLINE)); then
		printf 'error: guest SSH not reachable after %ss; inspect scripts/vm/diagnostics.sh\n' "$TIMEOUT_SECONDS" >&2
		exit 1
	fi
	if ((attempts % 12 == 0)); then
		printf 'still waiting (%ss elapsed)\n' "$SECONDS"
	fi
	sleep 5
done

printf 'guest SSH is reachable\n'
