#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s295 exit test: introspection findings are proposed or queued by the owner's
# `[proposals.introspection]` table, per severity and cost. Two findings of different severity
# under a config that splits them: one is queued, one proposed, and each row names the rule. An
# analyst draft's severity and cost are read, and bad ones refused. A renamed analyst task is
# still read, and another task with the old title is not.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the mapping and the keys" -p mason-core --lib -- introspection_defaults impact_cost_and_keys
run "analyst drafts and estimates" -p mason-core --lib -- a_draft_gives_severity detector_findings_are
run "the table is read and bad keys are refused" -p mason-daemon --lib -- proposals::
run "queued and proposed, and the renamed analyst" -p mason-daemon --test introspection_policy
echo "s295 OK"
