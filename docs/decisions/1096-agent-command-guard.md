# 1096: a command guard that denies, in every agent session

Agents run unwatched with `dontAsk`, and the stash stack is shared by every worktree, so the
guard answers `deny` with the way out; it never asks and never allows.

- **Registered by the adapter** with `--settings`, so it reaches every project's Claude agents
  and adds to project and owner settings (decision 0003). It registers `mason hook guard`, not
  `hook bash`: other projects do not get the build-queue rewrite they never asked for.
- **`hook bash` refuses only inside a MASON session** (`MASON_STEP_KIND` or `MASON_TASK`). It is
  in this repository's `.claude/settings.json`, which the owner's own sessions read too; those
  are watched, and they push and reset on purpose. To refuse there as well, drop the condition.
- **Fails closed.** A hook that exits other than 2 lets the command run, so a missing or
  crashing guard is turned into exit 2.
- **Roots are strict.** The worktree, state directory, task cache and `$TMPDIR` cannot
  themselves be removed. "State directory" is the agent state directory `<data>/a`, as in the
  sandbox, not all of `<data>` (which holds the database and every worktree). `/tmp` is not
  `$TMPDIR` on macOS and is refused.
- **Unknown is refused** for recursive `rm`: unset variables, substitutions, brace lists, loop
  variables, `xargs`.
- **All `git stash pop`** is refused, with or without an index: indices shift as other
  worktrees push. `drop` is refused even for one's own entry.
- The guard reads text and is not a security boundary; the sandbox is (off by default, the
  owner's choice).
