#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s536 exit test: a task blocked on the base-run lock records a wait that names the holder.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'struct BaseRunLock' crates/daemon/src/basecheck.rs || { echo "s536: BaseRunLock is missing" >&2; exit 1; }
grep -q 'class: Some("base-run".into())' crates/daemon/src/basecheck.rs || { echo "s536: no base-run wait is emitted" >&2; exit 1; }
if grep -rn 'BASE_RUN\.lock()' crates/daemon/src; then
  echo "s536: a taker still blocks on BASE_RUN without a wait record" >&2; exit 1
fi
cargo test -p mason-daemon --lib base_run
cargo test -p mason-core wait_class
echo "s536: ok"
