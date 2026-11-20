#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s700 exit test: the thermal gate has its die and power-limit conditions, a limit, a stage note
# and a decision. The behaviour itself is tested in thermal.rs and tests/it/limits.rs.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s700.md || { echo "s700: stage note has no Summary line" >&2; exit 1; }
[ -f docs/decisions/0693-thermal-gate-smc-die-temperature.md ] || { echo "s700: decision missing" >&2; exit 1; }
grep -q 'DEFAULT_THERMAL_MAX_C: u32 = 97' crates/daemon/src/sched.rs || { echo "s700: no default die limit" >&2; exit 1; }
grep -q 'AppleSMC' crates/daemon/src/thermal.rs || { echo "s700: no SMC read" >&2; exit 1; }
echo "s700: ok"
