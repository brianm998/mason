# 0561: prune and bound every build directory MASON builds in

On 2026-11-17 three shared build directories reached 390k to 750k entries and 46 rustc children of the one sccache server sat in `__getdirentries64` listing them; every cargo queued behind them. Decision 0482 pruned stale `.rcgu.o` sets only in task directories.

## Decision

1. **Every build directory is prepared before a build** (`taskcache::prepare_for_build`, from `run_build`, `run_on_base` and the test-only base path). The directory is `<parent of task_cache's {task}>/<basename of the build's cwd>`, the convention the project's `--target-dir` uses: a task number, `base-<suite>`, `<project>-b<slot>`, `wt`. The same `stale_objects` rule (newest set per crate stays, 15 minute grace) applies.
2. **Hard bound.** `debug/deps` is `stat`ed (never listed). Over 2 MB (`DEPS_LIMIT_BYTES`, about 60k entries; a task's is 0.5 MB) the whole directory is renamed to `.removing-<name>-<ms>` and the build starts cold. The prune is skipped then, because listing is what stalls. A note is logged and put on the broker's notes.
3. **Removal** goes through the existing one-at-a-time background thread (`in_background`), running `taskpolicy -b /bin/rm -rf`. Leftover `.removing-*` entries from before a restart are queued again at the next prepare.
4. **Visibility.** The Compilation section's `notes` and `mason doctor` list each shared directory's `deps` size, with a warning over the limit. (A new protobuf field would have needed regenerated Swift bindings; notes reach every monitor unchanged.)
5. **Health finding** `overloaded:blocked-compiles:<dir>`: at least `[health] blocked_compiles` (4) rustc in state `U`, running at least `blocked_compile_m` (10) minutes with under 5% CPU, grouped by `-L dependency=`; the summary names the directory.

## Update build

`update::build` compiles in a fresh `runtimes/src/<sha>` copy that is deleted afterwards, so it has no long-lived directory of MASON's own to prune. Unchanged.

## #475 (seeding from the landing slot)

It must copy only after `prepare_for_build` has run on the slot; a renamed-aside slot is simply absent, so a copy finds nothing and the task builds cold. It must never copy a `.removing-*` directory.

## Idle kill and the sccache server

`compile_idle_kill_s` watches the slot holder's process tree. The stuck rustc were children of the sccache server, so the check did not see them. Counting the server's children for the holder is not right: the server is shared by every cargo, and its children cannot be attributed to one holder. The idle check is left alone; the new health finding reads the whole process table and covers the case.

## split-debuginfo = "packed"

Not measured in this change: it needs a real sccache and a full workspace build, which a task step cannot do reliably, and 0482 recorded E0463 with it. `Cargo.toml` is unchanged. Pruning and the bound stand without it. To try: set `split-debuginfo = "packed"` in the dev profile, build twice through sccache, check there are no `.rcgu.o` in `deps` and that `crates/core/tests/backtrace_lines.rs` still passes.

## Not verified

No real huge directory was used; tests use a fake size source. The `U` state and 5% CPU rule come from the incident text, not a live process table. Protected `suites.rs` and `suites/base_red.rs` changed only to call the prepare step, no verdict logic.
