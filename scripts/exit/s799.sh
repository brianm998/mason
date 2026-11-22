#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s799 exit test (#815): landing's build floor, base floor and fast tier place each suite by the
# suite's own rules (runs_on, remote = false), not by the global switch alone. Structural check
# only: the end-to-end run with a real masond and a fake GitHub is not scripted here.
set -euo pipefail
cd "$(dirname "$0")/../.."
awk '/^\[suite.swift-clients\]/{f=1;next} /^\[/{f=0} f&&/^runs_on = "macos-latest"/{ok=1} END{exit !ok}' .mason/mason.toml
f=crates/daemon/src/landing.rs
# Every CheckSpec that names a suite in the floor and the fast tier carries the suite's placement.
n=$(grep -cE 'Box::new\(crate::hostedcheck::without_build\(suite\)\)' "$f")
[ "$n" -ge 2 ] || { echo "s799: floor and fast tier must pass the suite's placement" >&2; exit 1; }
# The floor spec must not be a global one.
awk '/id_prefix: format!\("\{prefix\}-floor-/{f=1} f&&/global: true/{bad=1} f&&/global:/{exit} END{exit bad}' "$f" \
  || { echo "s799: the floor spec is still global" >&2; exit 1; }
echo "s799: ok"
