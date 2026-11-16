#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s467 exit test: the features gate passes with three clients (swift, compose, ios).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -q -p mason-conformance --bins
out=$(target/debug/conformance features clients/features.toml 2>&1) || { echo "$out"; exit 1; }
echo "$out"
echo "$out" | grep -q "in ios " || { echo "no ios column in the report"; exit 1; }
echo "s467: ok"
