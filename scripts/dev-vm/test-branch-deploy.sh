#!/usr/bin/env bash
#
# End-to-end test for branch-based OCI deployment on the development VM
#
# Exercises the full Git -> OCI chain introduced in iteration v0.2: creates a
# fixture Git repository with two branches (main = v1.0, staging = v2.0),
# builds and pushes an image tagged with each commit SHA, imports the
# application from the Git URL, deploys by --branch and verifies the served
# version and the persisted source revision for each branch.
#
# Prerequisites:
#   scripts/dev-vm/sync-binary.sh   # binary must know --branch
#
# Usage:
#   scripts/dev-vm/test-branch-deploy.sh [ssh-host]
#
# Default ssh-host: pneuma-dev
# The SSH target must be root to prepare fixture and repository directories.
# Transport settings (forwarded port, identity, known-hosts file) come from
# the PNEUMA_SSH_* environment described in scripts/lib/remote.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"

remote_init "${1:-pneuma-dev}"
SSH_HOST="$REMOTE_HOST"
FIXTURE_SRC="$SCRIPT_DIR/fixtures/healthy-http"

echo "=========================================="
echo "Pneuma Branch Deploy E2E — $SSH_HOST"
echo "=========================================="

echo
echo "==> Verifying installed binary supports --branch..."
BRANCH_USAGE=$(remote_ssh "$SSH_HOST" '/usr/local/bin/pneuma app deploy --help 2>&1' || true)
if ! echo "$BRANCH_USAGE" | grep -q -- '--branch'; then
	echo "  ERROR: installed binary lacks --branch; run scripts/dev-vm/sync-binary.sh first"
	exit 1
fi
echo "  OK"

echo
echo "==> Step 1: Reset fixtures..."
"$SCRIPT_DIR/reset-fixtures.sh" "$SSH_HOST"

echo
echo "==> Step 2: Ensure registry is running and copy fixture sources..."
remote_ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "cd \$HOME && (podman start pneuma-registry 2>/dev/null || podman run -d --name pneuma-registry -p 5000:5000 docker.io/library/registry:2)"' 2>&1 | grep -v level=warning || true
remote_ssh "$SSH_HOST" 'mkdir -p /var/lib/pneuma/checkouts/fixtures'
remote_ssh "$SSH_HOST" 'mkdir -p /var/lib/pneuma/repos && chown pneuma:pneuma /var/lib/pneuma/repos'
remote_scp_to -rq "$FIXTURE_SRC" "$SSH_HOST":/var/lib/pneuma/checkouts/fixtures/
remote_ssh "$SSH_HOST" 'chown -R pneuma:pneuma /var/lib/pneuma/checkouts/fixtures'

echo
echo "==> Step 3: Create Git repository and tagged images on the VM..."
SHA_OUT=$(
	remote_ssh "$SSH_HOST" 'runuser -u pneuma -- bash -l -s' <<'REMOTE'
set -euo pipefail
cd "$HOME"
REPOS_ROOT="/var/lib/pneuma/repos"
REGISTRY="localhost:5000"
FIXTURE="/var/lib/pneuma/checkouts/fixtures/healthy-http"

rm -rf "$REPOS_ROOT"/*
mkdir -p "$REPOS_ROOT"

git init --quiet --initial-branch=main "$REPOS_ROOT/work"
cd "$REPOS_ROOT/work"
git config user.name "Pneuma Tests"
git config user.email "pneuma@example.invalid"
cp "$FIXTURE"/* .
git add -A
git commit --quiet -m "healthy-http v1.0"
SHA_MAIN=$(git rev-parse HEAD)

git checkout --quiet -b staging
sed -i 's/healthy-http v1.0/healthy-http v2.0/' server.py
git add server.py
git commit --quiet -m "healthy-http v2.0"
SHA_STAGING=$(git rev-parse HEAD)
git checkout --quiet main

git init --quiet --bare "$REPOS_ROOT/healthy-http.git"
git push --quiet "$REPOS_ROOT/healthy-http.git" main staging
git --git-dir="$REPOS_ROOT/healthy-http.git" symbolic-ref HEAD refs/heads/main

for branch in main staging; do
    git checkout --quiet "$branch"
    sha=$(git rev-parse HEAD)
    echo "  -> building $branch ($sha)"
    podman build --quiet --tag "$REGISTRY/healthy-http:$sha" . >/dev/null 2>&1 \
        || { echo "  ERROR: build failed for $branch" >&2; exit 1; }
    podman push --tls-verify=false "$REGISTRY/healthy-http:$sha" >/dev/null 2>&1 \
        || { echo "  ERROR: push failed for $branch" >&2; exit 1; }
done

echo "SHA_MAIN=$SHA_MAIN"
echo "SHA_STAGING=$SHA_STAGING"
REMOTE
)
SHA_MAIN=$(echo "$SHA_OUT" | grep '^SHA_MAIN=' | cut -d= -f2)
SHA_STAGING=$(echo "$SHA_OUT" | grep '^SHA_STAGING=' | cut -d= -f2)
echo "  main=$SHA_MAIN"
echo "  staging=$SHA_STAGING"

echo
echo "==> Step 4: Import, deploy by branch and verify..."
remote_ssh "$SSH_HOST" "runuser -u pneuma -- bash -l -s '$SHA_MAIN' '$SHA_STAGING'" <<'REMOTE'
set -euo pipefail
cd "$HOME"
SHA_MAIN="$1"
SHA_STAGING="$2"
REPOS_ROOT="/var/lib/pneuma/repos"
REGISTRY="localhost:5000"
REPO_URL="file://$REPOS_ROOT/healthy-http.git"

echo "  -> importing application from Git URL"
pneuma app import "$REPO_URL" 2>&1 | grep -v level=warning || true

check_deploy() {
    local branch="$1"
    local expected_sha="$2"
    local expected_body="$3"
    echo "  -> deploying branch $branch"
    local output
    output=$(pneuma app deploy healthy-http --branch "$branch" 2>&1 | grep -v level=warning || true)
    echo "$output"
    if ! echo "$output" | grep -q "Status: Succeeded"; then
        echo "  ERROR: deploy of $branch did not succeed"
        exit 1
    fi
    if ! echo "$output" | grep -q "Source revision: $expected_sha"; then
        echo "  ERROR: expected source revision $expected_sha for $branch"
        exit 1
    fi
    local port
    port=$(podman ps --format "{{.Ports}}" --filter name=pneuma-healthy-http \
        | cut -d: -f2 | cut -d- -f1 | head -1)
    local body
    body=$(curl -s "http://127.0.0.1:$port/")
    echo "  body: $body"
    if [ "$body" != "$expected_body" ]; then
        echo "  ERROR: expected '$expected_body', got: $body"
        exit 1
    fi
}

check_deploy main "$SHA_MAIN" "healthy-http v1.0"
check_deploy staging "$SHA_STAGING" "healthy-http v2.0"
check_deploy main "$SHA_MAIN" "healthy-http v1.0"

echo
echo "  -> deployment history"
pneuma app deployments healthy-http 2>&1 | grep -v level=warning || true

echo
echo "==> Branch deploy E2E complete."
REMOTE
