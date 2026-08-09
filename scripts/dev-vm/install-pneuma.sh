#!/usr/bin/env bash
#
# Pneuma development VM install script
#
# Installs a prebuilt Pneuma binary into the development VM without requiring a
# repository checkout or a cargo build on the VM. The VM is the integration
# target only; building always happens on the development host.
#
# Run as root on the VM after scripts/dev-vm/provision-host.sh.
#
# Usage:
#   bash install-pneuma.sh <binary-path>
#
# Example:
#   bash install-pneuma.sh /tmp/pneuma-new
#
# The binary is validated on the VM (version + doctor) before and after the
# install so a broken build never replaces a working runtime.

set -euo pipefail

PNEUMA_BINARY="${1:-}"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "Run this script as root."
    exit 1
fi

if [[ -z "$PNEUMA_BINARY" ]]; then
    echo "Missing Pneuma binary path."
    echo "Usage: bash install-pneuma.sh <binary-path>"
    exit 1
fi

if [[ ! -f "$PNEUMA_BINARY" ]]; then
    echo "Pneuma binary not found: $PNEUMA_BINARY"
    exit 1
fi

if ! "$PNEUMA_BINARY" version >/dev/null 2>&1; then
    echo "The provided binary failed its version check; aborting."
    exit 1
fi

install -o root -g root -m 0755 "$PNEUMA_BINARY" /usr/local/bin/pneuma

runuser -u pneuma -- \
    env HOME="/home/pneuma" XDG_RUNTIME_DIR="/run/user/$(id -u pneuma)" \
    bash -lc 'pneuma version && pneuma doctor'

echo
echo "Pneuma installed on the VM: $(pneuma version)"
