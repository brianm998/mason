#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s724 exit test: remote runs are set up and checked.
#  1. `mason doctor` on a scratch repository with [remote_runs] and no workflow names the missing
#     workflow.
#  2. `mason remote setup` writes .github/workflows/mason-run.yml on mason/setup-remote, pushes
#     nothing, and leaves the checked-out branch and files alone.
#  3. `mason doctor` against that branch's tree (a second project whose base is the setup
#     branch) reports the workflow OK.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo build -q -p mason-pipeline --bin mason
bin=$(pwd)/target/debug/mason
root=$(mktemp -d /tmp/mason-s724.XXXXXX)
trap 'rm -rf "$root"' EXIT
repo="$root/repo"; cdir="$root/c"; ddir="$root/d"
mkdir -p "$repo" "$cdir/projects" "$ddir"
g() { git -C "$repo" -c user.name=t -c user.email=t@t "$@"; }
g init -q -b main
printf '[workspace]\n' > "$repo/Cargo.toml"
printf '[toolchain]\nchannel = "1.96.0"\n' > "$repo/rust-toolchain.toml"
g add -A; g commit -q -m init
project() { # id base
  cat > "$cdir/projects/$1.toml" <<TOML
path = "$repo"
base = "$2"
test = "cargo test"
[remote_runs]
kind = "github-actions"
repo = "owner/name"
token = "env:S724_NO_SUCH_TOKEN"
api = "http://127.0.0.1:9"
TOML
}
m() { "$bin" --data-dir "$ddir" --config-dir "$cdir" "$@"; }
project scratch main
project scratchsetup mason/setup-remote

echo "== 1. doctor, no workflow"
out=$(m doctor 2>&1 || true)
echo "$out" | grep "project scratch:" | head -5
echo "$out" | grep -q "project scratch: remote runs: there is no .github/workflows/mason-run.yml"

echo "== 2. remote setup"
before=$(g rev-parse HEAD)
m remote setup --project scratch
g show mason/setup-remote:.github/workflows/mason-run.yml | grep -q 'mason/run/\*\*'
g show mason/setup-remote:.github/workflows/mason-run.yml | grep -q 'cache-on-failure: true'
g show mason/setup-remote:.github/workflows/mason-run.yml | grep -q 'toolchain: 1.96.0'
[ "$(g rev-parse HEAD)" = "$before" ]
[ "$(g rev-parse --abbrev-ref HEAD)" = main ]
[ ! -e "$repo/.github" ]
[ -z "$(g status --porcelain)" ]
[ -z "$(g branch -r)" ]

echo "== 3. doctor against the setup branch"
out=$(m doctor 2>&1 || true)
echo "$out" | grep "project scratchsetup:" | head -5
echo "$out" | grep -qF 'project scratchsetup: mason-run.yml on mason/setup-remote accepts `workflow_dispatch` and does not run on pushes to `mason/run/**`, and no other workflow does'
echo "s724: ok"
