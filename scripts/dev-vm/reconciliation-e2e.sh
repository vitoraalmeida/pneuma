#!/usr/bin/env bash
#
# Disposable-VM reconciliation catalog for v0.4.
#
# Every case starts from a freshly deployed fixture baseline and keeps its command
# output, SQLite rows, and external observations below LOG_ROOT. The target must
# be reachable as root; Pneuma commands intentionally run as the pneuma user.
#
# Usage:
#   scripts/dev-vm/reconciliation-e2e.sh [ssh-host] [R1|...|C4]
#
# Without a case ID the complete approved catalog is run. Case C2 uses the
# deterministic post-lock test gate (PNEUMA_TEST_GATE_DIRECTORY) to serialize
# two reconciliations without polling a short-lived process.
# Transport settings (forwarded port, identity, known-hosts file) come from
# the PNEUMA_SSH_* environment described in scripts/lib/remote.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/remote.sh
source "$SCRIPT_DIR/../lib/remote.sh"

remote_init "${1:-pneuma-dev}"
SSH_HOST="$REMOTE_HOST"
REQUESTED_CASE="${2:-all}"
LOG_ROOT="${PNEUMA_RECONCILIATION_LOG_ROOT:-${TMPDIR:-/tmp}/pneuma-reconciliation-e2e-$(date +%Y%m%d-%H%M%S)}"
DATABASE_PATH="/var/lib/pneuma/database/pneuma.sqlite3"
GATE_ROOT="/var/lib/pneuma/test-gates"
REGISTRY="localhost:5000"

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
CASE_DIR=""
APP=""
APP_ID=""
DEPLOYMENT_ID=""
RUNTIME_ID=""
RUNTIME_PORT=""
RUNTIME_CONTAINER=""
CANDIDATE_DIGEST=""

mkdir -p "$LOG_ROOT"

report() {
	local result="$1" message="$2"
	case "$result" in
	pass)
		PASS_COUNT=$((PASS_COUNT + 1))
		printf 'PASS  %s\n' "$message"
		;;
	fail)
		FAIL_COUNT=$((FAIL_COUNT + 1))
		printf 'FAIL  %s\n' "$message"
		;;
	skip)
		SKIP_COUNT=$((SKIP_COUNT + 1))
		printf 'SKIP  %s\n' "$message"
		;;
	esac
}

# Run a root-owned script on the VM. Use heredocs with this helper for commands
# that need several shell statements instead of interpolating into SSH strings.
root_ssh() {
	remote_ssh -o BatchMode=yes -o ConnectTimeout=10 "$SSH_HOST" 'bash -s'
}

# Run a login shell as the rootless runtime owner, preserving its Podman session.
pneuma_ssh() {
	remote_ssh -o BatchMode=yes -o ConnectTimeout=10 "$SSH_HOST" \
		'runuser -u pneuma -- bash -l -s'
}

sql() {
	printf '%s\n' "$1" | remote_ssh -o BatchMode=yes -o ConnectTimeout=10 "$SSH_HOST" \
		"sqlite3 -tabs '$DATABASE_PATH'"
}

remote_file() {
	local path="$1"
	root_ssh <<REMOTE
set -euo pipefail
test -f '$path'
REMOTE
}

assert_contains() {
	local needle="$1" file="$2"
	grep -qF -- "$needle" "$file" || {
		printf 'assertion failed: %s is absent from %s\n' "$needle" "$file" >&2
		return 1
	}
}

assert_result() {
	local expected="$1" output="$2"
	assert_contains "Result: $expected" "$output"
}

assert_query_equal() {
	local before="$1" after="$2" description="$3"
	if [[ "$(<"$before")" != "$(<"$after")" ]]; then
		printf 'assertion failed: %s changed\n' "$description" >&2
		diff -u "$before" "$after" >&2 || true
		return 1
	fi
}

