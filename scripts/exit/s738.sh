#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s738 exit test: the reuse tests pass, including the note's 12-char tree abbreviation.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s738.md || { echo "s738: stage note has no Summary line" >&2; exit 1; }
[ -f docs/decisions/0731-reuse-note-shows-a-12-char-tree.md ] || { echo "s738: decision missing" >&2; exit 1; }
cargo test -p mason-daemon --lib reuse:: 2>&1 | tail -20
echo "s738: ok"
