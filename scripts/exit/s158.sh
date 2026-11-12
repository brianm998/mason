#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 158 exit test: a refused push holds landing; the owner chooses the SSH key.
#  1. masond honours core.sshCommand and appends its BatchMode options.
#  2. auth_failure tells a refusal from a timeout and a non-fast-forward rejection.
#  3. A refused push holds landing (no failure), a key whose probe passes lifts it and the batch lands.
#  4. The key list marks the owner's account and passphrases; unlock never sees the passphrase.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q --workspace --bins

t() {  # PACKAGE TEST-TARGET
  [ -z "${MASON_TESTS_ALREADY_RAN:-}" ] || return 0
  cargo test -q -p "$1" --test "$2" 2>&1 | tail -5 | grep -q 'test result: ok' \
    || { echo "FAILED: $2" >&2; cargo test -p "$1" --test "$2" 2>&1 | tail -30 >&2; exit 1; }
}

echo "== 1. core.sshCommand"
t mason-landing ssh_config
echo "== 2. auth_failure"
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-landing --lib auth_tests 2>&1 | tail -3 | grep -q 'test result: ok'
fi
echo "== 3. hold and lift"
t mason-daemon push_auth
echo "== 4. keys and the passphrase"
t mason-daemon ssh_keys
echo "s158 OK"