snapshot() {
	local label="$1"
	local prefix="$CASE_DIR/$label"
	sql "SELECT a.id, a.name, a.desired_runtime_state, a.active_deployment_id, d.id, d.status, d.failure_code, r.id, r.external_runtime_id, r.state, r.host_port, r.removed_at FROM applications a LEFT JOIN deployments d ON d.application_id = a.id LEFT JOIN runtime_instances r ON r.deployment_id = d.id WHERE a.name = '$APP' ORDER BY d.requested_at, r.created_at;" >"$prefix-runtime.tsv"
	sql "SELECT a.id, e.desired_visibility, e.materialization_state, e.active_runtime_id, e.configuration_version, e.last_error_code, e.last_error_message FROM applications a JOIN exposures e ON e.application_id = a.id WHERE a.name = '$APP';" >"$prefix-exposure.tsv"
	root_ssh >"$prefix-podman.txt" 2>&1 <<REMOTE
set -euo pipefail
runuser -u pneuma -- bash -lc 'podman ps -a --noheading --format "{{.ID}} {{.Names}} {{.Image}} {{.Ports}} {{.Status}}"; systemctl --user list-units "pneuma-$APP-*.service" --all --no-legend' || true
REMOTE
	root_ssh >"$prefix-quadlet.txt" 2>&1 <<REMOTE
set -euo pipefail
runuser -u pneuma -- bash -lc 'for unit in \$HOME/.config/containers/systemd/pneuma-$APP-*.container; do [ -e "\$unit" ] || continue; printf "--- %s ---\\n" "\$unit"; cat "\$unit"; done' || true
REMOTE
	root_ssh >"$prefix-caddy.txt" 2>&1 <<REMOTE
set -euo pipefail
ls -l /etc/caddy/applications || true
if [ -n '$APP_ID' ] && [ -f '/etc/caddy/applications/$APP_ID.caddy' ]; then
	cat '/etc/caddy/applications/$APP_ID.caddy'
fi
REMOTE
}

load_identity() {
	local identity
	identity=$(sql "SELECT a.id, d.id, r.id, r.host_port, 'pneuma-' || a.name || '-' || d.id FROM applications a JOIN deployments d ON d.id = a.active_deployment_id JOIN runtime_instances r ON r.deployment_id = d.id AND r.state = 'running' AND r.removed_at IS NULL WHERE a.name = '$APP';")
	IFS=$'\t' read -r APP_ID DEPLOYMENT_ID RUNTIME_ID RUNTIME_PORT RUNTIME_CONTAINER <<<"$identity"
	[[ -n "$APP_ID" && -n "$DEPLOYMENT_ID" && -n "$RUNTIME_ID" && -n "$RUNTIME_PORT" ]]
}

run_reconcile() {
	local name="$1"
	local output="$CASE_DIR/$name.log"
	pneuma_ssh >"$output" 2>&1 <<REMOTE
set -euo pipefail
cd "\$HOME"
pneuma reconcile "$APP"
REMOTE
	printf '%s\n' "$output"
}

run_reconcile_with_path() {
	local name="$1"
	local output="$CASE_DIR/$name.log"
	pneuma_ssh >"$output" 2>&1 <<REMOTE
set -euo pipefail
cd "\$HOME"
PATH="\$HOME/.local/bin:\$PATH" pneuma reconcile "$APP"
REMOTE
	printf '%s\n' "$output"
}

reboot_vm() {
	local disconnected=false recovered=false
	root_ssh <<'REMOTE' || true
reboot
REMOTE
	for _ in $(seq 1 30); do
		if ! ssh -o BatchMode=yes -o ConnectTimeout=3 "$SSH_HOST" true >/dev/null 2>&1; then
			disconnected=true
			break
		fi
		sleep 2
	done
	[[ "$disconnected" == true ]]
	for _ in $(seq 1 60); do
		if ssh -o BatchMode=yes -o ConnectTimeout=3 "$SSH_HOST" true >/dev/null 2>&1; then
			recovered=true
			break
		fi
		sleep 3
	done
	[[ "$recovered" == true ]]
}

prepare_baseline() {
	local fixture="$1"
	"$SCRIPT_DIR/reset-fixtures.sh" "$SSH_HOST" >"$CASE_DIR/reset.log" 2>&1
	local digest
	digest=$(
		root_ssh <<REMOTE
set -euo pipefail
curl -fsS -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
  'http://$REGISTRY/v2/$fixture/manifests/latest' -D - -o /dev/null |
  awk 'BEGIN { IGNORECASE=1 } /^docker-content-digest:/ { print \$2 }' | tr -d '\r'
REMOTE
	)
	pneuma_ssh >"$CASE_DIR/deploy.log" 2>&1 <<REMOTE
set -euo pipefail
cd "\$HOME"
pneuma app import 'file:///var/lib/pneuma/repos/$fixture.git'
pneuma app deploy '$fixture' --image '$REGISTRY/$fixture@$digest'
REMOTE
	APP="$fixture"
	load_identity
	snapshot before
	printf 'fixture=%s\napplication_id=%s\ndeployment_id=%s\nruntime_id=%s\nport=%s\n' \
		"$fixture" "$APP_ID" "$DEPLOYMENT_ID" "$RUNTIME_ID" "$RUNTIME_PORT" >"$CASE_DIR/identity.txt"
}

remove_active_container() {
	pneuma_ssh <<REMOTE
set -euo pipefail
podman rm -f '$RUNTIME_CONTAINER'
REMOTE
}

remove_expected_unit() {
	pneuma_ssh <<REMOTE
set -euo pipefail
rm -f "\$HOME/.config/containers/systemd/$RUNTIME_CONTAINER.container"
systemctl --user daemon-reload
REMOTE
}

