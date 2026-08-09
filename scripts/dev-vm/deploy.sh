#!/usr/bin/env bash
#
# Pneuma development VM deploy script
#
# Runs the full cycle from the development host: validate, build, send the
# binary to the VM, install it, and run pneuma doctor. The VM never pulls the
# repository nor builds Pneuma.
#
# Run on the development host, inside the Pneuma repository, after the VM has
# been provisioned (scripts/dev-vm/provision-host.sh) and SSH access to
# `pneuma-dev` works.
#
# Usage:
#   bash scripts/dev-vm/deploy.sh [ssh-host]
#
# Example:
#   bash scripts/dev-vm/deploy.sh pneuma-dev
#
# Fails immediately on any error.

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"

cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release

BINARY="$(pwd)/target/release/pneuma"

scp "$BINARY" "$SSH_HOST:/tmp/pneuma-new"

ssh "$SSH_HOST" '
    set -euo pipefail
    /tmp/pneuma-new version
    sudo install -o root -g root -m 0755 /tmp/pneuma-new /usr/local/bin/pneuma
    rm -f /tmp/pneuma-new
    pneuma doctor
'

echo
echo "Pneuma deployed to $SSH_HOST."
