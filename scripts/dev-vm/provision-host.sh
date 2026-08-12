#!/usr/bin/env bash
#
# Pneuma development VM host provisioning script
#
# Prepares a clean Debian 13 (trixie) VM as the standard Pneuma integration
# target. Reproduces the relevant properties of the production VPS without
# turning the VM into a second development station:
#   - rootless Podman + Quadlet user generator
#   - Caddy importing only Pneuma-managed fragments
#   - dedicated pneuma user with subuid/subgid and linger
#   - persistent Pneuma directories with VPS-like permissions
#   - canonical PNEUMA_* environment in /etc/pneuma/environment and session
#     exports on the pneuma user profile
#
# The VM and the VPS share the same host invariants, implemented once in
# scripts/lib/provision-host.sh and sourced by both callers. This script owns
# only VM-specific setup: it never builds Pneuma, never clones the repository,
# never installs the CI key and never runs `pneuma doctor`. The Pneuma binary
# is installed as a separate step after provisioning (see section 4 of
# docs/operations/dev-vm-tutorial.md).
#
# Prerequisites:
# - Debian 13 (trixie) VM — Debian 12 ships Podman 4.3.1 without the Quadlet
#   user generator, which Pneuma requires to supervise runtimes across reboots.
# - The VM must already have the provisioning SSH key installed (for example
#   root's authorized_keys) so this script can be run over SSH.
# - Run this script as root
# - Internet access
# - Caddy available from the configured APT repositories
#
# Usage:
#   bash provision-host.sh
#
# Example (from the development host, after ssh-copy-id). The script sources
# scripts/lib/provision-host.sh, so the library must be deployed preserving the
# repository layout (script under /tmp/dev-vm/, library under /tmp/lib/):
#   scp scripts/dev-vm/provision-host.sh pneuma-dev:/tmp/dev-vm/
#   scp -r scripts/lib pneuma-dev:/tmp/
#   ssh pneuma-dev 'sudo bash /tmp/dev-vm/provision-host.sh'

set -euo pipefail

PNEUMA_USER="pneuma"
# Exported so the shared library and any rootless runtime inherit the same
# values even though this caller never references PNEUMA_HOME itself.
export PNEUMA_HOME="/home/$PNEUMA_USER"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
# shellcheck source=../lib/provision-host.sh
source "$SCRIPT_DIR/../lib/provision-host.sh"

if [[ "$(id -u)" -ne 0 ]]; then
    echo "Run this script as root."
    exit 1
fi

# Shared host invariants, exactly as the production bootstrap applies them.
validate_pneuma_account_and_subordinate_ids
provision_runtime_packages

# sqlite3 is a VM operator convenience, not a runtime invariant; the runtime
# package set never includes it.
apt-get install -y sqlite3

require_quadlet_generator
provision_pneuma_user
provision_subordinate_ids
provision_linger
provision_host_directories
provision_host_environment
provision_caddy_baseline
start_pneuma_user_manager
verify_rootless_podman

echo
echo "Provisioning complete."
echo "Open a Pneuma shell:"
echo "  sudo -iu $PNEUMA_USER"
echo
echo "Next steps:"
echo "  1. Install the Pneuma binary as a separate step (see section 4 of"
echo "     docs/operations/dev-vm-tutorial.md). The VM never builds, clones or"
echo "     installs the CI key as part of provisioning."
echo "  2. Run: pneuma doctor"
