#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s320 exit test: `mason task open` runs the daemon's provider argv in the worktree, for a fake
# Claude and a fake Codex binary.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-pipeline --bin mason open_session_tests
cargo test -q -p mason-daemon --test it the_resume_command_is_the_providers_own
