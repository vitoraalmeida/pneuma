#!/usr/bin/env bash
#
# Rebuild all fixtures on the development VM
#
# Copies the fixture sources from the host to the VM, builds each image with
# Podman and pushes it to the local registry, then prints the registry digests.
#
# Usage:
#   scripts/dev-vm/rebuild-fixtures.sh [ssh-host]
#
# Default ssh-host: pneuma-dev
# The SSH target must be root to repair ownership after copying fixtures.

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"
FIXTURES_DIR="scripts/dev-vm/fixtures"
REGISTRY="localhost:5000"

echo "==> Configuring the local test registry..."
ssh "$SSH_HOST" 'install -d -m 0755 /etc/containers/registries.conf.d && printf "[[registry]]\nlocation = \"localhost:5000\"\ninsecure = true\n" > /etc/containers/registries.conf.d/pneuma-dev.conf'

echo "==> Ensuring registry is running..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && (podman start pneuma-registry 2>/dev/null || podman run -d --name pneuma-registry -p 5000:5000 docker.io/library/registry:2)"' 2>&1 | grep -v level=warning || true

echo "==> Copying fixtures to VM..."
scp -rq "$FIXTURES_DIR" "$SSH_HOST":/var/lib/pneuma/checkouts/
ssh "$SSH_HOST" 'chown -R pneuma:pneuma /var/lib/pneuma/checkouts/fixtures'

echo "==> Building and pushing fixtures..."
for fixture in "$FIXTURES_DIR"/*/; do
    name=$(basename "$fixture")
    echo "  -> $name"
    ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && podman build -q -t $REGISTRY/$name:latest /var/lib/pneuma/checkouts/fixtures/$name 2>/dev/null && podman push --tls-verify=false $REGISTRY/$name:latest 2>/dev/null'"
done

echo
echo "==> Fixture digests:"
for fixture in "$FIXTURES_DIR"/*/; do
    name=$(basename "$fixture")
    digest=$(ssh "$SSH_HOST" "curl -s -H 'Accept: application/vnd.oci.image.manifest.v1+json' http://$REGISTRY/v2/$name/manifests/latest -D - -o /dev/null 2>/dev/null | grep -i docker-content-digest | awk '{print \$2}' | tr -d '\r'")
    echo "  $name: $digest"
done

echo
echo "==> Rebuild complete."
