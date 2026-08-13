#!/usr/bin/env bash
#
# Update only the installed Pneuma binary on an already bootstrapped host.
#
# This script backs up the database, resolves an immutable tag or full commit
# SHA from the existing checkout, builds as the pneuma user, installs the binary,
# and runs post-update checks. It does not provision packages, accounts, Caddy,
# environment, or CI keys.
#
# Usage:
#   sudo bash scripts/update-pneuma.sh --ref <tag-or-full-sha>
#
# Example:
#   sudo bash scripts/update-pneuma.sh --ref v0.3.0

set -euo pipefail

PNEUMA_USER="pneuma"
PNEUMA_HOME="/home/$PNEUMA_USER"
PNEUMA_SOURCE_PATH="$PNEUMA_HOME/pneuma"
PNEUMA_BACKUP_DIRECTORY="/var/backups/pneuma"
PNEUMA_REF=""

usage() {
	cat >&2 <<EOF
Usage: $0 --ref <tag-or-full-sha>

  --ref <ref>  Target Git tag or full 40-character commit SHA. Branches and
               abbreviated SHAs are rejected.
EOF
}

while [[ $# -gt 0 ]]; do
	case "$1" in
	--ref)
		if [[ $# -lt 2 || -z "$2" ]]; then
			echo "ERROR: --ref requires a value." >&2
			usage
			exit 1
		fi
		PNEUMA_REF="$2"
		shift 2
		;;
	--help | -h)
		usage
		exit 0
		;;
	*)
		echo "ERROR: Unknown argument: $1" >&2
		usage
		exit 1
		;;
	esac
done

if [[ "$(id -u)" -ne 0 ]]; then
	echo "ERROR: Run this script as root." >&2
	exit 1
fi

if [[ -z "$PNEUMA_REF" ]]; then
	echo "ERROR: --ref is required." >&2
	usage
	exit 1
fi

if [[ ! -d "$PNEUMA_SOURCE_PATH/.git" ]]; then
	echo "ERROR: Pneuma source checkout not found: $PNEUMA_SOURCE_PATH" >&2
	echo "Run scripts/bootstrap-vps.sh for the initial installation." >&2
	exit 1
fi

if [[ ! -x /usr/local/bin/pneuma ]]; then
	echo "ERROR: Installed Pneuma binary not found: /usr/local/bin/pneuma" >&2
	echo "Run scripts/bootstrap-vps.sh for the initial installation." >&2
	exit 1
fi

if [[ ! -f "$PNEUMA_HOME/.cargo/env" ]]; then
	echo "ERROR: Rust environment not found: $PNEUMA_HOME/.cargo/env" >&2
	echo "Run scripts/bootstrap-vps.sh to provision the build environment." >&2
	exit 1
fi

if [[ ! "$PNEUMA_REF" =~ ^[0-9a-f]{40}$ ]]; then
	if [[ "$PNEUMA_REF" =~ ^[0-9a-f]+$ ]]; then
		echo "ERROR: --ref must not be an abbreviated SHA: '$PNEUMA_REF'." >&2
		exit 1
	fi
	if [[ "$PNEUMA_REF" == *..* || "$PNEUMA_REF" == -* ]]; then
		echo "ERROR: invalid --ref value: '$PNEUMA_REF'." >&2
		exit 1
	fi
fi

echo "==> Fetching Pneuma source tags..."
runuser -u "$PNEUMA_USER" -- env HOME="$PNEUMA_HOME" GIT_TERMINAL_PROMPT=0 \
	git -C "$PNEUMA_SOURCE_PATH" fetch --prune --tags --force \
	origin '+refs/heads/*:refs/remotes/origin/*'

if [[ "$PNEUMA_REF" =~ ^[0-9a-f]{40}$ ]]; then
	if ! TARGET_SHA="$(runuser -u "$PNEUMA_USER" -- env HOME="$PNEUMA_HOME" \
		git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify --quiet \
		"$PNEUMA_REF^{commit}")"; then
		echo "ERROR: --ref SHA does not resolve to a commit: $PNEUMA_REF" >&2
		exit 1
	fi
else
	if ! runuser -u "$PNEUMA_USER" -- env HOME="$PNEUMA_HOME" \
		git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify --quiet \
		"refs/tags/$PNEUMA_REF^{commit}" >/dev/null; then
		if runuser -u "$PNEUMA_USER" -- env HOME="$PNEUMA_HOME" \
			git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify --quiet \
			"refs/remotes/origin/$PNEUMA_REF" >/dev/null; then
			echo "ERROR: --ref names a branch, not a tag: '$PNEUMA_REF'." >&2
		else
			echo "ERROR: Git tag not found: '$PNEUMA_REF'." >&2
		fi
		exit 1
	fi
	TARGET_SHA="$(runuser -u "$PNEUMA_USER" -- env HOME="$PNEUMA_HOME" \
		git -C "$PNEUMA_SOURCE_PATH" rev-parse --verify \
		"refs/tags/$PNEUMA_REF^{commit}")"
fi

mkdir -p "$PNEUMA_BACKUP_DIRECTORY"
BACKUP_PATH="$PNEUMA_BACKUP_DIRECTORY/pneuma-before-${PNEUMA_REF//\//-}.sqlite3"
if [[ -e "$BACKUP_PATH" ]]; then
	BACKUP_PATH="$PNEUMA_BACKUP_DIRECTORY/pneuma-before-${PNEUMA_REF//\//-}-$(date -u +%Y%m%dT%H%M%SZ).sqlite3"
fi

echo "==> Backing up database to $BACKUP_PATH..."
/usr/local/bin/pneuma database backup "$BACKUP_PATH"

echo "==> Checking out $TARGET_SHA..."
runuser -u "$PNEUMA_USER" -- env HOME="$PNEUMA_HOME" \
	git -C "$PNEUMA_SOURCE_PATH" checkout --force --detach "$TARGET_SHA"

echo "==> Building Pneuma..."
runuser -u "$PNEUMA_USER" -- bash -lc \
	"source '$PNEUMA_HOME/.cargo/env' && cd '$PNEUMA_SOURCE_PATH' && cargo build --release"

echo "==> Installing Pneuma..."
install -o root -g root -m 0755 \
	"$PNEUMA_SOURCE_PATH/target/release/pneuma" /usr/local/bin/pneuma

echo "==> Verifying installed version..."
/usr/local/bin/pneuma version

echo "==> Running Pneuma doctor..."
runuser -u "$PNEUMA_USER" -- bash -lc 'cd "$HOME" && pneuma doctor'

echo "==> Update complete. Database backup: $BACKUP_PATH"
