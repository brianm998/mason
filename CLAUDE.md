# MASON

The "How to work in this repository" footer on every pipeline task still applies (SPDX lines,
`git commit -s`, fmt/clippy/test/exit scripts, no weakening gates, owner approval for trust files).
This file adds what sessions keep tripping over.

## Read code from `origin/develop`

The pipeline branches every task from `origin/develop`. A session worktree may start from a local
`develop` that has drifted (on 2026-11-09 it was 5 ahead and 83 behind). Before you write up a
finding, a file:line or a ticket, read the code with `git show origin/develop:<path>` (or
`git archive origin/develop`), not from the worktree.

The running daemon updates itself from **GitHub** `develop`, not the local branch. A change is
live only after it is pushed. `mason update --check` builds and self-tests only; `mason update`
is what takes over.

## Tests

- **No timing assertions.** They go red when the machine is busy, which it usually is.
- **Test fixtures must ad-hoc sign any binary they copy** (`codesign -s -`). If an unsigned
  Mach-O is deleted while macOS is scanning it, `syspolicyd` segfaults, and until launchd
  restarts it every new non-Apple process hangs at 0% CPU.
- `pgrep -x masond` also matches the daemons MASON's own tests start. Stop a daemon by the pid in
  its `daemon.json`.

## Stage notes and decision records

Hand-made stage and decision numbers must be reserved in `number_reservations` first. Also check
them against open questions on tasks, because an owner answer may already have used the number
(s61 collided with #27's 61/0041).

## Filing tasks

```
mason task add "<title>" --project mason --body-file F --explored [--priority high|normal|low] [--after N,M]
```

- Use `--explored` only when the body really is the plan; such a task starts at develop.
- Body: one paragraph on the problem, **Cause** with file:line on `origin/develop`, **Change**,
  **Coordinate**, **Trust** (paths in `PROTECTED`, `crates/daemon/src/protect.rs`), **Tests** (no
  timing assertions), **Exit test**, then the "How to work in this repository" footer (copy it from
  `mason task show 54`).
- Use `--after` only where two tasks change the same function or one needs the other's result.
- A task for another project (e.g. `--project star`) uses that project's own format and has no
  footer.
- The shell is bash 3.2, so file a batch from a Python script, not a bash function with arrays.

## Sharing the machine with the pipeline

- An interactive `cargo` build or test in a worktree takes a `compile` slot ahead of all pipeline
  work. Keep them few while tasks are waiting, or develop agents' `run_check` calls starve.
- Steps never run macOS XCUITests, and nothing ever appears on the Mac's screen. Under
  `MASON_TASK` an `xcodebuild` shim refuses any run that would execute a `*UITests` bundle
  (exit 69), and a `caffeinate` shim drops `-u`. Screenshot work renders offscreen
  (ImageRenderer / cacheDisplay).
