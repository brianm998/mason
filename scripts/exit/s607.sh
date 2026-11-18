#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 607 exit test: every exit script that runs `cargo test -p mason-daemon --test X` names a
# test target that exists (the daemon's integration tests are one executable, `it`), and the
# protocol and apns scripts' tests run and pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
fail() { echo "s607: $1" >&2; exit 1; }
for s in scripts/exit/*.sh; do
  for t in $(grep -hvE '^ *#' "$s" | grep -oE -- '-p mason-daemon[^|;&]*' | grep -oE -- '--test [a-z_0-9]+' | awk '{print $2}'); do
    [ -f "crates/daemon/tests/$t.rs" ] || [ -f "crates/daemon/tests/$t/main.rs" ] || fail "$s names the test target '$t', which does not exist"
  done
done
cargo test -q -p mason-daemon --test it protocol::
cargo test -q -p mason-daemon --test it apns::
echo "s607: ok"
