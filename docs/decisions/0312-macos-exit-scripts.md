# 0312: macOS exit scripts: one portability defect, the rest green here

**Decision.** s226 ran `timeout 90` on Chrome. Stock macOS and GitHub's macos-latest have no `timeout`, so the command exits 127, the DOM is empty and the grep fails. `scripts/lib/bounded.sh` now provides `bounded SECS CMD...` (timeout, else gtimeout, else perl alarm) and s226 uses it. The three DOM assertions are unchanged. `scripts/tests/portable-timeout.sh` runs it with a PATH holding no `timeout`; before the helper existed it failed, after it passes.

**What was run.** On this Mac, one at a time, on the branch (origin/develop plus this change): s05, s20, s25, s40, s50, s131, s193, s209, s219, s220, s226, s266, s267 all exit 0. s09 exits 1 ("the task nearer to landing must be admitted first", masond killed at teardown). s09 also fails on Ubuntu, so it is not macOS-only; it belongs to #333 (Ubuntu runs of s09, s50, s127). s08, s129, s165, s166, s236 were left to #333 and not run.

**Caveat.** This Mac has `timeout` (/usr/local/bin), gradle, rustup and Chrome, so s226 passed here even before the change; the defect is shown by the new test with `timeout` off PATH, and by reading the runner image, not by reproducing the CI failure. s220 (gradle) and s193 (rustup) passing here says nothing about the runner's environment. Other nightly failures were develop being red (b164-b172), and do not reproduce.

**Not verified.** The 03:17 UTC nightly on develop, which this task cannot run.

**Is the nightly a sufficient guard?** No: a macOS-only failure surfaces up to a day late and push runs show the legs as skipped. Recommendation for the owner (ci.yml is protected and unchanged): run `exit-scripts-macos` on push when a change touches `scripts/**`, or on PRs to develop, keeping the full run nightly.

**Trust.** `scripts/exit/s226.sh` is in PROTECTED: it needs owner approval. It is needed so the script runs where `timeout` is absent, and still checks the picker tests, the site files, and the windows monitor link, windows cli link, and absence of macos.dmg in the DOM. `scripts/exit/s318.sh` is new.
