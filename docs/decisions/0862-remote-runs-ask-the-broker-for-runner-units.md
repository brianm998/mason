# 0862: every remote run asks the broker for runner units before it is pushed

## Context
Decision 0741 (#759) said a check bound for GitHub takes no local slot and assumed the `gha`
pools already bound GitHub concurrency for test steps. They did not bound the rest: agents'
checks, landing runs, shards and macOS suites were pushed at once, and GitHub's own queue has no
rank. On 2026-11-21 GitHub ran 20 jobs with 20 more queued while `mason resources` showed
`gha 20/20`.

## Decision
This revises 0741. A check that goes to GitHub still takes no local slot, but it now takes runner
units and waits for them in the broker, ranked as the agent's own step, before it is pushed.

- `remote::run_place` says, for a run, where it goes and what it holds: `Local`, or `GitHub`
  with one `gha` unit per job it starts (its shards, by the same rule `run_remote` splits by,
  `remote::shard_command`) and as many `gha-macos` on a macOS runner.
- A suite is placed by its own rules (`suite_pool_needs`), never by the global switch: a suite
  that runs here keeps its local slots, one that goes to GitHub takes units and no local slot.
  A landing hold is decided per suite (`Runner::hold_suites`).
- `remote::execute`, `execute_sharded`, `run_or_fallback` and `hostedcheck::run` take a `Grant`,
  made only from a broker lease. A run that falls back to this machine (`fallback = "local"`)
  gives the units back through its grant before it takes local slots.
- A check's adopted hold is taken over by its run id's prefix after a restart, as a test step's
  is by `task N`, so units are not counted twice.
- `mason resources` lists the `gha` waiters in line order with their kind and wait.

## Consequences
A check may wait for runner units where it used to be pushed at once; the wait is credited to the
develop deadline as any slot wait is. The order among kinds (landing, checks, test steps, debts)
is a separate task.

Not done here: `Grant::unplumbed` still stands for callers whose lease is not passed down to the
push (the suites' `run_remote`, landing's re-test and lint checks, covmap), so those pushes are
not yet impossible without units; the floor and fast-tier builds and debt payments still ask for
units as before rather than per suite.
