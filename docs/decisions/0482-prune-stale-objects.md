# 0482: prune stale `*.rcgu.o` before each suite build; leave `split-debuginfo` alone

Status: accepted (task #476)

**Problem.** On macOS cargo's default is `unpacked`, which keeps every codegen unit's `*.rcgu.o` in
`target/debug/deps` because the debug info is read from them. Each rebuild under a new crate hash
adds a set; one task directory held 204,381 of them (about 31 copies per unit of `mason_daemon`).

**Decision.** Prune. Before a suite build in a task's build directory, `taskcache::prune_objects`
(called from `run_build` in `suites.rs`) deletes the `.rcgu.o` files of every crate hash except
the one with the newest object, and never an object modified in the last 15 minutes. Objects of
the live hash stay, so current executables keep their line numbers.

## Why not the profile

| setting  | `*.rcgu.o` files | entries in `deps/` | `deps/` size (KiB) | wall   | user CPU |
|----------|------------------|--------------------|--------------------|--------|----------|
| unpacked | 321              | 327                | 35504              | 32.4 s | 36.6 s   |
| packed   | 0                | 15                 | 27772              | 35.0 s | 38.5 s   |
| off      | 0                | 6                  | 15020              | 10.3 s | 26.6 s   |

(`cargo test -p mason-core --no-run`, clean target dir, same commit, `mason-core` only.)

- `packed`: the daemon's build through MASON's check failed twice with `E0463 can't find crate`
  for `mason_protocol`, `axum`, `tower` and others; with the setting removed the same build ran.
  The cause was not found (sccache is the rustc wrapper; it is the suspect, not shown).
- `off`: the `backtrace_lines` test failed; frames had no `at file.rs:line` for the test's own code.
- `unpacked` (default) keeps lines and builds, so it stays and the `.o` sets are pruned.

`crates/core/tests/backtrace_lines.rs` stays as a guard that panics still name file and line.

## Not verified

The whole-workspace before/after (file count, size, wall/CPU of `deps/`) was not measured; only
`mason-core` above. The prune is covered by unit tests on the selection and on a directory, not
by a real multi-hash rebuild. Whether `packed` fails without sccache.
