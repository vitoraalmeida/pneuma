#!/usr/bin/env bash
#
# Portable SSH transport for Pneuma VM tooling.
#
# Builds ssh/scp option arrays from explicit environment settings so the
# dev-vm scripts can target either an ordinary SSH alias (the historical
# behavior) or a host reached through a forwarded loopback port with an
# ephemeral provisioning identity and a dedicated per-run known-hosts file.
# No ~/.ssh/config mutation is performed and no option string is evaluated.
#
# Settings (all optional):
#   PNEUMA_SSH_HOST                      target host or alias
#   PNEUMA_SSH_PORT                      forwarded SSH port (ssh -p / scp -P)
#   PNEUMA_SSH_IDENTITY                  provisioning identity
#   PNEUMA_SSH_KNOWN_HOSTS_FILE          dedicated known-hosts file
#   PNEUMA_SSH_STRICT_HOST_KEY_CHECKING  accept-new | yes | no (verbatim)
#
# Resolution order for the target: a positional argument given to the caller
# wins over PNEUMA_SSH_HOST. With no settings at all the transport reduces to
# plain ssh/scp against the resolved host.
#
# Usage (from a script in scripts/dev-vm/):
#   source "$SCRIPT_DIR/../lib/remote.sh"
#   remote_init "${1:-pneuma-dev}"
#   SSH_HOST="$REMOTE_HOST"
#   remote_ssh "$SSH_HOST" 'command'
#   remote_scp_to -q local-file "$SSH_HOST":/remote/path

# Initialize the transport. The optional argument is the caller's positional
# ssh-host (may be empty when PNEUMA_SSH_HOST should decide). Populates
# REMOTE_HOST, REMOTE_SSH_OPTS, and REMOTE_SCP_OPTS.
remote_init() {
	local host_arg="${1:-}"
	if [[ -n "$host_arg" ]]; then
		REMOTE_HOST="$host_arg"
	elif [[ -n "${PNEUMA_SSH_HOST:-}" ]]; then
		REMOTE_HOST="$PNEUMA_SSH_HOST"
	else
		# Consumed by the caller, not by this library.
		# shellcheck disable=SC2034
		REMOTE_HOST=""
	fi

	# Exported so per-connection helpers (remote_ssh_as) can run in a child
	# bash, e.g. under `timeout`, which can only exec programs.
	export REMOTE_HOST

	REMOTE_SSH_OPTS=()
	REMOTE_SCP_OPTS=()
	if [[ -n "${PNEUMA_SSH_PORT:-}" ]]; then
		REMOTE_SSH_OPTS+=(-p "$PNEUMA_SSH_PORT")
		REMOTE_SCP_OPTS+=(-P "$PNEUMA_SSH_PORT")
	fi
	if [[ -n "${PNEUMA_SSH_IDENTITY:-}" ]]; then
		REMOTE_SSH_OPTS+=(-i "$PNEUMA_SSH_IDENTITY")
		REMOTE_SCP_OPTS+=(-i "$PNEUMA_SSH_IDENTITY")
	fi
	if [[ -n "${PNEUMA_SSH_KNOWN_HOSTS_FILE:-}" ]]; then
		REMOTE_SSH_OPTS+=(-o "UserKnownHostsFile=$PNEUMA_SSH_KNOWN_HOSTS_FILE")
		REMOTE_SCP_OPTS+=(-o "UserKnownHostsFile=$PNEUMA_SSH_KNOWN_HOSTS_FILE")
	fi
	if [[ -n "${PNEUMA_SSH_STRICT_HOST_KEY_CHECKING:-}" ]]; then
		REMOTE_SSH_OPTS+=(-o "StrictHostKeyChecking=$PNEUMA_SSH_STRICT_HOST_KEY_CHECKING")
		REMOTE_SCP_OPTS+=(-o "StrictHostKeyChecking=$PNEUMA_SSH_STRICT_HOST_KEY_CHECKING")
	fi
}

# ssh to the configured endpoint. Additional ssh options (BatchMode,
# ConnectTimeout, ...) may be passed before the destination exactly as with
# plain ssh. Use a heredoc for multi-statement remote scripts.
remote_ssh() {
	ssh "${REMOTE_SSH_OPTS[@]}" "$@"
}

# scp a local path to the configured endpoint. The port is passed as the
# scp-specific uppercase -P. Additional scp options (-q, -r, ...) may be
# passed before the source/destination pair.
remote_scp_to() {
	scp "${REMOTE_SCP_OPTS[@]}" "$@"
}

# scp a remote path from the configured endpoint to a local path.
remote_scp_from() {
	scp "${REMOTE_SCP_OPTS[@]}" "$@"
}

# ssh to the same endpoint as another user, typically the restricted pneuma
# CI identity. The provisioning identity from PNEUMA_SSH_IDENTITY is not
# offered on this connection; the caller's identity is the only one added.
# Endpoint options (forwarded port, known-hosts file) are preserved. The
# destination user is replaced on the configured endpoint, which may carry a
# user part (e.g. "root@127.0.0.1"): it is replaced, never duplicated, e.g.:
#   remote_ssh_as pneuma "$CI_KEY" -o BatchMode=yes version
#
# The transport settings are read from the PNEUMA_SSH_* environment rather
# than the REMOTE_SSH_OPTS array because the function is exported: `timeout`
# and similar exec-only wrappers reach it through `bash -c`, where caller
# arrays are not visible. The env settings are the same source remote_init
# translates into the arrays, so in-process and child-bash calls agree.
remote_ssh_as() {
	local user="$1" identity="$2"
	shift 2

	local -a opts=()
	if [[ -n "${PNEUMA_SSH_PORT:-}" ]]; then
		opts+=(-p "$PNEUMA_SSH_PORT")
	fi
	if [[ -n "${PNEUMA_SSH_KNOWN_HOSTS_FILE:-}" ]]; then
		opts+=(-o "UserKnownHostsFile=$PNEUMA_SSH_KNOWN_HOSTS_FILE")
	fi
	if [[ -n "${PNEUMA_SSH_STRICT_HOST_KEY_CHECKING:-}" ]]; then
		opts+=(-o "StrictHostKeyChecking=$PNEUMA_SSH_STRICT_HOST_KEY_CHECKING")
	fi

	ssh "${opts[@]}" -i "$identity" "${user}@${REMOTE_HOST##*@}" "$@"
}
export -f remote_ssh_as
