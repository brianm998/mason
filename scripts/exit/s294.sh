#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s294 exit test: a scratch project with a custom `security-review` agent step (its own prompt
# file) and a `fix` pack; a task goes through one red round and the fake agent's recorded
# prompts are checked. Also the definition parser and `definition check` cases.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test prompt_packs
cargo test -q -p mason-daemon --lib pack_candidates
cargo test -q -p mason-definition prompt
cargo test -q -p mason-pipeline missing_prompt
echo "s294 OK"
