#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s677 exit test: split_cargo keeps at most one --no-run. No build is started.
set -euo pipefail
cd "$(dirname "$0")/../.."
python3 -I - <<'PY'
import importlib.util
spec = importlib.util.spec_from_file_location("tp", "scripts/test-parallel.py")
tp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tp)
argv = "cargo test --workspace --no-fail-fast --target-dir X --no-run".split()
flags, filters, libtest = tp.split_cargo(argv)
assert flags.count("--no-run") == 0, flags
assert "--no-fail-fast" in flags and "--target-dir" in flags, flags
assert libtest == [], libtest
f2, _, l2 = tp.split_cargo("cargo test --no-run -- --nocapture".split())
assert f2.count("--no-run") == 0 and l2 == ["--nocapture"], (f2, l2)
print("ok")
PY
