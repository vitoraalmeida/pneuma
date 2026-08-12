#!/usr/bin/env bash
#
# Deploy all fixtures on the development VM
#
# Imports each fixture from a local Git repository (built from the fixture
# source copied to the VM) and deploys it by digest from the local registry,
# then prints the final application status.
#
# Usage:
#   scripts/dev-vm/deploy-all-fixtures.sh [ssh-host]
#
# Default ssh-host: pneuma-dev
# The SSH target must be root to prepare the fixture repository directory.

set -euo pipefail

SSH_HOST="${1:-pneuma-dev}"
FIXTURES_DIR="scripts/dev-vm/fixtures"
REGISTRY="localhost:5000"

echo "==> Preparing Git repositories for fixtures..."
ssh "$SSH_HOST" 'mkdir -p /var/lib/pneuma/repos && chown pneuma:pneuma /var/lib/pneuma/repos'

echo "==> Importing fixtures..."
for fixture in "$FIXTURES_DIR"/*/; do
	name=$(basename "$fixture")
	echo "  -> Importing $name"
	output=$(
		ssh "$SSH_HOST" "runuser -u pneuma -- bash -l -s \"$name\"" <<'REMOTE'
set -euo pipefail
name="$1"
SRC="/var/lib/pneuma/checkouts/fixtures/$name"
REPOS_ROOT="/var/lib/pneuma/repos"
WORK="$REPOS_ROOT/$name-work"
REPO="$REPOS_ROOT/$name.git"

rm -rf "$WORK" "$REPO"
mkdir -p "$REPOS_ROOT"

git init --quiet --initial-branch=main "$WORK"
cd "$WORK"
git config user.name "Pneuma Fixtures"
git config user.email "pneuma@example.invalid"
cp "$SRC"/* .
git add -A
git commit --quiet -m "$name fixture"
git init --quiet --bare "$REPO"
git push --quiet "$REPO" main
git --git-dir="$REPO" symbolic-ref HEAD refs/heads/main

cd "$HOME"
pneuma app import "file://$REPO"
REMOTE
	)
	echo "$output" | grep -v level=warning || true
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
