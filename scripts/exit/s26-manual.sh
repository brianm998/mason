#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 26 owner checkpoint: NOT run by run-exit-scripts.sh and NOT run in Stage 26.
# M7's exit test is a stranger's first landed task within 10 minutes of starting from nothing.
# This prints the protocol and times you; it cannot do the install for you.
set -euo pipefail
cat <<'TXT'
On a machine (or user account) with none of MASON installed, and a project of your choice
(or a stranger who has never seen MASON):
  1. Start the clock.
  2. Install:  brew install mason | the install script | cargo install mason-pipeline
     (whichever exists by now; note if `mason` clashed with another tool and whether the message helped)
  3. cd project && mason init     (answer the eight questions; do not read the docs)
  4. Merge the setup branch, start masond, run: mason init --prove-only
  5. Stop the clock when the harmless task has landed.
Record: the time, every place the person hesitated or asked something, and any message that was
wrong or unclear. The target is under 10 minutes.
TXT
read -r -p "Press Enter when you start. " _
start=$(date +%s)
read -r -p "Press Enter when the first task has landed. " _
end=$(date +%s)
secs=$((end - start))
echo "Elapsed: $((secs / 60))m$((secs % 60))s ($([ $secs -le 600 ] && echo "within" || echo "OVER") the 10 minute target)"
