#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1261 exit test (#1291): beside the cargo suite the exit scripts are told its tests already ran,
# s06 leaves out its cargo-test part then, a script over the per-script limit is `NOT RUN` in a
# check and `FAIL` elsewhere, and the heavy scripts are not weighted as 60 s.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -qF '[ -n "${MASON_TESTS_ALREADY_RAN:-}" ] || cargo test' scripts/exit/s06.sh
for n in 06 11 14 15 16 158 1058; do
    awk -v s="s$n" '$1==s && $2>=900 {f=1} END {exit !f}' scripts/exit/weights.txt \
        || { echo "weights.txt: s$n is not weighted as heavy" >&2; exit 1; }
done
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
    cargo test -q -p mason-daemon --test exit_runner_budget
    cargo test -q -p mason-daemon --test it suite_builds::exit_scripts_
fi
echo "s1261 OK"
