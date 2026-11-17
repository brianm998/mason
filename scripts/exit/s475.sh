#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s475 exit test: the daemon's integration tests build into a few executables (it plus the
# separate ones), and the number of tests is the number before the merge (931).
set -euo pipefail
cd "$(dirname "$0")/../.."
json=$(mktemp)
trap 'rm -f "$json"' EXIT
cargo test -q -p mason-daemon --no-run --message-format=json >"$json"
python3 -I - "$json" <<'PY'
import json, subprocess, sys
exes = []
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if (m.get("reason") == "compiler-artifact" and m.get("executable")
            and m.get("profile", {}).get("test") and "mason-daemon" in m["package_id"]):
        exes.append(m["executable"])
tests = 0
for e in exes:
    r = subprocess.run([e, "--list"], capture_output=True, text=True, cwd="crates/daemon", check=True)
    tests += sum(1 for x in r.stdout.splitlines() if x.endswith(": test"))
print(f"test executables: {len(exes)}, tests: {tests}")
assert len(exes) <= 13, "the daemon's tests are not merged"
assert tests >= 931, "tests were dropped (931 before the merge)"
PY
echo "s475 OK"
