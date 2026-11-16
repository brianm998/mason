# 0473: sccache and workspace crates (spike, nothing switched on)

Status: proposed (measurement only; the owner decides what to set)

Measured 2026-11-15 on commit 2e26d10e with `scripts/spike/sccache-workspace.sh`, run under
`mason run --needs compile`: `cargo test --workspace --no-run --target-dir <fresh>` in two
worktrees of the same commit (a, then b), one sccache server per variant (same disk cache).
Variant A is as today (incremental), B is `CARGO_INCREMENTAL=0`, C adds
`SCCACHE_BASEDIRS=<worktrees root>`, D adds `SCCACHE_BASEDIRS=<worktree a>:<worktree b>`.

| build | wall s | CPU s (user+sys) | Rust hits | Rust misses | not cacheable (incremental / other) |
|---|---|---|---|---|---|
| A cold, worktree a | 610 | 2692 | 169 | 20 | 172 / 52 |
| A cold, worktree b | 456 | 3549 | 169 | 20 | 172 / 52 |
| B cold, worktree a | 314 | 1117 | 0 | 206 | 0 / 207 |
| B cold, worktree b | 436 | 1952 | 205 | 1 | 0 / 207 |
| C cold, worktree a | 629 | 1933 | 175 | 31 | 0 / 207 |
| C cold, worktree b | 529 | 1893 | 175 | 31 | 0 / 207 |
| D cold, worktree a | 644 | 1678 | 175 | 31 | 0 / 207 |
| D cold, worktree b | 470 | 1571 | 175 | 31 | 0 / 207 |
| A rebuild in a after a one-line `crates/daemon` edit | 285 | 940 | 0 | 0 | 95 / 3 |
| B rebuild in a after the same edit | 458 | 1322 | 0 | 1 | 0 / 97 |

## What the numbers say

- **Why workspace crates miss today:** the 172 "incremental" refusals in A are the workspace
  units. Nothing else keeps them out of the cache.
- **`CARGO_INCREMENTAL=0` alone is enough.** B in worktree b hit 205 of 206 cacheable calls,
  including the workspace library crates, although b is a different absolute path from a.
  So the per-worktree absolute path did not stop the hits (the task's assumption that it would
  was not borne out; cargo passes relative paths for workspace members).
- **`SCCACHE_BASEDIRS` did not help; it hurt.** C and D missed the same 31 calls in both
  worktrees, so the second worktree got no workspace hits at all. I did not find out why.
- **Most workspace compile work stays uncacheable.** 155 calls are refused as "crate-type" (the
  `--test` harness builds), 39 as "crate-type (bin)" and 7 as proc-macro. A `--no-run` test
  build mostly compiles those, so even with every library crate cached, the saving is the
  library units only, not the whole build.
- **The first switch costs once:** `CARGO_INCREMENTAL` is an environment variable sccache
  hashes, so B-a had 0 hits and 206 misses, and refilled the dependency entries. A,
  and B/C/D afterward, do not share dependency entries.
- **The run_check case gets slower without incremental:** 285 s to 458 s wall (+61%), 940 s to
  1322 s CPU (+41%), for the one-line edit.

## Recommendation (for the owner to set; I changed no config)

1. Do not set `CARGO_INCREMENTAL=0` globally in `~/.cargo/config.toml`: it slows the
   run_check edit-rebuild case, which is the common one inside a task.
2. If the cross-task saving is wanted, set `CARGO_INCREMENTAL=0` only in the project file's
   `test` and `build` commands (the cold-target-dir steps and batch builds), and leave
   run_check incremental. Expect the library crates to hit from the second task on.
3. Do not set `SCCACHE_BASEDIRS`.
4. Before relying on it, repeat the measurement on a quiet machine (see below), because the
   gain is bounded by the library units only.

## Not verified

- **Wall and CPU times are noisy.** Load average was about 80 to 95 on 36 logical CPUs, and
  other tasks compiled throughout. CPU time rises under contention (A's two builds differ by
  32% in CPU for identical work), so differences under about 30% between rows mean little.
  Each row is one run, not a median.
- Wall-time saving of B over A on cold builds (about 300 to 450 s against 610 s) is
  suggestive only; B-a also ran at a calmer moment than the others.
- The hit counts are per build, as sccache reports them; I did not split them per crate, so
  "workspace library crates hit in B-b" is inferred from 205 of 206 hits, not read per unit.
- I did not find why BASEDIRS misses, and did not test other sccache versions (0.19.0).
- The first attempt at the script was stopped at its one-hour limit while waiting for a compile
  slot and mid-run; B's second worktree and edit rebuild, C and D came from a second run on the
  same disk cache. A's and B-a's rows are from the first run.
