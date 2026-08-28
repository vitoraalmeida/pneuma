#!/usr/bin/env bash
#
# Transport regression for scripts/lib/remote.sh
#
# Runs the transport helper against fake ssh/scp binaries that record their
# argv, proving that:
#   1. default alias mode adds no synthetic port/identity flags;
#   2. explicit port becomes `ssh -p` and `scp -P`;
#   3. explicit identity is passed correctly;
#   4. known-hosts options target the dedicated file;
#   5. the restricted connection uses the CI identity rather than the
#      provisioning identity while preserving the endpoint port.
#
# Usage:
#   scripts/dev-vm/test-transport.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"

FAKE_BIN="$(mktemp -d)"
RECORD="$FAKE_BIN/argv"
trap 'rm -rf "$FAKE_BIN"' EXIT

cat >"$FAKE_BIN/ssh" <<'FAKE'
#!/usr/bin/env bash
printf '%s\n' "$@" >>"$PNEUMA_TRANSPORT_RECORD"
FAKE
cp "$FAKE_BIN/ssh" "$FAKE_BIN/scp"
chmod +x "$FAKE_BIN/ssh" "$FAKE_BIN/scp"

FAILURES=0

recorded() {
	cat "$RECORD"
}

assert_arg() {
	local expected="$1" description="$2"
	if grep -qx -- "$expected" "$RECORD"; then
		printf 'PASS  %s\n' "$description"
	else
		printf 'FAIL  %s (missing argv element: %s)\n' "$description" "$expected"
		FAILURES=$((FAILURES + 1))
	fi
}

assert_absent() {
	local forbidden="$1" description="$2"
	if grep -qx -- "$forbidden" "$RECORD"; then
		printf 'FAIL  %s (unexpected argv element: %s)\n' "$description" "$forbidden"
		FAILURES=$((FAILURES + 1))
	else
		printf 'PASS  %s\n' "$description"
	fi
}

begin_case() {
	: >"$RECORD"
}

export PNEUMA_TRANSPORT_RECORD="$RECORD"
PATH="$FAKE_BIN:$PATH"

# Case 1: plain alias mode adds no synthetic flags.
begin_case
(
	unset PNEUMA_SSH_HOST PNEUMA_SSH_PORT PNEUMA_SSH_IDENTITY
	unset PNEUMA_SSH_KNOWN_HOSTS_FILE PNEUMA_SSH_STRICT_HOST_KEY_CHECKING
	remote_init "pneuma-dev"
	remote_ssh "pneuma-dev" 'true'
)
assert_arg "pneuma-dev" "alias mode keeps the destination"
assert_arg "true" "alias mode keeps the remote command"
assert_absent "-p" "alias mode adds no ssh port flag"
assert_absent "-i" "alias mode adds no ssh identity flag"
assert_absent "-o" "alias mode adds no ssh option overrides"

# Case 2: explicit port uses ssh -p and scp -P.
begin_case
(
	export PNEUMA_SSH_PORT=2222
	remote_init ""
	remote_ssh "127.0.0.1" 'true'
)
assert_arg "-p" "ssh port uses -p"
assert_arg "2222" "ssh port value is forwarded"
begin_case
(
	export PNEUMA_SSH_PORT=2222
	remote_init ""
	remote_scp_to "local-file" "127.0.0.1:/remote/path"
)
assert_arg "-P" "scp port uses uppercase -P"
assert_arg "2222" "scp port value is forwarded"
assert_absent "-p" "scp does not receive the lowercase -p flag"

# Case 3: explicit identity is passed to both ssh and scp.
begin_case
(
	export PNEUMA_SSH_IDENTITY=/tmp/pneuma-vm/root-key
	remote_init ""
	remote_ssh "127.0.0.1" 'true'
)
assert_arg "/tmp/pneuma-vm/root-key" "ssh receives the provisioning identity"
begin_case
(
	export PNEUMA_SSH_IDENTITY=/tmp/pneuma-vm/root-key
	remote_init ""
	remote_scp_to "local-file" "127.0.0.1:/remote/path"
)
assert_arg "/tmp/pneuma-vm/root-key" "scp receives the provisioning identity"

# Case 4: dedicated known-hosts file and strict-host-key setting.
begin_case
(
	export PNEUMA_SSH_KNOWN_HOSTS_FILE=/tmp/pneuma-vm/known_hosts
	export PNEUMA_SSH_STRICT_HOST_KEY_CHECKING=accept-new
	remote_init ""
	remote_ssh "127.0.0.1" 'true'
)
assert_arg "UserKnownHostsFile=/tmp/pneuma-vm/known_hosts" \
	"known-hosts options target the dedicated file"
assert_arg "StrictHostKeyChecking=accept-new" \
	"strict host-key checking follows the explicit setting"

# Case 5: restricted connection swaps the identity and keeps the endpoint.
begin_case
(
	export PNEUMA_SSH_HOST=127.0.0.1
	export PNEUMA_SSH_PORT=2222
	export PNEUMA_SSH_IDENTITY=/tmp/pneuma-vm/root-key
	remote_init ""
	remote_ssh_as pneuma /tmp/pneuma-ci-key -o BatchMode=yes version
)
assert_arg "/tmp/pneuma-ci-key" "restricted connection uses the CI identity"
assert_absent "/tmp/pneuma-vm/root-key" \
	"restricted connection does not offer the provisioning identity"
assert_arg "-p" "restricted connection preserves the forwarded port"
assert_arg "2222" "restricted connection preserves the port value"
assert_arg "pneuma@127.0.0.1" \
	"restricted connection builds the endpoint user destination"

# Case 6: target resolution order.
unset PNEUMA_SSH_PORT PNEUMA_SSH_IDENTITY
unset PNEUMA_SSH_KNOWN_HOSTS_FILE PNEUMA_SSH_STRICT_HOST_KEY_CHECKING
PNEUMA_SSH_HOST=127.0.0.1 remote_init ""
if [[ "$REMOTE_HOST" == "127.0.0.1" ]]; then
	printf 'PASS  %s\n' "PNEUMA_SSH_HOST is used without a positional host"
else
	printf 'FAIL  %s\n' "PNEUMA_SSH_HOST is used without a positional host"
	FAILURES=$((FAILURES + 1))
fi
PNEUMA_SSH_HOST=127.0.0.1 remote_init "pneuma-dev"
if [[ "$REMOTE_HOST" == "pneuma-dev" ]]; then
	printf 'PASS  %s\n' "a positional host overrides PNEUMA_SSH_HOST"
else
	printf 'FAIL  %s\n' "a positional host overrides PNEUMA_SSH_HOST"
	FAILURES=$((FAILURES + 1))
fi

echo
if [[ "$FAILURES" -gt 0 ]]; then
	echo "$FAILURES transport check(s) failed."
	exit 1
fi
echo "Transport checks passed."
