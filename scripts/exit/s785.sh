#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s785 exit test: an edit to an integration-test file selects only scripts naming its module;
# a src edit still selects scripts naming `-p <package>`.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q -b main repo
cd repo
mkdir -p scripts/exit .mason crates/daemon/tests/it
cp "$root/scripts/select-exit-scripts.sh" scripts/
printf '[package]\nname = "mason-daemon"\n' > crates/daemon/Cargo.toml
printf '#!/bin/bash\ncargo test -p mason-daemon --test it landing_floor\n' > scripts/exit/s001.sh
printf '#!/bin/bash\ncargo test -p mason-daemon --test it other\n' > scripts/exit/s002.sh
printf '#!/bin/bash\ncargo test -p mason-daemon\n' > scripts/exit/s003.sh
printf '#!/bin/bash\ncargo test --test it landing_floor_x\n' > scripts/exit/s004.sh
fail() { echo "FAILED: $1" >&2; exit 1; }
echo "== test file: only the script naming the module"
printf 'crates/daemon/tests/it/landing_floor.rs\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "scripts/exit/s001.sh" ] || fail "test edit selected: $out"
echo "== src file: -p scripts still selected"
printf 'crates/daemon/src/lib.rs\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "$(printf 'scripts/exit/s001.sh\nscripts/exit/s002.sh\nscripts/exit/s003.sh')" ] || fail "src edit selected: $out"
echo "== changed script and docs"
printf 'scripts/exit/s002.sh\ndocs/x.md\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "scripts/exit/s002.sh" ] || fail "script edit selected: $out"
printf 'docs/x.md\n' > .mason/changed-files.txt
[ -z "$(bash scripts/select-exit-scripts.sh)" ] || fail "docs selected something"
echo "s785 OK"
