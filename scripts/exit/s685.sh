#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s685 exit test: `mason definition check` on this worktree lists no unmapped tracked file.
# `.mason/mason.toml` is left unmapped on purpose and the checker does not list it.
set -euo pipefail
cd "$(dirname "$0")/../.."
out=$(cargo run -q -p mason-pipeline --bin mason -- definition check .mason/mason.toml)
if printf '%s\n' "$out" | grep '^unmapped'; then
  echo "s685: tracked files no [impact] glob names" >&2
  exit 1
fi
printf '%s\n' "$out" | grep -q '^ok'