assert_same_logical_runtime() {
	local after
	after=$(sql "SELECT a.id, d.id, r.id, r.host_port, r.removed_at FROM applications a JOIN deployments d ON d.id = a.active_deployment_id JOIN runtime_instances r ON r.deployment_id = d.id AND r.state = 'running' WHERE a.name = '$APP';")
	local after_app after_deployment after_runtime after_port removed
	IFS=$'\t' read -r after_app after_deployment after_runtime after_port removed <<<"$after"
	[[ "$after_app" == "$APP_ID" && "$after_deployment" == "$DEPLOYMENT_ID" ]]
	[[ "$after_runtime" == "$RUNTIME_ID" && "$after_port" == "$RUNTIME_PORT" && -z "$removed" ]]
}

assert_runtime_healthy() {
	root_ssh <<REMOTE
set -euo pipefail
runuser -u pneuma -- bash -lc 'podman ps --format "{{.Names}}" --filter "name=^$RUNTIME_CONTAINER\$" | grep -qx "$RUNTIME_CONTAINER"'
curl -fsS "http://127.0.0.1:$RUNTIME_PORT/" >/dev/null
REMOTE
}

fragment_path() {
	printf '/etc/caddy/applications/%s.caddy\n' "$APP_ID"
}

assert_public_route() {
	local path
	path=$(fragment_path)
	root_ssh <<REMOTE
set -euo pipefail
test -s '$path'
REMOTE
}

candidate_digest() {
	local fixture="$1" tag="reconciliation-${CASE_DIR##*/}"
	pneuma_ssh <<REMOTE
set -euo pipefail
work="/tmp/pneuma-$tag"
rm -rf "\$work"
	cp -a "/var/lib/pneuma/repos/$fixture-work" "\$work"
sed -i 's/v1.0/candidate/g' "\$work/server.py"
podman build -q -t '$REGISTRY/$fixture:$tag' "\$work" >/dev/null
podman push --tls-verify=false '$REGISTRY/$fixture:$tag' >/dev/null
rm -rf "\$work"
curl -fsS -H 'Accept: application/vnd.oci.image.manifest.v1+json' 'http://$REGISTRY/v2/$fixture/manifests/$tag' -D - -o /dev/null | awk 'BEGIN { IGNORECASE=1 } /^docker-content-digest:/ { print \$2 }' | tr -d '\r'
REMOTE
}

spawn_gated_deploy() {
	local fixture="$1" gate="$2" digest pid
	digest=$(candidate_digest "$fixture")
	[[ -n "$digest" ]]
	CANDIDATE_DIGEST="$digest"
	root_ssh <<REMOTE
set -euo pipefail
rm -rf '$GATE_ROOT/${CASE_DIR##*/}'
install -d -o pneuma -g pneuma -m 0700 '$GATE_ROOT/${CASE_DIR##*/}'
REMOTE
	pid=$(
		pneuma_ssh <<REMOTE
set -euo pipefail
cd "\$HOME"
nohup env PNEUMA_TEST_GATE_DIRECTORY='$GATE_ROOT/${CASE_DIR##*/}' pneuma app deploy '$APP' --image '$REGISTRY/$fixture@$digest' >'$GATE_ROOT/${CASE_DIR##*/}/deploy.log' 2>&1 &
REMOTE
	)
	printf '%s\n' "$pid" >"$CASE_DIR/deploy.pid"
	local marker="$GATE_ROOT/${CASE_DIR##*/}/$gate.ready"
	for _ in $(seq 1 100); do
		if root_ssh <<REMOTE; then
test -f '$marker'
REMOTE
			return
		fi
		sleep 0.1
	done
	printf 'timed out waiting for deterministic gate %s\n' "$gate" >&2
	return 1
}

kill_gated_deploy() {
	local pid
	pid=$(<"$CASE_DIR/deploy.pid")
	root_ssh <<REMOTE
set -euo pipefail
kill -TERM '$pid' 2>/dev/null || true
for _ in \$(seq 1 50); do
  kill -0 '$pid' 2>/dev/null || exit 0
  sleep 0.1
done
kill -KILL '$pid' 2>/dev/null || true
REMOTE
	root_ssh <<REMOTE
cat '$GATE_ROOT/${CASE_DIR##*/}/deploy.log' 2>/dev/null || true
REMOTE
}

release_gated_deploy() {
	local gate="$1" pid
	touch_file="$GATE_ROOT/${CASE_DIR##*/}/$gate.release"
	root_ssh <<REMOTE
set -euo pipefail
touch '$touch_file'
chown pneuma:pneuma '$touch_file'
REMOTE
	pid=$(<"$CASE_DIR/deploy.pid")
	root_ssh <<REMOTE
set -euo pipefail
for _ in \$(seq 1 300); do
  kill -0 '$pid' 2>/dev/null || exit 0
  sleep 0.1
done
exit 1
REMOTE
}

