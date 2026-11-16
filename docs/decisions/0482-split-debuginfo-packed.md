# 0482: `split-debuginfo = "packed"` for the dev and test profiles

Status: accepted (task #476)

**Problem.** On macOS cargo's default is `unpacked`, which keeps every codegen unit's `*.rcgu.o` in
`target/debug/deps` because the debug info is read from them. Each rebuild under a new crate hash
adds a set; one task directory held 204,381 of them (about 31 copies per unit of `mason_daemon`).

**Decision.** `[profile.dev] split-debuginfo = "packed"` in `Cargo.toml` (the test profile inherits
it). Debug info goes into a `.dSYM` and the `.o` files are removed. Pruning was not needed because
the profile change keeps backtraces.

## Measured (this Mac, `cargo test -p mason-core --no-run`, clean target dir, same commit)

| setting  | `*.rcgu.o` files | entries in `deps/` | `deps/` size (KiB) | wall   | user CPU |
|----------|------------------|--------------------|--------------------|--------|----------|
| unpacked | 321              | 327                | 35504              | 32.4 s | 36.6 s   |
| packed   | 0                | 15                 | 27772              | 35.0 s | 38.5 s   |
| off      | 0                | 6                  | 15020              | 10.3 s | 26.6 s   |

`off` is not chosen: its wall time is from a run that sccache probably partly served, and with it
the executables carry no debug info of their own, so backtrace lines were not shown to survive
(not checked). `packed` costs about 2 s of `dsymutil` here.

Backtrace check: `crates/core/tests/backtrace_lines.rs` runs a panicking child with
`RUST_BACKTRACE=1` and requires an `at ...backtrace_lines.rs:N` frame; it passes under `packed`.

## Not verified

The whole-workspace before/after (file count, size, wall/CPU) and the link time of the daemon's
test executables: only `mason-core` was measured, so the table above is that crate alone and the
effect on the 204,381-file case is extrapolated from 321 files to 0. Whether `off` keeps line
numbers. Whether the `.dSYM` directories grow across rebuilds (they are overwritten per hash, so
they may still add one directory per hash).
