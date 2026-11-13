#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A retry is idempotent (s197): a task is failed, two retries with the same `seen` go through the
# HTTP API back to back; one `Retried` is emitted and the second gets 409. A retry of a running step
# needs `restart`; a retry of a failed task without `seen` still works.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test retry_idempotent