R1_runtime_container_removed_before_observation() {
	prepare_baseline healthy-http
	remove_active_container
	local output
	output=$(run_reconcile reconcile)
	assert_result repaired "$output"
	assert_same_logical_runtime
	assert_runtime_healthy
	snapshot after
}

R2_runtime_container_removed_after_status() {
	prepare_baseline healthy-http
	remove_active_container
	pneuma_ssh >"$CASE_DIR/status.log" 2>&1 <<REMOTE
set -euo pipefail
cd "\$HOME"
pneuma app status "$APP"
REMOTE
	assert_contains 'Observed state: Missing' "$CASE_DIR/status.log"
	local output
	output=$(run_reconcile reconcile)
	assert_result repaired "$output"
	assert_same_logical_runtime
	assert_runtime_healthy
	snapshot after
}

R3_runtime_unit_present_container_absent() {
	prepare_baseline healthy-http
	pneuma_ssh >"$CASE_DIR/unit-before.container" <<REMOTE
cat "\$HOME/.config/containers/systemd/$RUNTIME_CONTAINER.container"
REMOTE
	remove_active_container
	local output
	output=$(run_reconcile reconcile)
	assert_result repaired "$output"
	pneuma_ssh >"$CASE_DIR/unit-after.container" <<REMOTE
cat "\$HOME/.config/containers/systemd/$RUNTIME_CONTAINER.container"
REMOTE
	assert_query_equal "$CASE_DIR/unit-before.container" "$CASE_DIR/unit-after.container" 'canonical Quadlet source'
	assert_same_logical_runtime
	assert_runtime_healthy
	snapshot after
}

R4_runtime_unit_and_container_absent() {
	prepare_baseline healthy-http
	remove_active_container
	remove_expected_unit
	local output
	output=$(run_reconcile reconcile)
	assert_result repaired "$output"
	pneuma_ssh <<REMOTE
test -s "\$HOME/.config/containers/systemd/$RUNTIME_CONTAINER.container"
REMOTE
	assert_same_logical_runtime
	assert_runtime_healthy
	snapshot after
}

R5_runtime_divergent_identity() {
	local variant output
	for variant in unit name application-label digest-label port; do
		prepare_baseline healthy-http
		sql "SELECT id, external_runtime_id, host_port, removed_at FROM runtime_instances WHERE id = '$RUNTIME_ID';" >"$CASE_DIR/$variant-before.tsv"
		case "$variant" in
		unit)
			pneuma_ssh <<REMOTE
printf '\n# divergent test source\n' >> "\$HOME/.config/containers/systemd/$RUNTIME_CONTAINER.container"
REMOTE
			;;
		name)
			pneuma_ssh <<REMOTE
podman rename '$RUNTIME_CONTAINER' '$RUNTIME_CONTAINER-divergent'
REMOTE
			;;
		application-label | digest-label | port)
			remove_active_container
			pneuma_ssh <<REMOTE
set -euo pipefail
image=\$(sqlite3 '$DATABASE_PATH' "SELECT image_reference FROM releases WHERE id = (SELECT release_id FROM deployments WHERE id = '$DEPLOYMENT_ID');")
digest=\$(sqlite3 '$DATABASE_PATH' "SELECT image_digest FROM releases WHERE id = (SELECT release_id FROM deployments WHERE id = '$DEPLOYMENT_ID');")
case '$variant' in
application-label) podman run -d --name '$RUNTIME_CONTAINER' -p '127.0.0.1:$RUNTIME_PORT:8080' --label io.pneuma.application=wrong --label io.pneuma.image-digest="\$digest" "\$image" ;;
digest-label) podman run -d --name '$RUNTIME_CONTAINER' -p '127.0.0.1:$RUNTIME_PORT:8080' --label io.pneuma.application='$APP' --label io.pneuma.image-digest=sha256:wrong "\$image" ;;
port) podman run -d --name '$RUNTIME_CONTAINER' -p '127.0.0.1:$((RUNTIME_PORT + 1)):8080' --label io.pneuma.application='$APP' --label io.pneuma.image-digest="\$digest" "\$image" ;;
esac
REMOTE
			;;
		esac
		output=$(run_reconcile "reconcile-$variant")
		assert_result manual-intervention "$output"
		sql "SELECT id, external_runtime_id, host_port, removed_at FROM runtime_instances WHERE id = '$RUNTIME_ID';" >"$CASE_DIR/$variant-after.tsv"
		assert_query_equal "$CASE_DIR/$variant-before.tsv" "$CASE_DIR/$variant-after.tsv" "runtime row for $variant drift"
		snapshot "after-$variant"
	done
}

