#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s107 exit test: an expired agent login is a provider hold and a `credentials`
# alert, not a task failure. A real masond runs two queued tasks with fake-claude
# answering "Not logged in": neither fails, `mason alerts` shows credentials, and
# both land after the fake is switched back (or the owner runs `mason account
# release`).
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -1
}

run "adapter: each login-failure fixture is NotLoggedIn, stderr wording too, a plain crash stays a crash" \
    -p mason-adapters --lib claude
run "real masond and mason with fake-claude refusing login: two tasks stay unfailed, mason alerts \
shows credentials, both land once the fake works; release clears at once" \
    -p mason-daemon --test credentials

echo "s107 OK"
