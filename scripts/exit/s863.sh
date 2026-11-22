#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# mason init and adopt live in mason-onboard (s863, decision 0856); the CLI is a front end.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-onboard
cargo test -q -p mason-pipeline --test init
if grep -rn "mod questions" crates/cli/src; then echo "questions still in the CLI"; exit 1; fi
echo "s863 ok"