R6_runtime_reboot_running() {
	prepare_baseline healthy-http
	local before after output
	before=$(
		root_ssh <<'REMOTE'
cat /proc/sys/kernel/random/boot_id
REMOTE
	)
	reboot_vm
	after=$(
		root_ssh <<'REMOTE'
cat /proc/sys/kernel/random/boot_id
REMOTE
	)
	[[ "$before" != "$after" ]]
	local output
	output=$(run_reconcile reconcile)
	grep -Eq 'Result: (no-op|repaired)' "$output"
	assert_same_logical_runtime
	assert_runtime_healthy
	snapshot after
}

R7_runtime_reboot_stopped() {
	prepare_baseline healthy-http
	pneuma_ssh >"$CASE_DIR/stop.log" 2>&1 <<REMOTE
set -euo pipefail
cd "\$HOME"
pneuma app stop "$APP"
REMOTE
	reboot_vm
	local output
	output=$(run_reconcile reconcile)
	assert_result no-op "$output"
	sql "SELECT id, removed_at FROM runtime_instances WHERE id = '$RUNTIME_ID';" >"$CASE_DIR/runtime-after.tsv"
	assert_contains "$RUNTIME_ID" "$CASE_DIR/runtime-after.tsv"
	! pneuma_ssh <<REMOTE
podman ps --format '{{.Names}}' --filter 'name=^$RUNTIME_CONTAINER\$' | grep -q .
REMOTE
	snapshot after
}

E1_exposure_public_fragment_removed() {
	prepare_baseline redirect-public
	root_ssh <<REMOTE
rm -f '$(fragment_path)'
REMOTE
	local output
	output=$(run_reconcile reconcile)
	assert_result repaired "$output"
	assert_public_route
	assert_same_logical_runtime
	snapshot after
}

E2_exposure_divergent_upstream() {
	prepare_baseline redirect-public
	root_ssh <<REMOTE
sed -i 's/127.0.0.1:[0-9]\\+/127.0.0.1:1/' '$(fragment_path)'
caddy reload --config /etc/caddy/Caddyfile
REMOTE
	local output
	output=$(run_reconcile reconcile)
	assert_result repaired "$output"
	assert_public_route
	snapshot after
}

install_caddy_wrapper() {
	local failures="$1"
	pneuma_ssh <<REMOTE
set -euo pipefail
install -d -m 0700 "\$HOME/.local/bin" "\$HOME/.local/share"
printf '%s\n' '$failures' >"\$HOME/.local/share/pneuma-caddy-wrapper-failures"
cat >"\$HOME/.local/bin/caddy" <<'WRAPPER'
#!/usr/bin/env bash
set -euo pipefail
state="\$HOME/.local/share/pneuma-caddy-wrapper-count"
failures_file="\$HOME/.local/share/pneuma-caddy-wrapper-failures"
if [[ "\${1:-}" == reload ]]; then
  count=\$(cat "\$state" 2>/dev/null || printf 0)
  count=\$((count + 1))
  printf '%s\n' "\$count" >"\$state"
  if (( count <= \$(cat "\$failures_file") )); then
    printf 'intentional caddy reload failure\n' >&2
    exit 1
  fi
fi
exec /usr/bin/caddy "\$@"
WRAPPER
chmod 0700 "\$HOME/.local/bin/caddy"
rm -f "\$HOME/.local/share/pneuma-caddy-wrapper-count"
REMOTE
}

remove_caddy_wrapper() {
	pneuma_ssh <<'REMOTE'
rm -f "$HOME/.local/bin/caddy" "$HOME/.local/share/pneuma-caddy-wrapper-count" "$HOME/.local/share/pneuma-caddy-wrapper-failures"
REMOTE
}

install_observation_traps() {
	pneuma_ssh <<'REMOTE'
set -euo pipefail
install -d -m 0700 "$HOME/.local/bin" "$HOME/.local/share"
: >"$HOME/.local/share/pneuma-observation-trace"
for command in podman systemctl caddy curl; do
  cat >"$HOME/.local/bin/$command" <<WRAPPER
#!/usr/bin/env bash
printf '%s\n' '$command' >>"$HOME/.local/share/pneuma-observation-trace"
exec /usr/bin/$command "\$@"
WRAPPER
  chmod 0700 "$HOME/.local/bin/$command"
done
REMOTE
}

remove_observation_traps() {
	pneuma_ssh <<'REMOTE'
set -euo pipefail
cp "$HOME/.local/share/pneuma-observation-trace" /tmp/pneuma-observation-trace
rm -f "$HOME/.local/bin/podman" "$HOME/.local/bin/systemctl" "$HOME/.local/bin/caddy" "$HOME/.local/bin/curl"
REMOTE
	root_ssh >"$CASE_DIR/observation-trace.txt" <<'REMOTE'
cat /tmp/pneuma-observation-trace
REMOTE
}

