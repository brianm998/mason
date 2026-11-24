#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1044 exit test: the retrying test helper gates from_mode under cfg(unix) (decision 1037).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1044.md
python3 - <<'PY'
import re
s = open("crates/daemon/src/remote.rs").read()
for m in re.finditer(r"use std::os::unix::fs::PermissionsExt;", s):
    before = s[:m.start()].rstrip()
    assert before.endswith("{") and before[:-1].rstrip().endswith("#[cfg(unix)]"), "ungated unix use"
PY
cargo build -q --workspace --all-targets
