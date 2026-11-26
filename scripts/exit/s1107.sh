#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1107 exit test: agent sessions are refused destructive git and rm commands (decision 1096).
#  1. `mason hook bash` as an agent session answers deny with a reason to `git stash clear` and to
#     `cd /tmp && rm -rf ~/x`, and says nothing to `rm -rf target` in a temporary worktree.
#  2. The same entry refuses nothing outside a MASON session; `mason hook guard` always refuses.
#  3. The guard's unit tests, and the adapter registering it with `--settings`.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD

cargo build -q -p mason-pipeline --bins
mason="${CARGO_TARGET_DIR:-$root/target}/debug/mason"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/wt/target" "$tmp/home" "$tmp/scratch"
git init -q "$tmp/wt"

# ask <hook> <command> [NAME=value ...]: the hook's answer, with nothing else in its environment.
ask() {
    local hook=$1 command=$2
    shift 2
    printf '{"tool_name":"Bash","cwd":"%s","tool_input":{"command":"%s"}}' "$tmp/wt" "$command" |
        (cd "$tmp/wt" && env -i PATH="$PATH" HOME="$tmp/home" TMPDIR="$tmp/scratch" "$@" "$mason" hook "$hook")
}
denied() {
    case "$1" in
        *'"permissionDecision":"deny"'*) ;;
        *) echo "FAILED: $2 was not denied: $1" >&2; exit 1 ;;
    esac
    case "$1" in
        *'"permissionDecisionReason":"MASON refused '*) ;;
        *) echo "FAILED: $2 was denied without a reason: $1" >&2; exit 1 ;;
    esac
}
silent() {
    [ -z "$1" ] || { echo "FAILED: $2 should have had no answer: $1" >&2; exit 1; }
}

echo "== mason hook bash in an agent session"
out=$(ask bash "git stash clear" MASON_STEP_KIND=develop)
denied "$out" "git stash clear"
case "$out" in *"stash stack is shared"*) ;; *) echo "FAILED: no stash reason: $out" >&2; exit 1 ;; esac
out=$(ask bash "cd /tmp && rm -rf ~/x" MASON_STEP_KIND=develop)
denied "$out" "cd /tmp && rm -rf ~/x"
out=$(ask bash "rm -rf target" MASON_STEP_KIND=develop)
silent "$out" "rm -rf target"

echo "== outside a MASON session only hook guard refuses"
out=$(ask bash "git stash clear")
silent "$out" "hook bash for a person's session"
out=$(ask guard "git stash clear")
denied "$out" "hook guard"
out=$(ask guard "cargo build")
silent "$out" "hook guard on a build (the queue rewrite is hook bash's)"

echo "== unit tests"
cargo test -q -p mason-pipeline --bin mason guard::
cargo test -q -p mason-adapters --lib every_session_is_given_the_command_guard
echo "OK s1107"
