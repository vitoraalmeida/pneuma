#!/usr/bin/env bash
#
# Integration test: CI dispatcher deployment lifecycle
#
# Validates the complete deploy cycle on a bootstrapped Pneuma host, driven
# through the restricted CI dispatcher exactly as GitHub Actions would:
#   - deploy by branch (main) -> verify served revision
#   - upgrade by branch (staging) -> verify revision switch
#   - rollback -> verify previous revision restored
#   - deployment history records every attempt
#
# The host must already be bootstrapped with a CI deploy key (see
# scripts/test-bootstrap-vps.sh). The CI private key is read from the same
# LOG_DIR that script uses by default, or passed explicitly.
#
# Usage:
#   scripts/test-integration.sh <ssh-host> [--ci-key <path>]
#
# Example:
#   scripts/test-integration.sh pneuma-dev
#   scripts/test-integration.sh pneuma-dev --ci-key /tmp/ci-test-key

set -euo pipefail

SSH_HOST="${1:-}"
CI_KEY=""
shift || true
while [[ $# -gt 0 ]]; do
	case "$1" in
	--ci-key)
		CI_KEY="${2:-}"
		shift 2
		;;
	*)
		echo "Unknown option: $1"
		echo "Usage: $0 <ssh-host> [--ci-key <path>]"
		exit 1
		;;
	esac
done

if [[ -z "$SSH_HOST" ]]; then
	echo "Usage: $0 <ssh-host> [--ci-key <path>]"
	exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="${TMPDIR:-/tmp}/pneuma-test-bootstrap"
mkdir -p "$LOG_DIR"

if [[ -z "$CI_KEY" ]]; then
	CI_KEY="$LOG_DIR/ci-test-key"
fi
if [[ ! -f "$CI_KEY" ]]; then
	echo "CI key not found: $CI_KEY"
	echo "Generate one or run scripts/test-bootstrap-vps.sh first."
	exit 1
fi

FIXTURE_SRC="$SCRIPT_DIR/dev-vm/fixtures/healthy-http"

PASS_COUNT=0
FAIL_COUNT=0

report() {
	local result="$1" message="$2"
	case "$result" in
	ok)
		PASS_COUNT=$((PASS_COUNT + 1))
		printf 'PASS  %s\n' "$message"
		;;
	fail)
		FAIL_COUNT=$((FAIL_COUNT + 1))
		printf 'FAIL  %s\n' "$message"
		;;
	esac
}

dispatcher() {
	ssh -i "$CI_KEY" -o BatchMode=yes "pneuma@$SSH_HOST" "$1"
}

pneuma_as() {
	ssh "$SSH_HOST" "su - pneuma -c \"$1\""
}

echo "=========================================="
echo "Pneuma CI Dispatcher Integration Test — $SSH_HOST"
echo "=========================================="

# Step 0: Host sanity
echo
echo "==> Step 0: Host sanity..."
if VERSION_OUT=$(dispatcher "version" 2>&1); then
	report ok "CI dispatcher reachable (version)"
else
	report fail "CI dispatcher unreachable: $VERSION_OUT"
	exit 1
fi

# Step 1: Fixture setup (registry, Git repository, tagged images)
echo
echo "==> Step 1: Fixture setup..."
ssh "$SSH_HOST" 'mkdir -p /etc/containers/registries.conf.d
printf "[[registry]]\nlocation = \"localhost:5000\"\ninsecure = true\n" \
    > /etc/containers/registries.conf.d/pneuma-test.conf' 2>&1
ssh "$SSH_HOST" 'runuser -u pneuma -- bash -lc "podman start pneuma-registry 2>/dev/null || podman run -d --name pneuma-registry -p 5000:5000 docker.io/library/registry:2"' \
	2>&1 | grep -v level=warning || true
scp -rq "$FIXTURE_SRC" "$SSH_HOST":/var/lib/pneuma/checkouts/fixtures/
ssh "$SSH_HOST" 'chown -R pneuma:pneuma /var/lib/pneuma/checkouts/fixtures'

