# 0662: check what a change can break before it lands

Status: accepted (waits for owner approval: `.mason/**`, `scripts/check-*.sh`, `scripts/exit/**` are protected)

## Context
s638 landed without SPDX, s634 with a `grep -E` GNU rejects, and s486 with two cargo test names. Only GitHub CI ran the affected scripts.

## Decision
- `lint` (landing) runs `check-spdx.sh`, `check-exit-guards.sh` and `check-exit-portability.sh` before fmt and clippy.
- `run-changed-exit-scripts.sh` runs the scripts that `select-exit-scripts.sh` picks: added/edited ones, plus any whose text names a changed path, the file name of a changed `scripts/` file, `-p <crate>` of a changed crate, or `--test <target>` of a changed test target. Docs changes pick none.
- The same runner runs once per batch on the merged tree as suite `exit-scripts-landing`, tier `exit`.
- This adds checks and removes none.

## Still only CI catches
Linux-only tool differences beyond the lint's three patterns (e.g. `date -d`, `stat -c`, `readlink -f`, `xargs -r`, other GNU/BSD flag differences), Windows, scripts whose text does not name what they exercise, and the swift/gradle jobs.
