#!/usr/bin/env bash

# Exercises the pure subordinate-ID parser against disposable fixture files.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
# shellcheck source=lib/provision-host.sh
source "$SCRIPT_DIR/lib/provision-host.sh"

TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

pass=0
fail=0

assert_valid() {
    local name="$1" contents="$2"
    local file="$TEMP_DIR/$name"
    printf '%s\n' "$contents" >"$file"
    if validate_subordinate_id_file "$file"; then
        pass=$((pass + 1))
        printf 'PASS  %s\n' "$name"
    else
        fail=$((fail + 1))
        printf 'FAIL  %s\n' "$name"
    fi
}

assert_invalid() {
    local name="$1" contents="$2"
    local file="$TEMP_DIR/$name"
    printf '%s\n' "$contents" >"$file"
    if validate_subordinate_id_file "$file" >/dev/null 2>&1; then
        fail=$((fail + 1))
        printf 'FAIL  %s\n' "$name"
    else
        pass=$((pass + 1))
        printf 'PASS  %s\n' "$name"
    fi
}

assert_start() {
    local name="$1" contents="$2" expected="$3"
    local file="$TEMP_DIR/$name" actual
    printf '%s\n' "$contents" >"$file"
    if actual="$(subordinate_id_start "$file")" && [[ "$actual" == "$expected" ]]; then
        pass=$((pass + 1))
        printf 'PASS  %s\n' "$name"
    else
        fail=$((fail + 1))
        printf 'FAIL  %s\n' "$name"
    fi
}

assert_valid alternate-range 'other:1:99999
pneuma:200000:65536'
assert_valid adjacent-range 'other:1:99999
pneuma:100000:65536
other-two:165536:65536'
assert_valid missing-pneuma-safe 'other:1:99999'
assert_start missing-pneuma-default-conflict 'other:100000:65536' 165536
assert_start missing-pneuma-multiple-conflicts 'other:100000:65536
other-two:165536:65536' 231072
assert_invalid undersized 'pneuma:100000:65535'
assert_invalid duplicate 'pneuma:100000:65536
pneuma:200000:65536'
assert_invalid overlapping-other 'pneuma:100000:65536
other:120000:65536'
assert_invalid malformed 'pneuma:not-a-number:65536'

printf '%s passed, %s failed.\n' "$pass" "$fail"
[[ "$fail" -eq 0 ]]
