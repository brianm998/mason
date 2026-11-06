#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Spike: conditional requests and rate-limit accounting. Read-only. Needs gh auth.
set -euo pipefail
repo="${1:-brianm998/mason}"
rl() { gh api rate_limit --jq '.resources.core.remaining'; }
hdr() { { gh api -i "$@" 2>&1 || true; } | tr -d "\r"; }
r0=$(rl)
first=$(hdr "repos/$repo/issues?state=all&per_page=5")
etag=$(echo "$first" | awk -F': ' 'tolower($1)=="etag"{print $2}')
echo "etag: $etag"
r1=$(rl)
for i in 1 2 3 4 5; do
  hdr -H "If-None-Match: $etag" "repos/$repo/issues?state=all&per_page=5" | head -1
done
r2=$(rl)
echo "core remaining: start=$r0 after-unconditional=$r1 after-5-conditional=$r2"
echo "missing resource:"; hdr "repos/$repo/issues/999999" | head -1
echo "poll-interval header: $(echo "$first" | awk -F': ' 'tolower($1)=="x-poll-interval"{print $2}')"
