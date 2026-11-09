#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 26 against real things: NOT run by run-exit-scripts.sh and NOT run in Stage 26.
# Needs: a scratch GitHub repository you own, GITHUB_TOKEN for it, the real `claude` CLI logged in,
# and a published release for scripts/install.sh. It checks what the fakes cannot:
#  - `mason init --workflow reviewed-pr --landing pr` against a real GitHub remote opens a PR
#    for the proof task and the PR merges;
#  - `--tracker github` imports a labelled issue;
#  - scripts/install.sh against the real release verifies a real signature (needs minisign and
#    MASON_MINISIGN_PUBKEY).
# Usage: S26_REPO=owner/scratch bash scripts/exit/s26-real.sh
set -euo pipefail
: "${S26_REPO:?set S26_REPO=owner/scratch-repository}"
: "${GITHUB_TOKEN:?set GITHUB_TOKEN}"
command -v claude >/dev/null || { echo "needs the real claude CLI" >&2; exit 1; }
dir=$(mktemp -d); trap 'rm -rf "$dir"' EXIT
git clone "https://github.com/$S26_REPO" "$dir/repo"
cd "$dir/repo"
mason init --workflow reviewed-pr --landing pr --tracker github --yes --no-prove
git checkout -q "$(git symbolic-ref --short refs/remotes/origin/HEAD | cut -d/ -f2)"
git merge --ff-only mason/setup && git push origin HEAD
echo "Now start masond, then: mason init --prove-only, and watch the pull request open and merge."
