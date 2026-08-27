#!/usr/bin/env bash
#
# Disposable-VM regression orchestrator for Pneuma
#
# Automates the complete disposable lifecycle that the final regression used to
# perform by hand: clone the immutable `pneuma-dev-base` template as
# `pneuma-dev-base-test` through `qemu:///system`, pin a static DHCP lease so
# the address survives the in-suite reboot, boot and wait for SSH, provision
# the host, install the Pneuma binary, install the restricted CI dispatcher
# key, run the requested suites, and always destroy the clone including its
# storage — whether the suites pass or fail.
#
# Suites:
#   all             (default) two clones: a shared clone runs test-all.sh,
#                   then a verified reset plus registry restart, then
#                   reconciliation-e2e.sh; afterwards test-bootstrap-vps.sh
#                   runs alone on its own pristine clone.
#   e2e             functional battery only (test-all.sh) on its own clone.
#   reconciliation  drift catalog only (reconciliation-e2e.sh) on its own
#                   clone, prepared with fixtures and registry images.
#   bootstrap       clean-host bootstrap acceptance only, on a pristine clone;
#                   provisioning is intentionally skipped because exercising
#                   scripts/bootstrap-vps.sh is the point of this suite.
#
# Options:
#   --keep-on-fail     preserve the failed clone for debugging instead of
#                      destroying it; passing clones are never kept.
#   --ci-key <path>    restricted CI dispatcher private key
#                      (default: ~/.ssh/pneuma-ci-test).
#   --source-url <url> bootstrap suite source repository (default: origin
#                      rewritten to HTTPS when it is a GitHub SSH URL).
#   --ref <ref>        immutable ref passed to test-bootstrap-vps.sh
#                      (default: current HEAD SHA).
#
# Root access resolution for the fresh clone:
#   1. A provisioning SSH key: $PNEUMA_VM_PROVISION_KEY when set, otherwise
#      ~/.ssh/pneuma-e2e-final, otherwise a new pair is generated under
#      ~/.ssh/pneuma-provision. The key is loaded into a run-local ssh-agent so
#      the existing battery scripts authenticate without config changes.
#   2. Only when no key works: the root password taken from
#      $PNEUMA_VM_ROOT_PASSWORD through sshpass, used once to install the
#      provisioning public key. The password is never written to disk here.
#
# Prerequisites: libvirt (virsh/virt-clone) with the default NAT network,
# `pneuma-dev-base` shut off, host toolchain (cargo) for suites that sync the
# binary, and Internet access from the VM for provisioning and bootstrap.
#
# Usage:
#   scripts/dev-vm/test-regression.sh [suite] [options]

set -euo pipefail

LIBVIRT_URI="qemu:///system"
BASE_DOMAIN="pneuma-dev-base"
DOMAIN="pneuma-dev-base-test"
NETWORK="default"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LOG_ROOT="${TMPDIR:-/tmp}/pneuma-regression-$(date +%Y%m%d-%H%M%S)"

SUITE="all"
KEEP_ON_FAIL=false
SOURCE_URL="${PNEUMA_BOOTSTRAP_SOURCE_URL:-}"
REF="${PNEUMA_BOOTSTRAP_REF:-}"
CI_KEY="${PNEUMA_CI_KEY:-$HOME/.ssh/pneuma-ci-test}"

CLONE_IP=""
CLONE_ACTIVE=0
AGENT_PID=""
PROVISION_KEY=""
SHARED_RC=0
BOOTSTRAP_RC=0
SSH_OPTS=(-o ConnectTimeout=10 -o BatchMode=yes)

usage() {
	sed -n '/^# Suites:/,/^#   scripts\/dev-vm\/test-regression.sh/p' \
		"${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
	printf 'ERROR: %s\n' "$1" >&2
	exit 1
}

step() {
	printf '\n==> %s\n' "$1"
}

vsh() {
	virsh -c "$LIBVIRT_URI" "$@"
}