SHA_OUT=$(
	ssh "$SSH_HOST" 'runuser -u pneuma -- bash -l -s' <<'REMOTE'
set -euo pipefail
cd "$HOME"
REPOS_ROOT="/var/lib/pneuma/repos"
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
    podman build --quiet --tag "localhost:5000/healthy-http:$sha" . >/dev/null 2>&1
    podman push --tls-verify=false "localhost:5000/healthy-http:$sha" >/dev/null 2>&1
done

echo "SHA_MAIN=$SHA_MAIN"
echo "SHA_STAGING=$SHA_STAGING"
REMOTE
)
SHA_MAIN=$(echo "$SHA_OUT" | grep '^SHA_MAIN=' | cut -d= -f2)
SHA_STAGING=$(echo "$SHA_OUT" | grep '^SHA_STAGING=' | cut -d= -f2)
echo "  main=$SHA_MAIN staging=$SHA_STAGING"

# Step 2: Import
echo
echo "==> Step 2: Import application..."
if pneuma_as "pneuma app import file:///var/lib/pneuma/repos/healthy-http.git" >/dev/null 2>&1; then
	report ok "application imported"
else
	report fail "import failed"
	exit 1
fi

# Step 3: Deploy main via CI dispatcher
echo
echo "==> Step 3: Deploy main via CI dispatcher..."
DEPLOY_MAIN=$(dispatcher "deploy healthy-http main" 2>&1) || true
if printf '%s' "$DEPLOY_MAIN" | grep -q "Status: Succeeded"; then
	report ok "deploy main succeeded"
else
	report fail "deploy main failed"
	printf '        %s\n' "$DEPLOY_MAIN" | head -c 400
fi
printf '%s\n' "$DEPLOY_MAIN" | grep -v level=warning | sed 's/^/        /' || true
if printf '%s' "$DEPLOY_MAIN" | grep -q "Source revision: $SHA_MAIN"; then
	report ok "deploy main pinned to commit $SHA_MAIN"
else
	report fail "deploy main did not pin to $SHA_MAIN"
fi

# Step 4: Upgrade to staging via CI dispatcher
echo
echo "==> Step 4: Upgrade to staging via CI dispatcher..."
DEPLOY_STAGING=$(dispatcher "deploy healthy-http staging" 2>&1) || true
if printf '%s' "$DEPLOY_STAGING" | grep -q "Status: Succeeded"; then
	report ok "deploy staging succeeded"
else
	report fail "deploy staging failed"
	printf '        %s\n' "$DEPLOY_STAGING" | head -c 400
fi
printf '%s\n' "$DEPLOY_STAGING" | grep -v level=warning | sed 's/^/        /' || true
if printf '%s' "$DEPLOY_STAGING" | grep -q "Source revision: $SHA_STAGING"; then
	report ok "deploy staging pinned to commit $SHA_STAGING"
else
	report fail "deploy staging did not pin to $SHA_STAGING"
fi

echo "  -> verifying served revision..."
check_body() {
	local expected="$1" description="$2"
	local port body
	port=$(pneuma_as "podman ps --format '{{.Ports}}' --filter name=pneuma-healthy-http | cut -d: -f2 | cut -d- -f1")
	body=$(pneuma_as "curl -s http://127.0.0.1:$port/")
	if [[ "$body" == "$expected" ]]; then
		report ok "$description ($body)"
	else
		report fail "$description (got: $body)"
	fi
}
check_body "healthy-http v2.0" "staging served after upgrade"

# Step 5: Rollback
echo
echo "==> Step 5: Rollback..."
ROLLBACK_OUT=$(pneuma_as "pneuma deployment rollback healthy-http" 2>&1) || true
if printf '%s' "$ROLLBACK_OUT" | grep -q "Status: Succeeded"; then
	report ok "rollback succeeded"
else
	report fail "rollback failed"
	printf '        %s\n' "$ROLLBACK_OUT" | head -c 400
fi
printf '%s\n' "$ROLLBACK_OUT" | grep -v level=warning | sed 's/^/        /' || true
check_body "healthy-http v1.0" "main served after rollback"

# Step 6: Deployment history
echo
echo "==> Step 6: Deployment history..."
HISTORY=$(pneuma_as "pneuma app deployments healthy-http" 2>&1) || true
for revision in "$SHA_MAIN" "$SHA_STAGING"; do
	if printf '%s' "$HISTORY" | grep -q "$revision"; then
		report ok "history records revision $revision"
	else
		report fail "history missing revision $revision"
	fi
done

# Summary
echo
echo "============================================================"
echo "$PASS_COUNT check(s) passed, $FAIL_COUNT failed."
echo "============================================================"

if [[ "$FAIL_COUNT" -gt 0 ]]; then
	exit 1
fi
