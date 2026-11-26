#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1153 exit test: agent skills come from the base commit (decision 1142).
#  1. A fixture project with one skill in .mason/skills/ on its base commit: every scripted
#     session's init event lists it, also after the task has added a second skill on its branch,
#     and the run that called it is recorded.
#  2. The export reads the base commit, not the worktree.
#  3. The command line: `Skill` for every step kind, `--plugin-dir` only with skills,
#     `user_skills = false`, and the guard.
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo build -q -p mason-adapters -p mason-step-host --bins

echo "== a fixture project, run by the daemon"
cargo test -q -p mason-daemon --test it skills::
echo "== read from the base commit"
cargo test -q -p mason-daemon --lib skills::
cargo test -q -p mason-daemon --lib user_skills_load_unless_the_agent_table_turns_them_off
echo "== the command line and the guard"
cargo test -q -p mason-adapters --lib skill
cargo test -q -p mason-pipeline --bin mason the_skill_hook
echo "OK s1153"
