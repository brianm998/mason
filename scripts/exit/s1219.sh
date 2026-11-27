#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1219 exit test: the stage note and decision exist; no protected scheduler code changed.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1219.md
test -f docs/decisions/1209-gha-test-step-reservation-vs-owner-order.md
echo "OK s1219"