E3_exposure_correct_fragment_reload_unconfirmed() {
	prepare_baseline redirect-public
	# A correct file alone is not confirmation. Clear only the confirmed state so
	# reconciliation must validate and reload the still-canonical fragment.
	sql "UPDATE exposures SET materialization_state = 'failed', last_error_code = 'test_unconfirmed_reload', last_error_message = 'test setup' WHERE application_id = '$APP_ID';"
	install_caddy_wrapper 1
	local output
	output=$(run_reconcile_with_path reconcile)
	remove_caddy_wrapper
	grep -Eq 'Result: (failed|diverged)' "$output"
	sql "SELECT materialization_state, last_error_code FROM exposures WHERE application_id = '$APP_ID';" >"$CASE_DIR/exposure-after.tsv"
	grep -Eq '^(failed|diverged)\t' "$CASE_DIR/exposure-after.tsv"
	snapshot after
}

E4_exposure_public_intent_without_route() {
	prepare_baseline redirect-public
	root_ssh <<REMOTE
rm -f '$(fragment_path)'
REMOTE
	local output
	output=$(run_reconcile_with_path reconcile)
	grep -Eq 'Result: (repaired|failed)' "$output"
	assert_contains 'public' <(sql "SELECT desired_visibility FROM exposures WHERE application_id = '$APP_ID';")
	if grep -qF 'Result: repaired' "$output"; then assert_public_route; fi
	snapshot after
}

E5_exposure_internal_intent_with_route() {
	prepare_baseline redirect-public
	root_ssh <<REMOTE
cp '$(fragment_path)' '$(fragment_path).reconciliation-backup'
REMOTE
	pneuma_ssh >"$CASE_DIR/internal.log" 2>&1 <<REMOTE
set -euo pipefail
cd "\$HOME"
pneuma app visibility set "$APP" internal
REMOTE
	root_ssh <<REMOTE
cp '$(fragment_path).reconciliation-backup' '$(fragment_path)'
REMOTE
	local output
	output=$(run_reconcile reconcile)
	assert_result repaired "$output"
	! remote_file "$(fragment_path)"
	assert_runtime_healthy
	assert_contains 'not_materialized' <(sql "SELECT materialization_state FROM exposures WHERE application_id = '$APP_ID';")
	snapshot after
}

E6_exposure_compensation_fails() {
	prepare_baseline redirect-public
	root_ssh <<REMOTE
rm -f '$(fragment_path)'
REMOTE
	install_caddy_wrapper 99
	local output
	output=$(run_reconcile_with_path reconcile)
	remove_caddy_wrapper
	assert_result diverged "$output"
	assert_contains 'diverged' <(sql "SELECT materialization_state FROM exposures WHERE application_id = '$APP_ID';")
	assert_contains 'caddy_materialization_failed' <(sql "SELECT last_error_code FROM exposures WHERE application_id = '$APP_ID';")
	snapshot after
}

interrupted_case() {
	local fixture="$1" gate="$2" expected="$3"
	prepare_baseline "$fixture"
	local active_before="$DEPLOYMENT_ID" runtime_before="$RUNTIME_ID"
	spawn_gated_deploy "$fixture" "$gate"
	snapshot gated
	kill_gated_deploy >"$CASE_DIR/interrupted-deploy.log"
	local output
	output=$(run_reconcile reconcile)
	grep -Eq "Result: ($expected)" "$output"
	assert_contains "$active_before" <(sql "SELECT active_deployment_id FROM applications WHERE id = '$APP_ID';")
	assert_contains "$runtime_before" <(sql "SELECT id FROM runtime_instances WHERE id = '$runtime_before' AND removed_at IS NULL;")
	assert_contains 'operation_interrupted' <(sql "SELECT failure_code FROM deployments WHERE application_id = '$APP_ID' AND status = 'failed' ORDER BY requested_at DESC LIMIT 1;")
	snapshot after
}

I1_interrupted_pending() { interrupted_case healthy-http deployment.pending 'failed'; }
I2_interrupted_starting() { interrupted_case healthy-http deployment.starting-registered 'failed|manual-intervention'; }
I3_interrupted_verifying() { interrupted_case healthy-http deployment.verifying 'failed|manual-intervention'; }
I4_interrupted_activating() { interrupted_case redirect-public deployment.activating 'failed|diverged|manual-intervention'; }

C1_concurrency_repeated_reconcile() {
	prepare_baseline healthy-http
	local first second
	first=$(run_reconcile first-correct)
	second=$(run_reconcile second-correct)
	assert_result no-op "$first"
	assert_result no-op "$second"
	remove_active_container
	first=$(run_reconcile first-repair)
	second=$(run_reconcile second-repair)
	assert_result repaired "$first"
	assert_result no-op "$second"
	assert_same_logical_runtime
	[[ "$(sql "SELECT COUNT(*) FROM deployments WHERE application_id = '$APP_ID';")" -eq 1 ]]
	[[ "$(sql "SELECT COUNT(*) FROM runtime_instances WHERE application_id = '$APP_ID';")" -eq 1 ]]
	snapshot after
}