destroy_clone() {
	step "Destroying disposable clone $DOMAIN..."
	vsh destroy "$DOMAIN" >/dev/null 2>&1 || true
	if ! vsh undefine --nvram --remove-all-storage "$DOMAIN" >/dev/null 2>&1; then
		if ! vsh undefine --remove-all-storage "$DOMAIN" >/dev/null 2>&1; then
			printf 'WARNING: failed to fully destroy %s; inspect libvirt manually\n' \
				"$DOMAIN" >&2
			return 0
		fi
	fi
	if [[ -n "$CLONE_IP" ]]; then
		ssh-keygen -R "$CLONE_IP" >/dev/null 2>&1 || true
	fi
	# Remove this run's static lease entry so later clones of the same name
	# do not collide with it.
	vsh net-update "$NETWORK" delete ip-dhcp-host \
		"<host name='$DOMAIN'/>" --live --config >/dev/null 2>&1 || true
	CLONE_ACTIVE=0
	CLONE_IP=""
	echo "Clone destroyed."
}

cleanup() {
	local rc=$?
	if ((CLONE_ACTIVE)); then
		if ((rc == 0)) || ! $KEEP_ON_FAIL; then
			destroy_clone
		else
			printf '\nKeeping %s (%s) for debugging (--keep-on-fail).\n' \
				"$DOMAIN" "${CLONE_IP:-unknown ip}" >&2
		fi
	fi
	if [[ -n "$AGENT_PID" ]]; then
		ssh-agent -k >/dev/null 2>&1 || true
	fi
	return "$rc"
}

trap cleanup EXIT

parse_args() {
	while (($#)); do
		case "$1" in
		all | e2e | reconciliation | bootstrap)
			SUITE="$1"
			;;
		--keep-on-fail)
			KEEP_ON_FAIL=true
			;;
		--ci-key)
			[[ -n "${2:-}" ]] || die "--ci-key requires a path"
			CI_KEY="$2"
			shift
			;;
		--source-url)
			[[ -n "${2:-}" ]] || die "--source-url requires a value"
			SOURCE_URL="$2"
			shift
			;;
		--ref)
			[[ -n "${2:-}" ]] || die "--ref requires a value"
			REF="$2"
			shift
			;;
		-h | --help)
			usage
			exit 0
			;;
		*)
			die "unknown argument: $1 (see --help)"
			;;
		esac
		shift
	done
}

preflight() {
	step "Preflight..."
	for tool in virsh virt-clone ssh scp ssh-keyscan ssh-keygen tar awk timeout setsid; do
		command -v "$tool" >/dev/null || die "missing required tool: $tool"
	done
	[[ -f "$REPO_ROOT/scripts/dev-vm/provision-host.sh" ]] ||
		die "repository layout not found under $REPO_ROOT"

	local base_state
	base_state=$(vsh domstate "$BASE_DOMAIN" 2>/dev/null ||
		die "base domain $BASE_DOMAIN not found on $LIBVIRT_URI")
	case "$base_state" in
	"shut off") ;;
	*) die "base domain $BASE_DOMAIN must be shut off (found: $base_state); refusing to touch it" ;;
	esac

	if vsh dominfo "$DOMAIN" >/dev/null 2>&1; then
		die "domain $DOMAIN already exists; destroy it before rerunning"
	fi

	case "$SUITE" in
	e2e | all)
		[[ -f "$CI_KEY" && -f "${CI_KEY}.pub" ]] || die "CI key pair missing under $CI_KEY*"
		command -v cargo >/dev/null || die "cargo is required to sync the binary"
		;;
	reconciliation)
		command -v cargo >/dev/null || die "cargo is required to sync the binary"
		;;
	esac

	mkdir -p "$LOG_ROOT"
	echo "Suite: $SUITE"
	echo "Logs: $LOG_ROOT"
}

resolve_provision_key() {
	if [[ -n "${PNEUMA_VM_PROVISION_KEY:-}" ]]; then
		PROVISION_KEY="$PNEUMA_VM_PROVISION_KEY"
	elif [[ -f "$HOME/.ssh/pneuma-e2e-final" ]]; then
		PROVISION_KEY="$HOME/.ssh/pneuma-e2e-final"
	else
		PROVISION_KEY="$HOME/.ssh/pneuma-provision"
		if [[ ! -f "$PROVISION_KEY" ]]; then
			step "Generating provisioning key $PROVISION_KEY..."
			ssh-keygen -q -t ed25519 -N "" -f "$PROVISION_KEY" -C "pneuma regression provisioning"
		fi
	fi
	[[ -f "$PROVISION_KEY" ]] || die "provisioning key not found: $PROVISION_KEY"
	[[ -f "$PROVISION_KEY.pub" ]] || die "provisioning public key not found: $PROVISION_KEY.pub"
}

