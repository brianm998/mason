#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s168 exit test: a task that failed at landing on a rejected push, retried with no
# step after the hook is removed, lands, and `mason task show` lists exactly one
# develop run. The test makes a local bare remote whose pre-receive hook rejects pushes.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test retry_outside
echo "s168: ok"