C2_concurrency_parallel_reconcile() {
	prepare_baseline healthy-http
	root_ssh <<REMOTE
set -euo pipefail
rm -rf '$GATE_ROOT/${CASE_DIR##*/}'
install -d -o pneuma -g pneuma -m 0700 '$GATE_ROOT/${CASE_DIR##*/}'
REMOTE
	pneuma_ssh >"$CASE_DIR/reconcile-a.pid" <<REMOTE
set -euo pipefail
cd "\$HOME"
nohup env PNEUMA_TEST_GATE_DIRECTORY='$GATE_ROOT/${CASE_DIR##*/}' pneuma reconcile '$APP' >'$GATE_ROOT/${CASE_DIR##*/}/reconcile-a.log' 2>&1 &
printf '%s\n' "\$!"
REMOTE
	for _ in $(seq 1 100); do
		if root_ssh <<REMOTE; then
test -f '$GATE_ROOT/${CASE_DIR##*/}/reconciliation.application-lock-acquired.ready'
REMOTE
			break
		fi
		sleep 0.1
	done
	remote_file "$GATE_ROOT/${CASE_DIR##*/}/reconciliation.application-lock-acquired.ready"
	local output
	output=$(run_reconcile reconcile-b)
	assert_result deferred "$output"
	root_ssh <<REMOTE
set -euo pipefail
touch '$GATE_ROOT/${CASE_DIR##*/}/reconciliation.application-lock-acquired.release'
chown pneuma:pneuma '$GATE_ROOT/${CASE_DIR##*/}/reconciliation.application-lock-acquired.release'
REMOTE
	local pid
	pid=$(<"$CASE_DIR/reconcile-a.pid")
	root_ssh <<REMOTE
set -euo pipefail
for _ in \$(seq 1 300); do
  kill -0 '$pid' 2>/dev/null || exit 0
  sleep 0.1
done
exit 1
REMOTE
	root_ssh >"$CASE_DIR/reconcile-a.log" <<REMOTE
cat '$GATE_ROOT/${CASE_DIR##*/}/reconcile-a.log'
REMOTE
	assert_result no-op "$CASE_DIR/reconcile-a.log"
	assert_same_logical_runtime
	snapshot after
}

C3_concurrency_deploy_deploy() {
	prepare_baseline healthy-http
	spawn_gated_deploy healthy-http deployment.pending
	pneuma_ssh >"$CASE_DIR/deploy-b.log" 2>&1 <<REMOTE || true
set +e
cd "\$HOME"
pneuma app deploy "$APP" --image '$REGISTRY/healthy-http@$CANDIDATE_DIGEST'
REMOTE
	assert_contains 'already has an operation in progress' "$CASE_DIR/deploy-b.log"
	release_gated_deploy deployment.pending
	root_ssh >"$CASE_DIR/deploy-a.log" <<REMOTE
cat '$GATE_ROOT/${CASE_DIR##*/}/deploy.log'
REMOTE
	assert_contains 'Status: Succeeded' "$CASE_DIR/deploy-a.log"
	[[ "$(sql "SELECT COUNT(*) FROM runtime_instances WHERE application_id = '$APP_ID' AND state = 'running' AND removed_at IS NULL;")" -eq 1 ]]
	snapshot after
}

C4_concurrency_deploy_reconcile() {
	prepare_baseline healthy-http
	spawn_gated_deploy healthy-http deployment.pending
	install_observation_traps
	local output
	output=$(run_reconcile_with_path reconcile)
	remove_observation_traps
	assert_result deferred "$output"
	snapshot gated
	assert_query_equal "$CASE_DIR/before-podman.txt" "$CASE_DIR/gated-podman.txt" 'Podman state before pending deployment release'
	[[ ! -s "$CASE_DIR/observation-trace.txt" ]]
	kill_gated_deploy >"$CASE_DIR/interrupted-deploy.log"
	snapshot after
}

