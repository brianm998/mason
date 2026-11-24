#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1064 exit test: project rename names its caller from USER, and the checks that were red pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'std::env::set_var("MASON_AS", u)' crates/cli/src/main.rs
env -u MASON_AS -u USER bash scripts/exit/s965.sh
bash scripts/exit/s193.sh
cargo run -q -p mason-conformance -- features clients/features.toml
