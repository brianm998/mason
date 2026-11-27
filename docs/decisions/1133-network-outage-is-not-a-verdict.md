# 1133: a network outage is not a verdict

Task #1185. Stage s1144.

## Context

A few minutes without DNS failed a four-task batch and used up a task's last test round.

## Decision

- One matcher (`net.rs`). A refused credential never matches. For git calls it covers name resolution, no route, cut or timed out connections and 5xx. `remote rejected` stays a reason to retry only for the remote runs' snapshot push (#772): at landing it is usually a hook or a branch rule, which waiting does not change.
- A suite's log is read more narrowly than git's error: "timed out" and "connection reset" are things tests say about the product.
- A failing test is always a red round. So is a run stopped at its limit or by the stall watcher. Only a nameless, non-zero exit can be the network's.
- A batch the network stops is held, not failed. It reuses the yield path: tasks go back into the queue, the batch's kept runs and tree evidence stay. The hold ends when a fetch of the base works. A hold caused by a suite waits 10 minutes first, because the base can be fetched while the suite's own host is still away.
- The step's class is set in `jobs.rs` from the reason `suites.rs` writes, so `core/task.rs` is unchanged.
- The promote push is retried around `Batch::finish_with`, so `crates/landing` is unchanged. A retry after a folded base move may verify that fold again.
- The one alert reuses `autoretry::bursts`. `attention.rs` changes only to take the burst alerts from `autoretry.rs` and to leave out the `failed:` alerts the outage entry covers.

## Consequences

- While a remote answers fetches but fails pushes with a network message, a project cycles through held batches, one per round of waits.
- A host a suite needs that stays away keeps the project's landing held, retried every 10 minutes, until it answers or the owner acts.
- Protected files changed: `landing.rs` (retry and hold), `suites.rs` and `suites/base_red.rs` (the network-red rule), `attention.rs` (the alert). No verdict with a failing test changes.
