#!/usr/bin/env bash
#
# End-to-end test battery for the development VM
#
# Runs the full fixture cycle: reset, rebuild, deploy, prove a failed candidate
# preserves v1, upgrade healthy-http to v2, rollback to v1, reboot the VM and
# verify recovery.
# The host checkout is the source of truth; the script creates a temporary
# v2 build and restores v1 afterwards.
#
# Usage:
#   scripts/dev-vm/e2e.sh [ssh-host]
#
# Default ssh-host: pneuma-dev
# The SSH target must be root for fixture ownership and VM reboot operations.

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REGISTRY="localhost:5000"
FIXTURE_SRC="$SCRIPT_DIR/fixtures/healthy-http"
TMP_V2="$(mktemp -d)"

cleanup() {
	rm -rf "$TMP_V2"
}
trap cleanup EXIT

echo "=========================================="
echo "Pneuma E2E Test Battery — $SSH_HOST"
echo "=========================================="

echo
echo "==> Step 1: Reset fixtures..."
"$SCRIPT_DIR/reset-fixtures.sh" "$SSH_HOST"

echo
echo "==> Step 2: Rebuild fixtures..."
"$SCRIPT_DIR/rebuild-fixtures.sh" "$SSH_HOST"

echo
echo "==> Step 3: Deploy all fixtures..."
"$SCRIPT_DIR/deploy-all-fixtures.sh" "$SSH_HOST"

echo
echo "==> Step 4: Verify baseline status..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app status healthy-http"' 2>&1 | grep -v level=warning

echo
echo "==> Step 5: Failed candidate preserves healthy-http v1..."
ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && podman build -q -t $REGISTRY/healthy-http:unhealthy /var/lib/pneuma/checkouts/fixtures/unhealthy-http >/dev/null && podman push --tls-verify=false $REGISTRY/healthy-http:unhealthy >/dev/null'"
UNHEALTHY_DIGEST=$(ssh "$SSH_HOST" "curl -fsS -H 'Accept: application/vnd.oci.image.manifest.v1+json' http://$REGISTRY/v2/healthy-http/manifests/unhealthy -D - -o /dev/null | grep -i docker-content-digest | awk '{print \$2}' | tr -d '\r'")
if DEPLOY_OUT=$(ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && pneuma app deploy healthy-http --image $REGISTRY/healthy-http@$UNHEALTHY_DIGEST'" 2>&1); then
	echo "  ERROR: unhealthy candidate deploy unexpectedly succeeded"
	echo "$DEPLOY_OUT" | grep -v level=warning || true
	exit 1
fi
echo "$DEPLOY_OUT" | grep -v level=warning || true
if ! ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app deployments healthy-http"' | grep -q $'\tDeploy\t.*\tFailed'; then
	echo "  ERROR: unhealthy candidate failure is absent from deployment history"
	exit 1
fi
if ! ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app status healthy-http"' | grep -q "Observed state: Running"; then
	echo "  ERROR: healthy-http is not Running after failed candidate"
	exit 1
fi
PORT=$(ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && podman ps --format \"{{.Ports}}\" --filter name=pneuma-healthy-http | cut -d: -f2 | cut -d- -f1"')
BODY=$(ssh "$SSH_HOST" "curl -fsS http://127.0.0.1:$PORT/")
if [[ "$BODY" != "healthy-http v1.0" ]]; then
	echo "  ERROR: failed candidate changed healthy-http body to: $BODY"
	exit 1
fi
echo "  OK: failed candidate preserved $BODY"

echo
echo "==> Step 6: Upgrade healthy-http to v2..."
sed 's/healthy-http v1.0/healthy-http v2.0/' "$FIXTURE_SRC/server.py" >"$TMP_V2/server.py"
scp -q "$TMP_V2/server.py" "$SSH_HOST":/var/lib/pneuma/checkouts/fixtures/healthy-http/server.py
ssh "$SSH_HOST" 'chown pneuma:pneuma /var/lib/pneuma/checkouts/fixtures/healthy-http/server.py'
ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && podman build -q -t $REGISTRY/healthy-http:latest /var/lib/pneuma/checkouts/fixtures/healthy-http 2>/dev/null && podman push --tls-verify=false $REGISTRY/healthy-http:latest 2>/dev/null'"
DIGEST=$(ssh "$SSH_HOST" "curl -fsS -H 'Accept: application/vnd.oci.image.manifest.v1+json' http://$REGISTRY/v2/healthy-http/manifests/latest -D - -o /dev/null | grep -i docker-content-digest | awk '{print \$2}' | tr -d '\r'")
DEPLOY_OUT=$(ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && pneuma app deploy healthy-http --image $REGISTRY/healthy-http@$DIGEST 2>&1'")
echo "$DEPLOY_OUT" | grep -v level=warning || true
if ! echo "$DEPLOY_OUT" | grep -q "Succeeded"; then
	echo "  ERROR: upgrade deploy did not succeed"
	exit 1
fi
PORT=$(ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && podman ps --format \"{{.Ports}}\" --filter name=pneuma-healthy-http | cut -d: -f2 | cut -d- -f1"')
echo "  healthy-http on host port: $PORT"
BODY=$(ssh "$SSH_HOST" "curl -fsS http://127.0.0.1:$PORT/")
if [[ "$BODY" != "healthy-http v2.0" ]]; then
	echo "  ERROR: expected 'healthy-http v2.0', got: $BODY"
	exit 1
fi
echo "  OK: $BODY"

echo
echo "==> Step 7: Roll back healthy-http to v1..."
DEPLOY_OUT=$(ssh "$SSH_HOST" "runuser -u pneuma -- bash -lc 'cd \$HOME && pneuma deployment rollback healthy-http'" 2>&1)
echo "$DEPLOY_OUT" | grep -v level=warning || true
if ! echo "$DEPLOY_OUT" | grep -q "Succeeded"; then
	echo "  ERROR: rollback did not succeed"
	exit 1
fi
PORT=$(ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && podman ps --format \"{{.Ports}}\" --filter name=pneuma-healthy-http | cut -d: -f2 | cut -d- -f1"')
BODY=$(ssh "$SSH_HOST" "curl -fsS http://127.0.0.1:$PORT/")
if [[ "$BODY" != "healthy-http v1.0" ]]; then
	echo "  ERROR: expected 'healthy-http v1.0', got: $BODY"
	exit 1
fi
echo "  OK: $BODY"
if ! ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app deployments healthy-http"' | grep -q $'\tRollback\t.*\tSucceeded'; then
	echo "  ERROR: deployment history has no succeeded rollback"
	exit 1
fi
echo "  OK: rollback deployment recorded"

echo
echo "==> Step 8: Reboot VM..."
ssh "$SSH_HOST" 'reboot' 2>&1 || true
echo "  Waiting for VM to come back..."
for _ in $(seq 1 60); do
	if ssh -o ConnectTimeout=3 -o BatchMode=yes "$SSH_HOST" 'uptime' 2>/dev/null; then
		break
	fi
	sleep 5
done
sleep 15

echo
echo "==> Step 9: Verify apps after reboot..."
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app status healthy-http && pneuma app status redirect-public"' 2>&1 | grep -v level=warning || true

echo
echo "=========================================="
echo "E2E Test Battery Complete"
echo "=========================================="
