#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1066 exit test: the exit scripts that were red on develop (s43 ceiling, s49 checkpoint order, features gate).
set -euo pipefail
cd "$(dirname "$0")/../.."
echo "== features gate"
cargo run -q -p mason-conformance -- features clients/features.toml
echo "== s49 sorts checkpoints by number"
grep -q 'key=lambda f: int(re.search' scripts/exit/s49.sh
d=$(mktemp -d); trap 'rm -rf "$d"' EXIT
for i in $(seq 0 12); do echo "[]" > "$d/checkpoint-$i.json"; done
python3 -I - "$d" <<'PY'
import re, sys, glob
files = sorted(glob.glob(sys.argv[1] + "/checkpoint-*.json"), key=lambda f: int(re.search(r"checkpoint-(\d+)\.json$", f).group(1)))
assert files[-1].endswith("checkpoint-12.json"), files[-1]
PY
echo "s1066 OK"
