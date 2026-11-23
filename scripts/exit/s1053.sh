#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1053 exit test: the Compose desktop monitor has File > Add Project… and a + under the project
# picker, and the Android app has neither (decision 1046). The Gradle tests themselves run in the
# compose-clients check; this script checks the wiring.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1053.md
c=clients/compose
grep -q 'Item(' $c/app/src/main/kotlin/mason/monitor/Main.kt
grep -q '"Add Project…"' $c/app/src/main/kotlin/mason/monitor/Main.kt
grep -q 'testTag("add-project-dialog")' $c/app/src/main/kotlin/mason/monitor/ui/AddProject.kt
grep -q '"add-project"' $c/app/src/main/kotlin/mason/monitor/ui/TopBar.kt
test -f $c/core/src/test/kotlin/mason/monitor/AddProjectTest.kt
test -f $c/app/src/test/kotlin/mason/monitor/ui/AddProjectSnapshotTest.kt
if grep -rEq 'add-project|AddProject|addProject' $c/android/src; then
  echo "FAIL: the Android app mentions Add Project" >&2
  exit 1
fi
echo "OK s1053"
