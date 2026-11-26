#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s785 exit test: an edit to an integration-test file selects only scripts naming its module;
# a src module edit selects scripts running the crate AND naming the module; crate roots select
# only scripts naming the path (2026-11-24, owner approved).
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q -b main repo
cd repo
mkdir -p scripts/exit .mason crates/daemon/tests/it crates/daemon/src/landing_floor
cp "$root/scripts/select-exit-scripts.sh" scripts/
printf '[package]\nname = "mason-daemon"\n' > crates/daemon/Cargo.toml
printf '#!/bin/bash\ncargo test -p mason-daemon --test it landing_floor::\n' > scripts/exit/s001.sh
printf '#!/bin/bash\ncargo test -p mason-daemon --test it other\n' > scripts/exit/s002.sh
printf '#!/bin/bash\ncargo test -p mason-daemon\n' > scripts/exit/s003.sh
printf '#!/bin/bash\ncargo test --test it landing_floor_x\n' > scripts/exit/s004.sh
printf '#!/bin/bash\ncargo test -p mason-daemon --lib mason-daemon-landing_floor-x\n' > scripts/exit/s005.sh
printf '#!/bin/bash\n# touches crates/daemon/src/lib.rs\ncargo test -p other\n' > scripts/exit/s006.sh
printf '#!/bin/bash\ncargo test -p mason-daemon --lib daemon_thing\n' > scripts/exit/s007.sh
printf '#!/bin/bash\n# the landing_floor rules, in prose only (s1089)\ncargo test -p other\n' > scripts/exit/s008.sh
fail() { echo "FAILED: $1" >&2; exit 1; }
echo "== test file: only the script naming the module as a test path (s1089)"
printf 'crates/daemon/tests/it/landing_floor.rs\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "scripts/exit/s001.sh" ] || fail "test edit selected: $out"
echo "== src module: only scripts running the crate and naming the module"
printf 'crates/daemon/src/landing_floor.rs\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "scripts/exit/s001.sh" ] || fail "src module edit selected: $out"
echo "== src mod.rs uses the directory name"
printf 'crates/daemon/src/landing_floor/mod.rs\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "scripts/exit/s001.sh" ] || fail "mod.rs edit selected: $out"
echo "== crate root: only scripts naming the path"
printf 'crates/daemon/src/lib.rs\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "scripts/exit/s006.sh" ] || fail "lib.rs edit selected: $out"
printf 'crates/daemon/Cargo.toml\ncrates/daemon/src/main.rs\n' > .mason/changed-files.txt
[ -z "$(bash scripts/select-exit-scripts.sh)" ] || fail "crate root selected crate-wide scripts"
echo "== hyphenated or longer words do not match a stem"
printf 'crates/daemon/src/daemon.rs\n' > .mason/changed-files.txt
[ -z "$(bash scripts/select-exit-scripts.sh)" ] || fail "stem inside mason-daemon matched"
echo "== changed script and docs"
printf 'scripts/exit/s002.sh\ndocs/x.md\n' > .mason/changed-files.txt
out=$(bash scripts/select-exit-scripts.sh)
[ "$out" = "scripts/exit/s002.sh" ] || fail "script edit selected: $out"
printf 'docs/x.md\n' > .mason/changed-files.txt
[ -z "$(bash scripts/select-exit-scripts.sh)" ] || fail "docs selected something"
echo "s785 OK"
