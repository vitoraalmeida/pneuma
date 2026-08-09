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

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"

echo "==> Building Pneuma binary (release)..."
cargo build --release

echo "==> Copying binary to $SSH_HOST..."
scp -q target/release/pneuma "$SSH_HOST":/tmp/pneuma-new

echo "==> Installing binary as root..."
ssh "$SSH_HOST" 'sudo install -o root -g root -m 0755 /tmp/pneuma-new /usr/local/bin/pneuma && rm /tmp/pneuma-new'

echo "==> Validating installation..."
ssh "$SSH_HOST" '/usr/local/bin/pneuma version'

echo "==> Running pneuma doctor as pneuma user..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma doctor"'

echo
echo "==> Sync complete."
