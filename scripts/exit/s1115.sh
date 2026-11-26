#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1115 exit test: the node doctor finds a cargo in $HOME/.cargo/bin when PATH is /usr/bin:/bin
# (a temporary HOME with a stub), and docs/nodes.md has the setup steps that were missing.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1115.md
grep -q 'rustup default 1.96.0' docs/nodes.md
grep -q '<key>PATH</key>' docs/nodes.md
if grep -q '\.local/bin/masond' docs/nodes.md; then
  echo "docs/nodes.md still names a masond that does not exist" >&2
  exit 1
fi
# The test gives the doctor PATH=/usr/bin:/bin and a temporary HOME, and expects the cargo line ok.
cargo test -q -p mason-daemon --test nodes the_doctor_finds_cargo_on_the_path_a_job_gets
cargo test -q -p mason-daemon --lib -- node_doctor:: node_run:: node_runtime::
echo "OK s1115"
