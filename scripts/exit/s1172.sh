#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^Summary: ' docs/stages/s1172.md
for t in project_skills_become_a_plugin_and_the_worktrees_are_turned_off \
  the_guard_lets_through_only_the_projects_and_the_owners_skills \
  user_skills_off_turns_off_every_skill_and_plugin_of_the_owners; do
  cargo test -q -p mason-adapters --lib -- --exact "skills::tests::$t" 2>&1 | grep -q '1 passed'
done
