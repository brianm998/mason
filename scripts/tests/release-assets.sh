#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The asset list the publish job of release.yml checks must equal the table in docs/RELEASING.md.
set -uo pipefail
cd "$(dirname "$0")/../.."
wf=$(sed -n '/# EXPECTED-ASSETS-BEGIN/,/# EXPECTED-ASSETS-END/p' .github/workflows/release.yml \
  | grep -vE 'EXPECTED-ASSETS|^ *expected="|^ *"$' | sed 's/^ *//' | grep . | LC_ALL=C sort)
docs=$(python3 -I - docs/RELEASING.md <<'PY'
import re, sys
out = []
for line in open(sys.argv[1]):
    if not line.startswith("| `"):
        continue
    cell = line.split("|")[1]
    toks = re.findall(r"`([^`]+)`", cell)
    targets = [t for t in toks if "<ver>" not in t and re.fullmatch(r"[a-z0-9_]+(-[a-z0-9_]+)+", t)]
    for t in toks:
        if "<ver>" in t:
            for x in (targets if "<target>" in t else [None]):
                out.append((t.replace("<target>", x) if x else t).replace("<ver>", "@VER@"))
        elif t == "SHA256SUMS":
            out.append(t)
print("\n".join(sorted(set(out))))
PY
)
docs=$(printf '%s\n' "$docs" | LC_ALL=C sort)
if [ -z "$wf" ] || [ -z "$docs" ]; then echo "FAIL: empty asset list (workflow ${#wf} bytes, docs ${#docs} bytes)"; exit 1; fi
if [ "$wf" != "$docs" ]; then
  echo "FAIL: release.yml and docs/RELEASING.md list different assets"
  diff <(printf '%s\n' "$wf") <(printf '%s\n' "$docs")
  exit 1
fi
echo "release-assets: ok ($(printf '%s\n' "$wf" | wc -l | tr -d ' ') assets)"