start_agent() {
	eval "$(ssh-agent -s)" >/dev/null
	AGENT_PID="$SSH_AGENT_PID"
	ssh-add "$PROVISION_KEY" >/dev/null ||
		die "could not load $PROVISION_KEY into the run-local agent (passphrase protected?)"
}

clone_vm() {
	step "Cloning $BASE_DOMAIN as $DOMAIN..."
	virt-clone --connect "$LIBVIRT_URI" \
		--original "$BASE_DOMAIN" --name "$DOMAIN" --auto-clone >/dev/null
	CLONE_ACTIVE=1
}

pin_dhcp_lease() {
	local mac gateway subnet used ip candidate i
	# Capture first, then parse: closing the read end early (awk exit, head)
	# SIGPIPEs the libvirt writer and, under pipefail, kills this script.
	local ifaces net_xml
	ifaces=$(vsh domiflist "$DOMAIN") ||
		die "could not list interfaces of $DOMAIN"
	mac=$(awk '$2 == "network" { print $5; exit }' <<<"$ifaces")
	[[ -n "$mac" ]] || die "no network interface found on $DOMAIN"
	net_xml=$(vsh net-dumpxml "$NETWORK") ||
		die "could not dump libvirt network $NETWORK"
	gateway=$(grep -oE "ip address='[0-9.]+'" <<<"$net_xml" |
		grep -oE '[0-9.]+' | sed -n '1p')
	[[ -n "$gateway" ]] || die "could not determine subnet of libvirt network $NETWORK"
	subnet="${gateway%.*}"

	used=" $gateway "
	while read -r leased; do
		used+="$leased "
	done < <(vsh net-dhcp-leases "$NETWORK" | grep -oE "${subnet}\.[0-9]+" | sort -u)

	ip=""
	for i in $(seq 200 249); do
		candidate="$subnet.$i"
		case "$used" in
		*" $candidate "*) ;;
		*)
			ip="$candidate"
			break
			;;
		esac
	done
	[[ -n "$ip" ]] || die "no free DHCP slot found in $subnet.200-249"

	# A previous run may have left a static entry for this clone name behind
	# (entries persist in the network config across clone lifecycles).
	vsh net-update "$NETWORK" delete ip-dhcp-host \
		"<host name='$DOMAIN'/>" --live --config >/dev/null 2>&1 || true

	vsh net-update "$NETWORK" add ip-dhcp-host \
		"<host mac='$mac' name='$DOMAIN' ip='$ip'/>" --live --config >/dev/null
	CLONE_IP="$ip"
	echo "Static lease pinned: $mac -> $ip"
}

wait_for_boot() {
	local i up=0
	step "Starting $DOMAIN and waiting for SSH on $CLONE_IP..."
	vsh start "$DOMAIN" >/dev/null
	for i in $(seq 1 60); do
		if timeout 4 bash -c "exec 3<>/dev/tcp/$CLONE_IP/22" 2>/dev/null; then
			up=1
			break
		fi
		sleep 5
	done
	((up)) || die "SSH port never became reachable on $CLONE_IP"

	ssh-keygen -R "$CLONE_IP" >/dev/null 2>&1 || true
	local keys
	keys=$(ssh-keyscan -T 10 -t ed25519,ecdsa "$CLONE_IP" 2>/dev/null)
	[[ -n "$keys" ]] || die "could not scan the host key of $CLONE_IP"
	mkdir -p "$HOME/.ssh"
	touch "$HOME/.ssh/known_hosts"
	while IFS= read -r line; do
		[[ -n "$line" ]] || continue
		grep -qxF "$line" "$HOME/.ssh/known_hosts" ||
			printf '%s\n' "$line" >>"$HOME/.ssh/known_hosts"
	done <<<"$keys"
	echo "VM is up (host key recorded)."
}

root_reachable() {
	ssh "${SSH_OPTS[@]}" "root@$CLONE_IP" 'true' >/dev/null 2>&1
}