case_function() {
	case "$1" in
	R1) printf '%s\n' R1_runtime_container_removed_before_observation ;;
	R2) printf '%s\n' R2_runtime_container_removed_after_status ;;
	R3) printf '%s\n' R3_runtime_unit_present_container_absent ;;
	R4) printf '%s\n' R4_runtime_unit_and_container_absent ;;
	R5) printf '%s\n' R5_runtime_divergent_identity ;;
	R6) printf '%s\n' R6_runtime_reboot_running ;;
	R7) printf '%s\n' R7_runtime_reboot_stopped ;;
	E1) printf '%s\n' E1_exposure_public_fragment_removed ;;
	E2) printf '%s\n' E2_exposure_divergent_upstream ;;
	E3) printf '%s\n' E3_exposure_correct_fragment_reload_unconfirmed ;;
	E4) printf '%s\n' E4_exposure_public_intent_without_route ;;
	E5) printf '%s\n' E5_exposure_internal_intent_with_route ;;
	E6) printf '%s\n' E6_exposure_compensation_fails ;;
	I1) printf '%s\n' I1_interrupted_pending ;;
	I2) printf '%s\n' I2_interrupted_starting ;;
	I3) printf '%s\n' I3_interrupted_verifying ;;
	I4) printf '%s\n' I4_interrupted_activating ;;
	C1) printf '%s\n' C1_concurrency_repeated_reconcile ;;
	C2) printf '%s\n' C2_concurrency_parallel_reconcile ;;
	C3) printf '%s\n' C3_concurrency_deploy_deploy ;;
	C4) printf '%s\n' C4_concurrency_deploy_reconcile ;;
	*) return 1 ;;
	esac
}

preflight() {
	root_ssh <<'REMOTE'
set -euo pipefail
command -v sqlite3 >/dev/null
command -v caddy >/dev/null
test -x /usr/local/bin/pneuma
runuser -u pneuma -- bash -lc 'command -v podman >/dev/null && systemctl --user show-environment >/dev/null'
REMOTE
}

# Build/push fixture images and create their local Git repositories once. Every
# case still resets the database and materialization, then imports/deploys only
# its own fixture from these immutable inputs.
prepare_catalog_fixtures() {
	"$SCRIPT_DIR/reset-fixtures.sh" "$SSH_HOST" >"$LOG_ROOT/catalog-reset.log" 2>&1
	"$SCRIPT_DIR/rebuild-fixtures.sh" "$SSH_HOST" >"$LOG_ROOT/catalog-rebuild.log" 2>&1
	"$SCRIPT_DIR/deploy-all-fixtures.sh" "$SSH_HOST" >"$LOG_ROOT/catalog-repositories.log" 2>&1
	"$SCRIPT_DIR/reset-fixtures.sh" "$SSH_HOST" >"$LOG_ROOT/catalog-ready.log" 2>&1
}

run_case() {
	local id="$1" function
	function=$(case_function "$id") || {
		report fail "$id unknown catalog case"
		return
	}
	CASE_DIR="$LOG_ROOT/$id"
	mkdir -p "$CASE_DIR"
	if ("$function") >"$CASE_DIR/case.log" 2>&1; then
		report pass "$id ${function#*_} ($CASE_DIR)"
	else
		local status=$?
		if [[ "$status" -eq 77 ]]; then
			report skip "$id ${function#*_}: $(<"$CASE_DIR/skip.txt")"
		else
			report fail "$id ${function#*_} (see $CASE_DIR/case.log)"
		fi
	fi
}

ALL_CASES=(R1 R2 R3 R4 R5 R6 R7 E1 E2 E3 E4 E5 E6 I1 I2 I3 I4 C1 C2 C3 C4)

if ! preflight >"$LOG_ROOT/preflight.log" 2>&1; then
	for id in "${ALL_CASES[@]}"; do
		if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == "$id" ]]; then
			report skip "$id unavailable VM dependency; see $LOG_ROOT/preflight.log"
		fi
	done
	printf '\n%d passed, %d failed, %d skipped. Logs: %s\n' "$PASS_COUNT" "$FAIL_COUNT" "$SKIP_COUNT" "$LOG_ROOT"
	exit 0
fi

if ! prepare_catalog_fixtures; then
	for id in "${ALL_CASES[@]}"; do
		if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == "$id" ]]; then
			report skip "$id unavailable fixture dependency; see $LOG_ROOT/catalog-*.log"
		fi
	done
	printf '\n%d passed, %d failed, %d skipped. Logs: %s\n' "$PASS_COUNT" "$FAIL_COUNT" "$SKIP_COUNT" "$LOG_ROOT"
	exit 0
fi

for id in "${ALL_CASES[@]}"; do
	if [[ "$REQUESTED_CASE" == all || "$REQUESTED_CASE" == "$id" ]]; then
		run_case "$id"
	fi
done

if [[ "$REQUESTED_CASE" != all ]] && ! case_function "$REQUESTED_CASE" >/dev/null; then
	printf 'unknown case ID: %s\n' "$REQUESTED_CASE" >&2
	exit 2
fi

printf '\n%d passed, %d failed, %d skipped. Logs: %s\n' "$PASS_COUNT" "$FAIL_COUNT" "$SKIP_COUNT" "$LOG_ROOT"
[[ "$FAIL_COUNT" -eq 0 ]]
