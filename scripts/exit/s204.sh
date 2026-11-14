#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s204 exit test: issue and CI references on the task detail page are links (task numbers open
# the task's page, CI references open in the browser). Nothing is opened and nothing is shown:
# both clients' core test targets assert the segments linkify produces for a fixture whose
# why-waiting, failure and body mention a known task, an unknown task, `PR #N` with a CiLink and a
# failed check; the daemon test checks a red batch pull request gives ci_links; the adapter test
# checks `details_url` reaches JobFailure.url; the feature check needs the new feature done in both.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# The daemon and the adapter.
cargo test -q -p mason-adapters --test github a_failed_checks_details_url_is_carried_into_its_job_failure
cargo test -q -p mason-daemon --test landing_modes a_red_batch_pull_request_gives_the_task_detail_ci_links
cargo test -q -p mason-daemon --test editor

# Both clients' core targets (the fixtures are in LinkifyTests / LinkifyTest) and their
# detail-view tests (offscreen: attributed and annotated strings, a hosting controller that is
# never shown).
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  swift test --package-path clients/swift --filter 'LinkifyTests|LinkedTextTests'
else
  echo "swift not run here" >&2
fi
if command -v java >/dev/null && command -v gradle >/dev/null && [ -z "${MASON_SKIP_COMPOSE:-}" ]; then
  (cd clients/compose && gradle --no-daemon -q :core:test --tests 'mason.monitor.LinkifyTest' :app:test --tests 'mason.monitor.ui.LinkedTextTest')
else
  echo "compose not run here" >&2
fi

# Parity: the new feature is done in both clients.
cargo run -q -p mason-conformance -- features clients/features.toml | tail -1
entry=$(awk '/^\[feature.task-detail-links\]/{f=1;next} /^\[/{f=0} f' clients/features.toml)
echo "$entry" | grep -q '^swift = "done"'
echo "$entry" | grep -q '^compose = "done"'
echo "s204 ok"