ensure_root_access() {
	step "Establishing root SSH access to $CLONE_IP..."
	if root_reachable; then
		echo "Provisioning key accepted."
		return
	fi
	if [[ -z "${PNEUMA_VM_ROOT_PASSWORD:-}" ]] || ! command -v sshpass >/dev/null; then
		die "no working provisioning key for $CLONE_IP; set PNEUMA_VM_ROOT_PASSWORD (with sshpass installed) or provide a key via PNEUMA_VM_PROVISION_KEY"
	fi
	sshpass -p "$PNEUMA_VM_ROOT_PASSWORD" ssh \
		-o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new \
		"root@$CLONE_IP" 'mkdir -p ~/.ssh && chmod 700 ~/.ssh &&
			cat >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys' \
		<"$PROVISION_KEY.pub"
	root_reachable || die "password fallback did not grant key access to $CLONE_IP"
	echo "Provisioning public key installed; key access confirmed."
}

provision_host() {
	step "Provisioning host (packages, pneuma user, Caddy baseline)..."
	tar -C "$REPO_ROOT/scripts" -cf - dev-vm/provision-host.sh lib/provision-host.sh |
		ssh "${SSH_OPTS[@]}" "root@$CLONE_IP" \
			'tar -C /tmp -xf - && bash /tmp/dev-vm/provision-host.sh' |
		grep -vE '^(Get|Unpacking|Preparing|Setting up|Selecting|Processing triggers|Update-alternatives)' || true
	echo "Provisioning complete."
}

sync_binary() {
	step "Building and installing the Pneuma binary..."
	(
		cd "$REPO_ROOT"
		bash scripts/dev-vm/sync-binary.sh "root@$CLONE_IP"
	)
}

install_ci_key() {
	step "Installing the restricted CI dispatcher key..."
	local pub line
	pub="$(cat "${CI_KEY}.pub")"
	line="restrict,command=\"/usr/local/bin/pneuma ci dispatch\" $pub"
	printf '%s\n' "$line" | ssh "${SSH_OPTS[@]}" "root@$CLONE_IP" '
		set -euo pipefail
		mkdir -p /home/pneuma/.ssh
		chown pneuma:pneuma /home/pneuma/.ssh
		chmod 700 /home/pneuma/.ssh
		key=$(cat)
		touch /home/pneuma/.ssh/authorized_keys
		if ! grep -qxF -- "$key" /home/pneuma/.ssh/authorized_keys; then
			printf "%s\n" "$key" >> /home/pneuma/.ssh/authorized_keys
		fi
		chown pneuma:pneuma /home/pneuma/.ssh/authorized_keys
		chmod 600 /home/pneuma/.ssh/authorized_keys'
	echo "CI dispatcher key installed."
}

prepare_fixtures() {
	step "Building fixture images and Git repositories on the clone..."
	(
		cd "$REPO_ROOT"
		bash scripts/dev-vm/rebuild-fixtures.sh "root@$CLONE_IP"
		bash scripts/dev-vm/deploy-all-fixtures.sh "root@$CLONE_IP"
	)
}

ensure_registry() {
	ssh "${SSH_OPTS[@]}" "root@$CLONE_IP" \
		'runuser -u pneuma -- bash -lc "cd \$HOME && (podman start pneuma-registry 2>/dev/null || podman run -d --name pneuma-registry -p 5000:5000 docker.io/library/registry:2)"' \
		>/dev/null
	ssh "${SSH_OPTS[@]}" "root@$CLONE_IP" \
		'curl -fsS http://localhost:5000/v2/_catalog >/dev/null' ||
		die "local registry did not come back after reset"
}

verified_reset() {
	step "Verified reset between batteries..."
	(
		cd "$REPO_ROOT"
		bash scripts/dev-vm/reset-fixtures.sh "root@$CLONE_IP"
	)
	local listing
	listing=$(ssh "${SSH_OPTS[@]}" "root@$CLONE_IP" \
		'runuser -u pneuma -- bash -lc "cd \$HOME && pneuma app list"' 2>/dev/null || true)
	if printf '%s' "$listing" | grep -q $'\tRegistered'; then
		die "reset left registered applications behind:\n$listing"
	fi
	echo "Reset verified: no registered applications."
	ensure_registry
	echo "Registry container running again."
}

