#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 0 exit test: workspace builds, and the SPDX and DCO checks reject
# deliberately bad samples while accepting good ones.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

cargo build --workspace
[ -f LICENSE ] && [ -f LICENSE-ADDITIONAL-PERMISSION ] && [ -f docs/DESIGN.md ]
[ -f rust-toolchain.toml ] && [ -f Cargo.lock ]
! git ls-files | grep -qx 'MASON_MIGRATION_PLAN.md' || { echo "plan must not be tracked" >&2; exit 1; }
! grep -rq 'pull_request_target' .github/workflows || { echo "pull_request_target is forbidden" >&2; exit 1; }
! grep -rq 'self-hosted' .github/workflows || { echo "self-hosted runners are forbidden" >&2; exit 1; }

# SPDX: good sample passes, bad sample fails.
printf '// SPDX-License-Identifier: GPL-3.0-or-later\nfn main() {}\n' > "$tmp/good.rs"
printf 'fn main() {}\n' > "$tmp/bad.rs"
bash scripts/check-spdx.sh "$tmp/good.rs"
if bash scripts/check-spdx.sh "$tmp/bad.rs" 2>/dev/null; then echo "SPDX check accepted a bad sample" >&2; exit 1; fi
bash scripts/check-spdx.sh   # the whole repo is clean

# DCO: signed message passes, unsigned fails.
printf 'x\n\nSigned-off-by: A B <a@example.com>\n' > "$tmp/good.msg"
printf 'x\n\nno sign off here\n' > "$tmp/bad.msg"
bash scripts/check-dco.sh --message "$tmp/good.msg"
if bash scripts/check-dco.sh --message "$tmp/bad.msg" 2>/dev/null; then echo "DCO check accepted a bad sample" >&2; exit 1; fi
echo "s00 OK"
