#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 20, for real: the Codex adapter's launch against the real `codex` CLI.
# MANUAL: never run by CI, and never by an agent on its own. It makes one `codex exec` call with
# a tiny prompt, which spends real tokens on your Codex/OpenAI account.
#
# Needs, all explicit:
#   MASON_REAL_APPROVED=yes     you have read this script and approve running it
#   MASON_REAL_LIVE=yes         you accept that it spends tokens
#   codex                       the Codex CLI, signed in
#
# It runs `codex exec --json -s read-only -C <scratch repo>` as the adapter does (the prompt on
# stdin) and checks that the output is JSON lines and that the run ended. What the lines look
# like is printed, so the fixtures in crates/adapters/fixtures/codex can be compared with them.
set -euo pipefail
[ "${MASON_REAL_APPROVED:-}" = yes ] || { echo "refusing: set MASON_REAL_APPROVED=yes once you approve running this" >&2; exit 2; }
[ "${MASON_REAL_LIVE:-}" = yes ] || { echo "refusing: set MASON_REAL_LIVE=yes (this spends Codex tokens)" >&2; exit 2; }
command -v codex >/dev/null || { echo "needs the real codex CLI" >&2; exit 2; }
root=$(mktemp -d /tmp/mason-s20r.XXXXXX)
trap 'rm -rf "$root"' EXIT
git init -q "$root/repo"
echo "scratch" > "$root/repo/README.md"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=s20 -c user.email=s20@example.invalid commit -q -m scratch
echo "Reply with the single word ok." | codex exec --json -s read-only -C "$root/repo" - > "$root/out.jsonl"
lines=$(wc -l < "$root/out.jsonl" | tr -d ' ')
echo "== the first lines codex printed ($lines in all)"
head -5 "$root/out.jsonl"
[ "$lines" -gt 0 ] || { echo "codex printed nothing" >&2; exit 1; }
if command -v python3 >/dev/null; then
  python3 -I - "$root/out.jsonl" <<'PY'
import json, sys
kinds = []
for line in open(sys.argv[1]):
    line = line.strip()
    if line:
        kinds.append(json.loads(line).get("type", "?"))
print("event types:", ", ".join(sorted(set(kinds))))
PY
fi
echo "RESULT s299-real: codex exec --json ran once and printed $lines JSON lines $(date +%F)"