bootstrap_target() {
	if [[ -z "$SOURCE_URL" ]]; then
		local raw
		raw=$(git -C "$REPO_ROOT" remote get-url origin 2>/dev/null ||
			die "cannot derive the bootstrap source URL; pass --source-url")
		case "$raw" in
		git@github.com:*) SOURCE_URL="https://github.com/${raw#git@github.com:}" ;;
		*) SOURCE_URL="$raw" ;;
		esac
	fi
	if [[ -z "$REF" ]]; then
		REF=$(git -C "$REPO_ROOT" rev-parse HEAD) ||
			die "cannot derive the bootstrap ref; pass --ref"
	fi
}

run_shared_clone() {
	local rc=0 label="$1"
	set -e
	step "=== Shared clone: $label ==="
	clone_vm
	pin_dhcp_lease
	wait_for_boot
	ensure_root_access
	provision_host
	sync_binary

	if [[ "$label" != "reconciliation-only" ]]; then
		install_ci_key
	fi
	if [[ "$label" == "reconciliation-only" ]]; then
		prepare_fixtures
	fi

	set +e
	if [[ "$label" == "reconciliation-only" ]]; then
		(
			cd "$REPO_ROOT"
			setsid bash scripts/dev-vm/reconciliation-e2e.sh "root@$CLONE_IP" </dev/null
		)
	else
		(
			cd "$REPO_ROOT"
			setsid bash scripts/dev-vm/test-all.sh "root@$CLONE_IP" "$CI_KEY" </dev/null
		)
	fi
	rc=$?
	set -e

	if ((rc == 0)) && [[ "$label" == "all-batteries" ]]; then
		verified_reset
		set +e
		(
			cd "$REPO_ROOT"
			setsid bash scripts/dev-vm/reconciliation-e2e.sh "root@$CLONE_IP" </dev/null
		)
		rc=$?
		set -e
	fi

	finish_clone_group "$rc"
	SHARED_RC="$rc"
}

run_bootstrap_suite() {
	local rc=0
	set -e
	step "=== Pristine clone: bootstrap acceptance ==="
	bootstrap_target
	clone_vm
	pin_dhcp_lease
	wait_for_boot
	ensure_root_access

	set +e
	(
		cd "$REPO_ROOT"
		setsid bash scripts/test-bootstrap-vps.sh "root@$CLONE_IP" "$SOURCE_URL" --ref "$REF" </dev/null
	)
	rc=$?
	set -e

	finish_clone_group "$rc"
	BOOTSTRAP_RC="$rc"
}

finish_clone_group() {
	local rc=$1
	if [[ "$rc" -eq 0 || "$KEEP_ON_FAIL" != "true" ]]; then
		destroy_clone
	else
		printf '\nKeeping %s (%s) for debugging (--keep-on-fail).\n' \
			"$DOMAIN" "$CLONE_IP"
	fi
}

main() {
	parse_args "$@"
	preflight
	resolve_provision_key
	start_agent

	if [[ "$SUITE" == "all" || "$SUITE" == "e2e" || "$SUITE" == "reconciliation" ]]; then
		case "$SUITE" in
		e2e) run_shared_clone "e2e-only" ;;
		reconciliation) run_shared_clone "reconciliation-only" ;;
		all) run_shared_clone "all-batteries" ;;
		esac
	fi

	if [[ "$SUITE" == "all" || "$SUITE" == "bootstrap" ]] && [[ "$SHARED_RC" -eq 0 ]]; then
		run_bootstrap_suite
	fi

	echo
	echo "============================================================"
	if [[ "$SHARED_RC" -eq 0 && "$BOOTSTRAP_RC" -eq 0 ]]; then
		echo "Regression suites passed (suite: $SUITE). Logs: $LOG_ROOT"
	else
		echo "REGRESSION FAILED (suite: $SUITE, shared=$SHARED_RC, bootstrap=$BOOTSTRAP_RC)."
		echo "Logs: $LOG_ROOT"
	fi
	echo "============================================================"

	((SHARED_RC == 0 && BOOTSTRAP_RC == 0)) || exit 1
}

main "$@"
