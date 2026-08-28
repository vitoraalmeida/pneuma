#!/usr/bin/env bash
#
# Sync Pneuma binary to the development VM
#
# Builds the binary on the host, copies it to the VM, installs it as root,
# and validates with `pneuma version` and `pneuma doctor`.
#
# Usage:
#   scripts/dev-vm/sync-binary.sh [ssh-host]
#
# Example:
#   scripts/dev-vm/sync-binary.sh pneuma-dev
#
# Default ssh-host: pneuma-dev
# The SSH target must be root to install the binary under /usr/local/bin.
# Transport settings (forwarded port, identity, known-hosts file) come from
# the PNEUMA_SSH_* environment described in scripts/lib/remote.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"

remote_init "${1:-pneuma-dev}"
SSH_HOST="$REMOTE_HOST"

echo "==> Building Pneuma binary (release)..."
cargo build --release

echo "==> Copying binary to $SSH_HOST..."
remote_scp_to -q target/release/pneuma "$SSH_HOST":/tmp/pneuma-new

echo "==> Installing binary as root..."
remote_ssh "$SSH_HOST" 'install -o root -g root -m 0755 /tmp/pneuma-new /usr/local/bin/pneuma && rm /tmp/pneuma-new'

echo "==> Validating installation..."
remote_ssh "$SSH_HOST" '/usr/local/bin/pneuma version'

echo "==> Running pneuma doctor as pneuma user..."
remote_ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma doctor"'

echo
echo "==> Sync complete."
