#!/usr/bin/env bash
#
# Deploy all fixtures on the development VM
#
# Imports each fixture from the VM checkout and deploys it by digest from the
# local registry, then prints the final application status.
#
# Usage:
#   scripts/dev-vm/deploy-all-fixtures.sh [ssh-host]
#
# Default ssh-host: pneuma-dev

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"
FIXTURES_DIR="scripts/dev-vm/fixtures"
REGISTRY="localhost:5000"

echo "==> Importing fixtures..."
for fixture in "$FIXTURES_DIR"/*/; do
    name=$(basename "$fixture")
    echo "  -> Importing $name"
    ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && pneuma app import /var/lib/pneuma/checkouts/fixtures/$name 2>&1'" 2>&1 | grep -v level=warning || true
done

echo
echo "==> Deploying fixtures..."
for fixture in "$FIXTURES_DIR"/*/; do
    name=$(basename "$fixture")
    digest=$(ssh "$SSH_HOST" "curl -s -H 'Accept: application/vnd.oci.image.manifest.v1+json' http://$REGISTRY/v2/$name/manifests/latest -D - -o /dev/null 2>/dev/null | grep -i docker-content-digest | awk '{print \$2}' | tr -d '\r'")
    echo "  -> Deploying $name ($digest)"
    ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && pneuma app deploy $name --image $REGISTRY/$name@$digest 2>&1'" 2>&1 | grep -v level=warning || true
done

echo
echo "==> Final status:"
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app list"' 2>&1 | grep -v level=warning || true

echo
echo "==> Deploy complete."
