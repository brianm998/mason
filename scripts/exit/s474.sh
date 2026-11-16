#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s474 exit test: Cargo.toml sets the dev profile to line tables only, none for dependencies.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -Eq '^\[profile\.dev\]' Cargo.toml
awk '/^\[profile\.dev\]/{f=1;next} /^\[/{f=0} f' Cargo.toml | grep -Eq '^debug = "line-tables-only"'
awk '/^\[profile\.dev\.package\."\*"\]/{f=1;next} /^\[/{f=0} f' Cargo.toml | grep -Eq '^debug = false'
grep -q 'Summary: ' docs/stages/s474.md
grep -q 'target dir' docs/stages/s474.md
echo "s474: ok"
