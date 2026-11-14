#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A self-update waiting for a landing batch says so, offers "Update now" and takes over when
# the owner asks (s208): the banner and confirm text are pure tests; the drain test holds a
# batch in verify, checks the waiting view and that update_now ends the wait and names the
# batch; the daemon test checks POST /update/now is 409 with nothing waiting.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib a_waiting_update_says_so_offers_update_now_and_confirms
cargo test -q -p mason-daemon --test takeover_drain
cargo test -q -p mason-daemon --test update update_now_with_no_update_waiting_is_a_409_with_a_reason
