#!/usr/bin/env bash
#
# Pneuma development VM smoke script
#
# Runs the basic health checks on the VM after a deploy, mirroring the minimum
# required for a freshly provisioned host (see docs/operations/dev-vm-tutorial.md
# section "Verificação"). Run as the pneuma user on the VM.
#
# Usage:
#   bash smoke.sh [ssh-host]
#
# Example:
#   ssh pneuma-dev 'bash -s' < scripts/dev-vm/smoke.sh
#
# If an ssh-host argument is given, the script runs the checks over SSH from
# the development host. Transport settings (forwarded port, identity,
# known-hosts file) come from the PNEUMA_SSH_* environment described in
# scripts/lib/remote.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"

SSH_HOST="${1:-}"

if [[ -n "$SSH_HOST" ]]; then
	remote_init "$SSH_HOST"
	remote_ssh "$REMOTE_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && bash -s'" <"$0"
	exit $?
fi

if [[ -f "$HOME/.profile" ]]; then
	# shellcheck source=/dev/null
	source "$HOME/.profile"
fi

pneuma version
pneuma doctor
pneuma app list

echo
echo "Smoke checks passed on $(hostname)."
