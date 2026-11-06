#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 2 exit test: reducer tests pass, and core and the reducer have no IO or
# clock dependencies (neither in Cargo dependencies nor in source).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

cargo test -p mason-core -p mason-engine

# Dependencies: core has none; engine depends only on core.
deps() { cargo metadata --format-version 1 --no-deps | python3 -c '
import json,sys
m=json.load(sys.stdin)
for p in m["packages"]:
    if p["name"]==sys.argv[1]: print(" ".join(sorted(d["name"] for d in p["dependencies"])))' "$1"; }
[ -z "$(deps mason-core)" ] || { echo "mason-core must have no dependencies: $(deps mason-core)" >&2; exit 1; }
[ "$(deps mason-engine)" = "mason-core" ] || { echo "mason-engine may depend only on mason-core: $(deps mason-engine)" >&2; exit 1; }

# Source: no IO, clock, threads, randomness or async runtime.
bash scripts/check-pure.sh crates/core/src crates/engine/src

# The checker itself rejects a planted violation.
mkdir "$tmp/bad"; printf 'fn f() { let _ = std::time::SystemTime::now(); }\n' > "$tmp/bad/x.rs"
if bash scripts/check-pure.sh "$tmp/bad" >/dev/null; then echo "purity check missed a clock read" >&2; exit 1; fi
echo "s02 OK"
